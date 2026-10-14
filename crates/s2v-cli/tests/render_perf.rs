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

const LINES: usize = 1000;
const LIMIT: Duration = Duration::from_secs(30 * 60);
const DISK_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

const HEADER: &str =
    "@scene 居間\n@pause\nsentence 0\ncast 0\nparagraph 0\n@cast\nA:田中,pan=-30\n\n@script\n";

fn font_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("s2v-render/tests/fonts/test-vrt2.ttf")
}

/// 同梱の config.toml(既定の設定)の `[render]` の font_path だけを、同梱のテスト用フォントに差し替えた設定。
fn config_text(extra: &str) -> String {
    let base = include_str!("../../../config.toml");
    let a = base.find("\n[render]").expect("[render] 節");
    let b = base.find("\n[prompts]").expect("[prompts] 節");
    format!(
        "{}\n[render]\nfont_path = '{}'\n{extra}\n{}",
        &base[..a],
        font_path().display(),
        &base[b..]
    )
}

/// 1.8 秒の収録(全体が音。トリムで短くならない)。
fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(48_000 * 18 / 10) {
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

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    config: PathBuf,
}

/// 台本・収録・設定を書いて build を通す。素材は呼び出し側が `tracks/` に置く(`make_assets`)。
fn project(body: &str, lines: usize, make_assets: impl FnOnce(&Path), extra: &str) -> Project {
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
    fs::write(&config, config_text(extra)).unwrap();
    let t = Instant::now();
    let o = run_build(&BuildArgs {
        script: script.clone(),
        voice_dir: None,
        config: Some(config.clone()),
        no_cache: false,
    });
    assert!(o != Outcome::Failed, "build が失敗しました");
    println!("build: {:.1} 秒", t.elapsed().as_secs_f64());
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

fn run_a(lines: usize, cuts: usize, check: bool) {
    let p = project(
        &script_text(lines, cuts, "image"),
        lines,
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
        "",
    );
    measure(&p, "(a) 多数のカット", check);
}

fn run_b(lines: usize, video_secs: u32, check: bool) {
    let p = project(
        &script_text(lines, 1, "video"),
        lines,
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
        "",
    );
    measure(&p, "(b) 長い動画の素材", check);
}

/// 本番の試験の生成部分(台本・収録・素材・設定)が通ることを小さな規模で確かめる。
#[test]
fn generator_smoke() {
    if std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1") {
        return;
    }
    run_a(6, 3, false);
    run_b(4, 8, false);
}

#[test]
#[ignore = "手動の性能試験(最大で 30 分以上かかる)"]
fn perf_many_cuts() {
    run_a(LINES, 200, true);
}

#[test]
#[ignore = "手動の性能試験(最大で 30 分以上かかる)"]
fn perf_long_video_many_bubbles() {
    run_b(LINES, 1800, true);
}
