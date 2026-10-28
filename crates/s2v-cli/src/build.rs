//! `script2video build`(設計書 5.2)。
//! 取り込み → 無音削除 → ラウドネス正規化 → 配置 → 音響処理 → ミックス → 出力。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use s2v_audio::detect::single_used_range;
use s2v_audio::input::{read_mono_48k, InputError};
use s2v_audio::loudness::{apply_edge_fades, apply_gain_db, decide_gain, measure_mono, Loudness};
use s2v_audio::AudioProcessor;
use s2v_core::ids::LineId;
use s2v_core::placement::{flatten, place, FlatItem, LineSource, MissingReason};
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

use s2v_engines::{build_engine_manager_with_paths, EngineManager};

use crate::enginelog;
use crate::layout::{check_layout, script_dir};
use crate::proclog::{utc_stamp, ProcLog};
use crate::synth;
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
    // 台本がリンクのときは、何も書かずに止める(ロックより前)
    if let Err(e) = crate::layout::check_not_symlink(&args.script) {
        eprintln!("[ERROR] {e}");
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }
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
    // コミットポイントの直前に古い完成動画を改名する。Process.log を作る(切り詰める)より前に行い、
    // 失敗したときは前回の成果物も Process.log も変えず、標準エラーにだけ出す
    let renamed = match rename_stale_output(&out_dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[ERROR] 古い output.mp4 を output.stale.mp4 に改名できません: {e}");
            eprintln!("[ERROR] 出力していません");
            return Ok(Outcome::Failed);
        }
    };
    let mut log = ProcLog::create(&out_dir.join("Process.log"))?;
    if renamed {
        log.info("古い output.mp4 を output.stale.mp4 に改名しました");
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
    // 4a. 合成役の台詞を合成する(設計書 §3.3)。失敗した台詞は、ファイルが無ければ下の照合で不足になる
    let synth_failed: HashSet<LineId> =
        synthesize_lines(&flat, config, voice_dir, args.voice_dir.is_none(), log)?;
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
        let mut files = voices.files.get(id).cloned().unwrap_or_default();
        if synth_failed.contains(id) && !files.is_empty() {
            // 合成に失敗した台詞は、残っているファイル(退避から戻した旧版・肉声)を使わず不足にする
            log.warn(format!(
                "{}: 合成に失敗したため、既存のファイルを使いません: {}",
                id.display(),
                files
                    .iter()
                    .map(|p| file_name(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            files.clear();
        }
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
    Ok(
        if log.warning_count() > 0 || missing > 0 || !synth_failed.is_empty() {
            Outcome::Warnings
        } else {
            Outcome::Ok
        },
    )
}

/// 終わるとき(エラーで抜けるときも)エンジンを止める。
struct EngineGuard(EngineManager);

impl Drop for EngineGuard {
    fn drop(&mut self) {
        self.0.shutdown_all();
    }
}

/// 合成役の台詞を合成し、失敗した台詞を返す。合成役が無ければ何もしない。
/// 合成が要る台詞が無ければエンジンを起動しない。
/// 収録フォルダが無いときは、既定の `voices/` に限り、合成するものがあれば作る
/// (`--voice-dir` の指定先が無い・フォルダでないときは合成せず、従来のエラーに任せる)。
fn synthesize_lines(
    flat: &[FlatItem<'_>],
    config: &Config,
    voice_dir: &Path,
    default_voice_dir: bool,
    log: &mut ProcLog,
) -> anyhow::Result<HashSet<LineId>> {
    let items: Vec<(LineId, &ScriptItem)> = flat
        .iter()
        .filter_map(|f| match f.item {
            ScriptItem::Speech { id: Some(id), .. } => Some((*id, f.item)),
            _ => None,
        })
        .collect();
    if !items
        .iter()
        .any(|(_, i)| matches!(i, ScriptItem::Speech { synth: Some(_), .. }))
    {
        return Ok(HashSet::new());
    }
    let exists = voice_dir.exists();
    if (exists && !voice_dir.is_dir()) || (!exists && !default_voice_dir) {
        return Ok(HashSet::new());
    }
    let mut record = synth::load_record(voice_dir, log);
    let plan = synth::plan(
        &items,
        voice_dir,
        &record,
        &utc_stamp(),
        config.voice.allow_flac,
    )
    .with_context(|| format!("収録フォルダを読めません: {}", voice_dir.display()))?;
    for w in &plan.warnings {
        log.warn(w);
    }
    if plan.jobs.is_empty() {
        // 合成済みの台詞にある別の綴りのファイルだけを退避する(エンジンは起動しない)
        synth::move_aside(&plan.backups, log);
        log.info(format!(
            "音声合成: 合成記録と一致 {} 行(合成なし)",
            plan.skipped
        ));
        return Ok(HashSet::new());
    }
    if !exists {
        std::fs::create_dir_all(voice_dir)
            .with_context(|| format!("収録フォルダを作れません: {}", voice_dir.display()))?;
        log.info(format!("収録フォルダを作りました: {}", voice_dir.display()));
    }
    let used: HashSet<String> = plan.jobs.iter().map(|j| j.engine.clone()).collect();
    // エンジンの探索・起動・停止の記録(s2v-engines の tracing)を集めて Process.log に書く
    let (o, engine_log) = enginelog::capture(|| {
        let (manager, paths) = build_engine_manager_with_paths(&config.engines);
        for (name, exe) in &paths {
            if used.contains(*name) {
                log.info(engine_line(name, exe.as_deref(), &config.engines));
            }
        }
        let engines = EngineGuard(manager);
        let o = synth::run(plan, &mut record, &engines.0, voice_dir, log);
        drop(engines);
        o
    });
    for (level, msg) in engine_log {
        match level {
            tracing::Level::ERROR => log.error(msg),
            tracing::Level::WARN => log.warn(msg),
            _ => log.info(msg),
        }
    }
    log.info(format!(
        "音声合成: 合成 {} 行、合成記録と一致 {} 行、失敗 {} 行",
        o.synthesized,
        o.skipped,
        o.failed.len()
    ));
    Ok(o.failed)
}

/// 使うエンジンの接続先と実行ファイル(自動検出を含む)を Process.log に残す 1 行。
fn engine_line(name: &str, exe: Option<&str>, cfg: &s2v_core::config::EnginesConfig) -> String {
    let url = match name {
        "voicevox" => &cfg.voicevox.url,
        "aivis" => &cfg.aivis.url,
        _ => &cfg.xtts.url,
    };
    match exe {
        Some(path) => format!("エンジン {name}: 接続先 {url}、実行ファイル {path}"),
        None => format!(
            "エンジン {name}: 接続先 {url}、実行ファイルは見つかりません(起動済みのエンジンへの接続だけ試みます)"
        ),
    }
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

    use crate::synth::run_tests::{tone_wav, FakeVoicevox};
    use wiremock::ResponseTemplate;

    const MIXED: &str = "@scene 居間
@cast
A:ずんだもん:ノーマル,voicevox

B:佐藤

@script
#paragraph[S01]
[S01-L001] A:こんにちは
[S01-L002] B:やあ
";

    const SYNTH_ONLY: &str = "@scene 居間
@cast
A:ずんだもん:ノーマル,voicevox

@script
#paragraph[S01]
[S01-L001] A:こんにちは
";

    /// 同梱の config.toml の VOICEVOX の接続先を偽サーバにした設定ファイルを書く。
    fn config_for(dir: &Path, fake: &FakeVoicevox) -> PathBuf {
        let base = include_str!("../../../config.toml");
        let from = "url = \"http://127.0.0.1:50021\"";
        assert!(base.contains(from));
        let p = dir.join("config.toml");
        std::fs::write(&p, base.replace(from, &format!("url = \"{}\"", fake.uri()))).unwrap();
        p
    }

    fn timeline_lines(out: &Path) -> Vec<serde_json::Value> {
        let s = std::fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        v["lines"].as_array().unwrap().clone()
    }

    #[test]
    fn build_integration_mixed_script() {
        let fake = FakeVoicevox::ok();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        let voices = dir.join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        let script = dir.join("ep.txt");
        std::fs::write(&script, MIXED).unwrap();
        std::fs::write(voices.join("S01_L002.wav"), tone_wav(48_000, 1)).unwrap();
        let args = BuildArgs {
            script,
            voice_dir: Some(voices.clone()),
            config: Some(config_for(tmp.path(), &fake)),
            no_cache: false,
        };
        let o = run_build(&args);
        assert!(matches!(o, Outcome::Ok | Outcome::Warnings), "{o:?}");
        assert!(voices.join("S01_L001.wav").is_file());
        assert!(voices.join("synth.json").is_file());
        let lines = timeline_lines(&dir);
        let ids: Vec<&str> = lines.iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["S01-L001", "S01-L002"]);
        for l in &lines {
            assert!(!l["measured_lufs"].is_null(), "音声のある台詞: {l}");
        }
        // 使ったエンジンの接続先・実行ファイルと、s2v-engines の記録が Process.log に残る
        let log = std::fs::read_to_string(dir.join("Process.log")).unwrap();
        assert!(
            log.contains(&format!("エンジン voicevox: 接続先 {}", fake.uri())),
            "{log}"
        );
        assert!(log.contains("実行ファイル"), "{log}");
        assert!(log.contains("[voicevox] 接続確認 OK"), "{log}");
        assert!(!log.contains("エンジン aivis:"), "使わないエンジンは書かない: {log}");
        // 2回目はエンジンを呼ばない
        let n = fake.requests();
        let o = run_build(&args);
        assert!(matches!(o, Outcome::Ok | Outcome::Warnings), "{o:?}");
        assert_eq!(fake.requests(), n);
    }

    #[test]
    fn build_creates_voices_folder_for_synth_only_script() {
        let fake = FakeVoicevox::ok();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("ep.txt");
        std::fs::write(&script, SYNTH_ONLY).unwrap();
        let o = run_build(&BuildArgs {
            script,
            voice_dir: None,
            config: Some(config_for(tmp.path(), &fake)),
            no_cache: false,
        });
        assert!(matches!(o, Outcome::Ok | Outcome::Warnings), "{o:?}");
        assert!(dir.join("voices").join("S01_L001.wav").is_file());
        assert!(!timeline_lines(&dir)[0]["measured_lufs"].is_null());
    }

    #[test]
    fn build_synth_failure_is_missing_line() {
        let fake = FakeVoicevox::start(ResponseTemplate::new(500));
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        let voices = dir.join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        let script = dir.join("ep.txt");
        std::fs::write(&script, SYNTH_ONLY).unwrap();
        let o = run_build(&BuildArgs {
            script,
            voice_dir: Some(voices.clone()),
            config: Some(config_for(tmp.path(), &fake)),
            no_cache: false,
        });
        assert_eq!(o, Outcome::Warnings);
        assert!(!voices.join("S01_L001.wav").exists());
        let lines = timeline_lines(&dir);
        assert!(lines[0]["measured_lufs"].is_null(), "不足: {}", lines[0]);
        let log = std::fs::read_to_string(dir.join("Process.log")).unwrap();
        assert!(log.contains("合成に失敗しました"), "{log}");
        assert!(log.contains("不足"), "{log}");
        assert_eq!(timeline_takes(&dir)[0]["reason"], "no_file");
    }

    /// 合成役の台詞に別の綴りのファイル(`S01_L1.wav`・許可された `.flac`)があっても、
    /// 退避してから合成するので重複(不足)にならない。2回目以降も同じ。
    #[test]
    fn build_other_spelling_files_are_backed_up_not_duplicate() {
        let fake = FakeVoicevox::ok();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        let voices = dir.join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        let script = dir.join("ep.txt");
        std::fs::write(&script, SYNTH_ONLY).unwrap();
        std::fs::write(voices.join("S01_L1.wav"), tone_wav(48_000, 1)).unwrap();
        std::fs::write(voices.join("S01_L001.flac"), b"flac").unwrap();
        let config = config_for(tmp.path(), &fake);
        let base = std::fs::read_to_string(&config).unwrap();
        assert!(base.contains("allow_flac = false"));
        std::fs::write(
            &config,
            base.replace("allow_flac = false", "allow_flac = true"),
        )
        .unwrap();
        let args = BuildArgs {
            script,
            voice_dir: Some(voices.clone()),
            config: Some(config),
            no_cache: false,
        };
        for _ in 0..2 {
            let o = run_build(&args);
            assert!(matches!(o, Outcome::Ok | Outcome::Warnings), "{o:?}");
            let log = std::fs::read_to_string(dir.join("Process.log")).unwrap();
            assert!(!log.contains("複数あります"), "{log}");
            assert_eq!(timeline_takes(&dir)[0]["reason"], serde_json::Value::Null);
            assert!(!timeline_lines(&dir)[0]["measured_lufs"].is_null());
        }
        assert!(!voices.join("S01_L1.wav").exists());
        assert!(!voices.join("S01_L001.flac").exists());
        let mut names: Vec<String> = std::fs::read_dir(voices.join("backup"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names[0].starts_with("S01_L001.") && names[0].ends_with(".flac"),
            "{names:?}"
        );
        assert!(
            names[1].starts_with("S01_L1.") && names[1].ends_with(".wav"),
            "{names:?}"
        );
    }

    fn timeline_takes(out: &Path) -> Vec<serde_json::Value> {
        let s = std::fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        v["takes"].as_array().unwrap().clone()
    }

    /// 合成に失敗した台詞は、退避から戻した既存ファイルがあっても使わず不足にする(設計書 §3.3)。
    #[test]
    fn build_synth_failure_with_existing_file_is_missing_line() {
        let fake = FakeVoicevox::start(ResponseTemplate::new(500));
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ep");
        let voices = dir.join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        let script = dir.join("ep.txt");
        std::fs::write(&script, SYNTH_ONLY).unwrap();
        let human = tone_wav(48_000, 1);
        let wav = voices.join("S01_L001.wav");
        std::fs::write(&wav, &human).unwrap();
        let o = run_build(&BuildArgs {
            script,
            voice_dir: Some(voices.clone()),
            config: Some(config_for(tmp.path(), &fake)),
            no_cache: false,
        });
        assert_eq!(o, Outcome::Warnings);
        assert_eq!(
            std::fs::read(&wav).unwrap(),
            human,
            "既存ファイルは元のまま残す"
        );
        let lines = timeline_lines(&dir);
        assert!(lines[0]["measured_lufs"].is_null(), "不足: {}", lines[0]);
        assert_eq!(timeline_takes(&dir)[0]["reason"], "no_file");
        let log = std::fs::read_to_string(dir.join("Process.log")).unwrap();
        assert_eq!(lines[0]["missing"], true);
        assert!(log.contains("S01-L001: 合成に失敗したため"), "{log}");
        assert!(log.contains("不足"), "{log}");
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
