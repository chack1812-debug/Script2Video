//! `script2video render`(映像工程設計書 §4)。
//! 素材・吹き出し・ト書き・音声を合成して `output.mp4` を作る。失敗したときは `output.mp4` を変えない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use s2v_core::config::{Fit as CoreFit, RenderConfig};
use s2v_core::Config;
use s2v_export::wavio::read_stereo_48k;
use s2v_render::assets::{match_assets, Asset, AssetKind};
use s2v_render::bubbles::{parse_bubbles, scale_to, BubblesFile, Placed};
use s2v_render::draw::Overlay;
use s2v_render::font::{FontFace, VerticalFeature};
use s2v_render::scene::{
    build_items, compose, draw_item, missing_glyph_warnings, place_items, Item, Placement,
};
use s2v_render::ItemKind;
use s2v_timeline::VisualKind;
use s2v_video::audio_check::{check_audio, check_av_length, decode_args, f32le, is_silent};
use s2v_video::ffmpeg::{
    check_ffprobe_version, check_version, concat_args, concat_list, cut_args, mux_args, Background,
    EncodeSettings, Fit, VersionCheck,
};
use s2v_video::frames::{overlay_list, plan_cuts, total_frames, CutPlan, ShownItem};
use s2v_video::probe::{
    asset_probe_args, check_cut, check_cuts_consistent, check_output, parse_probe, probe_args,
    video_color_input, Probe, Stream,
};

use crate::layout::{check_contained, check_not_symlink, check_reserved_name, script_dir};
use crate::layout::{normalize_path, script_folder};
use crate::proclog::ProcLog;
use crate::Outcome;

pub struct RenderArgs {
    pub script: PathBuf,
    /// 設定ファイル(省略時は同梱の既定設定)。
    pub config: Option<PathBuf>,
}

/// 実行して終了コードを返す。予期しないエラーは終了コード 2。
pub fn run_render(args: &RenderArgs) -> Outcome {
    match render(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

fn render(args: &RenderArgs) -> anyhow::Result<Outcome> {
    // 何かを書く前の検査。失敗は標準エラーにだけ出す(Process.log はまだ触らない)
    let fail = |msg: String| -> anyhow::Result<Outcome> {
        eprintln!("[ERROR] {msg}");
        eprintln!("[ERROR] 出力していません");
        Ok(Outcome::Failed)
    };
    if let Err(e) = check_reserved_name(&args.script) {
        return fail(format!("{e:#}"));
    }
    if let Err(e) = check_not_symlink(&args.script) {
        return fail(format!("{e:#}"));
    }
    if !args.script.is_file() {
        return fail(format!("台本を読めません: {}", args.script.display()));
    }
    let config = match &args.config {
        Some(p) => match Config::from_file(p) {
            Ok(c) => c,
            Err(e) => return fail(format!("設定ファイルを読めません: {}: {e:#}", p.display())),
        },
        None => Config::builtin(),
    };
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        return fail(config_errors.join("\n[ERROR] "));
    }
    let dir = script_dir(&args.script);
    let _lock = match crate::lock::ScriptLock::acquire(&dir) {
        Ok(l) => l,
        Err(e) => return fail(format!("{e:#}")),
    };

    // 2. Process.log(追記)
    let mut log = ProcLog::append(&dir.join("Process.log"), "render")?;
    let result = render_main(args, &config.render, &dir, &mut log);
    match result {
        // 成功: output.mp4 は置き換え済み。ログの書込み確認は置き換えの前に済ませてあるので、
        // ここで失敗扱いにしない(置き換え後の結果を覆さない)
        Ok(()) if log.warning_count() > 0 => Ok(Outcome::Warnings),
        Ok(()) => Ok(Outcome::Ok),
        Err(msg) => {
            log.error(&msg);
            log.error("output.mp4 は変更していません");
            if let Err(e) = log.check() {
                eprintln!("[ERROR] {e:#}");
            }
            Ok(Outcome::Failed)
        }
    }
}

/// `max(1, 論理コア数 ÷ jobs)`。
fn threads_for(cores: usize, jobs: u32) -> u32 {
    (cores / jobs.max(1) as usize).max(1) as u32
}

/// 区間ごとの項目の組に重ね画像のファイル名を割り当てる。同じ組は 1 つのファイルにまとめる。
/// 戻り値: (組 → ファイル名, カットごとの (r_k, ファイル名))。空の組は `ov_empty.png`。
#[allow(clippy::type_complexity)]
fn assign_overlay_files(
    plans: &[CutPlan],
) -> (BTreeMap<Vec<String>, String>, Vec<Vec<(i64, String)>>) {
    let mut files: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut next = 1;
    let mut per_cut = Vec::new();
    for plan in plans {
        let mut v = Vec::new();
        for iv in &plan.intervals {
            let name = files
                .entry(iv.items.clone())
                .or_insert_with(|| {
                    if iv.items.is_empty() {
                        "ov_empty.png".to_string()
                    } else {
                        let n = format!("ov_{next:04}.png");
                        next += 1;
                        n
                    }
                })
                .clone();
            v.push((iv.rel_start, name));
        }
        per_cut.push(v);
    }
    (files, per_cut)
}

/// 標準エラーの末尾 `n` 行。
fn tail_lines(bytes: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// 外部コマンドを引数の配列で起動する(シェルを経由しない)。標準入力は閉じ、出力は受け取る。
fn run_tool(
    exe: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    cmd.output()
        .map_err(|e| format!("{exe} を起動できません: {e}"))
}

/// 終了コードが 0 でなければ、標準エラーの末尾 20 行つきの Err にする。
fn run_checked(exe: &str, args: &[String], cwd: &Path) -> Result<Vec<u8>, String> {
    let out = run_tool(exe, args, Some(cwd))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "{exe} が異常終了しました({})\n{}",
            out.status,
            tail_lines(&out.stderr, 20)
        ))
    }
}

