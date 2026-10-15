//! 性能試験(手動で実行: cargo test -p s2v-cli --release --test render_perf -- --ignored --nocapture)
//! 設計書 §8: (a) 200 カット・1000 吹き出し・30 分(素材は画像)、(b) 30 分の動画の素材1つのカットに 1000 吹き出し。
//! どちらも 30 分以内、プロセス全体のメモリ 2 GiB 以内、work/render/ 2 GiB 以内。
//! メモリは測れる範囲で表示するだけ。Windows ではタスクマネージャで ffmpeg を含む合計を目視すること。
//! ffmpeg が無い環境では失敗する。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use s2v_cli::{run_build, run_render, BuildArgs, Outcome, RenderArgs};
use s2v_video::frames::total_frames;

const LINES: usize = 1000;
const LIMIT: Duration = Duration::from_secs(30 * 60);
const DISK_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

const HEADER: &str =
    "@scene 居間\n@pause\nsentence 0\ncast 0\nparagraph 0\n@cast\nA:田中,pan=-30\n\n@script\n";

/// 同梱の config.toml(既定の設定)そのまま。既定のフォントが無い環境では、無いファイルを示して失敗する。
fn config_text() -> String {
    let text = include_str!("../../../config.toml").to_string();
    let font = text
        .lines()
        .find_map(|l| l.strip_prefix("font_path = '"))
        .and_then(|r| r.split('\'').next())
        .expect("config.toml の font_path");
    assert!(
        Path::new(font).exists(),
        "既定のフォント {font} がありません(性能試験は既定の設定で測ります)"
    );
    text
}

/// 1.7 秒の収録(全体が音。トリムで短くならない)。
fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(48_000 * 17 / 10) {
        let v = 0.1 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 48_000.0).sin();
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

/// 台詞 `lines` 個を `cuts` 個のカットに均等に分けた台本。カット n の素材の名前(拡張子なし)は S01(n=1)・S01_C{n:02}。
fn script_text(lines: usize, cuts: usize, visual: &str) -> String {
    let mut s = String::from("#paragraph[S01] 朝\n");
    let per = lines.div_ceil(cuts);
    for n in 1..=cuts {
        if n > 1 {
            s += &format!("#cut[C{n:02}]\n");
        }
        s += &format!("#visual ({visual}) 場面{n}\n");
        for i in (n - 1) * per + 1..=(n * per).min(lines) {
            s += &format!("[S01-L{i:04}] A:おはよう。\n");
        }
    }
    s
}

fn asset_stem(n: usize) -> String {
    if n == 1 {
        "S01".to_string()
    } else {
        format!("S01_C{n:02}")
    }
}

fn ffmpeg(cwd: &Path, args: &[&str]) {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-n", "-v", "error"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("ffmpeg を起動できません(PATH に ffmpeg が必要です)");
    assert!(
        out.status.success(),
        "ffmpeg {args:?} が失敗しました: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => dir_size(&p),
                Ok(m) => m.len(),
                Err(_) => 0,
            }
        })
        .sum()
}

/// 全体のコマ数(既定の 30fps)が目標のコマ数と 1 コマ以内か。
fn frames_close(d: i64, target_ms: i64) -> bool {
    (total_frames(d, 30) - total_frames(target_ms, 30)).abs() <= 1
}

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    config: PathBuf,
}

