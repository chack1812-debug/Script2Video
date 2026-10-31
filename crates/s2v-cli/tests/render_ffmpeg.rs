//! `render` コマンドの結合テスト。実際の ffmpeg・ffprobe を使う。
//! ffmpeg が無い環境ではテストは失敗する。飛ばすのは `S2V_SKIP_FFMPEG_TESTS=1` のときだけ。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use s2v_cli::{run_build, run_render, BuildArgs, Outcome, RenderArgs};
use s2v_export::wavio::read_stereo_48k;
use s2v_video::audio_check::{best_lag, decode_args, f32le};
use s2v_video::frames::{frame, total_frames};

const W: usize = 320;
const H: usize = 180;

fn skip() -> bool {
    std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")
}

const HEADER: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中,pan=-30\n\n@script\n";

/// 台詞 1 つ + ト書き 1 つの、画像のカット 1 つの台本。
const SIMPLE: &str =
    "#paragraph[S01] 朝\n#visual (image) 朝の居間。\n#note[N01] 翌朝、港。\n[S01-L001] A:おはよう。\n";

/// 台詞 2 つ(1110ms と 1410ms から)。ト書きなし。
const TWO_LINES: &str = "#paragraph[S01] 朝\n#visual (image) 朝の居間。\n[S01-L001] A:おはよう。\n[S01-L002] A:おはよう。\n";

enum Asset {
    /// 単色の PNG
    Png([u8; 3]),
    /// 任意の方法で作る(出力先のパスを受け取る)
    Make(Box<dyn Fn(&Path)>),
}

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    config: PathBuf,
}

impl Project {
    fn seg(&self) -> PathBuf {
        self.dir.join("work").join("render").join("seg")
    }
    fn output(&self) -> PathBuf {
        self.dir.join("output.mp4")
    }
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("Process.log")).unwrap_or_default()
    }
    fn render(&self) -> Outcome {
        run_render(&RenderArgs {
            script: self.script.clone(),
            config: Some(self.config.clone()),
        })
    }
    /// 成功(0 または 1)を確かめる。失敗なら Process.log を添えて落とす。
    fn render_ok(&self) -> Outcome {
        let o = self.render();
        assert!(
            o != Outcome::Failed,
            "render が失敗しました\n{}",
            self.log()
        );
        o
    }
    fn timeline(&self) -> s2v_timeline::Timeline {
        let text = fs::read_to_string(self.dir.join("timeline").join("timeline.json")).unwrap();
        s2v_timeline::load_str(&text).unwrap()
    }
    fn rewrite_config(&self, render_section: &str) {
        fs::write(&self.config, config_text(render_section)).unwrap();
    }
}

fn font_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("s2v-render/tests/fonts/test-vrt2.ttf")
}

/// 同梱の config.toml の `[render]` だけを差し替えた設定(他の節は同梱のまま)。
fn config_text(render_section: &str) -> String {
    let base = include_str!("../../../config.toml");
    let a = base.find("\n[render]").expect("[render] 節");
    let b = base.find("\n[prompts]").expect("[prompts] 節");
    format!("{}\n[render]\n{render_section}\n{}", &base[..a], &base[b..])
}

fn base_render(fps: u32, extra: &str) -> String {
    format!(
        "width = {W}\nheight = {H}\nfps = {fps}\nfont_path = '{}'\nfont_size = 16.0\nbubble_padding = 6\nnote_padding = 4\nmargin = 8\nbubble_gap = 6\ntail_length = 12\npreset = \"ultrafast\"\ncrf = 10\n{extra}\n",
        font_path().display()
    )
}

/// 440Hz の収録(build_golden.rs の write_voice と同じ: 200ms 無音 + 1000ms + 300ms 無音)。
fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..72_000usize {
        let v = if (9_600..57_600).contains(&i) {
            0.1 * (2.0 * std::f64::consts::PI * 440.0 * (i - 9_600) as f64 / 48_000.0).sin()
        } else {
            0.0
        };
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn solid_png(rgb: [u8; 3]) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(W as u32, H as u32).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
    pm.encode_png().unwrap()
}

