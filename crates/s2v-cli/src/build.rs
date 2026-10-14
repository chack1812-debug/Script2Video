//! `script2video build`(設計書 5.2)。
//! 取り込み → 無音削除 → ラウドネス正規化 → 配置 → 音響処理 → ミックス → 出力。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Context;
use s2v_audio::detect::single_used_range;
use s2v_audio::input::{read_mono_48k, InputError};
use s2v_audio::loudness::{apply_edge_fades, apply_gain_db, decide_gain, measure_mono, Loudness};
use s2v_audio::AudioProcessor;
use s2v_core::ids::LineId;
use s2v_core::placement::{flatten, place, LineSource, MissingReason};
use s2v_core::time::{ceil_ms, floor_ms};
use s2v_core::visual_timeline::layout_visuals;
use s2v_core::voices::match_voice_files;
use s2v_core::{validate, Config, Scene, ScriptItem, ScriptParser};
use s2v_export::fcpxml::render_fcpxml;
use s2v_export::srt::render_srt;
use s2v_export::timeline_json::{
    write_timeline_json, CastInfo, LineAudioInfo, TakeInfo, TimelineInput,
};
use s2v_export::wavio::{write_f32_stereo, write_i24_stereo};
use s2v_export::{mix, MixRequest};

use crate::layout::{check_layout, script_dir};
use crate::proclog::ProcLog;
use crate::work::{line_key, resampled_key, sha256_hex, LineKeyParams, NormMeta, WorkDir};
use crate::Outcome;

pub struct BuildArgs {
    pub script: PathBuf,
    /// 収録フォルダ。省略時は `<台本フォルダ>/voices`。
    pub voice_dir: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub no_cache: bool,
}

/// 実行して終了コードを返す。読み書きの失敗などの予期しないエラーは終了コード 2。
pub fn run_build(args: &BuildArgs) -> Outcome {
    match build(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

/// 正規化済みの台詞。
struct PreparedLine {
    /// 正規化・フェード後のモノラル(使用範囲)
    samples: Vec<f32>,
    meta: NormMeta,
    /// 収録ファイル(48kHz 変換後)の長さ
    file_len: usize,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn build(args: &BuildArgs) -> anyhow::Result<Outcome> {
    // 台本が無いときは、ロックファイルも作らずに止める
    if !args.script.is_file() {
        eprintln!("[ERROR] 台本を読めません: {}", args.script.display());
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }
    let out_dir = script_dir(&args.script);
    let voice_dir = args
        .voice_dir
        .clone()
        .unwrap_or_else(|| out_dir.join("voices"));
    let cwd = std::env::current_dir().context("現在のフォルダを取得できません")?;
    if let Err(e) = check_layout(&args.script, args.voice_dir.as_deref(), &cwd) {
        eprintln!("[ERROR] {e:#}");
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }
    // 同じ台本フォルダで他のコマンドが動いている間は何も書かない。build の終わりまで保持する
    let _lock = match crate::lock::ScriptLock::acquire(&out_dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            eprintln!("[ERROR] 出力していません");
            return Ok(Outcome::Failed);
        }
    };
    // 1. 設定
    let config = match &args.config {
        Some(p) => Config::from_file(p)
            .with_context(|| format!("設定ファイルを読めません: {}", p.display()))?,
        None => Config::builtin(),
    };
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        for e in config_errors {
            eprintln!("[ERROR] {e}");
        }
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }

    // 2. 台本
    let script_bytes = std::fs::read(&args.script)
        .with_context(|| format!("台本を読めません: {}", args.script.display()))?;
    let script_sha256 = sha256_hex(&script_bytes);
    let text = String::from_utf8(script_bytes).with_context(|| {
        format!(
            "台本を読めません(UTF-8 で保存してください): {}",
            args.script.display()
        )
    })?;
    let mut parser = ScriptParser::new();
    let scenes = parser.parse_str(&text)?;
    let diag = validate(&scenes);
    let errors: Vec<_> = parser.errors().iter().chain(diag.errors.iter()).collect();
    if !errors.is_empty() {
        for e in errors {
            eprintln!("[ERROR] {} 行目: {}", e.line_no, e.message);
        }
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }

    // 3. 出力フォルダ・Process.log・work/
    let stem = args
        .script
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "script".to_string());
    let mut log = ProcLog::create(&out_dir.join("Process.log"))?;
    // コミットポイントの直前に古い完成動画を改名する。失敗しても前回の成果物は消さない
    match rename_stale_output(&out_dir) {
        Ok(true) => log.info("古い output.mp4 を output.stale.mp4 に改名しました"),
        Ok(false) => {}
        Err(e) => {
            log.error(format!(
                "古い output.mp4 を output.stale.mp4 に改名できません: {e}"
            ));
            log.error("出力していません");
            return Ok(Outcome::Failed);
        }
    }
    let script_warnings: Vec<String> = parser
        .warnings()
        .iter()
        .chain(diag.warnings.iter())
        .map(|w| format!("{} 行目: {}", w.line_no, w.message))
        .collect();
    // ここから先の失敗は Process.log に記録する
    let job = Job {
        scenes: &scenes,
        script_warnings: &script_warnings,
        stem: &stem,
        script_sha256: &script_sha256,
    };
    Ok(run_after_commit(
        args, &config, &job, &out_dir, &voice_dir, &mut log,
    ))
}

