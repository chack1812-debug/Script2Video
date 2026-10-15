//! `render` と `bubbles` が共有する起動時の検査(映像工程設計書 §4.1・§5.1)。
//! 予約名・設定・ロックの検査(`preflight`)と、鮮度・フォント・ffmpeg・項目・配置・素材の検査(`prepare`)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use s2v_core::config::RenderConfig;
use s2v_core::Config;
use s2v_render::assets::{match_assets, Asset, AssetKind};
use s2v_render::bubbles::{parse_bubbles, scale_to, BubblesFile, Placed};
use s2v_render::font::{FontFace, VerticalFeature};
use s2v_render::scene::{build_items, missing_glyph_warnings, place_items, Item, Placement};
use s2v_render::ItemKind;
use s2v_timeline::{Timeline, VisualKind};
use s2v_video::ffmpeg::{check_ffprobe_version, check_version, Background, VersionCheck};
use s2v_video::frames::{plan_cuts, CutPlan, ShownItem};
use s2v_video::probe::{asset_probe_args, parse_probe, video_color_input, Probe, Stream};

use crate::layout::{check_not_symlink, check_reserved_name, normalize_path, script_dir};
use crate::lock::ScriptLock;
use crate::proclog::ProcLog;

/// Process.log を開く前の検査を通ったもの。`lock` は持っている間だけ台本フォルダを占有する。
pub struct Preflight {
    pub config: Config,
    pub dir: PathBuf,
    pub lock: ScriptLock,
}

/// 予約名・リンク・台本の有無・設定の読み込みと検証・ロック。Err は標準エラーにだけ出す文。
pub fn preflight(script: &Path, config: Option<&Path>) -> Result<Preflight, String> {
    check_reserved_name(script).map_err(|e| format!("{e:#}"))?;
    check_not_symlink(script).map_err(|e| format!("{e:#}"))?;
    if !script.is_file() {
        return Err(format!("台本を読めません: {}", script.display()));
    }
    let config = match config {
        Some(p) => Config::from_file(p)
            .map_err(|e| format!("設定ファイルを読めません: {}: {e:#}", p.display()))?,
        None => Config::builtin(),
    };
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        return Err(config_errors.join("\n[ERROR] "));
    }
    let dir = script_dir(script);
    let lock = ScriptLock::acquire(&dir).map_err(|e| format!("{e:#}"))?;
    Ok(Preflight { config, dir, lock })
}

/// 素材が足りないときの扱い。`render` は止め、`bubbles` は警告して続ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingAssets {
    Error,
    Allow,
}

/// カットに使う素材(ffprobe で中身を確かめたもの)。
pub struct PreparedAsset {
    pub cut_id: String,
    pub path: PathBuf,
    pub kind: AssetKind,
    pub looped: bool,
    pub background: Background,
    /// 素材の ffprobe の映像ストリーム(`asset_frames` の計算に使う)。
    pub video: Option<Stream>,
}

pub struct Prepared {
    pub timeline: Timeline,
    pub face: FontFace,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub manual: BTreeMap<String, Placed>,
    pub items: Vec<Item>,
    pub placement: Placement,
    /// カットのコマの割り当て(`render` が使う)。
    pub plans: Vec<CutPlan>,
    /// カット ID → 素材。不足していて `default_image` も無いカットは入らない。
    pub assets: BTreeMap<String, PreparedAsset>,
    /// 素材が不足しているカット ID(`default_image` で補うものも含む)。
    pub missing: Vec<String>,
    /// Process.log に書いた警告(全部)。
    pub warnings: Vec<String>,
}

/// 警告を Process.log に書き、`all` にも控える。
fn warn(log: &mut ProcLog, all: &mut Vec<String>, msg: impl Into<String>) {
    let msg = msg.into();
    log.warn(&msg);
    all.push(msg);
}

