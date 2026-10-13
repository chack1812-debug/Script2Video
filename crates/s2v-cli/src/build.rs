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
use s2v_core::voices::match_voice_files;
use s2v_core::{validate, Config, Scene, ScriptItem, ScriptParser};
use s2v_export::fcpxml::render_fcpxml;
use s2v_export::srt::render_srt;
use s2v_export::timeline_json::{
    write_timeline_json, CastInfo, LineAudioInfo, TakeInfo, TimelineInput,
};
use s2v_export::wavio::{write_f32_stereo, write_i24_stereo};
use s2v_export::{mix, MixRequest};

use crate::proclog::ProcLog;
use crate::work::{line_key, resampled_key, sha256_hex, LineKeyParams, NormMeta, WorkDir};
use crate::Outcome;

pub struct BuildArgs {
    pub script: PathBuf,
    pub voice_dir: PathBuf,
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
    let text = std::fs::read_to_string(&args.script).with_context(|| {
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
    let out_dir = args.script.parent().unwrap_or(Path::new(".")).join(&stem);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("出力フォルダを作れません: {}", out_dir.display()))?;
    let mut log = ProcLog::create(&out_dir.join("Process.log"))?;
    let script_warnings: Vec<String> = parser
        .warnings()
        .iter()
        .chain(diag.warnings.iter())
        .map(|w| format!("{} 行目: {}", w.line_no, w.message))
        .collect();
    // ここから先の失敗は Process.log に記録する
    let result = produce(
        args,
        &config,
        &scenes,
        &script_warnings,
        &stem,
        &out_dir,
        &mut log,
    )
    .and_then(|o| log.check().map(|()| o));
    match result {
        Ok(o) => Ok(o),
        Err(e) => {
            log.error(format!("{e:#}"));
            Ok(Outcome::Failed)
        }
    }
}

/// 前回の最終成果物(これだけ)を消す。`work/` と `Process.log` には触れない。
fn remove_stale_outputs(out_dir: &Path) -> anyhow::Result<()> {
    let finals = [
        "full_dialogue.wav",
        "tracks/dialogue.wav",
        "tracks/bgm.wav",
        "tracks/se.wav",
        "timeline.json",
        "subtitles.srt",
        "timeline.fcpxml",
    ];
    for rel in finals {
        let path = out_dir.join(rel);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("前回の出力を消せません: {}", path.display()))
            }
        }
    }
    // 空になったときだけ tracks/ を消す(中身があれば残す)
    let tracks = out_dir.join("tracks");
    if tracks.is_dir() {
        let _ = std::fs::remove_dir(&tracks);
    }
    Ok(())
}

/// コミットポイントより後の処理。失敗は呼び出し側が Process.log に記録して終了コード 2 にする。
fn produce(
    args: &BuildArgs,
    config: &Config,
    scenes: &[Scene],
    script_warnings: &[String],
    stem: &str,
    out_dir: &Path,
    log: &mut ProcLog,
) -> anyhow::Result<Outcome> {
    // コミットポイント: 前回の成果物が古いまま残らないよう、新しく作る前に消す
    remove_stale_outputs(out_dir)?;
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
    let voices = match_voice_files(&args.voice_dir, &ids, config.voice.allow_flac)
        .with_context(|| format!("収録フォルダを読めません: {}", args.voice_dir.display()))?;
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
    write_i24_stereo(&out_dir.join("full_dialogue.wav"), &mixed.master)?;
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
    write_timeline_json(
        &out_dir.join("timeline.json"),
        &TimelineInput {
            script: file_name(&args.script),
            duration_samples: total,
            target_lufs: config.normalize.target_lufs,
            casts: casts.into_values().collect(),
            placement: &placement,
            takes,
            line_audio: &line_audio,
        },
    )?;
    std::fs::write(out_dir.join("subtitles.srt"), render_srt(&placement.lines))?;
    let track_paths = [
        ("dialogue", tracks_dir.join("dialogue.wav")),
        ("bgm", tracks_dir.join("bgm.wav")),
        ("se", tracks_dir.join("se.wav")),
    ];
    std::fs::write(
        out_dir.join("timeline.fcpxml"),
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
