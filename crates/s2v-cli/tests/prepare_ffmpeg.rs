//! `prepare` の結合テスト。実際の ffmpeg・ffprobe を使う。
//! ffmpeg が無い環境ではテストは失敗する。飛ばすのは `S2V_SKIP_FFMPEG_TESTS=1` のときだけ。

use std::fs;
use std::path::{Path, PathBuf};

use s2v_cli::layout::script_dir;
use s2v_cli::prepare::{preflight, prepare, MissingAssets};
use s2v_cli::proclog::ProcLog;
use s2v_cli::{run_build, BuildArgs, Outcome};

const HEADER: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中,pan=-30\n\n@script\n";
const SIMPLE: &str = "#paragraph[S01] 朝\n#visual (image) 朝の居間。\n[S01-L001] A:おはよう。\n";

fn skip() -> bool {
    std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")
}

fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..72_000usize {
        w.write_sample(0i16).unwrap();
    }
    w.finalize().unwrap();
}

/// 素材なしの台本フォルダを作り、build を通す。(一時フォルダ, 台本, 設定)
fn project_without_assets() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    fs::create_dir_all(dir.join("voices")).unwrap();
    fs::create_dir_all(dir.join("tracks")).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{SIMPLE}")).unwrap();
    write_voice(&dir.join("voices").join("S01_L001.wav"));
    let font = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("s2v-render/tests/fonts/test-vrt2.ttf");
    let base = include_str!("../../../config.toml");
    let a = base.find("\n[render]").unwrap();
    let b = base.find("\n[prompts]").unwrap();
    let render = format!(
        "width = 320\nheight = 180\nfps = 10\nfont_path = '{}'\nfont_size = 16.0\nbubble_padding = 6\nnote_padding = 4\nmargin = 8\nbubble_gap = 6\ntail_length = 12\npreset = \"ultrafast\"\ncrf = 10\n",
        font.display()
    );
    let config = dir.join("config.toml");
    fs::write(
        &config,
        format!("{}\n[render]\n{render}\n{}", &base[..a], &base[b..]),
    )
    .unwrap();
    let o = run_build(&BuildArgs {
        script: script.clone(),
        voice_dir: None,
        config: Some(config.clone()),
        no_cache: false,
    });
    assert!(o != Outcome::Failed, "build が失敗しました");
    (tmp, script, config)
}

#[test]
fn missing_assets_policy() {
    if skip() {
        return;
    }
    let (_tmp, script, config) = project_without_assets();
    let pf = preflight(&script, Some(&config)).unwrap();
    let dir = script_dir(&script);
    assert_eq!(dir, pf.dir);
    let mut log = ProcLog::from_writer(Box::new(Vec::<u8>::new()));

    let e = prepare(
        &script,
        &pf.config.render,
        &pf.dir,
        MissingAssets::Error,
        &mut log,
    )
    .err()
    .expect("素材なしは Err");
    assert!(e.contains("素材が足りません"), "{e}");

    let p = prepare(
        &script,
        &pf.config.render,
        &pf.dir,
        MissingAssets::Allow,
        &mut log,
    )
    .expect("Allow は Ok");
    assert_eq!(p.missing, vec!["S01-C01".to_string()]);
    assert!(p.assets.is_empty());
    assert!(
        p.warnings.iter().any(|w| w.contains("素材なし")),
        "{:?}",
        p.warnings
    );
}