/// 台本を書き、収録を置き、素材を置き、build を通して timeline と full_dialogue.wav を作る。
fn project(body: &str, assets: &[(&str, Asset)], fps: u32, extra: &str) -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("voices");
    let tracks = dir.join("tracks");
    fs::create_dir_all(&voices).unwrap();
    fs::create_dir_all(&tracks).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{body}")).unwrap();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((id, _)) = rest.split_once(']') {
                write_voice(&voices.join(format!("{}.wav", id.replace('-', "_"))));
            }
        }
    }
    for (name, asset) in assets {
        let path = tracks.join(name);
        match asset {
            Asset::Png(rgb) => fs::write(&path, solid_png(*rgb)).unwrap(),
            Asset::Make(f) => f(&path),
        }
    }
    let config = dir.join("config.toml");
    fs::write(&config, config_text(&base_render(fps, extra))).unwrap();
    let o = run_build(&BuildArgs {
        script: script.clone(),
        voice_dir: None,
        config: Some(config.clone()),
        no_cache: false,
    });
    assert!(o != Outcome::Failed, "build が失敗しました");
    Project {
        _tmp: tmp,
        dir,
        script,
        config,
    }
}

/// ffmpeg を引数の配列で起動する。起動できない・失敗したときは落とす。
fn ffmpeg(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-n", "-v", "error"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("ffmpeg を起動できません(PATH に ffmpeg が必要です。飛ばすなら S2V_SKIP_FFMPEG_TESTS=1)");
    assert!(
        out.status.success(),
        "ffmpeg {args:?} が失敗しました: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn ffprobe_json(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-of", "json"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("ffprobe を起動できません");
    assert!(
        out.status.success(),
        "ffprobe {args:?} が失敗しました: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn streams(mp4: &Path) -> Vec<serde_json::Value> {
    let v = ffprobe_json(
        mp4.parent().unwrap(),
        &["-show_streams", mp4.file_name().unwrap().to_str().unwrap()],
    );
    v["streams"].as_array().unwrap().clone()
}

/// コマ n を rgb24 で取り出す。出力のタグ(bt709・tv)で明示的に変換する。
fn frame_rgb(mp4: &Path, n: i64) -> Vec<u8> {
    let vf = format!("select=eq(n\\,{n}),scale=in_color_matrix=bt709:in_range=tv");
    let buf = ffmpeg(
        mp4.parent().unwrap(),
        &[
            "-i",
            mp4.file_name().unwrap().to_str().unwrap(),
            "-vf",
            &vf,
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ],
    );
    assert_eq!(buf.len(), W * H * 3, "コマ {n} を取り出せません");
    buf
}

fn px(buf: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * W + x) * 3;
    [buf[i], buf[i + 1], buf[i + 2]]
}

fn count_near(buf: &[u8], target: [u8; 3], tol: i32) -> usize {
    buf.as_chunks::<3>()
        .0
        .iter()
        .filter(|p| (0..3).all(|c| (i32::from(p[c]) - i32::from(target[c])).abs() <= tol))
        .count()
}

fn count_red(buf: &[u8]) -> usize {
    buf.as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] >= 220 && p[1] < 40 && p[2] < 40)
        .count()
}

const RED: [u8; 3] = [230, 20, 20];

/// コマ k の左半分が灰色 k*4 の PNG を 60 枚並べた動画(ほぼ可逆)。
fn make_gray_video(out: &Path) {
    let work = tempfile::tempdir().unwrap();
    for k in 0..60u8 {
        let mut pm = tiny_skia::Pixmap::new(W as u32, H as u32).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(k * 4, k * 4, k * 4, 255);
        paint.anti_alias = false;
        let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, (W / 2) as f32, H as f32).unwrap();
        pm.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
        pm.save_png(work.path().join(format!("f_{k:03}.png")))
            .unwrap();
    }
    ffmpeg(
        work.path(),
        &[
            "-framerate",
            "30",
            "-i",
            "f_%03d.png",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-pix_fmt",
            "yuv420p",
            "clip.mp4",
        ],
    );
    fs::copy(work.path().join("clip.mp4"), out).unwrap();
}