/// コミットポイントの直前に、古い完成動画を output.stale.mp4 に改名する(映像工程設計書 §2.2)。
/// 古い output.stale.mp4 は上書きする。output.mp4 がファイルでなければエラー。
fn rename_stale_output(out_dir: &Path) -> std::io::Result<bool> {
    let mp4 = out_dir.join("output.mp4");
    match std::fs::symlink_metadata(&mp4) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
        Ok(m) if !m.is_file() => Err(std::io::Error::other("output.mp4 がファイルではありません")),
        Ok(_) => {
            std::fs::rename(&mp4, out_dir.join("output.stale.mp4"))?;
            Ok(true)
        }
    }
}

/// 検証済みの台本まわりの入力。
struct Job<'a> {
    scenes: &'a [Scene],
    script_warnings: &'a [String],
    /// 台本のファイル名(拡張子なし。FCPXML のプロジェクト名)
    stem: &'a str,
    /// 台本ファイルのバイト列の SHA-256(timeline.json に記録する)
    script_sha256: &'a str,
}

/// コミットポイント以降を実行する。失敗(Process.log に書けない場合を含む)は
/// `[ERROR]` として記録し、成果物を残さずに Failed を返す。
fn run_after_commit(
    args: &BuildArgs,
    config: &Config,
    job: &Job<'_>,
    out_dir: &Path,
    voice_dir: &Path,
    log: &mut ProcLog,
) -> Outcome {
    let result =
        produce(args, config, job, out_dir, voice_dir, log).and_then(|o| log.check().map(|()| o));
    match result {
        Ok(o) => o,
        Err(e) => {
            log.error(format!("{e:#}"));
            if let Err(e) = remove_final_artifacts(out_dir) {
                eprintln!("[ERROR] 出力の取り消しに失敗しました: {e}");
            }
            Outcome::Failed
        }
    }
}

