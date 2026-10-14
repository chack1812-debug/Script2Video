use std::fs;
use std::path::{Path, PathBuf};

use s2v_cli::{run_build, BuildArgs, Outcome};

const SCRIPT: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中,pan=-30\n\nB:佐藤,pan=30\n\n@script\n#paragraph[S01] 朝\n#visual (image) 朝の居間。\n[S01-L001] A:おはよう。\n[S01-L002] B:おはよう。\n[S01-L003] A(overlap=200):今日は早いね。\n#paragraph[S02] 夜\n#pause 2000\n#paragraph[S03]\n[S03-L001] B:おやすみ。\n";

/// 200ms 無音 + 1000ms の 440Hz(振幅 0.1)+ 300ms 無音、16bit モノラル 48kHz。
fn write_voice(path: &Path) {
    write_samples(path, &voice_samples(0.1, None));
}

fn voice_samples(amp: f64, click_at: Option<usize>) -> Vec<f64> {
    (0..72_000)
        .map(|i| {
            if let Some(c) = click_at {
                if (c..c + 10).contains(&i) {
                    return 0.99;
                }
            }
            if (9_600..57_600).contains(&i) {
                amp * (2.0 * std::f64::consts::PI * 440.0 * (i - 9_600) as f64 / 48_000.0).sin()
            } else {
                0.0
            }
        })
        .collect()
}