/// 単色の動画(2 秒)。`vf` と `tags` で色の扱いを指定する。
fn make_color_video(out: &Path, rgb: [u8; 3], vf: &str, tags: &[&str]) {
    let work = tempfile::tempdir().unwrap();
    fs::write(work.path().join("c.png"), solid_png(rgb)).unwrap();
    let mut args = vec![
        "-loop",
        "1",
        "-framerate",
        "30",
        "-i",
        "c.png",
        "-frames:v",
        "60",
        "-vf",
        vf,
        "-c:v",
        "libx264",
        "-qp",
        "0",
    ];
    args.extend_from_slice(tags);
    args.push("clip.mp4");
    ffmpeg(work.path(), &args);
    fs::copy(work.path().join("clip.mp4"), out).unwrap();
}

fn video_asset(f: impl Fn(&Path) + 'static) -> Asset {
    Asset::Make(Box::new(f))
}

#[test]
fn renders_image_cut_with_bubble_and_passes_output_checks() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, "");
    p.render_ok();
    assert!(p.output().is_file());
    let s = streams(&p.output());
    let v = s.iter().find(|s| s["codec_type"] == "video").unwrap();
    let a = s.iter().find(|s| s["codec_type"] == "audio").unwrap();
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(320), Some(180))
    );
    assert_eq!(v["r_frame_rate"], "30/1");
    assert_eq!(a["codec_name"], "aac");
    // 台詞の表示中のコマ
    let tl = p.timeline();
    let l = &tl.lines[0];
    let n = (frame(l.start_ms, 30) + frame(l.end_ms, 30)) / 2;
    let buf = frame_rgb(&p.output(), n);
    assert!(
        count_near(&buf, [255, 255, 255], 3) > 100,
        "吹き出しの白がありません"
    );
    assert!(count_red(&buf) > 100, "背景の赤がありません");
    assert!(
        p.log().contains("output.mp4 を書き出しました"),
        "{}",
        p.log()
    );
}

fn boundary_frame_check(fps: u32) {
    let p = project(TWO_LINES, &[("S01.png", Asset::Png(RED))], fps, "");
    p.render_ok();
    let tl = p.timeline();
    let start = frame(tl.lines[1].start_ms, fps);
    let at = frame_rgb(&p.output(), start);
    let before = frame_rgb(&p.output(), start - 1);
    assert!(
        count_near(&at, [255, 255, 255], 3) > 100,
        "fps {fps}: コマ {start} に吹き出しの白がありません"
    );
    assert_eq!(
        count_near(&before, [255, 255, 255], 30),
        0,
        "fps {fps}: コマ {} に吹き出しの白があります",
        start - 1
    );
    assert!(count_red(&before) > 1000, "背景の赤だけのはず");
}

#[test]
fn overlay_switches_exactly_at_the_boundary_frame() {
    if skip() {
        return;
    }
    boundary_frame_check(30);
    boundary_frame_check(60);
}