/// 最終成果物(これだけ)を消す。`work/` と `Process.log` には触れない。
/// 途中で失敗しても残りを消し続け、最初の失敗を返す。
fn remove_final_artifacts(out_dir: &Path) -> std::io::Result<()> {
    let finals = [
        "full_dialogue.wav",
        "tracks/dialogue.wav",
        "tracks/bgm.wav",
        "tracks/se.wav",
        "timeline/timeline.json",
        "timeline/subtitles.srt",
        "timeline/timeline.fcpxml",
    ];
    let mut first_err = None;
    for rel in finals {
        let path = out_dir.join(rel);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                first_err.get_or_insert(std::io::Error::new(
                    e.kind(),
                    format!("{}: {e}", path.display()),
                ));
            }
        }
    }
    // 空になったときだけ tracks/ と timeline/ を消す(他のファイルがあれば、それごと残す)
    for sub in ["tracks", "timeline"] {
        let dir = out_dir.join(sub);
        match std::fs::remove_dir(&dir) {
            Ok(()) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::DirectoryNotEmpty
                    || matches!(e.raw_os_error(), Some(145 | 39 | 66)) => {}
            Err(e) => {
                first_err.get_or_insert(std::io::Error::new(
                    e.kind(),
                    format!("{}: {e}", dir.display()),
                ));
            }
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// コミットポイントより後の処理。失敗は呼び出し側が Process.log に記録して終了コード 2 にする。
fn produce(
    args: &BuildArgs,
    config: &Config,
    job: &Job<'_>,
    out_dir: &Path,
    voice_dir: &Path,
    log: &mut ProcLog,
) -> anyhow::Result<Outcome> {
    let Job {
        scenes,
        script_warnings,
        stem,
        script_sha256,
    } = *job;
    // コミットポイント: 前回の成果物が古いまま残らないよう、新しく作る前に消す
    remove_final_artifacts(out_dir)
        .with_context(|| format!("前回の出力を消せません: {}", out_dir.display()))?;
    for w in script_warnings {
        log.warn(w);
    }
    let (mut work, note) = WorkDir::open(&out_dir.join("work"), args.no_cache)
        .with_context(|| format!("work/ に書き込めません: {}", out_dir.display()))?;
    if let Some(n) = note {
        log.info(n);
    }

    // 4. 照合
    let flat = flatten(scenes);
    let ids: Vec<LineId> = flat
        .iter()
        .filter_map(|f| match f.item {
            ScriptItem::Speech { id, .. } => *id,
            _ => None,
        })
        .collect();
    let is_dir = std::fs::metadata(voice_dir)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    if !is_dir && (args.voice_dir.is_none() || !voice_dir.exists()) {
        if args.voice_dir.is_none() {
            anyhow::bail!(
                "収録フォルダがありません: {}(台本と同じ場所に voices フォルダを作るか、--voice-dir で指定してください)",
                voice_dir.display()
            );
        }
        anyhow::bail!("収録フォルダがありません: {}", voice_dir.display());
    }
    let voices = match_voice_files(voice_dir, &ids, config.voice.allow_flac)
        .with_context(|| format!("収録フォルダを読めません: {}", voice_dir.display()))?;
    for w in &voices.warnings {
        log.warn(w);
    }

    // 5. 台詞ごとの取り込み・無音削除・正規化
    let mut sources: HashMap<LineId, LineSource> = HashMap::new();
    let mut prepared: HashMap<LineId, PreparedLine> = HashMap::new();
    let mut takes: Vec<TakeInfo> = Vec::new();
    for id in &ids {
        let files = voices.files.get(id).cloned().unwrap_or_default();
        let names: Vec<String> = files.iter().map(|p| file_name(p)).collect();
        let result = match files.as_slice() {
            [] => Err(MissingReason::NoFile),
            [one] => prepare_line(*id, one, config, &mut work, log)?,
            _ => Err(MissingReason::Duplicate),
        };
        match result {
            Ok(line) => {
                sources.insert(
                    *id,
                    LineSource::Present {
                        len: line.samples.len() as i64,
                    },
                );
                let head = floor_ms(line.meta.src_start as i64);
                let tail = ceil_ms(line.file_len as i64) - ceil_ms(line.meta.src_end as i64);
                takes.push(TakeInfo {
                    id: id.file_stem(),
                    files: names,
                    trims: Some((head, tail)),
                    missing: None,
                });
                prepared.insert(*id, line);
            }
            Err(reason) => {
                let detail = match reason {
                    MissingReason::NoFile => "収録ファイルがありません".to_string(),
                    MissingReason::Duplicate => {
                        format!("同じ台詞のファイルが複数あります: {}", names.join(", "))
                    }
                    _ => "収録ファイルを使えません".to_string(),
                };
                log.warn(format!(
                    "{}: 不足({})。仮の無音を置きます: {detail}",
                    id.display(),
                    reason.as_str()
                ));
                sources.insert(*id, LineSource::Missing(reason));
                takes.push(TakeInfo {
                    id: id.file_stem(),
                    files: names,
                    trims: None,
                    missing: Some(reason),
                });
            }
        }
    }

    // 6. 配置
    let placement = place(&flat, &sources, config.voice.placeholder_ms_per_char);
    for w in &placement.warnings {
        log.warn(w);
    }
    if !placement.errors.is_empty() {
        for e in &placement.errors {
            log.error(e);
        }
        log.error("出力していません");
        return Ok(Outcome::Failed);
    }

    // 7. 音響処理
    let processor =
        AudioProcessor::new(config.audio.clone()).context("音響処理の較正に失敗しました")?;
    log.info(format!(
        "基準ゲイン(較正値): {:.4}",
        processor.calibration_gain()
    ));
    let mut dialogue = Vec::new();
    for line in &placement.lines {
        let Some(p) = prepared.get(&line.id) else {
            continue;
        };
        let fi = &flat[line.flat_index];
        let ScriptItem::Speech {
            cast_name,
            offset_params,
            scene_config,
            ..
        } = fi.item
        else {
            continue;
        };
        let cast = scenes[fi.scene_index]
            .casts
            .get(cast_name)
            .context("検証済みの役名が見つかりません")?
            .with_offsets(offset_params);
        dialogue.push((
            line.start,
            processor.render(&p.samples, &cast, scene_config)?,
        ));
    }

    // 8. ミックス
    let mixed = mix(MixRequest {
        dialogue,
        events: &placement.events,
        timeline_end: placement.end_sample(),
        bgm: &config.bgm,
        master_ceiling_db: config.mix.master_ceiling_db,
    })?;
    for w in &mixed.warnings {
        log.warn(w);
    }
    let tracks_dir = out_dir.join("tracks");
    write_f32_stereo(&tracks_dir.join("dialogue.wav"), &mixed.dialogue)?;
    write_f32_stereo(&tracks_dir.join("bgm.wav"), &mixed.bgm)?;
    write_f32_stereo(&tracks_dir.join("se.wav"), &mixed.se)?;
    let dialogue_path = out_dir.join("full_dialogue.wav");
    write_i24_stereo(&dialogue_path, &mixed.master)?;
    let audio_sha256 = crate::fsutil::sha256_file(&dialogue_path).with_context(|| {
        format!(
            "full_dialogue.wav を読み直せません: {}",
            dialogue_path.display()
        )
    })?;
    let total = mixed.master.len() as i64;

    // 9. timeline.json・字幕・FCPXML
    let mut casts: BTreeMap<String, CastInfo> = BTreeMap::new();
    for s in scenes {
        for (name, c) in &s.casts {
            casts.insert(
                name.clone(),
                CastInfo {
                    name: name.clone(),
                    speaker: c.speaker_name.clone(),
                    appearance: c.appearance.clone(),
                },
            );
        }
    }
    let line_audio: HashMap<LineId, LineAudioInfo> = prepared
        .iter()
        .map(|(id, p)| {
            (
                *id,
                LineAudioInfo {
                    src_start: p.meta.src_start as i64,
                    src_end: p.meta.src_end as i64,
                    measured_lufs: p.meta.measured_lufs,
                    requested_gain_db: p.meta.requested_gain_db,
                    norm_gain_db: p.meta.norm_gain_db,
                    norm_clamped: p.meta.norm_clamped,
                },
            )
        })
        .collect();
    let timeline_dir = out_dir.join("timeline");
    std::fs::create_dir_all(&timeline_dir)
        .with_context(|| format!("timeline/ を作れません: {}", timeline_dir.display()))?;
    let (visuals, visual_warnings) = layout_visuals(&placement.scenes, ceil_ms(total));
    for w in &visual_warnings {
        log.warn(w);
    }
    write_timeline_json(
        &timeline_dir.join("timeline.json"),
        &TimelineInput {
            script: file_name(&args.script),
            script_sha256: script_sha256.to_string(),
            audio_sha256,
            visuals: &visuals,
            duration_samples: total,
            target_lufs: config.normalize.target_lufs,
            casts: casts.into_values().collect(),
            placement: &placement,
            takes,
            line_audio: &line_audio,
        },
    )?;
    std::fs::write(
        timeline_dir.join("subtitles.srt"),
        render_srt(&placement.lines),
    )?;
    let track_paths = [
        ("dialogue", tracks_dir.join("dialogue.wav")),
        ("bgm", tracks_dir.join("bgm.wav")),
        ("se", tracks_dir.join("se.wav")),
    ];
    std::fs::write(
        timeline_dir.join("timeline.fcpxml"),
        render_fcpxml(stem, total, &track_paths, config.export.fcpxml_2997fps),
    )?;
    work.finish()?;
    log.check()?;

    let missing = placement
        .lines
        .iter()
        .filter(|l| l.missing.is_some())
        .count();
    log.info(format!(
        "台詞 {} 行(不足 {} 行)を出力しました: {}",
        placement.lines.len(),
        missing,
        out_dir.display()
    ));
    Ok(if log.warning_count() > 0 || missing > 0 {
        Outcome::Warnings
    } else {
        Outcome::Ok
    })
}

/// 1台詞を取り込み・無音削除・正規化する。不足なら Ok(Err(理由))、work/ に書けなければ Err。
fn prepare_line(
    id: LineId,
    path: &Path,
    config: &Config,
    work: &mut WorkDir,
    log: &mut ProcLog,
) -> anyhow::Result<Result<PreparedLine, MissingReason>> {
    let name = file_name(path);
    let src_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log.warn(format!("{}: 読み込めません: {name}: {e}", id.display()));
            return Ok(Err(MissingReason::Unreadable));
        }
    };

    // 48kHz モノラルへの変換(レベルは変えない)
    let rkey = resampled_key(&sha256_hex(&src_bytes));
    let rrel = format!("resampled/{}.wav", id.file_stem());
    let resampled = if work.lookup(&rrel, &rkey).is_some() {
        work.read_mono(&rrel)?
    } else {
        match read_mono_48k(path) {
            Ok(s) => {
                work.store(&rrel, &rkey, &s, None)?;
                s
            }
            Err(InputError::BadFormat(m)) => {
                log.warn(format!("{}: 形式が不正です: {name}: {m}", id.display()));
                return Ok(Err(MissingReason::BadFormat));
            }
            Err(InputError::Unreadable(m)) => {
                log.warn(format!("{}: 読み込めません: {name}: {m}", id.display()));
                return Ok(Err(MissingReason::Unreadable));
            }
        }
    };

    // 使用範囲(単独ファイルは無音削除だけで決める)
    let v = &config.voice;
    let Some(range) = single_used_range(
        &resampled,
        v.trim_threshold_db,
        v.trim_head_margin_ms,
        v.trim_tail_margin_ms,
    ) else {
        log.warn(format!(
            "{}: 音がありません(全区間が {} dBFS 以下): {name}",
            id.display(),
            v.trim_threshold_db
        ));
        work.release(&rrel);
        return Ok(Err(MissingReason::Silent));
    };

    let n = &config.normalize;
    let lkey = line_key(
        &rkey,
        &LineKeyParams {
            src_start: range.used.start as u64,
            src_end: range.used.end as u64,
            trim_threshold_db: v.trim_threshold_db,
            target_lufs: n.target_lufs,
            max_boost_db: n.max_boost_db,
            max_cut_db: n.max_cut_db,
            prefade: n.dump_prefade,
        },
    );
    let lrel = format!("lines/{}.wav", id.display());
    let prel = format!("lines_prefade/{}.wav", id.display());
    let hit = work.lookup(&lrel, &lkey).and_then(|e| e.norm);
    let prefade_ok = !n.dump_prefade || work.lookup(&prel, &lkey).is_some();
    if let (Some(meta), true) = (hit, prefade_ok) {
        for w in &meta.warnings {
            log.warn(format!("{}: {w}", id.display()));
        }
        let samples = work.read_mono(&lrel)?;
        return Ok(Ok(PreparedLine {
            samples,
            meta,
            file_len: resampled.len(),
        }));
    }

    // ラウドネスの測定(測定範囲 = マージンを除いた発声部分、フェード前)
    let measured = match measure_mono(&resampled[range.measure.start..range.measure.end]) {
        Loudness::Lufs(l) => l,
        Loudness::Unmeasurable => {
            log.warn(format!(
                "{}: ラウドネスを測定できません: {name}",
                id.display()
            ));
            work.release(&rrel);
            return Ok(Err(MissingReason::Unmeasurable));
        }
    };
    let Some(g) = decide_gain(measured, n) else {
        log.warn(format!(
            "{}: ラウドネスを測定できません(有限でない値): {name}",
            id.display()
        ));
        work.release(&rrel);
        return Ok(Err(MissingReason::Unmeasurable));
    };
    let mut warnings = Vec::new();
    if g.clamped {
        warnings.push(format!(
            "正規化のゲインが制限に掛かりました(必要 {:+.1} dB、適用 {:+.1} dB)",
            g.requested_db, g.applied_db
        ));
    }
    let mut samples = resampled[range.used.start..range.used.end].to_vec();
    apply_gain_db(&mut samples, g.applied_db);
    if n.dump_prefade {
        work.store(&prel, &lkey, &samples, None)?;
    }
    apply_edge_fades(&mut samples);
    let meta = NormMeta {
        measured_lufs: measured,
        requested_gain_db: g.requested_db,
        norm_gain_db: g.applied_db,
        norm_clamped: g.clamped,
        measure_start: range.measure.start as u64,
        measure_end: range.measure.end as u64,
        src_start: range.used.start as u64,
        src_end: range.used.end as u64,
        warnings: warnings.clone(),
    };
    work.store(&lrel, &lkey, &samples, Some(meta.clone()))?;
    for w in &warnings {
        log.warn(format!("{}: {w}", id.display()));
    }
    Ok(Ok(PreparedLine {
        samples,
        meta,
        file_len: resampled.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing;
    impl std::io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("書き込み失敗"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn log_write_failure_after_output_removes_final_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("ep");
        std::fs::create_dir_all(&out).unwrap();
        let script = tmp.path().join("ep.txt");
        let text = "@scene 居間
@cast
A:田中

@script
#paragraph[S01]
[S01-L001] A:あ
";
        std::fs::write(&script, text).unwrap();
        let voices = tmp.path().join("v");
        std::fs::create_dir_all(&voices).unwrap();
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(text).unwrap();
        let args = BuildArgs {
            script,
            voice_dir: Some(voices.clone()),
            config: None,
            no_cache: false,
        };
        let mut log = ProcLog::from_writer(Box::new(Failing));
        let o = run_after_commit(
            &args,
            &Config::builtin(),
            &Job {
                scenes: &scenes,
                script_warnings: &[],
                stem: "ep",
                script_sha256: &"0".repeat(64),
            },
            &out,
            &voices,
            &mut log,
        );
        assert_eq!(o, Outcome::Failed);
        assert!(!out.join("tracks").exists(), "tracks/");
        assert!(!out.join("timeline").exists(), "timeline/");
        for n in [
            "full_dialogue.wav",
            "timeline/timeline.json",
            "timeline/subtitles.srt",
            "timeline/timeline.fcpxml",
            "tracks/dialogue.wav",
            "tracks/bgm.wav",
            "tracks/se.wav",
        ] {
            assert!(!out.join(n).exists(), "{n}");
        }
    }

    #[test]
    fn foreign_files_in_tracks_and_timeline_survive_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path();
        std::fs::create_dir_all(out.join("tracks")).unwrap();
        std::fs::create_dir_all(out.join("timeline")).unwrap();
        for n in ["dialogue", "bgm", "se"] {
            std::fs::write(out.join("tracks").join(format!("{n}.wav")), b"x").unwrap();
        }
        for n in ["timeline.json", "subtitles.srt", "timeline.fcpxml"] {
            std::fs::write(out.join("timeline").join(n), b"x").unwrap();
        }
        std::fs::write(out.join("tracks").join("keep.txt"), b"keep").unwrap();
        std::fs::write(out.join("timeline").join("keep.txt"), b"keep").unwrap();
        // 触れてはいけないもの
        std::fs::write(out.join("output.mp4"), b"mp4").unwrap();
        std::fs::write(out.join("Process.log"), b"log").unwrap();
        std::fs::create_dir_all(out.join("voices")).unwrap();
        std::fs::write(out.join("voices").join("a.wav"), b"v").unwrap();
        std::fs::create_dir_all(out.join("work")).unwrap();
        remove_final_artifacts(out).unwrap();
        assert!(out.join("tracks").join("keep.txt").exists());
        assert!(out.join("timeline").join("keep.txt").exists());
        for n in ["dialogue", "bgm", "se"] {
            assert!(!out.join("tracks").join(format!("{n}.wav")).exists());
        }
        for n in ["timeline.json", "subtitles.srt", "timeline.fcpxml"] {
            assert!(!out.join("timeline").join(n).exists());
        }
        assert_eq!(std::fs::read(out.join("output.mp4")).unwrap(), b"mp4");
        assert!(out.join("Process.log").exists());
        assert!(out.join("voices").join("a.wav").exists());
        assert!(out.join("work").exists());
    }
}