fn write_samples(path: &Path, data: &[f64]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &v in data {
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    voices: PathBuf,
}

fn fixture(script: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("voices");
    fs::create_dir_all(&voices).unwrap();
    let script_path = dir.join("第1話.txt");
    fs::write(&script_path, script).unwrap();
    Fixture {
        _tmp: tmp,
        dir,
        script: script_path,
        voices,
    }
}

fn args(f: &Fixture, config: Option<PathBuf>, no_cache: bool) -> BuildArgs {
    BuildArgs {
        script: f.script.clone(),
        voice_dir: Some(f.voices.clone()),
        config,
        no_cache,
    }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn wav_frames(path: &Path) -> u32 {
    hound::WavReader::open(path).unwrap().duration()
}

#[test]
fn golden_timeline_and_outputs() {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S03_L001"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    assert_eq!(
        run_build(&args(&f, None, false)),
        Outcome::Warnings,
        "L003 が不足"
    );

    let out = f.dir.clone();
    let v = read_json(&out.join("timeline").join("timeline.json"));
    assert_eq!(v["schema_version"], 2);
    assert_eq!(v["script"], "第1話.txt");
    let ms = |i: usize, k: &str| v["lines"][i][k].as_i64().unwrap();
    assert_eq!((ms(0, "start_ms"), ms(0, "end_ms")), (0, 1_110));
    assert_eq!((ms(1, "start_ms"), ms(1, "end_ms")), (1_410, 2_520));
    assert_eq!((ms(2, "start_ms"), ms(2, "end_ms")), (2_320, 3_720));
    assert_eq!((ms(3, "start_ms"), ms(3, "end_ms")), (6_720, 7_830));
    assert_eq!((ms(0, "src_start_ms"), ms(0, "src_end_ms")), (170, 1_280));
    assert_eq!(v["lines"][2]["missing"], true);
    assert_eq!(v["lines"][2]["reason"], "no_file");
    assert!(v["lines"][2]["measured_lufs"].is_null());
    let measured = v["lines"][0]["measured_lufs"].as_f64().unwrap();
    // 振幅 0.1 の正弦波は平均二乗 0.005(-23.01 dB)。LUFS = -0.691 + 10*log10(平均二乗) なので約 -23.70
    // (440Hz では K 特性の増減はほぼ 0)。
    let expected = -0.691 + 10.0 * (0.1f64 * 0.1 / 2.0).log10();
    assert!(
        (measured - expected).abs() < 0.1,
        "{measured} vs {expected}"
    );
    assert_eq!(
        v["lines"][0]["norm_gain_db"],
        v["lines"][0]["requested_gain_db"]
    );
    assert_eq!(v["scenes"][1]["silent"], true);
    assert_eq!(
        (
            v["scenes"][1]["start_ms"].as_i64(),
            v["scenes"][1]["end_ms"].as_i64()
        ),
        (Some(3_720), Some(5_720))
    );
    assert_eq!(v["scenes"][2]["id"], "S03");
    assert_eq!(v["scenes"][0]["visual"]["text"], "朝の居間。");
    assert_eq!(v["takes"][0]["trims"][0]["head_ms"], 170);
    assert_eq!(v["takes"][0]["trims"][0]["tail_ms"], 220);
    assert_eq!(v["takes"][2]["files"].as_array().unwrap().len(), 0);
    assert!(v["takes"][2]["trims"].is_null());

    // 時刻の一致: duration_ms = full_dialogue の長さ(切り上げ)
    let frames = wav_frames(&out.join("full_dialogue.wav")) as i64;
    assert_eq!(v["duration_ms"].as_i64().unwrap(), (frames + 47) / 48);
    assert!(frames >= 7_830 * 48);
    for t in ["dialogue", "bgm", "se"] {
        assert_eq!(
            wav_frames(&out.join("tracks").join(format!("{t}.wav"))) as i64,
            frames
        );
    }
    let srt = fs::read_to_string(out.join("timeline").join("subtitles.srt")).unwrap();
    assert!(
        srt.starts_with("1\n00:00:00,000 --> 00:00:01,110\nおはよう。\n"),
        "{srt}"
    );
    assert!(srt.contains("00:00:02,320 --> 00:00:03,720\n今日は早いね。"));
    let ticks = (frames * 5 + 7) / 8;
    assert!(
        fs::read_to_string(out.join("timeline").join("timeline.fcpxml"))
            .unwrap()
            .contains(&format!("duration=\"{ticks}/30000s\""))
    );
    assert!(fs::read_to_string(out.join("Process.log"))
        .unwrap()
        .contains("S01-L003"));
    assert_eq!(
        fs::read_dir(out.join("work").join("lines"))
            .unwrap()
            .count(),
        3
    );

    // キャッシュあり・なしで同じ結果
    let first = fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    assert_eq!(
        fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap(),
        first
    );
    assert_eq!(run_build(&args(&f, None, true)), Outcome::Warnings);
    assert_eq!(
        fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap(),
        first
    );

    // 元の収録ファイルは変わらない
    let again = f.dir.join("比較.wav");
    write_voice(&again);
    assert_eq!(
        fs::read(f.voices.join("S01_L001.wav")).unwrap(),
        fs::read(&again).unwrap()
    );
}

#[test]
fn same_line_number_in_two_scenes_builds_distinct_lines_and_takes() {
    let f = fixture(
        "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L001] B:二\n[S02-L002] A(at=L001+100):三\n#se pop.wav at=S01-L001+50\n",
    );
    write_voice(&f.voices.join("S01_L001.wav"));
    write_voice(&f.voices.join("S02_L001.wav"));
    write_voice(&f.voices.join("S02_L002.wav"));
    let outcome = run_build(&args(&f, None, false));
    assert_ne!(outcome, Outcome::Failed);
    let v = read_json(&f.dir.join("timeline").join("timeline.json"));
    let lines = v["lines"].as_array().unwrap();
    let ids: Vec<_> = lines.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["S01-L001", "S02-L001", "S02-L002"]);
    let takes: Vec<_> = lines.iter().map(|l| l["take"].as_str().unwrap()).collect();
    assert_eq!(takes, ["S01_L001", "S02_L001", "S02_L002"]);
    // どの台詞にも収録ファイルが対応している(別シーンの同番号を取り違えない)
    assert!(lines.iter().all(|l| l["missing"] != true), "{lines:?}");
    // at=L001+100 は S02-L001 の開始に対する差分(S01-L001 ではない)
    let start = |i: usize| lines[i]["start_ms"].as_i64().unwrap();
    assert_eq!(start(2), start(1) + 100);
    assert!(start(1) > start(0));
}

#[test]
fn empty_voice_dir_produces_placeholders() {
    let f = fixture(SCRIPT);
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    let out = f.dir.clone();
    let v = read_json(&out.join("timeline").join("timeline.json"));
    assert!(v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["missing"] == true));
    assert!(out.join("full_dialogue.wav").exists());
    assert!(
        fs::read_to_string(out.join("timeline").join("subtitles.srt"))
            .unwrap()
            .contains("おやすみ。")
    );
}

#[test]
fn loud_click_is_limited_only_at_master() {
    let f = fixture("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:あ\n");
    // 小さな声(振幅 0.01)の中に 0dBFS 近いクリック: 正規化で大きく増幅される
    write_samples(
        &f.voices.join("S01_L001.wav"),
        &voice_samples(0.01, Some(30_000)),
    );
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    let out = f.dir.clone();
    let mut d = hound::WavReader::open(out.join("tracks").join("dialogue.wav")).unwrap();
    let peak_track = d
        .samples::<f32>()
        .map(|s| s.unwrap().abs())
        .fold(0.0f32, f32::max);
    assert!(peak_track > 1.0, "{peak_track}");
    let mut m = hound::WavReader::open(out.join("full_dialogue.wav")).unwrap();
    let peak_master =
        m.samples::<i32>().map(|s| s.unwrap().abs()).max().unwrap() as f64 / 8_388_607.0;
    assert!(
        peak_master <= 10f64.powf(-1.0 / 20.0) + 1e-6,
        "{peak_master}"
    );
}

