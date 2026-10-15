//! `prompts` コマンドの結合テスト(build を通した台本で確かめる)。

use std::fs;
use std::path::{Path, PathBuf};

use s2v_cli::{run_build, run_prompts, BuildArgs, Outcome, PromptsArgs};

const HEADER: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\n田中:田中,pan=-30\n30代の男性。紺のカーディガン。\n\n佐藤:佐藤,pan=30\n\n@script\n";

/// 2 シーン。S01 は画像のカットと動画(loop)のカット、S02 は #visual なし。
const BODY: &str = "#paragraph[S01] 朝の居間\n#visual (image) 朝の光。\n[S01-L001] 田中:おはよう。\n#cut[C02]\n#visual (video loop) 波。\n[S01-L002] 佐藤:早いね。\n\n#paragraph[S02] 駅前\n[S02-L001] 田中:行こう。\n";

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
}

impl Project {
    fn prompts(&self) -> PathBuf {
        self.dir.join("prompts")
    }
    fn run(&self) -> Outcome {
        run_prompts(&PromptsArgs {
            script: self.script.clone(),
            config: None,
        })
    }
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("Process.log")).unwrap_or_default()
    }
}

/// 440Hz の収録(render_ffmpeg.rs の write_voice と同じ: 200ms 無音 + 1000ms + 300ms 無音)。
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

/// 台本を書き、収録を置いて build する。`build` が false なら build しない。
fn project(body: &str, build: bool) -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("voices");
    fs::create_dir_all(&voices).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{body}")).unwrap();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((id, _)) = rest.split_once(']') {
                write_voice(&voices.join(format!("{}.wav", id.replace('-', "_"))));
            }
        }
    }
    if build {
        let o = run_build(&BuildArgs {
            script: script.clone(),
            voice_dir: None,
            config: None,
            no_cache: false,
        });
        assert!(o != Outcome::Failed, "build が失敗しました");
    }
    Project {
        _tmp: tmp,
        dir,
        script,
    }
}

#[test]
fn writes_scene_files_and_index() {
    let p = project(BODY, true);
    // S02 に #visual が無いので警告
    assert_eq!(p.run(), Outcome::Warnings, "{}", p.log());
    let s01 = fs::read_to_string(p.prompts().join("S01.md")).unwrap();
    assert!(s01.starts_with("# S01 朝の居間\n"), "{s01}");
    assert!(
        s01.contains("## S01-C01\n\n- 素材: `tracks/S01.png`\n- 種類: 画像\n"),
        "{s01}"
    );
    assert!(
        s01.contains(
            "## S01-C02\n\n- 素材: `tracks/S01_C02.mp4`\n- 種類: 動画(ループする)\n- 長さ: "
        ),
        "{s01}"
    );
    assert!(
        s01.contains("- 田中: 30代の男性。紺のカーディガン。\n"),
        "{s01}"
    );
    assert!(s01.contains("\n- 佐藤\n"), "{s01}");
    assert!(
        s01.contains("縦横比 16:9。画像の中に文字・吹き出しを入れない。\n"),
        "{s01}"
    );
    let s02 = fs::read_to_string(p.prompts().join("S02.md")).unwrap();
    assert!(
        s02.contains("- #visual: なし(台本に #visual を書いてください)\n"),
        "{s02}"
    );
    let index = fs::read_to_string(p.prompts().join("index.md")).unwrap();
    assert!(
        index.contains("| S01-C01 | `tracks/S01.png` | 画像 | "),
        "{index}"
    );
    assert!(index.contains("| なし |"), "{index}");
    assert!(
        p.log().contains("S02-C01: #visual がありません"),
        "{}",
        p.log()
    );
    assert!(p.log().contains("== prompts "), "{}", p.log());
}

#[test]
fn existing_asset_is_shown_in_index() {
    let p = project(BODY, true);
    let tracks = p.dir.join("tracks");
    fs::create_dir_all(&tracks).unwrap();
    fs::write(tracks.join("s01.JPG"), b"x").unwrap();
    p.run();
    let index = fs::read_to_string(p.prompts().join("index.md")).unwrap();
    assert!(index.contains("| あり(`s01.JPG`) |"), "{index}");
}

#[test]
fn all_visuals_present_is_ok() {
    let body = BODY.replace(
        "#paragraph[S02] 駅前\n",
        "#paragraph[S02] 駅前\n#visual 駅前の広場。\n",
    );
    let p = project(&body, true);
    assert_eq!(p.run(), Outcome::Ok, "{}", p.log());
}

#[test]
fn keeps_user_files_and_removes_stale_generated_ones() {
    // Review Focus 1
    let p = project(BODY, true);
    let dir = p.prompts();
    fs::create_dir_all(dir.join("下書き")).unwrap();
    for (name, body) in [
        ("memo.md", "メモ"),
        ("S01_note.md", "注"),
        ("S1x.md", "x"),
        ("S01.md.bak", "bak"),
        ("S09.md", "古いシーン"),
        ("index.md", "古い一覧"),
    ] {
        fs::write(dir.join(name), body).unwrap();
    }
    fs::write(dir.join("下書き").join("S01.md"), "下書き").unwrap();
    p.run();
    assert_eq!(fs::read_to_string(dir.join("memo.md")).unwrap(), "メモ");
    assert_eq!(fs::read_to_string(dir.join("S01_note.md")).unwrap(), "注");
    assert_eq!(fs::read_to_string(dir.join("S1x.md")).unwrap(), "x");
    assert_eq!(fs::read_to_string(dir.join("S01.md.bak")).unwrap(), "bak");
    assert_eq!(
        fs::read_to_string(dir.join("下書き").join("S01.md")).unwrap(),
        "下書き"
    );
    assert!(!dir.join("S09.md").exists(), "前回の生成ファイルは消える");
    assert!(fs::read_to_string(dir.join("index.md"))
        .unwrap()
        .starts_with("# 素材の一覧"));
}