/// 鮮度(音声も確かめる)・フォント・ffmpeg/ffprobe の版・`bubbles.json`・項目・配置・素材の照合と中身の検査。
pub fn prepare(
    script: &Path,
    cfg: &RenderConfig,
    dir: &Path,
    missing: MissingAssets,
    log: &mut ProcLog,
) -> Result<Prepared, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("現在のフォルダを取得できません: {e}"))?;
    let mut all_warnings: Vec<String> = Vec::new();

    // 鮮度
    let fresh = crate::freshness::check_fresh(script, true)?;
    for w in &fresh.warnings {
        warn(log, &mut all_warnings, w);
    }
    let tl = fresh.timeline;

    // フォント・ffmpeg・ffprobe
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
        warn(log, &mut all_warnings, w);
    }

    // bubbles.json
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

    // 項目
    let mut item_warnings = Vec::new();
    let items = build_items(&tl, &manual, &mut item_warnings)?;
    item_warnings.extend(missing_glyph_warnings(&face, &items));

    // 大きさと位置
    let placement = place_items(&items, &manual, cfg, &face, &mut item_warnings);
    for w in &item_warnings {
        warn(log, &mut all_warnings, w);
    }

    // コマの割り当てと素材
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
            warn(
                log,
                &mut all_warnings,
                format!("{id}: コマ数が 0 なので映像に出しません"),
            );
        } else if s >= e {
            warn(
                log,
                &mut all_warnings,
                format!("{id}: 長さが 0 のカットです"),
            );
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
    if missing == MissingAssets::Error && !matched.missing.is_empty() && default_image.is_none() {
        return Err(format!(
            "素材が足りません(tracks/ に置くか、config.toml の render.default_image を指定してください): {}",
            matched.missing.join("、")
        ));
    }
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
    let assets = resolve_assets(
        &ffprobe,
        dir, // 作業フォルダはまだ無いので、台本フォルダを作業フォルダにする
        &cuts_looped,
        &matched.found,
        default_image.as_ref().map(|(p, k)| (p.as_path(), *k)),
        &cwd,
        &mut asset_warnings,
    );
    for w in &asset_warnings {
        warn(log, &mut all_warnings, w);
    }
    let assets = assets?;

    Ok(Prepared {
        timeline: tl,
        face,
        ffmpeg,
        ffprobe,
        manual,
        items,
        placement,
        plans,
        assets,
        missing: matched.missing,
        warnings: all_warnings,
    })
}

/// カットごとの素材(ffprobe で中身を確かめたもの)。素材も `default_image` も無いカットは警告して除く。
fn resolve_assets(
    ffprobe: &str,
    work_dir: &Path,
    cuts: &[(String, bool)],
    found: &BTreeMap<String, Asset>,
    default_image: Option<(&Path, AssetKind)>,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<String, PreparedAsset>, String> {
    let abs = |p: &Path| normalize_path(p, cwd).to_string_lossy().into_owned();
    let mut out = BTreeMap::new();
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
                None => {
                    warnings.push(format!("{cut_id}: 素材なし"));
                    continue;
                }
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
        let background = match kind {
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
        };
        out.insert(
            cut_id.clone(),
            PreparedAsset {
                cut_id: cut_id.clone(),
                path,
                kind,
                looped: *looped,
                background,
                video: Some(video.clone()),
            },
        );
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors.join("\n"))
    }
}

/// 標準エラーの末尾 `n` 行。
fn tail_lines(bytes: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// 外部コマンドを引数の配列で起動する(シェルを経由しない)。標準入力は閉じ、出力は受け取る。
pub(crate) fn run_tool(
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
pub(crate) fn run_checked(exe: &str, args: &[String], cwd: &Path) -> Result<Vec<u8>, String> {
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

pub(crate) fn probe_file(ffprobe: &str, args: &[String], cwd: &Path) -> Result<Probe, String> {
    let out = run_checked(ffprobe, args, cwd)?;
    parse_probe(&String::from_utf8_lossy(&out))
}

/// パス区切りを含む相対パスは、カレントディレクトリを変えて起動しても通るよう絶対パスにする。
pub(crate) fn tool_path(configured: &Option<String>, default: &str, cwd: &Path) -> String {
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
pub(crate) fn tool_version(label: &str, exe: &str) -> Result<String, String> {
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
pub(crate) fn gate_version(
    first_line: &str,
    probe: bool,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
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
pub(crate) fn check_tools(
    ffmpeg: &str,
    ffprobe: &str,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    gate_version(&tool_version("ffmpeg", ffmpeg)?, false, warnings)?;
    gate_version(&tool_version("ffprobe", ffprobe)?, true, warnings)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