#[test]
fn out_of_range_config_fails_without_output() {
    let f = fixture(SCRIPT);
    let cfg = f.dir.join("bad.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    fs::write(
        &cfg,
        builtin.replace("target_lufs = -23.0", "target_lufs = -5.0"),
    )
    .unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Failed);
    assert_untouched(&f.dir);
}

#[test]
fn syntax_error_fails_without_output() {
    let f =
        fixture("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] C:だれ？\n");
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert_untouched(&f.dir);
}

const ONE_LINE: &str = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:あ\n";

#[test]
fn stereo_silent_and_duplicate_recordings_become_missing_without_panic() {
    let f = fixture(
        "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:あ\n[S01-L002] A:い\n[S01-L003] A:う\n",
    );
    // L001: ステレオ(形式不正)
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(f.voices.join("S01_L001.wav"), spec).unwrap();
    for _ in 0..4_800 {
        w.write_sample(1000i16).unwrap();
        w.write_sample(1000i16).unwrap();
    }
    w.finalize().unwrap();
    // L002: 完全な無音
    write_samples(&f.voices.join("S01_L002.wav"), &vec![0.0; 48_000]);
    // L003: .wav と .flac が重複(flac は許可されていなくても重複扱いにならない場合があるため中身は問わない)
    write_voice(&f.voices.join("S01_L003.wav"));
    fs::write(f.voices.join("S01_L003.flac"), b"not a flac").unwrap();
    // 範囲を示す名前は無視される
    write_voice(&f.voices.join("S01_L001-L003.wav"));
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    let v = read_json(&f.dir.clone().join("timeline").join("timeline.json"));
    assert_eq!(v["lines"][0]["reason"], "bad_format");
    assert_eq!(v["lines"][1]["reason"], "silent");
    assert!(fs::read_to_string(f.dir.clone().join("Process.log"))
        .unwrap()
        .contains("[WARN]"));
}

#[test]
fn silent_scene_without_speech_lines_does_not_panic() {
    let f = fixture("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n#pause 1000\n");
    let r = run_build(&args(&f, None, false));
    assert_ne!(r, Outcome::Failed);
    assert!(f.dir.clone().join("full_dialogue.wav").exists());
}