fn half_grid_check(fps: u32) {
    let p = project(TWO_LINES, &[("S01.png", Asset::Png(RED))], fps, "");
    p.render_ok();
    let tl = p.timeline();
    let n = total_frames(tl.duration_ms, fps);
    // 区間の始まり: 0、各台詞の始め・終わり
    let mut bounds: Vec<i64> = vec![0];
    for l in &tl.lines {
        for f in [frame(l.start_ms, fps), frame(l.end_ms, fps)] {
            if f > 0 && f < n && !bounds.contains(&f) {
                bounds.push(f);
            }
        }
    }
    bounds.sort();
    let mut want: Vec<i64> = bounds
        .iter()
        .map(|&r| if r == 0 { 0 } else { 2 * r - 1 })
        .collect();
    want.push(2 * n);
    let v = ffprobe_json(
        &p.seg(),
        &[
            "-show_streams",
            "-show_packets",
            "-select_streams",
            "v",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            "ovl_0001.txt",
        ],
    );
    let tb = format!("1/{}", 2 * fps);
    assert_eq!(v["streams"][0]["time_base"], tb.as_str(), "fps {fps}");
    let got: Vec<i64> = v["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["pts"].as_i64().unwrap())
        .collect();
    assert_eq!(got, want, "fps {fps}");
}

#[test]
fn overlay_list_pts_are_on_the_half_frame_grid() {
    if skip() {
        return;
    }
    half_grid_check(30);
    half_grid_check(60);
}

const VIDEO_SCRIPT: &str = "#paragraph[S01] 朝\n#visual (video) 朝の居間。\n[S01-L001] A:おはよう。\n#pause 800\n[S01-L002] A:おはよう。\n#pause 1500\n";
const LOOP_SCRIPT: &str = "#paragraph[S01] 朝\n#visual (video loop) 朝の居間。\n[S01-L001] A:おはよう。\n#pause 800\n[S01-L002] A:おはよう。\n#pause 1500\n";

fn gray_at(buf: &[u8]) -> i32 {
    // 左下(吹き出しの無い場所)の灰色
    i32::from(px(buf, 8, 170)[0])
}

#[test]
fn video_asset_continues_across_intervals_and_freezes_or_loops() {
    if skip() {
        return;
    }
    let assets = [("S01.mp4", video_asset(make_gray_video))];
    // loop なし
    let p = project(VIDEO_SCRIPT, &assets, 30, "");
    p.render_ok();
    let n = total_frames(p.timeline().duration_ms, 30);
    assert!(n > 100, "カットが短すぎます: {n} コマ");
    for k in 0..n {
        let g = gray_at(&frame_rgb(&p.output(), k));
        let want = i32::from(k.min(59) as u8) * 4;
        assert!((g - want).abs() <= 3, "コマ {k}: 灰色 {g} (期待 {want} ±3)");
    }
    // 区間が分かれている(台詞が出入りする)
    let tl = p.timeline();
    assert!(frame(tl.lines[0].end_ms, 30) > 0 && frame(tl.lines[1].start_ms, 30) < n);

    // loop あり
    let p = project(LOOP_SCRIPT, &assets, 30, "");
    p.render_ok();
    let n = total_frames(p.timeline().duration_ms, 30);
    for k in 0..n {
        let g = gray_at(&frame_rgb(&p.output(), k));
        let want = i32::from((k % 60) as u8) * 4;
        assert!(
            (g - want).abs() <= 3,
            "loop のコマ {k}: 灰色 {g} (期待 {want} ±3)"
        );
    }
}

#[test]
fn colors_survive_within_tolerance() {
    if skip() {
        return;
    }
    let sample = [200, 100, 50];
    let bt709 = "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p";
    let cases: Vec<(&str, Asset)> = vec![
        ("sRGB PNG", Asset::Png(sample)),
        (
            "BT.709",
            video_asset(move |o| {
                make_color_video(
                    o,
                    sample,
                    bt709,
                    &[
                        "-colorspace",
                        "bt709",
                        "-color_primaries",
                        "bt709",
                        "-color_trc",
                        "bt709",
                        "-color_range",
                        "tv",
                    ],
                )
            }),
        ),
        (
            "BT.601",
            video_asset(move |o| {
                make_color_video(
                    o,
                    sample,
                    "scale=out_color_matrix=bt601:out_range=tv,format=yuv420p",
                    &["-colorspace", "smpte170m", "-color_range", "tv"],
                )
            }),
        ),
        (
            "full レンジ",
            video_asset(move |o| {
                make_color_video(
                    o,
                    sample,
                    "scale=out_color_matrix=bt709:out_range=pc,format=yuv420p",
                    &["-colorspace", "bt709", "-color_range", "pc"],
                )
            }),
        ),
    ];
    for (label, asset) in cases {
        let name = if label == "sRGB PNG" {
            "S01.png"
        } else {
            "S01.mp4"
        };
        let body = if name.ends_with(".mp4") {
            SIMPLE.replace("(image)", "(video)")
        } else {
            SIMPLE.to_string()
        };
        let p = project(&body, &[(name, asset)], 30, "");
        p.render_ok();
        let l = &p.timeline().lines[0];
        let n = (frame(l.start_ms, 30) + frame(l.end_ms, 30)) / 2;
        let buf = frame_rgb(&p.output(), n);
        let got = px(&buf, 8, 170);
        // PNG は RGB→YUV の 1 回、動画は素材の作成と出力で 2 回、8 ビットの YUV 4:2:0 を通るので、
        // 動画の許容は 5(行列を取り違えると 10 以上ずれるので、取り違えは見つかる)
        let tol = if label == "sRGB PNG" { 3 } else { 5 };
        for c in 0..3 {
            assert!(
                (i32::from(got[c]) - i32::from(sample[c])).abs() <= tol,
                "{label}: {got:?} (期待 {sample:?} ±{tol})"
            );
        }
        // 吹き出しの白
        assert!(
            count_near(&buf, [255, 255, 255], 3) > 100,
            "{label}: 白が崩れています"
        );
        // 出力のタグ
        let s = streams(&p.output());
        let v = s.iter().find(|s| s["codec_type"] == "video").unwrap();
        assert_eq!(v["color_space"], "bt709", "{label}");
        assert_eq!(v["color_range"], "tv", "{label}");
        assert_eq!(v["color_primaries"], "bt709", "{label}");
        assert_eq!(v["color_transfer"], "bt709", "{label}");
    }
}

#[test]
fn hdr_or_bt2020_asset_is_rejected() {
    if skip() {
        return;
    }
    let asset = video_asset(|o| {
        make_color_video(
            o,
            [200, 100, 50],
            "format=yuv420p",
            &[
                "-colorspace",
                "bt2020nc",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-color_range",
                "tv",
            ],
        )
    });
    let p = project(
        &SIMPLE.replace("(image)", "(video)"),
        &[("S01.mp4", asset)],
        30,
        "",
    );
    assert_eq!(p.render(), Outcome::Failed);
    assert!(!p.output().exists());
    assert!(p.log().contains("対応していません"), "{}", p.log());
}

#[test]
fn audio_matches_full_dialogue() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, "");
    p.render_ok();
    // render の検査と同じ引数(decode_args)で、独立に復号する
    let out = Command::new("ffmpeg")
        .args(decode_args("output.mp4"))
        .current_dir(&p.dir)
        .output()
        .expect("ffmpeg を起動できません");
    assert!(out.status.success());
    let raw = out.stdout;
    let decoded = f32le(&raw);
    let reference: Vec<f32> = read_stereo_48k(&p.dir.join("full_dialogue.wav"))
        .unwrap()
        .iter()
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();
    let lag = best_lag(&reference, &decoded, 2048);
    assert!(lag.abs() <= 2, "ずれ {lag} サンプル");
    let diff = (decoded.len() as i64 - reference.len() as i64).abs();
    assert!(diff <= 1024, "長さの差 {diff} サンプル");
}