#[test]
fn edited_script_is_a_warning_and_still_writes() {
    // Review Focus 2
    let body = BODY.replace(
        "#paragraph[S02] 駅前\n",
        "#paragraph[S02] 駅前\n#visual 駅前の広場。\n",
    );
    let p = project(&body, true);
    let mut text = fs::read_to_string(&p.script).unwrap();
    text.push('\n');
    fs::write(&p.script, text).unwrap();
    assert_eq!(p.run(), Outcome::Warnings, "{}", p.log());
    assert!(
        p.log().contains("build をやり直してください"),
        "{}",
        p.log()
    );
    assert!(p.prompts().join("S01.md").exists());
}

#[test]
fn missing_timeline_is_failed_without_writing() {
    // Review Focus 5
    let p = project(BODY, false);
    assert_eq!(p.run(), Outcome::Failed);
    assert!(!p.prompts().exists(), "prompts/ を作らない");
    assert!(p.log().contains("build を実行してください"), "{}", p.log());
}

#[test]
fn prompts_link_or_file_is_failed_without_deleting() {
    // Review Focus 5: prompts という名前のファイル
    let p = project(BODY, true);
    fs::write(p.prompts(), "ファイル").unwrap();
    assert_eq!(p.run(), Outcome::Failed);
    assert_eq!(fs::read_to_string(p.prompts()).unwrap(), "ファイル");

    // 台本フォルダの外を指すリンク: 中の生成名のファイルも消さない
    let p = project(BODY, true);
    let foreign = p._tmp.path().join("foreign");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("S01.md"), "外").unwrap();
    if make_dir_link(&p.prompts(), &foreign) {
        assert_eq!(p.run(), Outcome::Failed);
        assert_eq!(fs::read_to_string(foreign.join("S01.md")).unwrap(), "外");
    }
}

#[test]
fn prompts_link_to_inside_folder_is_failed_without_deleting() {
    // 台本フォルダの内側を指すリンクも拒否し、リンク先の生成名のファイルを消さない
    let p = project(BODY, true);
    let inner = p.dir.join("elsewhere");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join("S01.md"), "内").unwrap();
    fs::write(inner.join("S09.md"), "内2").unwrap();
    if make_dir_link(&p.prompts(), &inner) {
        assert_eq!(p.run(), Outcome::Failed);
        assert_eq!(fs::read_to_string(inner.join("S01.md")).unwrap(), "内");
        assert_eq!(fs::read_to_string(inner.join("S09.md")).unwrap(), "内2");
    }
}

#[test]
fn write_failure_keeps_existing_generated_files() {
    // S01.md という名前のフォルダがあると書き込みが失敗する。その時点で古い S09.md は消えない
    let p = project(BODY, true);
    let dir = p.prompts();
    fs::create_dir_all(dir.join("S01.md")).unwrap();
    fs::write(dir.join("S09.md"), "古いシーン").unwrap();
    fs::write(dir.join("index.md"), "古い一覧").unwrap();
    assert_eq!(p.run(), Outcome::Failed, "{}", p.log());
    assert_eq!(
        fs::read_to_string(dir.join("S09.md")).unwrap(),
        "古いシーン"
    );
    assert!(dir.join("S01.md").is_dir());
}

/// timeline.json の時刻をすべて 0 にする(`duration_ms` が 0 の台本を再現する)。
fn zero_all_times(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if matches!(k.as_str(), "start_ms" | "end_ms" | "duration_ms") {
                    *x = serde_json::json!(0);
                } else {
                    zero_all_times(x);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(zero_all_times),
        _ => {}
    }
}

#[test]
fn zero_length_script_writes_only_index() {
    // build は台詞も #pause も無いシーンを受け付けない(「無言のシーンには #pause が必要です」)ので、
    // 通常の台本を build してから timeline.json の時刻をすべて 0 にして、duration_ms = 0 を再現する
    let p = project(BODY, true);
    let tl_path = p.dir.join("timeline").join("timeline.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&tl_path).unwrap()).unwrap();
    zero_all_times(&mut v);
    fs::write(&tl_path, serde_json::to_string(&v).unwrap()).unwrap();
    p.run();
    assert!(p.prompts().join("index.md").exists(), "{}", p.log());
    assert!(!p.prompts().join("S01.md").exists());
    assert!(!p.prompts().join("S02.md").exists());
    let index = fs::read_to_string(p.prompts().join("index.md")).unwrap();
    assert!(index.contains("不要(長さ 0)"), "{index}");
}

/// ディレクトリのリンクを作る(build_golden.rs の make_dir_link と同じ)。
/// Windows では mklink /J に権限は要らないので、作れなければテストの準備の不具合として止める。
fn make_dir_link(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "ディレクトリのリンクを作れません: {}", link.display());
        true
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