#[test]
fn work_files_of_lines_that_became_missing_are_removed_and_warnings_replay() {
    let f = fixture(ONE_LINE);
    let wav = f.voices.join("S01_L001.wav");
    write_voice(&wav);
    // 増幅の上限を 0.2 dB にして、正規化が制限に掛かる(警告が出る)ようにする
    let cfg = f.dir.join("limit.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    assert!(builtin.contains("max_boost_db = 24.0"));
    fs::write(
        &cfg,
        builtin.replace("max_boost_db = 24.0", "max_boost_db = 0.2"),
    )
    .unwrap();
    let a = args(&f, Some(cfg), false);
    let first = run_build(&a);
    assert_eq!(first, Outcome::Warnings);
    let out = f.dir.clone();
    let log1 = fs::read_to_string(out.join("Process.log")).unwrap();
    assert!(log1.contains("制限に掛かりました"), "{log1}");
    assert_eq!(run_build(&a), first, "キャッシュ命中でも同じ終了コード");
    let log2 = fs::read_to_string(out.join("Process.log")).unwrap();
    assert_eq!(
        log1.matches("[WARN]").count(),
        log2.matches("[WARN]").count()
    );
    assert!(
        log2.contains("制限に掛かりました"),
        "キャッシュ命中でも警告を再現する"
    );
    assert_eq!(
        fs::read_dir(out.join("work").join("lines"))
            .unwrap()
            .count(),
        1
    );
    // 録音が無音に差し替わる: lines/ と resampled/ の中間ファイルは消える
    write_samples(&wav, &vec![0.0; 48_000]);
    assert_eq!(run_build(&a), Outcome::Warnings);
    assert_eq!(
        fs::read_dir(out.join("work").join("lines"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(out.join("work").join("resampled"))
            .unwrap()
            .count(),
        0
    );
}

/// 失敗しても何も作らない(Process.log・work・出力フォルダを含む)。
fn assert_untouched(dir: &Path) {
    for n in [
        "Process.log",
        "work",
        "tracks",
        "timeline",
        "full_dialogue.wav",
    ] {
        assert!(!dir.join(n).exists(), "{n} が作られています");
    }
}

const FINAL_FILES: [&str; 7] = [
    "full_dialogue.wav",
    "timeline/timeline.json",
    "timeline/subtitles.srt",
    "timeline/timeline.fcpxml",
    "tracks/dialogue.wav",
    "tracks/bgm.wav",
    "tracks/se.wav",
];

fn assert_no_final_artifacts(out: &Path) {
    for n in FINAL_FILES {
        assert!(!out.join(n).exists(), "古い成果物が残っています: {n}");
    }
}

fn assert_all_final_artifacts(out: &Path) {
    for n in FINAL_FILES {
        assert!(out.join(n).exists(), "成果物がありません: {n}");
    }
}

/// シーンの開始が前のシーンより早くなる(配置エラーになる)台本。
const REGRESSION_SCRIPT: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L001] B(overlap=3500):二\n";
const REGRESSION_OK_SCRIPT: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L001] B:二\n";

#[test]
fn placement_error_after_good_build_removes_stale_artifacts() {
    let f = fixture(REGRESSION_OK_SCRIPT);
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    let out = f.dir.clone();
    assert_all_final_artifacts(&out);
    fs::write(&f.script, REGRESSION_SCRIPT).unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert_no_final_artifacts(&out);
    let log = fs::read_to_string(out.join("Process.log")).unwrap();
    assert!(log.contains("[ERROR]") && log.contains("S02"), "{log}");
    assert!(out.join("work").exists(), "work/ には触れない");
}

#[test]
fn io_failure_after_output_starts_leaves_no_stale_artifacts() {
    let f = fixture(SCRIPT);
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    let out = f.dir.clone();
    // tracks を「ファイル」にして、トラックの書き込みを失敗させる
    fs::remove_dir_all(out.join("tracks")).unwrap();
    fs::write(out.join("tracks"), b"x").unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert_no_final_artifacts(&out);
    let log = fs::read_to_string(out.join("Process.log")).unwrap();
    assert!(log.contains("[ERROR]"), "{log}");
}

#[test]
fn syntax_error_rerun_leaves_previous_outputs_untouched() {
    let f = fixture(ONE_LINE);
    write_voice(&f.voices.join("S01_L001.wav"));
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    let out = f.dir.clone();
    let before: Vec<Vec<u8>> = FINAL_FILES
        .iter()
        .map(|n| fs::read(out.join(n)).unwrap())
        .collect();
    let log_before = fs::read(out.join("Process.log")).unwrap();
    fs::write(
        &f.script,
        "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] C:だれ？\n",
    )
    .unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    for (n, b) in FINAL_FILES.iter().zip(before) {
        assert_eq!(fs::read(out.join(n)).unwrap(), b, "{n}");
    }
    assert_eq!(fs::read(out.join("Process.log")).unwrap(), log_before);
    // 設定の範囲外でも同じ
    let cfg = f.dir.join("bad.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    fs::write(
        &cfg,
        builtin.replace("target_lufs = -23.0", "target_lufs = -5.0"),
    )
    .unwrap();
    fs::write(&f.script, ONE_LINE).unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Failed);
    assert_all_final_artifacts(&out);
}

#[test]
fn missing_voice_dir_is_failed_and_logged() {
    let f = fixture(ONE_LINE);
    let mut a = args(&f, None, false);
    a.voice_dir = Some(f.dir.join("無い フォルダ"));
    assert_eq!(run_build(&a), Outcome::Failed);
    let log = fs::read_to_string(f.dir.clone().join("Process.log")).unwrap();
    assert!(
        log.contains("[ERROR]") && log.contains("無い フォルダ"),
        "{log}"
    );
    assert_no_final_artifacts(&f.dir.clone());
}

#[test]
fn voice_dir_that_is_a_file_is_failed_and_logged() {
    let f = fixture(ONE_LINE);
    let file = f.dir.join("収録.txt");
    fs::write(&file, b"x").unwrap();
    let mut a = args(&f, None, false);
    a.voice_dir = Some(file);
    assert_eq!(run_build(&a), Outcome::Failed);
    let log = fs::read_to_string(f.dir.clone().join("Process.log")).unwrap();
    assert!(log.contains("[ERROR]") && log.contains("収録.txt"), "{log}");
}