#[test]
fn failed_render_keeps_previous_output() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[], 30, "");
    fs::write(p.output(), b"old").unwrap();
    assert_eq!(p.render(), Outcome::Failed);
    assert_eq!(fs::read(p.output()).unwrap(), b"old");
    assert!(p.log().contains("素材が足りません"), "{}", p.log());
}

#[test]
fn broken_bubbles_json_keeps_previous_output() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, "");
    fs::write(p.output(), b"old").unwrap();
    fs::write(p.dir.join("bubbles.json"), "{").unwrap();
    assert_eq!(p.render(), Outcome::Failed);
    assert_eq!(fs::read(p.output()).unwrap(), b"old");
    assert!(p.log().contains("bubbles.json"), "{}", p.log());
}

#[test]
fn missing_ffmpeg_is_failed_without_writing() {
    // ffmpeg を起動しない経路なので、S2V_SKIP_FFMPEG_TESTS でも実行する
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, "");
    p.rewrite_config(&base_render(30, "ffmpeg = \"存在しない-ffmpeg.exe\""));
    assert_eq!(p.render(), Outcome::Failed);
    assert!(!p.dir.join("work").join("render").exists());
    assert!(!p.output().exists());
    let log = p.log();
    assert!(
        log.contains("ffmpeg が見つかりません: 存在しない-ffmpeg.exe"),
        "{log}"
    );
}