fn probe_file(ffprobe: &str, args: &[String], cwd: &Path) -> Result<Probe, String> {
    let out = run_checked(ffprobe, args, cwd)?;
    parse_probe(&String::from_utf8_lossy(&out))
}

/// パス区切りを含む相対パスは、カレントディレクトリを変えて起動しても通るよう絶対パスにする。
fn tool_path(configured: &Option<String>, default: &str, cwd: &Path) -> String {
    match configured {
        Some(s) if !s.is_empty() => {
            if s.contains('/') || s.contains('\\') {
                normalize_path(Path::new(s), cwd)
                    .to_string_lossy()
                    .into_owned()
            } else {
                s.clone()
            }
        }
        _ => default.to_string(),
    }
}

/// `<exe> -version` の 1 行目。起動できなければ Err(利用者向けの文)。`label` は設定のキー名。
fn tool_version(label: &str, exe: &str) -> Result<String, String> {
    let not_found = |why: String| {
        format!(
            "{label} が見つかりません: {exe}(PATH か config.toml の render.{label} を確かめてください)({why})"
        )
    };
    let out = run_tool(
        exe,
        &if label == "ffmpeg" {
            // ffprobe は -nostdin・-n を受け付けないので、ffmpeg だけに付ける
            vec!["-nostdin", "-hide_banner", "-n", "-version"]
        } else {
            vec!["-hide_banner", "-version"]
        }
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>(),
        None,
    )
    .map_err(not_found)?;
    if !out.status.success() {
        return Err(not_found(format!(
            "-version が失敗しました: {}",
            out.status
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string())
}

/// `-version` の 1 行目で処理を進めてよいか決める。古ければ Err、読み取れなければ警告を足す。
/// `probe` が true なら ffprobe の行として判定する。
fn gate_version(first_line: &str, probe: bool, warnings: &mut Vec<String>) -> Result<(), String> {
    let r = if probe {
        check_ffprobe_version(first_line)
    } else {
        check_version(first_line)
    };
    match r {
        VersionCheck::Ok => Ok(()),
        VersionCheck::TooOld(m) => Err(m),
        VersionCheck::Unknown(m) => {
            warnings.push(m);
            Ok(())
        }
    }
}

/// ffmpeg・ffprobe を確かめる(起動・版)。版の警告は `warnings` に足す。
fn check_tools(ffmpeg: &str, ffprobe: &str, warnings: &mut Vec<String>) -> Result<(), String> {
    gate_version(&tool_version("ffmpeg", ffmpeg)?, false, warnings)?;
    gate_version(&tool_version("ffprobe", ffprobe)?, true, warnings)?;
    Ok(())
}

/// カットごとの背景の素材(ffprobe で中身を確かめたもの)。
fn resolve_backgrounds(
    ffprobe: &str,
    work_dir: &Path,
    cuts: &[(String, bool)],
    found: &BTreeMap<String, Asset>,
    default_image: Option<(&Path, AssetKind)>,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<Background>, String> {
    let abs = |p: &Path| normalize_path(p, cwd).to_string_lossy().into_owned();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (cut_id, looped) in cuts {
        let (path, kind) = match found.get(cut_id) {
            Some(a) => (a.path.clone(), a.kind),
            None => match default_image {
                Some((p, k)) => {
                    warnings.push(format!(
                        "{cut_id}: 素材がありません。default_image を代わりに使います"
                    ));
                    (p.to_path_buf(), k)
                }
                None => unreachable!("不足は呼び出し側で止めている"),
            },
        };
        let path_s = abs(&path);
        let probe = probe_file(ffprobe, &asset_probe_args(&path_s), work_dir)
            .map_err(|e| format!("{cut_id}: 素材 {path_s} を読めません: {e}"))?;
        let video: Option<&Stream> = probe
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"));
        let Some(video) = video else {
            errors.push(format!(
                "{cut_id}: 素材 {path_s} に映像のストリームがありません"
            ));
            continue;
        };
        out.push(match kind {
            AssetKind::Png => Background::Png { path: path_s },
            AssetKind::Jpeg => Background::Jpeg { path: path_s },
            AssetKind::Video => match video_color_input(video) {
                Ok((matrix, range)) => Background::Video {
                    path: path_s,
                    looped: *looped,
                    matrix,
                    range,
                },
                Err(e) => {
                    errors.push(format!("{cut_id}: {e}"));
                    continue;
                }
            },
        });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors.join("\n"))
    }
}

/// 重ね画像を描いて `seg` に PNG として書く。
fn write_overlays(
    seg: &Path,
    files: &BTreeMap<Vec<String>, String>,
    items: &[Item],
    placed: &BTreeMap<String, Placed>,
    clipped: &BTreeSet<String>,
    cfg: &RenderConfig,
    face: &FontFace,
) -> Result<(), String> {
    // 使われる項目だけを 1 回ずつ描く
    let used: BTreeSet<&String> = files.keys().flatten().collect();
    let by_id: BTreeMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut drawn: BTreeMap<&str, Overlay> = BTreeMap::new();
    for id in used {
        let ov = draw_item(
            by_id[id.as_str()],
            &placed[id],
            clipped.contains(id.as_str()),
            cfg,
            face,
        )
        .map_err(|e| format!("{id}: {e}"))?;
        drawn.insert(id.as_str(), ov);
    }
    for (set, name) in files {
        let layers: Vec<(&Placed, &Overlay)> = set
            .iter()
            .map(|id| (&placed[id], &drawn[id.as_str()]))
            .collect();
        let img = compose(cfg, &layers);
        let png = img
            .encode_png()
            .map_err(|e| format!("{name} を PNG にできません: {e}"))?;
        std::fs::write(seg.join(name), png).map_err(|e| format!("{name} を書けません: {e}"))?;
    }
    Ok(())
}

fn render_main(
    args: &RenderArgs,
    cfg: &RenderConfig,
    dir: &Path,
    log: &mut ProcLog,
) -> Result<(), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("現在のフォルダを取得できません: {e}"))?;
    let abs = |p: &Path| normalize_path(p, &cwd).to_string_lossy().into_owned();

    // 2. 鮮度
    let fresh = crate::freshness::check_fresh(&args.script, true)?;
    for w in &fresh.warnings {
        log.warn(w);
    }
    let tl = fresh.timeline;
    let total = total_frames(tl.duration_ms, cfg.fps);

    // 3. フォント・ffmpeg・ffprobe
    let font_path = if Path::new(&cfg.font_path).is_absolute() {
        PathBuf::from(&cfg.font_path)
    } else {
        dir.join(&cfg.font_path)
    };
    let face = FontFace::load(&font_path, cfg.font_index)
        .map_err(|e| format!("{}: {e}", font_path.display()))?;
    log.info(format!(
        "フォント: {}(縦書きの機能: {})",
        font_path.display(),
        match face.vertical {
            VerticalFeature::Vrt2 => "vrt2",
            VerticalFeature::Vert => "vert",
        }
    ));
    let ffmpeg = tool_path(&cfg.ffmpeg, "ffmpeg", &cwd);
    let ffprobe = tool_path(&cfg.ffprobe, "ffprobe", &cwd);
    let mut tool_warnings = Vec::new();
    check_tools(&ffmpeg, &ffprobe, &mut tool_warnings)?;
    for w in &tool_warnings {
        log.warn(w);
    }

    // 4. bubbles.json
    let bubbles_path = dir.join("bubbles.json");
    let manual: BTreeMap<String, Placed> = if bubbles_path.exists() {
        let text = std::fs::read_to_string(&bubbles_path)
            .map_err(|e| format!("bubbles.json を読めません: {e}"))?;
        let file = parse_bubbles(&text)
            .map_err(|errs| format!("bubbles.json が正しくありません: {}", errs.join(" / ")))?;
        let file: BubblesFile = scale_to(&file, cfg.width, cfg.height)
            .map_err(|e| format!("bubbles.json を画面に合わせられません: {e}"))?;
        file.items
    } else {
        BTreeMap::new()
    };

    // 5. 項目
    let mut warnings = Vec::new();
    let items = build_items(&tl, &manual, &mut warnings)?;
    warnings.extend(missing_glyph_warnings(&face, &items));

    // 6. 大きさと位置
    let Placement { placed, clipped } = place_items(&items, &manual, cfg, &face, &mut warnings);
    for w in &warnings {
        log.warn(w);
    }

    // 7. コマの割り当てと素材
    let cut_ranges: Vec<(String, i64, i64)> = tl
        .scenes
        .iter()
        .flat_map(|s| s.cuts.iter())
        .map(|c| (c.id.clone(), c.start_ms, c.end_ms))
        .collect();
    let shown: Vec<ShownItem> = items
        .iter()
        .map(|i| ShownItem {
            id: i.id.clone(),
            is_note: i.kind == ItemKind::Note,
            start_ms: i.start_ms,
            end_ms: i.end_ms,
            order: i.order,
        })
        .collect();
    let plans = plan_cuts(&cut_ranges, &shown, tl.duration_ms, cfg.fps);
    for (id, s, e) in &cut_ranges {
        if !plans.iter().any(|p| &p.cut_id == id) && s < e {
            log.warn(format!("{id}: コマ数が 0 なので映像に出しません"));
        } else if s >= e {
            log.warn(format!("{id}: 長さが 0 のカットです"));
        }
    }
    let visuals: BTreeMap<&str, Option<&s2v_timeline::Visual>> = tl
        .scenes
        .iter()
        .flat_map(|s| s.cuts.iter())
        .map(|c| (c.id.as_str(), c.visual.as_ref()))
        .collect();
    let want: Vec<(String, Option<VisualKind>)> = plans
        .iter()
        .map(|p| {
            (
                p.cut_id.clone(),
                visuals[p.cut_id.as_str()].and_then(|v| v.kind),
            )
        })
        .collect();
    let matched = match_assets(&dir.join("tracks"), &want);
    if !matched.errors.is_empty() {
        return Err(matched.errors.join("\n"));
    }
    let default_image: Option<(PathBuf, AssetKind)> = match &cfg.default_image {
        Some(p) => {
            let path = if Path::new(p).is_absolute() {
                PathBuf::from(p)
            } else {
                dir.join(p)
            };
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let kind = match ext.as_str() {
                "png" => AssetKind::Png,
                "jpg" | "jpeg" => AssetKind::Jpeg,
                _ => {
                    return Err(format!(
                        "default_image の拡張子は png・jpg・jpeg のどれかにしてください: {p}"
                    ))
                }
            };
            Some((path, kind))
        }
        None => None,
    };
    if !matched.missing.is_empty() && default_image.is_none() {
        return Err(format!(
            "素材が足りません(tracks/ に置くか、config.toml の render.default_image を指定してください): {}",
            matched.missing.join("、")
        ));
    }
    let seg = dir.join("work").join("render").join("seg");
    let cuts_looped: Vec<(String, bool)> = plans
        .iter()
        .map(|p| {
            (
                p.cut_id.clone(),
                visuals[p.cut_id.as_str()].is_some_and(|v| v.looped),
            )
        })
        .collect();
    let mut asset_warnings = Vec::new();
    let backgrounds = resolve_backgrounds(
        &ffprobe,
        dir, // seg はまだ無いので、台本フォルダを作業フォルダにする
        &cuts_looped,
        &matched.found,
        default_image.as_ref().map(|(p, k)| (p.as_path(), *k)),
        &cwd,
        &mut asset_warnings,
    );
    for w in &asset_warnings {
        log.warn(w);
    }
    let backgrounds = backgrounds?;

    // 8. 作業フォルダ
    let root = script_folder(&args.script, &cwd);
    check_contained(&root, "work").map_err(|e| format!("{e:#}"))?;
    check_contained(&root, "work/render").map_err(|e| format!("{e:#}"))?;
    let render_dir = dir.join("work").join("render");
    if render_dir.exists() {
        std::fs::remove_dir_all(&render_dir)
            .map_err(|e| format!("{} を消せません: {e}", render_dir.display()))?;
    }
    std::fs::create_dir_all(&seg).map_err(|e| format!("{} を作れません: {e}", seg.display()))?;

    // 9. 重ね画像と一覧
    let (files, per_cut) = assign_overlay_files(&plans);
    write_overlays(&seg, &files, &items, &placed, &clipped, cfg, &face)?;
    let settings = EncodeSettings {
        width: cfg.width,
        height: cfg.height,
        fps: cfg.fps,
        crf: cfg.crf,
        preset: cfg.preset.clone(),
        threads: threads_for(
            std::thread::available_parallelism().map_or(1, |n| n.get()),
            cfg.jobs,
        ),
        fit: match cfg.fit {
            CoreFit::Contain => Fit::Contain,
            CoreFit::Cover => Fit::Cover,
        },
        background: RenderConfig::rgb(&cfg.background).unwrap_or([0, 0, 0]),
    };
    let mut cut_files = Vec::new();
    let mut ffmpeg_jobs: Vec<Vec<String>> = Vec::new();
    for (i, plan) in plans.iter().enumerate() {
        let no = i + 1;
        let (cut_name, list_name) = (format!("cut_{no:04}.mp4"), format!("ovl_{no:04}.txt"));
        std::fs::write(
            seg.join(&list_name),
            overlay_list(&per_cut[i], plan.frames, cfg.fps),
        )
        .map_err(|e| format!("{list_name} を書けません: {e}"))?;
        ffmpeg_jobs.push(cut_args(
            &backgrounds[i],
            &list_name,
            plan.frames,
            &cut_name,
            &settings,
        ));
        cut_files.push(cut_name);
    }

    // 10. カットファイル(jobs 個まで並列)
    let results = run_parallel(&ffmpeg, &seg, &ffmpeg_jobs, cfg.jobs as usize);
    let mut first_error = None;
    for (i, r) in results.iter().enumerate() {
        match r {
            Some(Ok(d)) => log.info(format!(
                "{} を作りました({}、{:.2} 秒)",
                cut_files[i],
                plans[i].cut_id,
                d.as_secs_f64()
            )),
            Some(Err(e)) => {
                first_error.get_or_insert_with(|| format!("{}: {e}", plans[i].cut_id));
            }
            None => {}
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }

    // 11. カットの検査・連結・多重化
    let mut probes = Vec::new();
    let mut internal = Vec::new();
    for (i, name) in cut_files.iter().enumerate() {
        let p = probe_file(&ffprobe, &probe_args(name), &seg)?;
        internal.extend(
            check_cut(&p, plans[i].frames, cfg.fps)
                .into_iter()
                .map(|e| format!("{name}: {e}")),
        );
        probes.push((name.clone(), p));
    }
    internal.extend(check_cuts_consistent(&probes));
    if !internal.is_empty() {
        return Err(format!(
            "内部の誤り: カットファイルが想定と違います\n{}",
            internal.join("\n")
        ));
    }
    std::fs::write(seg.join("concat.txt"), concat_list(&cut_files))
        .map_err(|e| format!("concat.txt を書けません: {e}"))?;
    run_checked(&ffmpeg, &concat_args("concat.txt", "video.mp4"), &seg)?;
    let wav = dir.join("full_dialogue.wav");
    run_checked(
        &ffmpeg,
        &mux_args(
            "video.mp4",
            &abs(&wav),
            "output.tmp.mp4",
            &cfg.audio_bitrate,
        ),
        &seg,
    )?;

    // 12. 出力の検証
    let out_probe = probe_file(&ffprobe, &probe_args("output.tmp.mp4"), &seg)?;
    let mut errs = check_output(&out_probe, cfg.width, cfg.height, cfg.fps, total);
    let decoded = f32le(&run_checked(&ffmpeg, &decode_args("output.tmp.mp4"), &seg)?);
    let reference: Vec<f32> = read_stereo_48k(&wav)
        .map_err(|e| format!("full_dialogue.wav を読めません: {e:#}"))?
        .iter()
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();
    if is_silent(&reference) {
        log.info("full_dialogue.wav 全体が無音なので、音声の時刻の検査を飛ばしました");
    }
    errs.extend(check_audio(&reference, &decoded));
    errs.extend(check_av_length(total, cfg.fps, decoded.len()));
    if !errs.is_empty() {
        return Err(format!("出力の検証に失敗しました\n{}", errs.join("\n")));
    }

    // 13. 置き換え
    commit_output(
        log,
        &seg.join("output.tmp.mp4"),
        &dir.join("output.mp4"),
        &format!(
            "output.mp4 を書き出しました({total} コマ、{:.2} 秒)",
            total as f64 / f64::from(cfg.fps)
        ),
    )
}

/// 成功の記録を先に書き、ログに書けていることを確かめてから output.mp4 へ置き換える。
/// 置き換えの後には失敗しうる処理を何も置かない(置き換え後に失敗扱いにならない)。
fn commit_output(
    log: &mut ProcLog,
    tmp: &Path,
    out: &Path,
    success_msg: &str,
) -> Result<(), String> {
    log.info(success_msg);
    log.check().map_err(|e| format!("{e:#}"))?;
    std::fs::rename(tmp, out)
        .map_err(|e| format!("output.mp4 に置き換えられません: {}: {e}", out.display()))
}

/// ffmpeg を `jobs` 個まで並列に起動する。失敗が出たら新しい起動をやめる。
/// 戻り値は引数と同じ並びで、起動しなかったものは None。
fn run_parallel(
    ffmpeg: &str,
    seg: &Path,
    commands: &[Vec<String>],
    jobs: usize,
) -> Vec<Option<Result<Duration, String>>> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let results: Mutex<Vec<Option<Result<Duration, String>>>> =
        Mutex::new(vec![None; commands.len()]);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, commands.len().max(1)) {
            s.spawn(|| loop {
                if failed.load(Ordering::SeqCst) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= commands.len() {
                    break;
                }
                let t = Instant::now();
                let r = run_checked(ffmpeg, &commands[i], seg).map(|_| t.elapsed());
                if r.is_err() {
                    failed.store(true, Ordering::SeqCst);
                }
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_video::frames::Interval;

    fn plan(cut: &str, sets: &[(i64, &[&str])]) -> CutPlan {
        CutPlan {
            cut_id: cut.into(),
            start_frame: 0,
            frames: 100,
            intervals: sets
                .iter()
                .map(|(r, ids)| Interval {
                    rel_start: *r,
                    items: ids.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn distinct_overlay_sets_are_written_once() {
        let plans = vec![
            plan(
                "S01-C01",
                &[(0, &[]), (10, &["A"]), (20, &[]), (30, &["A"])],
            ),
            plan("S01-C02", &[(0, &["A"]), (5, &["A", "B"])]),
        ];
        let (files, per_cut) = assign_overlay_files(&plans);
        assert_eq!(files.len(), 3, "{files:?}"); // 空・{A}・{A,B}
        assert_eq!(files[&Vec::<String>::new()], "ov_empty.png");
        assert_eq!(files[&vec!["A".to_string()]], "ov_0001.png");
        assert_eq!(
            files[&vec!["A".to_string(), "B".to_string()]],
            "ov_0002.png"
        );
        let names: Vec<&str> = per_cut[0].iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["ov_empty.png", "ov_0001.png", "ov_empty.png", "ov_0001.png"]
        );
        assert_eq!(per_cut[1][1], (5, "ov_0002.png".to_string()));
    }

    #[test]
    fn write_overlays_passes_the_clipped_set_to_drawing() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = RenderConfig {
            width: 320,
            height: 180,
            note_padding: 20,
            ..RenderConfig::default()
        };
        let items = vec![Item {
            id: "S01-N01".into(),
            kind: ItemKind::Note,
            text: "一二三四五".into(),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        }];
        // 文字が収まらない小さな枠
        let placed: BTreeMap<String, Placed> = [(
            "S01-N01".to_string(),
            Placed {
                kind: ItemKind::Note,
                x: 100,
                y: 50,
                w: 60,
                h: 60,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        )]
        .into();
        let files: BTreeMap<Vec<String>, String> =
            [(vec!["S01-N01".to_string()], "ov_0001.png".to_string())].into();

        // 切り詰めた印が無い項目は、収まらなければ失敗
        let d = tempfile::tempdir().unwrap();
        let e = write_overlays(
            d.path(),
            &files,
            &items,
            &placed,
            &BTreeSet::new(),
            &cfg,
            &face,
        )
        .unwrap_err();
        assert!(e.contains("S01-N01") && e.contains("収まりません"), "{e}");
        assert!(!d.path().join("ov_0001.png").exists());

        // 印がある項目は PNG が書かれ、枠(100..160, 50..110)の外は透明のまま
        let clipped: BTreeSet<String> = ["S01-N01".to_string()].into();
        write_overlays(d.path(), &files, &items, &placed, &clipped, &cfg, &face).unwrap();
        let pm = tiny_skia::Pixmap::load_png(d.path().join("ov_0001.png")).unwrap();
        assert_eq!((pm.width(), pm.height()), (320, 180));
        let mut inside = 0;
        for y in 0..180u32 {
            for x in 0..320u32 {
                let a = pm.pixel(x, y).unwrap().alpha();
                if (100..160).contains(&x) && (50..110).contains(&y) {
                    inside += u32::from(a > 0);
                } else {
                    assert_eq!(a, 0, "枠の外に描かれています: ({x}, {y})");
                }
            }
        }
        assert!(inside > 0);
    }

    #[test]
    fn manual_item_too_big_for_the_screen_is_clipped_inside_it() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = RenderConfig {
            width: 320,
            height: 180,
            ..RenderConfig::default()
        };
        let items = vec![Item {
            id: "S01-L001".into(),
            kind: ItemKind::Bubble,
            text: "あ".repeat(500),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        }];
        let manual: BTreeMap<String, Placed> = [(
            "S01-L001".to_string(),
            Placed {
                kind: ItemKind::Bubble,
                x: 200,
                y: 100,
                w: 60,
                h: 60,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        )]
        .into();
        let mut w = Vec::new();
        let Placement { placed, clipped } = place_items(&items, &manual, &cfg, &face, &mut w);
        let p = &placed["S01-L001"];
        assert!(p.x >= 0 && p.y >= 0, "{p:?}");
        assert!(p.x as u32 + p.w <= 320 && p.y as u32 + p.h <= 180, "{p:?}");
        assert!(clipped.contains("S01-L001"), "{clipped:?}");
        assert!(w.iter().any(|m| m.contains("画面")), "{w:?}");

        let files: BTreeMap<Vec<String>, String> =
            [(vec!["S01-L001".to_string()], "ov_0001.png".to_string())].into();
        let d = tempfile::tempdir().unwrap();
        write_overlays(d.path(), &files, &items, &placed, &clipped, &cfg, &face).unwrap();
        assert!(d.path().join("ov_0001.png").exists());
    }

    struct FailingWriter;
    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("書き込み失敗"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn commit_output_does_not_replace_when_the_log_cannot_be_written() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, out) = (d.path().join("t.mp4"), d.path().join("output.mp4"));
        std::fs::write(&tmp, "new").unwrap();
        std::fs::write(&out, "old").unwrap();
        let mut log = ProcLog::from_writer(Box::new(FailingWriter));
        let e = commit_output(&mut log, &tmp, &out, "ok").unwrap_err();
        assert!(e.contains("Process.log"), "{e}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "old");
        assert!(tmp.exists());
    }

    #[test]
    fn commit_output_replaces_and_reports_rename_failure() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, out) = (d.path().join("t.mp4"), d.path().join("output.mp4"));
        std::fs::write(&tmp, "new").unwrap();
        std::fs::write(&out, "old").unwrap();
        let mut log = ProcLog::from_writer(Box::new(Vec::<u8>::new()));
        commit_output(&mut log, &tmp, &out, "ok").unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "new");
        // 置き換え元が無ければ Err
        let e = commit_output(&mut log, &tmp, &out, "ok").unwrap_err();
        assert!(e.contains("置き換えられません"), "{e}");
    }

    #[test]
    fn version_check_gates_the_run() {
        let mut w = Vec::new();
        assert!(gate_version("ffmpeg version 5.1.2 Copyright", false, &mut w).is_err());
        assert!(gate_version("ffmpeg version 6.1.1 Copyright", false, &mut w).is_ok());
        assert!(gate_version(
            "ffmpeg version 2026-03-26-git-fd9f1e9c52-full_build",
            false,
            &mut w
        )
        .is_ok());
        assert!(w.is_empty());
        assert!(gate_version("ffmpeg version N-12345-gabcdef", false, &mut w).is_ok());
        assert_eq!(w.len(), 1, "{w:?}");
        // ffprobe も同じ規則
        assert!(gate_version("ffprobe version 5.1.2 Copyright", true, &mut w).is_err());
        assert!(gate_version("ffprobe version 7.0 Copyright", true, &mut w).is_ok());
        assert!(gate_version("ffprobe version N-1-gabc", true, &mut w).is_ok());
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn threads_split_cores_by_jobs() {
        assert_eq!(threads_for(8, 1), 8);
        assert_eq!(threads_for(8, 3), 2);
        assert_eq!(threads_for(2, 4), 1);
        assert_eq!(threads_for(1, 64), 1);
    }
}