#[test]
fn duplicate_wav_and_flac_is_missing_duplicate() {
    let f = fixture(ONE_LINE);
    let cfg = f.dir.join("flac.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    assert!(builtin.contains("allow_flac = false"));
    fs::write(
        &cfg,
        builtin.replace("allow_flac = false", "allow_flac = true"),
    )
    .unwrap();
    write_voice(&f.voices.join("S01_L001.wav"));
    fs::write(f.voices.join("S01_L001.flac"), b"not a flac").unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Warnings);
    let out = f.dir.clone();
    let v = read_json(&out.join("timeline").join("timeline.json"));
    assert_eq!(v["lines"][0]["missing"], true);
    assert_eq!(v["lines"][0]["reason"], "duplicate");
    let take = &v["takes"][0];
    assert_eq!(
        take["files"],
        serde_json::json!(["S01_L001.flac", "S01_L001.wav"])
    );
    assert!(take["trims"].is_null());
    assert_eq!(
        fs::read_dir(out.join("work").join("resampled"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(out.join("work").join("lines"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn bom_and_crlf_script_gives_same_timeline_as_lf() {
    let lf = fixture(SCRIPT);
    let crlf = fixture(SCRIPT);
    for f in [&lf, &crlf] {
        for id in ["S01_L001", "S01_L002", "S03_L001"] {
            write_voice(&f.voices.join(format!("{id}.wav")));
        }
    }
    let converted = format!("\u{feff}{}", SCRIPT.replace('\n', "\r\n"));
    fs::write(&crlf.script, converted).unwrap();
    assert_eq!(run_build(&args(&lf, None, false)), Outcome::Warnings);
    assert_eq!(run_build(&args(&crlf, None, false)), Outcome::Warnings);
    let a = read_json(&lf.dir.clone().join("timeline").join("timeline.json"));
    let b = read_json(&crlf.dir.clone().join("timeline").join("timeline.json"));
    assert_eq!(a["lines"], b["lines"]);
    assert_eq!(a["scenes"], b["scenes"]);
    assert_eq!(a["duration_ms"], b["duration_ms"]);
}

/// dump_prefade = true で 3 台詞を build し、出力フォルダを返す。
fn prefade_build() -> (Fixture, PathBuf) {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S03_L001"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    let cfg = f.dir.join("prefade.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    fs::write(
        &cfg,
        builtin.replace("dump_prefade = false", "dump_prefade = true"),
    )
    .unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Warnings);
    let out = f.dir.clone();
    (f, out)
}

fn edit_cache(out: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = out.join("work").join("cache.json");
    let mut v = read_json(&path);
    edit(&mut v);
    fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
}

#[test]
fn verify_tool_passes_on_a_prefade_build() {
    let (_f, out) = prefade_build();
    assert_eq!(
        fs::read_dir(out.join("work").join("lines_prefade"))
            .unwrap()
            .count(),
        3
    );
    let checks = s2v_cli::verify::verify_loudness(&out).unwrap();
    assert_eq!(checks.len(), 3);
    assert!(checks.iter().all(|c| c.passed), "{checks:?}");
}

#[test]
fn verify_rejects_inconsistent_cache_without_panic() {
    let (_f, out) = prefade_build();
    // 測定範囲が使用範囲より前にある(引き算が負になる)
    edit_cache(&out, |v| {
        let n = &mut v["entries"]["lines/S01-L001.wav"]["norm"];
        n["src_start"] = serde_json::json!(1_000_000u64);
        n["measure_start"] = serde_json::json!(10u64);
        n["measure_end"] = serde_json::json!(40_000u64);
    });
    assert!(s2v_cli::verify::verify_loudness(&out).is_err());
}

#[test]
fn verify_rejects_measure_range_beyond_prefade_file() {
    let (_f, out) = prefade_build();
    edit_cache(&out, |v| {
        let n = &mut v["entries"]["lines/S01-L001.wav"]["norm"];
        let s = n["src_start"].as_u64().unwrap();
        n["measure_start"] = serde_json::json!(s);
        n["measure_end"] = serde_json::json!(s + 900_000_000u64);
    });
    assert!(s2v_cli::verify::verify_loudness(&out).is_err());
}

#[test]
fn verify_errors_when_prefade_file_is_missing() {
    let (_f, out) = prefade_build();
    fs::remove_file(out.join("work").join("lines_prefade").join("S01-L001.wav")).unwrap();
    let e = s2v_cli::verify::verify_loudness(&out).unwrap_err();
    assert!(format!("{e:#}").contains("dump_prefade"), "{e:#}");
}

#[test]
fn verify_errors_when_no_lines_can_be_verified() {
    let (_f, out) = prefade_build();
    edit_cache(&out, |v| v["entries"] = serde_json::json!({}));
    assert!(s2v_cli::verify::verify_loudness(&out).is_err());
}

#[test]
fn verify_ignores_resampled_entries() {
    let (_f, out) = prefade_build();
    let n = s2v_cli::verify::verify_loudness(&out).unwrap().len();
    edit_cache(&out, |v| {
        let e = v["entries"]["lines/S01-L001.wav"].clone();
        v["entries"]["resampled/S01-L001.wav"] = e;
    });
    assert_eq!(s2v_cli::verify::verify_loudness(&out).unwrap().len(), n);
}

#[test]
fn verify_fails_short_line_with_wrong_gain() {
    let (_f, out) = prefade_build();
    edit_cache(&out, |v| {
        let n = &mut v["entries"]["lines/S01-L001.wav"]["norm"];
        let s = n["measure_start"].as_u64().unwrap();
        n["measure_end"] = serde_json::json!(s + 1000u64);
        n["norm_clamped"] = serde_json::json!(false);
        let g = n["norm_gain_db"].as_f64().unwrap();
        n["norm_gain_db"] = serde_json::json!(g + 3.0);
    });
    let checks = s2v_cli::verify::verify_loudness(&out).unwrap();
    let c = checks.iter().find(|c| c.id == "S01-L001").unwrap();
    assert_eq!(c.kind, s2v_cli::verify::CheckKind::ShortLine);
    assert!(!c.passed, "{c:?}");
}

#[test]
fn verify_fails_long_line_with_tampered_prefade() {
    let (_f, out) = prefade_build();
    let p = out.join("work").join("lines_prefade").join("S01-L002.wav");
    let mut r = hound::WavReader::open(&p).unwrap();
    let spec = r.spec();
    let s: Vec<f32> = r.samples::<f32>().map(|x| x.unwrap() * 0.25).collect();
    drop(r);
    let mut w = hound::WavWriter::create(&p, spec).unwrap();
    for x in s {
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
    let checks = s2v_cli::verify::verify_loudness(&out).unwrap();
    let c = checks.iter().find(|c| c.id == "S01-L002").unwrap();
    assert_eq!(c.kind, s2v_cli::verify::CheckKind::Measured);
    assert!(!c.passed, "{c:?}");
}

fn set_norm(out: &Path, set: impl FnOnce(&mut serde_json::Value)) {
    edit_cache(
        out,
        |v| set(&mut v["entries"]["lines/S01-L001.wav"]["norm"]),
    );
}

#[test]
fn verify_rejects_huge_or_inverted_positions_without_panic() {
    let max = serde_json::json!(u64::MAX);
    let cases: Vec<(u64, u64, u64)> = vec![
        // (src_start, measure_start, measure_end)
        (0, 0, u64::MAX),
        (0, 30_000, 10_000),
        (1_000_000, 10, 40_000),
        (0, u64::MAX - 30_000, u64::MAX),
        (0, 0, 900_000_000),
    ];
    for (src, ms, me) in cases {
        let (_f, out) = prefade_build();
        set_norm(&out, |n| {
            n["src_start"] = serde_json::json!(src);
            n["measure_start"] = serde_json::json!(ms);
            n["measure_end"] = serde_json::json!(me);
            n["norm_clamped"] = serde_json::json!(false);
        });
        assert!(
            s2v_cli::verify::verify_loudness(&out).is_err(),
            "{src} {ms} {me} {max}"
        );
    }
}

fn cli(args: &[&str]) -> (u8, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    s2v_cli::verify::run_cli(&a)
}

#[test]
fn verify_cli_exit_codes() {
    assert_eq!(cli(&[]).0, 2);
    let (_f, out) = prefade_build();
    let missing = out.join("存在しない");
    assert_eq!(cli(&[missing.to_str().unwrap()]).0, 2);

    let (code, text) = cli(&[out.to_str().unwrap()]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("3 件中 0 件が不合格"), "{text}");

    // 長い台詞の prefade を改ざん -> 1
    let p = out.join("work").join("lines_prefade").join("S01-L002.wav");
    let mut r = hound::WavReader::open(&p).unwrap();
    let spec = r.spec();
    let s: Vec<f32> = r.samples::<f32>().map(|x| x.unwrap() * 0.25).collect();
    drop(r);
    let mut w = hound::WavWriter::create(&p, spec).unwrap();
    for x in s {
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
    let (code, text) = cli(&[out.to_str().unwrap()]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("NG "), "{text}");
}

fn no_voice_args(f: &Fixture) -> BuildArgs {
    BuildArgs {
        script: f.script.clone(),
        voice_dir: None,
        config: None,
        no_cache: false,
    }
}

#[test]
fn default_voices_folder_is_used_when_voice_dir_is_omitted() {
    let f = fixture(ONE_LINE);
    write_voice(&f.dir.join("voices").join("S01_L001.wav"));
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Ok);
    let v = read_json(&f.dir.join("timeline").join("timeline.json"));
    assert_eq!(v["lines"][0]["missing"], false);
}

#[test]
fn explicit_voice_dir_overrides_default() {
    let f = fixture(ONE_LINE);
    let other = f.dir.join("別の 収録");
    fs::create_dir_all(&other).unwrap();
    write_voice(&other.join("S01_L001.wav"));
    // 既定の voices/ は空: 明示した方が使われる
    let mut a = no_voice_args(&f);
    a.voice_dir = Some(other);
    assert_eq!(run_build(&a), Outcome::Ok);
    // 明示しなければ空の voices/ を見て不足になる
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Warnings);
}

#[test]
fn missing_default_voices_folder_is_failed_with_clear_message() {
    let f = fixture(ONE_LINE);
    fs::remove_dir_all(&f.voices).unwrap();
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Failed);
    let log = fs::read_to_string(f.dir.join("Process.log")).unwrap();
    assert!(
        log.contains("voices") && log.contains("--voice-dir") && log.contains("ありません"),
        "{log}"
    );
    assert_no_final_artifacts(&f.dir);
}

#[test]
fn failed_rerun_keeps_output_mp4_and_voices() {
    let f = fixture(REGRESSION_OK_SCRIPT);
    write_voice(&f.voices.join("S01_L001.wav"));
    fs::write(f.dir.join("output.mp4"), b"mp4").unwrap();
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Warnings);
    fs::write(&f.script, REGRESSION_SCRIPT).unwrap();
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Failed);
    assert_no_final_artifacts(&f.dir);
    assert_eq!(fs::read(f.dir.join("output.mp4")).unwrap(), b"mp4");
    assert!(f.voices.join("S01_L001.wav").exists());
    assert!(f.dir.join("work").exists());
}

#[test]
fn voice_dir_clashing_with_output_folders_is_rejected_before_any_write() {
    for name in ["work", "tracks", "timeline"] {
        let f = fixture(ONE_LINE);
        let clash = f.dir.join(name);
        fs::create_dir_all(&clash).unwrap();
        let mut a = no_voice_args(&f);
        a.voice_dir = Some(clash.clone());
        assert_eq!(run_build(&a), Outcome::Failed, "{name}");
        assert!(!f.dir.join("Process.log").exists(), "{name}");
        assert_eq!(fs::read_dir(&clash).unwrap().count(), 0, "{name}");
        // 別の書き方(末尾の ./ など)でも同じ場所は拒否する
        a.voice_dir = Some(f.dir.join("voices").join("..").join(name));
        assert_eq!(run_build(&a), Outcome::Failed, "{name} (..)");
        assert!(!f.dir.join("Process.log").exists(), "{name}");
    }
}

#[test]
fn script_named_like_an_artifact_is_rejected_before_any_write() {
    for name in ["full_dialogue.wav", "Process.log", "output.mp4"] {
        let f = fixture(ONE_LINE);
        let script = f.dir.join(name);
        fs::write(&script, ONE_LINE).unwrap();
        let mut a = no_voice_args(&f);
        a.script = script.clone();
        assert_eq!(run_build(&a), Outcome::Failed, "{name}");
        assert_eq!(fs::read_to_string(&script).unwrap(), ONE_LINE, "{name}");
        assert!(!f.dir.join("work").exists(), "{name}");
        assert!(!f.dir.join("timeline").exists(), "{name}");
    }
    // voices.txt などは問題ない
    let f = fixture(ONE_LINE);
    let script = f.dir.join("voices.txt");
    fs::write(&script, ONE_LINE).unwrap();
    let mut a = no_voice_args(&f);
    a.script = script;
    assert_eq!(run_build(&a), Outcome::Warnings);
}

#[test]
fn verify_loudness_reads_the_script_folder_layout() {
    let f = fixture(ONE_LINE);
    write_voice(&f.voices.join("S01_L001.wav"));
    let cfg = f.dir.join("prefade.toml");
    let builtin =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml"))
            .unwrap();
    assert!(builtin.contains("dump_prefade = false"));
    fs::write(
        &cfg,
        builtin.replace("dump_prefade = false", "dump_prefade = true"),
    )
    .unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Ok);
    let (code, text) = s2v_cli::verify::run_cli(&[f.dir.to_string_lossy().into_owned()]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("S01-L001"), "{text}");
}

#[test]
fn reserved_script_names_are_rejected_case_insensitively() {
    for name in [
        "PROCESS.LOG",
        "Process.Log",
        "FULL_DIALOGUE.WAV",
        "Full_Dialogue.wav",
        "Output.MP4",
        "OUTPUT.mp4",
    ] {
        let f = fixture(ONE_LINE);
        let script = f.dir.join(name);
        fs::write(&script, ONE_LINE).unwrap();
        let mut a = no_voice_args(&f);
        a.script = script.clone();
        assert_eq!(run_build(&a), Outcome::Failed, "{name}");
        assert_eq!(fs::read_to_string(&script).unwrap(), ONE_LINE, "{name}");
        assert!(!f.dir.join("work").exists(), "{name}");
        assert!(!f.dir.join("timeline").exists(), "{name}");
    }
}

#[test]
fn relative_and_case_variant_voice_dirs_are_rejected_before_any_write() {
    let cases: Vec<(&str, PathBuf)> = vec![
        ("WORK", PathBuf::from("WORK")),
        ("Tracks/", PathBuf::from("Tracks").join("")),
        ("./timeline", PathBuf::from(".").join("timeline")),
        (
            "work/../work",
            PathBuf::from("work").join("..").join("work"),
        ),
    ];
    for (label, rel) in cases {
        let f = fixture(ONE_LINE);
        // 相対パスはカレントディレクトリ基準。テストではカレントを変えず、絶対パスに直して渡す純粋関数のテストは layout.rs にある。
        // ここでは絶対パスの大文字違いだけを確かめる。
        let abs = f.dir.join(&rel);
        let mut a = no_voice_args(&f);
        a.voice_dir = Some(abs);
        assert_eq!(run_build(&a), Outcome::Failed, "{label}");
        assert!(!f.dir.join("Process.log").exists(), "{label}");
        assert!(!f.dir.join("work").exists(), "{label}");
    }
}

#[test]
fn default_voices_that_is_a_file_gives_the_same_guidance_as_missing() {
    let f = fixture(ONE_LINE);
    fs::remove_dir_all(&f.voices).unwrap();
    fs::write(&f.voices, b"x").unwrap();
    assert_eq!(run_build(&no_voice_args(&f)), Outcome::Failed);
    let log = fs::read_to_string(f.dir.join("Process.log")).unwrap();
    assert!(
        log.contains("voices") && log.contains("--voice-dir"),
        "{log}"
    );
}

/// ディレクトリのリンクを作る。作れなければ false。
fn try_dir_link(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

/// リンクを作る。Windows では mklink /J に権限は要らないので、作れなければテストの準備の不具合として止める。
/// それ以外の OS では作れなければ false(テストを省略する)。
fn make_dir_link(link: &Path, target: &Path) -> bool {
    if try_dir_link(link, target) {
        return true;
    }
    if cfg!(windows) {
        panic!(
            "ディレクトリのリンク(シンボリックリンクとジャンクション)を作れません: {}。テストの準備に失敗しました",
            link.display()
        );
    }
    eprintln!("リンクを作れないためテストを省略します: {}", link.display());
    false
}

#[test]
fn linked_output_folders_pointing_outside_are_refused_without_deleting() {
    for sub in ["tracks", "timeline", "work"] {
        let f = fixture(ONE_LINE);
        let foreign = f._tmp.path().join("foreign");
        fs::create_dir_all(&foreign).unwrap();
        for n in [
            "dialogue.wav",
            "bgm.wav",
            "se.wav",
            "timeline.json",
            "keep.txt",
        ] {
            fs::write(foreign.join(n), b"precious").unwrap();
        }
        if !make_dir_link(&f.dir.join(sub), &foreign) {
            return;
        }
        assert_eq!(run_build(&no_voice_args(&f)), Outcome::Failed, "{sub}");
        for n in [
            "dialogue.wav",
            "bgm.wav",
            "se.wav",
            "timeline.json",
            "keep.txt",
        ] {
            assert_eq!(fs::read(foreign.join(n)).unwrap(), b"precious", "{sub}/{n}");
        }
        assert!(!f.dir.join("Process.log").exists(), "{sub}");
    }
}

#[test]
fn linked_work_subfolders_pointing_outside_are_refused_without_writing_or_deleting() {
    for sub in ["resampled", "lines", "lines_prefade"] {
        let f = fixture(ONE_LINE);
        write_voice(&f.voices.join("S01_L001.wav"));
        let foreign = f._tmp.path().join("foreign");
        fs::create_dir_all(&foreign).unwrap();
        let decoys = ["S01-L001.wav", "S01_L001.wav", "decoy.wav", "keep.txt"];
        for n in decoys {
            fs::write(foreign.join(n), b"precious").unwrap();
        }
        fs::create_dir_all(f.dir.join("work")).unwrap();
        if !make_dir_link(&f.dir.join("work").join(sub), &foreign) {
            return;
        }
        assert_eq!(run_build(&no_voice_args(&f)), Outcome::Failed, "{sub}");
        for n in decoys {
            assert_eq!(fs::read(foreign.join(n)).unwrap(), b"precious", "{sub}/{n}");
        }
        assert_eq!(
            fs::read_dir(&foreign).unwrap().count(),
            decoys.len(),
            "{sub}"
        );
        assert!(!f.dir.join("Process.log").exists(), "{sub}");
        assert!(!f.dir.join("timeline").exists(), "{sub}");
    }
}