#[test]
fn default_image_is_used_with_warning() {
    if skip() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let img = tmp.path().join("代わり.png");
    fs::write(&img, solid_png([20, 20, 230])).unwrap();
    let extra = format!("default_image = '{}'", img.display());
    let p = project(SIMPLE, &[], 30, &extra);
    assert_eq!(p.render(), Outcome::Warnings, "{}", p.log());
    assert!(p.output().is_file());
    assert!(p.log().contains("default_image"), "{}", p.log());
    let l = &p.timeline().lines[0];
    let buf = frame_rgb(
        &p.output(),
        (frame(l.start_ms, 30) + frame(l.end_ms, 30)) / 2,
    );
    let got = px(&buf, 8, 170);
    assert!(got[2] > 200 && got[0] < 50, "青のはず: {got:?}");
}

#[test]
fn stale_build_is_rejected() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, "");
    fs::write(p.dir.join("full_dialogue.wav"), b"x").unwrap();
    assert_eq!(p.render(), Outcome::Failed);
    assert!(!p.output().exists());
    assert!(p.log().contains("別の build"), "{}", p.log());
}

#[test]
fn missing_glyph_is_warned_in_process_log() {
    if skip() {
        return;
    }
    let body = "#paragraph[S01] 朝\n#visual (image) 朝の居間。\n[S01-L001] A:龘おはよう。\n";
    let p = project(body, &[("S01.png", Asset::Png(RED))], 30, "");
    assert_eq!(p.render(), Outcome::Warnings, "{}", p.log());
    assert!(p.output().is_file());
    let log = p.log();
    assert!(
        log.contains("S01-L001") && log.contains("フォントに無い文字") && log.contains('龘'),
        "{log}"
    );
}

#[test]
fn parallel_jobs_render_every_cut_in_order() {
    if skip() {
        return;
    }
    let body = "#paragraph[S01] 朝\n#visual (image) 朝。\n[S01-L001] A:おはよう。\n#paragraph[S02] 昼\n#visual (image) 昼。\n[S02-L001] A:おはよう。\n#paragraph[S03] 夜\n#visual (image) 夜。\n[S03-L001] A:おはよう。\n";
    let assets = [
        ("S01.png", Asset::Png([230, 20, 20])),
        ("S02.png", Asset::Png([20, 230, 20])),
        ("S03.png", Asset::Png([20, 20, 230])),
    ];
    let p = project(body, &assets, 30, "jobs = 2");
    p.render_ok();
    let tl = p.timeline();
    // 各カットの中ほどのコマの背景色が、台本の並びどおり(赤・緑・青)
    for (i, want) in [[230, 20, 20], [20, 230, 20], [20, 20, 230]]
        .iter()
        .enumerate()
    {
        let c = &tl.scenes[i].cuts[0];
        let n = (frame(c.start_ms, 30) + frame(c.end_ms, 30)) / 2;
        let got = px(&frame_rgb(&p.output(), n), 8, 170);
        for k in 0..3 {
            assert!(
                (i32::from(got[k]) - want[k]).abs() <= 4,
                "カット {i}: {got:?} (期待 {want:?})"
            );
        }
    }
    assert!(p.seg().join("cut_0003.mp4").is_file());
}