/// 台本・収録・設定を書いて build を通す。台本の長さは、末尾の `#pause` で `target_ms` ちょうど(1 コマ以内)にする。素材は呼び出し側が `tracks/` に置く(`make_assets`)。
fn project(body: &str, lines: usize, target_ms: i64, make_assets: impl FnOnce(&Path)) -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 性能");
    let voices = dir.join("voices");
    let tracks = dir.join("tracks");
    fs::create_dir_all(&voices).unwrap();
    fs::create_dir_all(&tracks).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{body}")).unwrap();
    // 録音は同じ wav を複製する
    let first = voices.join("S01_L0001.wav");
    write_voice(&first);
    for i in 2..=lines {
        fs::copy(&first, voices.join(format!("S01_L{i:04}.wav"))).unwrap();
    }
    make_assets(&tracks);
    let config = dir.join("config.toml");
    fs::write(&config, config_text()).unwrap();
    let build = || {
        let o = run_build(&BuildArgs {
            script: script.clone(),
            voice_dir: None,
            config: Some(config.clone()),
            no_cache: false,
        });
        assert!(o != Outcome::Failed, "build が失敗しました");
        let text = fs::read_to_string(dir.join("timeline").join("timeline.json")).unwrap();
        s2v_timeline::load_str(&text).unwrap().duration_ms
    };
    let t = Instant::now();
    let d = build();
    assert!(
        d <= target_ms,
        "台詞だけで {d} ms あり、目標 {target_ms} ms を超えています"
    );
    // 末尾の #pause で目標の長さにそろえる(#pause の長さがそのまま足されるとは限らないので、ずれを見て直す)
    let base = fs::read_to_string(&script).unwrap();
    let mut pause = target_ms - d;
    let mut d = d;
    for _ in 0..3 {
        fs::write(
            &script,
            format!(
                "{base}#pause {pause}
"
            ),
        )
        .unwrap();
        d = build();
        if frames_close(d, target_ms) {
            break;
        }
        pause += target_ms - d;
    }
    println!(
        "build: {:.1} 秒、台本の長さ {d} ms",
        t.elapsed().as_secs_f64()
    );
    assert!(
        frames_close(d, target_ms),
        "台本の長さ {d} ms が目標 {target_ms} ms と 1 コマ以上ずれています"
    );
    Project {
        _tmp: tmp,
        dir,
        script,
        config,
    }
}

/// render を測って表示する。`check` が真なら 30 分・2 GiB の上限を assert する。
fn measure(p: &Project, name: &str, check: bool) {
    let t = Instant::now();
    let o = run_render(&RenderArgs {
        script: p.script.clone(),
        config: Some(p.config.clone()),
    });
    let elapsed = t.elapsed();
    let disk = dir_size(&p.dir.join("work").join("render"));
    println!(
        "{name}: render {:.1} 秒、work/render/ {:.1} MiB、結果 {o:?}(メモリは ffmpeg を含む合計をタスクマネージャで目視)",
        elapsed.as_secs_f64(),
        disk as f64 / 1_048_576.0
    );
    let log = fs::read_to_string(p.dir.join("Process.log")).unwrap_or_default();
    assert!(o != Outcome::Failed, "render が失敗しました\n{log}");
    assert!(p.dir.join("output.mp4").exists(), "output.mp4 がありません");
    if check {
        assert!(elapsed <= LIMIT, "{name}: 30 分を超えました");
        assert!(
            disk <= DISK_LIMIT,
            "{name}: work/render/ が 2 GiB を超えました"
        );
    }
}

fn png_1080p(path: &Path) {
    let mut pm = tiny_skia::Pixmap::new(1920, 1080).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(60, 90, 140, 255));
    fs::write(path, pm.encode_png().unwrap()).unwrap();
}

fn run_a(lines: usize, cuts: usize, target_ms: i64, check: bool) {
    let p = project(
        &script_text(lines, cuts, "image"),
        lines,
        target_ms,
        |tracks| {
            png_1080p(&tracks.join("S01.png"));
            for n in 2..=cuts {
                fs::copy(
                    tracks.join("S01.png"),
                    tracks.join(format!("{}.png", asset_stem(n))),
                )
                .unwrap();
            }
        },
    );
    measure(&p, "(a) 多数のカット", check);
}

fn run_b(lines: usize, video_secs: u32, check: bool) {
    let p = project(
        &script_text(lines, 1, "video"),
        lines,
        video_secs as i64 * 1000,
        |tracks| {
            let secs = video_secs.to_string();
            ffmpeg(
                tracks,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=1920x1080:rate=30",
                    "-t",
                    &secs,
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    "S01.mp4",
                ],
            );
        },
    );
    measure(&p, "(b) 長い動画の素材", check);
}

/// 本番の試験の生成部分(台本・収録・素材・設定)が通ることを小さな規模で確かめる。
// 既定のフォントが必要で、本番の試験と同じく手動で動かす(生成部分の確認用)
#[test]
#[ignore = "既定の設定(既定のフォント)が必要な、小規模の生成確認"]
fn generator_smoke() {
    run_a(6, 3, 12_000, false);
    run_b(4, 8, false);
}

#[test]
#[ignore = "手動の性能試験(最大で 30 分以上かかる)"]
fn perf_many_cuts() {
    run_a(LINES, 200, 1_800_000, true);
}

#[test]
#[ignore = "手動の性能試験(最大で 30 分以上かかる)"]
fn perf_long_video_many_bubbles() {
    run_b(LINES, 1800, true);
}