#[test]
fn overlong_auto_item_is_clipped_with_warning() {
    if skip() {
        return;
    }
    let long = "おはよう。".repeat(60);
    let body = format!("#paragraph[S01] 朝\n#visual (image) 朝。\n[S01-L001] A:{long}\n");
    let p = project(&body, &[("S01.png", Asset::Png(RED))], 30, "");
    assert_eq!(p.render(), Outcome::Warnings, "{}", p.log());
    assert!(p.output().is_file());
    assert!(p.log().contains("画面に収まりません"), "{}", p.log());
    // 枠(画面いっぱいの白)が出ている
    let l = &p.timeline().lines[0];
    let buf = frame_rgb(
        &p.output(),
        (frame(l.start_ms, 30) + frame(l.end_ms, 30)) / 2,
    );
    assert!(count_near(&buf, [255, 255, 255], 3) > 1000);
}

const SUBTITLE: &str = "style = \"subtitle\"";

#[test]
fn subtitle_style_with_four_line_line_refuses_and_exits_1() {
    if skip() {
        return;
    }
    // 4 行になる長い台詞が 2 件
    let long = "あ".repeat(60);
    let body = format!(
        "#paragraph[S01] 朝\n#visual (image) 朝。\n[S01-L001] A:{long}\n[S01-L002] A:おはよう。\n#pause 2000\n[S01-L003] A:{long}\n"
    );
    let p = project(&body, &[("S01.png", Asset::Png(RED))], 30, SUBTITLE);
    fs::write(p.output(), b"old").unwrap();
    let mtime = fs::metadata(p.output()).unwrap().modified().unwrap();
    assert_eq!(p.render(), Outcome::Warnings, "{}", p.log());
    assert_eq!(fs::read(p.output()).unwrap(), b"old");
    assert_eq!(fs::metadata(p.output()).unwrap().modified().unwrap(), mtime);
    assert!(!p.dir.join("work").join("render").exists());
    let log = p.log();
    for id in ["S01-L001", "S01-L003"] {
        let line = log
            .lines()
            .find(|l| l.contains(id) && l.contains("台詞が四行以上"))
            .unwrap_or_else(|| panic!("{id} の警告がありません\n{log}"));
        assert!(line.contains("A:"), "{line}");
        assert!(line.contains(&long), "{line}");
        assert!(line.contains("(4 行)") || line.contains("(5 行)"), "{line}");
    }
    assert!(!log.contains("S01-L002: 台詞が四行以上"), "{log}");
    assert!(
        log.contains("output.mp4 は作っていません(変更もしていません)"),
        "{log}"
    );
}

#[test]
fn subtitle_style_renders_with_three_line_line() {
    if skip() {
        return;
    }
    let p = project(SIMPLE, &[("S01.png", Asset::Png(RED))], 30, SUBTITLE);
    p.render_ok();
    assert!(p.output().is_file(), "{}", p.log());
    assert!(!p.log().contains("台詞が四行以上"), "{}", p.log());
}

/// 字幕の本文の 1 行に入る "あ" は 16 字(幅 320 - 余白 2*8 - 字幕の余白 2*20 = 264、文字 16)。
/// 3 行ちょうど(48 字)は出力でき、4 行になる最小(49 字)は止まる。
fn subtitle_boundary(chars: usize) -> Project {
    let t = "あ".repeat(chars);
    let body = format!(
        "#paragraph[S01] 朝
#visual (image) 朝。
[S01-L001] A:{t}
"
    );
    project(&body, &[("S01.png", Asset::Png(RED))], 30, SUBTITLE)
}

#[test]
fn subtitle_style_three_full_lines_render_and_four_lines_refuse() {
    if skip() {
        return;
    }
    let p = subtitle_boundary(48);
    p.render_ok();
    assert!(p.output().is_file(), "{}", p.log());
    assert!(!p.log().contains("台詞が四行以上"), "{}", p.log());

    let p = subtitle_boundary(49);
    assert_eq!(p.render(), Outcome::Warnings, "{}", p.log());
    assert!(!p.output().exists());
    assert!(p.log().contains("(4 行)"), "{}", p.log());
}
