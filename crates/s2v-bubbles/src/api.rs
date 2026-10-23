//! GUI サーバーの API の中身(設計書 §5.4)。HTTP には依存しない。
//!
//! 項目の組み立て・配置・描画・コマの計算・検証は、`render` と同じ共有の関数
//! (`s2v_render::scene`・`s2v_render::bubbles::validate_file`・`s2v_video::frames`)を使う。
//! ここに同じ処理を書き直さない。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use s2v_core::config::{Fit, RenderConfig};
use s2v_render::assets::AssetKind;
use s2v_render::bubbles::{effective_times, validate_file, BubblesFile, Canvas, Placed};
use s2v_render::font::FontFace;
use s2v_render::layout::{fits, min_size};
use s2v_render::scene::{build_items, draw_item, layout_ctx, place_items, Item};
use s2v_render::ItemKind;
use s2v_timeline::Timeline;
use s2v_video::frames::{frame, plan_cuts, stacking_order, total_frames, ShownItem};

/// カットに対応づけた素材(起動時に照合・読み込み済みのもの)。
#[derive(Debug, Clone)]
pub struct SessionAsset {
    pub cut_id: String,
    pub kind: AssetKind,
    pub looped: bool,
    /// 動画の素材の長さ(出力の fps のコマ数)。画像は None
    pub frames: Option<i64>,
    pub matrix: String,
    pub range: String,
    pub path: PathBuf,
}

/// 起動時に用意した、API が使う情報一式。
pub struct Session {
    pub timeline: Timeline,
    pub cfg: RenderConfig,
    pub face: FontFace,
    /// 台本のフォルダ(`bubbles.json` の置き場所)
    pub dir: PathBuf,
    pub items: Vec<Item>,
    /// カット ID → 素材
    pub assets: BTreeMap<String, SessionAsset>,
    /// 現在保存されている `bubbles.json` の全項目(拡大縮小済み、timeline に無い ID を含む)。
    /// 起動時に読み込み、保存に成功するたびに保存した中身で置き換える。
    pub loaded: Mutex<BTreeMap<String, Placed>>,
    pub warnings: Vec<String>,
    /// 項目の画像の大きさの記憶(`/api/layout` が全項目を描き直さないため)。
    pub draw_dims: DrawDims,
}

/// 画像の原点と大きさ `(dx, dy, w, h)`、または描けない理由。
type DrawResult = Result<(i32, i32, u32, u32), String>;

/// 画像の大きさは項目の ID・枠の大きさ・尻尾・切り詰めの有無だけで決まる(位置 x・y には依存しない)。
/// 項目の本文と種類は起動中は変わらないので、鍵に入れない。
type DrawKey = (String, u32, u32, Option<(i32, i32)>, bool);

#[derive(Default)]
pub struct DrawDims(Mutex<HashMap<DrawKey, DrawResult>>);

#[derive(Debug, PartialEq)]
pub enum ApiError {
    BadRequest(String),
    NotFound,
    Unprocessable(Vec<String>),
    Internal(String),
}

const BUBBLES_FILE: &str = "bubbles.json";
const BAK_FILE: &str = "bubbles.json.bak";

fn canvas_of(cfg: &RenderConfig) -> Canvas {
    Canvas {
        w: cfg.width,
        h: cfg.height,
    }
}

/// 手動配置の一式で項目を組み立て直す(offset が表示時刻に効く)。
/// 逆転した offset は `validate_layout` が先に拒否するので、ここで失敗するのは検証を通していない入力だけ。
fn items_for(s: &Session, manual: &BTreeMap<String, Placed>) -> Result<Vec<Item>, ApiError> {
    build_items(&s.timeline, manual, &mut Vec::new()).map_err(|m| ApiError::Unprocessable(vec![m]))
}

fn shown(items: &[Item]) -> Vec<ShownItem> {
    items
        .iter()
        .map(|i| ShownItem {
            id: i.id.clone(),
            is_note: i.kind == ItemKind::Note,
            start_ms: i.start_ms,
            end_ms: i.end_ms,
            order: i.order,
        })
        .collect()
}

fn placed_json(p: &Placed) -> Value {
    json!({
        "x": p.x, "y": p.y, "w": p.w, "h": p.h,
        "tail": p.tail.map(|t| json!({"dx": t.dx, "dy": t.dy})),
        "start_offset_ms": p.start_offset_ms,
        "end_offset_ms": p.end_offset_ms,
    })
}

/// 全項目の配置の結果(`/api/state` の `items` と `/api/layout` の応答)と配置の警告。
fn items_value(
    s: &Session,
    items: &[Item],
    manual: &BTreeMap<String, Placed>,
) -> (Vec<Value>, Vec<String>) {
    let mut warnings = Vec::new();
    let placement = place_items(items, manual, &s.cfg, &s.face, &mut warnings);
    let rank: BTreeMap<String, usize> = stacking_order(&shown(items))
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    let mut scene_of: BTreeMap<&str, &str> = BTreeMap::new();
    for l in &s.timeline.lines {
        scene_of.insert(&l.id, &l.scene);
    }
    for sc in &s.timeline.scenes {
        for n in &sc.notes {
            scene_of.insert(&n.id, &sc.id);
        }
    }
    let fps = s.cfg.fps;
    let mut out = Vec::new();
    for it in items {
        let Some(p) = placement.placed.get(&it.id) else {
            continue;
        };
        let clipped = placement.clipped.contains(&it.id);
        let key: DrawKey = (it.id.clone(), p.w, p.h, p.tail.map(|t| (t.dx, t.dy)), clipped);
        let cached = s
            .draw_dims
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        let dims = cached.unwrap_or_else(|| {
            let d = draw_item(it, p, clipped, &s.cfg, &s.face)
                .map(|ov| (ov.dx, ov.dy, ov.pixmap.width(), ov.pixmap.height()));
            s.draw_dims
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, d.clone());
            d
        });
        let image = match dims {
            Ok((dx, dy, w, h)) => json!({"dx": dx, "dy": dy, "w": w, "h": h}),
            Err(m) => {
                warnings.push(format!("{}: {m}", it.id));
                json!({"dx": 0, "dy": 0, "w": 0, "h": 0})
            }
        };
        let (mw, mh) = min_size(&layout_ctx(&s.face, &s.cfg, it.kind), &it.text);
        out.push(json!({
            "id": it.id,
            "kind": match it.kind { ItemKind::Bubble => "bubble", ItemKind::Note => "note" },
            "scene": scene_of.get(it.id.as_str()).copied().unwrap_or(""),
            "text": it.text,
            "frames": [frame(it.start_ms, fps), frame(it.end_ms, fps)],
            "order": rank.get(&it.id).copied().unwrap_or(0),
            "manual": manual.get(&it.id).map(placed_json),
            "placed": placed_json(p),
            "image": image,
            "min_size": {"w": mw, "h": mh},
            "clipped": clipped,
        }));
    }
    (out, warnings)
}

fn asset_json(a: &SessionAsset) -> Value {
    let (kind, is_video) = match a.kind {
        AssetKind::Png => ("png", false),
        AssetKind::Jpeg => ("jpeg", false),
        AssetKind::Video => ("video", true),
    };
    json!({
        "url": format!("/media/asset/{}", a.cut_id),
        "kind": kind,
        "looped": a.looped,
        "frames": a.frames,
        "frame_url": is_video.then(|| format!("/media/frame/{}", a.cut_id)),
    })
}

/// カットのコマの範囲 `[n0, n1)`。コマ数 0 のカットは plan_cuts に含まれないので `[n0, n0]` にする。
fn cut_pair(
    plans: &[s2v_video::frames::CutPlan],
    c: &s2v_timeline::CutEntry,
    fps: u32,
) -> [i64; 2] {
    match plans.iter().find(|p| p.cut_id == c.id) {
        Some(p) => [p.start_frame, p.start_frame + p.frames],
        None => {
            let n0 = frame(c.start_ms, fps);
            [n0, n0]
        }
    }
}

/// カット `cut_id` のコマの範囲 `[n0, n1)`(`/api/state` の `frames` と同じ値。`/media/frame` の n の検査に使う)。
/// カットの範囲は項目の表示時刻に依らない。
pub fn cut_frames(s: &Session, cut_id: &str) -> Option<[i64; 2]> {
    let fps = s.cfg.fps;
    let cuts: Vec<(String, i64, i64)> = s
        .timeline
        .scenes
        .iter()
        .flat_map(|sc| sc.cuts.iter())
        .map(|c| (c.id.clone(), c.start_ms, c.end_ms))
        .collect();
    let plans = plan_cuts(&cuts, &[], s.timeline.duration_ms, fps);
    let c = s
        .timeline
        .scenes
        .iter()
        .flat_map(|sc| sc.cuts.iter())
        .find(|c| c.id == cut_id)?;
    Some(cut_pair(&plans, c, fps))
}

/// `/api/state` の応答。
pub fn state_json(s: &Session, manual: &BTreeMap<String, Placed>) -> Value {
    let fps = s.cfg.fps;
    // 起動時に検証済みの手動配置が渡される。逆転があれば手動を使わない項目に戻す
    let items = items_for(s, manual)
        .or_else(|_| items_for(s, &BTreeMap::new()))
        .unwrap_or_default();
    let (items_v, place_warnings) = items_value(s, &items, manual);
    let cut_ranges: Vec<(String, i64, i64)> = s
        .timeline
        .scenes
        .iter()
        .flat_map(|sc| sc.cuts.iter())
        .map(|c| (c.id.clone(), c.start_ms, c.end_ms))
        .collect();
    let plans = plan_cuts(&cut_ranges, &shown(&items), s.timeline.duration_ms, fps);
    let scenes: Vec<Value> = s
        .timeline
        .scenes
        .iter()
        .map(|sc| {
            let cuts: Vec<Value> = sc
                .cuts
                .iter()
                .map(|c| {
                    let frames = cut_pair(&plans, c, fps);
                    json!({
                        "id": c.id,
                        "frames": frames,
                        "asset": s.assets.get(&c.id).map(asset_json),
                    })
                })
                .collect();
            json!({"id": sc.id, "name": sc.name, "cuts": cuts})
        })
        .collect();
    let mut warnings = s.warnings.clone();
    warnings.extend(place_warnings);
    json!({
        "canvas": {"w": s.cfg.width, "h": s.cfg.height},
        "fps": fps,
        "total_frames": total_frames(s.timeline.duration_ms, fps),
        "fit": match s.cfg.fit { Fit::Contain => "contain", Fit::Cover => "cover" },
        "background": s.cfg.background,
        "scenes": scenes,
        "items": items_v,
        "warnings": warnings,
    })
}

/// `/api/layout` の応答(`state_json` の `items` と同じ形)。
/// 検証(未知の ID・上限・表示時刻の逆転)に外れれば Unprocessable。
pub fn layout_json(s: &Session, manual: &BTreeMap<String, Placed>) -> Result<Value, ApiError> {
    check_layout(s, manual)?;
    let items = items_for(s, manual)?;
    Ok(Value::Array(items_value(s, &items, manual).0))
}

/// timeline にある全ての項目 ID(長さ 0 で映像に出ないものも含む)。
fn known_ids(tl: &Timeline) -> BTreeSet<&str> {
    let mut ids: BTreeSet<&str> = tl.lines.iter().map(|l| l.id.as_str()).collect();
    for sc in &tl.scenes {
        ids.extend(sc.notes.iter().map(|n| n.id.as_str()));
    }
    ids
}

/// 手動配置の一式の検証(未知の ID と `validate_file`)。誤りの一覧(空なら正しい)。
fn validate_manual(s: &Session, manual: &BTreeMap<String, Placed>) -> Vec<String> {
    let known = known_ids(&s.timeline);
    let mut errs: Vec<String> = manual
        .keys()
        .filter(|id| !known.contains(id.as_str()))
        .map(|id| format!("{id}: timeline.json にない ID です"))
        .collect();
    errs.extend(validate_file(&BubblesFile {
        version: 1,
        canvas: canvas_of(&s.cfg),
        items: manual.clone(),
    }));
    errs
}

/// 台本の項目の (種類, 本文, 開始, 終了)(長さ 0 で映像に出ないものも含む)。
type BaseItems<'a> = BTreeMap<&'a str, (ItemKind, &'a str, i64, i64)>;

fn base_items(tl: &Timeline) -> BaseItems<'_> {
    let mut base = BTreeMap::new();
    for l in &tl.lines {
        base.insert(
            l.id.as_str(),
            (ItemKind::Bubble, l.text.as_str(), l.start_ms, l.end_ms),
        );
    }
    for sc in &tl.scenes {
        for n in &sc.notes {
            base.insert(
                n.id.as_str(),
                (ItemKind::Note, n.text.as_str(), n.start_ms, n.end_ms),
            );
        }
    }
    base
}

/// 配置を計算する API(render・layout・save)共通の検証: 未知の ID・上限(`validate_file`)・表示時刻の逆転。
fn validate_layout(s: &Session, manual: &BTreeMap<String, Placed>) -> Vec<String> {
    let mut errs = validate_manual(s, manual);
    let base = base_items(&s.timeline);
    for (id, p) in manual {
        if let Some(&(_, _, st, en)) = base.get(id.as_str()) {
            if let Err(m) = effective_times(st, en, Some(p), s.timeline.duration_ms) {
                errs.push(format!("{id}: {m}"));
            }
        }
    }
    errs
}

fn check_layout(s: &Session, manual: &BTreeMap<String, Placed>) -> Result<(), ApiError> {
    let errs = validate_layout(s, manual);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Unprocessable(errs))
    }
}

#[derive(serde::Deserialize)]
struct ManualBody {
    manual: BTreeMap<String, Placed>,
}

/// 要求の本文 `{"manual":{…}}` を読んで検証する(`/api/render` の本文のように他のキーがあってもよい)。
pub fn parse_manual(body: &[u8], s: &Session) -> Result<BTreeMap<String, Placed>, ApiError> {
    let b: ManualBody = serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("JSON を読めません: {e}")))?;
    let errs = validate_manual(s, &b.manual);
    if errs.is_empty() {
        Ok(b.manual)
    } else {
        Err(ApiError::Unprocessable(errs))
    }
}

/// 項目 `id` の PNG を描く。全体の配置を計算し直し、その中の `id` を描く(`render` と同じ関数)。
/// 戻り値は (PNG, dx, dy, clipped)。
pub fn render_png(
    s: &Session,
    id: &str,
    manual: &BTreeMap<String, Placed>,
) -> Result<(Vec<u8>, i32, i32, bool), ApiError> {
    check_layout(s, manual)?;
    let items = items_for(s, manual)?;
    let item = items
        .iter()
        .find(|i| i.id == id)
        .ok_or(ApiError::NotFound)?;
    // place_items が広げる前に、手動の大きさで文字が収まるかを見る
    if let Some(p) = manual.get(id) {
        let ctx = layout_ctx(&s.face, &s.cfg, item.kind);
        if !fits(&ctx, &item.text, p.w, p.h) {
            return Err(ApiError::Unprocessable(vec![format!(
                "{id}: 文字が枠に収まりません"
            )]));
        }
    }
    let placement = place_items(&items, manual, &s.cfg, &s.face, &mut Vec::new());
    let placed = placement.placed.get(id).ok_or(ApiError::NotFound)?;
    let clipped = placement.clipped.contains(id);
    let ov = draw_item(item, placed, clipped, &s.cfg, &s.face).map_err(ApiError::Internal)?;
    let png = ov
        .pixmap
        .encode_png()
        .map_err(|_| ApiError::Internal("PNG を作れません".into()))?;
    Ok((png, ov.dx, ov.dy, clipped))
}

/// 同じフォルダに一時ファイル(`.<名前>.<乱数16進>.tmp`)を `create_new` で作り、
/// 書いて `sync_all` し、`rename` で置き換える。失敗時は一時ファイルを消す。
pub fn atomic_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    atomic_replace_with(path, bytes, &|_, _| Ok(()))
}

/// 失敗の注入の段階(テスト用。製品の経路では何もしない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// 一時ファイルへ書く前
    Write,
    /// 置き換え(`rename`)の前
    Replace,
}

fn atomic_replace_with(
    path: &Path,
    bytes: &[u8],
    hook: &dyn Fn(&Path, Stage) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "ファイル名がありません")
        })?
        .to_string_lossy()
        .into_owned();
    let mut last_err = None;
    for _ in 0..8 {
        let mut r = [0u8; 8];
        getrandom::fill(&mut r).map_err(|_| std::io::Error::other("乱数を得られません"))?;
        let hex: String = r.iter().map(|b| format!("{b:02x}")).collect();
        let tmp = dir.join(format!(".{name}.{hex}.tmp"));
        let mut f = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        let res = hook(path, Stage::Write)
            .and_then(|_| f.write_all(bytes))
            .and_then(|_| f.sync_all())
            .and_then(|_| {
                drop(f);
                hook(path, Stage::Replace)
            })
            .and_then(|_| fs::rename(&tmp, path));
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        return res;
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("一時ファイルを作れません")))
}

/// 保存先がリンクか、あるのに通常のファイルでなければ Err。無い、または通常のファイルなら Ok(存在するか)。
fn check_target(path: &Path) -> Result<bool, ApiError> {
    let bad = || ApiError::Internal("保存先が通常のファイルではないため保存できません".into());
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() || !m.file_type().is_file() => Err(bad()),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ApiError::Internal("保存先を調べられません".into())),
    }
}

/// 検証して `bubbles.json` を保存する(設計書 §5.2)。戻り値は警告。
pub fn save(s: &Session, manual: &BTreeMap<String, Placed>) -> Result<Vec<String>, ApiError> {
    save_with(s, manual, &|_, _| Ok(()))
}

fn save_with(
    s: &Session,
    manual: &BTreeMap<String, Placed>,
    hook: &dyn Fn(&Path, Stage) -> std::io::Result<()>,
) -> Result<Vec<String>, ApiError> {
    // render・layout と同じ検証に、各項目の文字が収まる大きさかを足す
    let mut errs = validate_layout(s, manual);
    let base = base_items(&s.timeline);
    for (id, p) in manual {
        let Some(&(kind, text, _, _)) = base.get(id.as_str()) else {
            continue; // 未知の ID は報告済み
        };
        if p.kind == kind && !fits(&layout_ctx(&s.face, &s.cfg, kind), text, p.w, p.h) {
            errs.push(format!("{id}: 文字が枠に収まりません"));
        }
    }
    if !errs.is_empty() {
        return Err(ApiError::Unprocessable(errs));
    }

    // 中身 = 要求の手動配置 + 読み込んだもののうち timeline に無い ID
    let known = known_ids(&s.timeline);
    let mut items = manual.clone();
    for (id, p) in s.loaded.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !known.contains(id.as_str()) {
            items.insert(id.clone(), p.clone());
        }
    }
    let file = BubblesFile {
        version: 1,
        canvas: canvas_of(&s.cfg),
        items,
    };
    let mut text = serde_json::to_string_pretty(&file)
        .map_err(|_| ApiError::Internal("保存の内容を作れません".into()))?;
    text.push('\n');

    // 何かを変える前に、保存先を全て調べる
    let main = s.dir.join(BUBBLES_FILE);
    let bak = s.dir.join(BAK_FILE);
    let main_exists = check_target(&main)?;
    check_target(&bak)?;
    if main_exists {
        let old = fs::read(&main)
            .map_err(|_| ApiError::Internal("既存の bubbles.json を読めません".into()))?;
        atomic_replace_with(&bak, &old, hook)
            .map_err(|_| ApiError::Internal("bubbles.json.bak を書けません".into()))?;
    }
    atomic_replace_with(&main, text.as_bytes(), hook)
        .map_err(|_| ApiError::Internal("bubbles.json を書けません".into()))?;

    // 保存に成功したので、以後の /api/state と未知の ID の引き継ぎは保存した中身を使う
    *s.loaded.lock().unwrap_or_else(|e| e.into_inner()) = file.items;

    let items = items_for(s, manual)?;
    let mut warnings = Vec::new();
    place_items(&items, manual, &s.cfg, &s.face, &mut warnings);
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use s2v_core::config::RenderConfig;
    use s2v_render::bubbles::{parse_bubbles, Placed, Tail};
    use s2v_render::font::FontFace;
    use s2v_render::scene::{build_items, draw_item, place_items};
    use s2v_render::ItemKind;
    use s2v_timeline::{CutEntry, LineEntry, Normalize, NoteEntry, SceneEntry, Timeline};
    use s2v_video::frames::{frame, plan_cuts, stacking_order, ShownItem};

    fn line(id: &str, text: &str, s: i64, e: i64) -> LineEntry {
        LineEntry {
            id: id.into(),
            scene: "S01".into(),
            cast: "A".into(),
            text: text.into(),
            start_ms: s,
            end_ms: e,
            take: id.into(),
            track: None,
            measured_lufs: None,
            requested_gain_db: None,
            norm_gain_db: None,
            norm_clamped: None,
            src_start_ms: None,
            src_end_ms: None,
            missing: false,
            reason: None,
        }
    }

    fn cut(id: &str, s: i64, e: i64) -> CutEntry {
        CutEntry {
            id: id.into(),
            start_ms: s,
            end_ms: e,
            visual: None,
        }
    }

    /// C01 0–3000、C02 3000–6000、C03 長さ 0(6000)。台詞 L001・L002、ト書き N01。
    fn timeline(l1_text: &str) -> Timeline {
        Timeline {
            schema_version: 3,
            script: "s.txt".into(),
            script_sha256: "0".repeat(64),
            audio_sha256: "0".repeat(64),
            sample_rate: 48_000,
            duration_ms: 6_000,
            normalize: Normalize { target_lufs: -16.0 },
            cast: vec![],
            scenes: vec![SceneEntry {
                id: "S01".into(),
                name: Some("朝".into()),
                index: 1,
                silent: false,
                start_ms: 0,
                end_ms: 6_000,
                cuts: vec![
                    cut("S01-C01", 0, 3_000),
                    cut("S01-C02", 3_000, 6_000),
                    cut("S01-C03", 6_000, 6_000),
                ],
                notes: vec![NoteEntry {
                    id: "S01-N01".into(),
                    text: "翌朝。".into(),
                    start_ms: 0,
                    end_ms: 2_000,
                }],
            }],
            takes: vec![],
            lines: vec![
                line("S01-L001", l1_text, 500, 2_000),
                line("S01-L002", "さようなら。", 2_500, 3_500),
            ],
            events: vec![],
        }
    }

    fn font() -> FontFace {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../s2v-render/tests/fonts/test-vrt2.ttf");
        FontFace::load(&p, 0).unwrap()
    }

    fn session_in(dir: &Path, l1_text: &str) -> Session {
        let cfg = RenderConfig {
            width: 320,
            height: 180,
            ..RenderConfig::default()
        };
        let timeline = timeline(l1_text);
        let items = build_items(&timeline, &BTreeMap::new(), &mut Vec::new()).unwrap();
        Session {
            timeline,
            cfg,
            face: font(),
            dir: dir.to_path_buf(),
            items,
            assets: BTreeMap::new(),
            loaded: Mutex::new(BTreeMap::new()),
            warnings: vec![],
            draw_dims: Default::default(),
        }
    }

    fn bubble(x: i32, y: i32, w: u32, h: u32) -> Placed {
        Placed {
            kind: ItemKind::Bubble,
            x,
            y,
            w,
            h,
            tail: None,
            start_offset_ms: 0,
            end_offset_ms: 0,
        }
    }

    fn one(id: &str, p: Placed) -> BTreeMap<String, Placed> {
        BTreeMap::from([(id.to_string(), p)])
    }

    #[test]
    fn state_matches_render_frames_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あいう");
        let v = state_json(&s, &BTreeMap::new());
        let fps = s.cfg.fps;

        let shown: Vec<ShownItem> = s
            .items
            .iter()
            .map(|i| ShownItem {
                id: i.id.clone(),
                is_note: i.kind == ItemKind::Note,
                start_ms: i.start_ms,
                end_ms: i.end_ms,
                order: i.order,
            })
            .collect();
        let cuts: Vec<(String, i64, i64)> = s.timeline.scenes[0]
            .cuts
            .iter()
            .map(|c| (c.id.clone(), c.start_ms, c.end_ms))
            .collect();
        let plans = plan_cuts(&cuts, &shown, s.timeline.duration_ms, fps);
        let jcuts = v["scenes"][0]["cuts"].as_array().unwrap();
        assert_eq!(jcuts.len(), 3);
        for p in &plans {
            let c = jcuts.iter().find(|c| c["id"] == p.cut_id.as_str()).unwrap();
            assert_eq!(
                c["frames"],
                serde_json::json!([p.start_frame, p.start_frame + p.frames])
            );
        }
        // コマ数 0 のカットは [n0, n0]
        let n0 = frame(6_000, fps);
        assert_eq!(jcuts[2]["frames"], serde_json::json!([n0, n0]));
        assert_eq!(v["canvas"], serde_json::json!({"w": 320, "h": 180}));
        assert_eq!(v["total_frames"], serde_json::json!(180));
        assert_eq!(v["fit"], "contain");

        let order = stacking_order(&shown);
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        for it in items {
            let src = s.items.iter().find(|i| it["id"] == i.id.as_str()).unwrap();
            assert_eq!(
                it["frames"],
                serde_json::json!([frame(src.start_ms, fps), frame(src.end_ms, fps)])
            );
            let rank = order.iter().position(|id| it["id"] == id.as_str()).unwrap();
            assert_eq!(it["order"], serde_json::json!(rank));
            assert_eq!(it["scene"], "S01");
            assert!(it["image"]["w"].as_u64().unwrap() > 0);
            assert!(it["min_size"]["w"].as_u64().unwrap() > 0);
        }
        // ト書きが最初
        let n = items.iter().find(|i| i["id"] == "S01-N01").unwrap();
        assert_eq!(n["order"], 0);
        assert_eq!(n["manual"], serde_json::Value::Null);
    }

    #[test]
    fn state_json_escapes_script_text() {
        let dir = tempfile::tempdir().unwrap();
        let text = "<script>alert(1)</script>\"\n";
        let s = session_in(dir.path(), text);
        let v = state_json(&s, &BTreeMap::new());
        let raw = serde_json::to_string(&v).unwrap();
        assert!(!raw.contains('\n')); // 改行は \n にエスケープされる
        assert!(raw.contains("\\\"\\n")); // 引用符もエスケープされる
        let back: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let it = back["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "S01-L001")
            .unwrap();
        assert_eq!(it["text"], text);
    }

    #[test]
    fn render_rejects_manual_that_does_not_fit_and_draws_clipped_auto() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あ");
        // 10x10 の手動配置は 422
        let r = render_png(&s, "S01-L001", &one("S01-L001", bubble(10, 10, 10, 10)));
        assert!(matches!(r, Err(ApiError::Unprocessable(_))), "{r:?}");
        // 未知の ID は NotFound
        assert_eq!(
            render_png(&s, "S01-L099", &BTreeMap::new()).unwrap_err(),
            ApiError::NotFound
        );
        // 尻尾つき: dx・dy が draw_item の Overlay と一致する
        let mut p = bubble(140, 50, 120, 120);
        p.tail = Some(Tail { dx: -50, dy: -40 });
        let manual = one("S01-L001", p.clone());
        let (png, dx, dy, clipped) = render_png(&s, "S01-L001", &manual).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(!clipped);
        let ov = draw_item(&s.items[0], &p, false, &s.cfg, &s.face).unwrap();
        assert_eq!((dx, dy), (ov.dx, ov.dy));
        assert!(dx != 0 || dy != 0);
        // 画面より大きい min_size の長文の自動項目は、切り詰めて描く
        let long = "あ".repeat(500);
        let s2 = session_in(dir.path(), &long);
        let (png, _, _, clipped) = render_png(&s2, "S01-L001", &BTreeMap::new()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(clipped);
        // 全体の配置と同じ位置・大きさを使う
        let items = build_items(&s2.timeline, &BTreeMap::new(), &mut Vec::new()).unwrap();
        let pl = place_items(&items, &BTreeMap::new(), &s2.cfg, &s2.face, &mut Vec::new());
        assert!(pl.clipped.contains("S01-L001"));
    }

    #[test]
    fn parse_manual_validates() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あ");
        let body = |m: &str| format!("{{\"manual\":{m}}}").into_bytes();
        let item = |id: &str, kind: &str, x: i32, so: i64| {
            format!(
                "\"{id}\":{{\"kind\":\"{kind}\",\"x\":{x},\"y\":10,\"w\":100,\"h\":100,\
                 \"tail\":null,\"start_offset_ms\":{so},\"end_offset_ms\":0}}"
            )
        };
        let ok = parse_manual(
            &body(&format!("{{{}}}", item("S01-L001", "bubble", 10, 0))),
            &s,
        );
        assert_eq!(ok.unwrap().len(), 1);
        let unprocessable = |b: Vec<u8>| {
            assert!(
                matches!(parse_manual(&b, &s), Err(ApiError::Unprocessable(_))),
                "{}",
                String::from_utf8_lossy(&b[..b.len().min(200)])
            );
        };
        // 未知の ID
        unprocessable(body(&format!("{{{}}}", item("S09-L001", "bubble", 10, 0))));
        // 種類の不一致
        unprocessable(body(&format!("{{{}}}", item("S01-L001", "note", 10, 0))));
        // 画面の外
        unprocessable(body(&format!("{{{}}}", item("S01-L001", "bubble", 300, 0))));
        // |offset| > 60000
        unprocessable(body(&format!(
            "{{{}}}",
            item("S01-L001", "bubble", 10, 60_001)
        )));
        // 10001 件
        let many: Vec<String> = (0..10_001)
            .map(|i| item(&format!("S01-L{i:05}"), "bubble", 10, 0))
            .collect();
        unprocessable(body(&format!("{{{}}}", many.join(","))));
        // JSON の誤り
        assert!(matches!(
            parse_manual(b"{not json", &s),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            parse_manual(b"{}", &s),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn save_writes_bak_and_keeps_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あ");
        s.loaded
            .lock()
            .unwrap()
            .insert("S09-L001".into(), bubble(20, 20, 60, 60));
        let original = br#"{"version":1,"canvas":{"w":320,"h":180},"items":{}}"#.to_vec();
        std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
        let manual = one("S01-L001", bubble(100, 40, 120, 120));
        save(&s, &manual).unwrap();
        let saved = std::fs::read_to_string(dir.path().join("bubbles.json")).unwrap();
        let file = parse_bubbles(&saved).unwrap();
        let ids: Vec<&str> = file.items.keys().map(String::as_str).collect();
        assert_eq!(ids, vec!["S01-L001", "S09-L001"]);
        assert_eq!((file.canvas.w, file.canvas.h), (320, 180));
        assert_eq!(
            std::fs::read(dir.path().join("bubbles.json.bak")).unwrap(),
            original
        );
        let tmp: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(tmp.is_empty(), "{tmp:?}");
        // 要求の新しい未知の ID は 422
        let bad = one("S09-L777", bubble(1, 1, 50, 50));
        assert!(matches!(save(&s, &bad), Err(ApiError::Unprocessable(_))));
    }

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

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn save_failures_keep_original() {
        let manual = one("S01-L001", bubble(100, 40, 120, 120));
        let original = b"ORIGINAL".to_vec();
        let setup = || {
            let dir = tempfile::tempdir().unwrap();
            let s = session_in(dir.path(), "あ");
            (dir, s)
        };

        // (a) bubbles.json.bak がフォルダ
        let (dir, s) = setup();
        std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
        std::fs::create_dir(dir.path().join("bubbles.json.bak")).unwrap();
        assert!(matches!(save(&s, &manual), Err(ApiError::Internal(_))));
        assert_eq!(
            std::fs::read(dir.path().join("bubbles.json")).unwrap(),
            original
        );
        assert_eq!(names(dir.path()), vec!["bubbles.json", "bubbles.json.bak"]);

        // (b) bubbles.json がフォルダ
        let (dir, s) = setup();
        std::fs::create_dir(dir.path().join("bubbles.json")).unwrap();
        assert!(matches!(save(&s, &manual), Err(ApiError::Internal(_))));
        assert_eq!(names(dir.path()), vec!["bubbles.json"]);
        assert!(dir.path().join("bubbles.json").is_dir());

        // (c) bubbles.json・.bak がリンク(Windows はジャンクションでも可)
        for link_name in ["bubbles.json", "bubbles.json.bak"] {
            let (dir, s) = setup();
            let foreign = tempfile::tempdir().unwrap();
            if !try_dir_link(&dir.path().join(link_name), foreign.path()) {
                if cfg!(windows) {
                    panic!("リンクを作れません(テストの準備の不具合)");
                }
                continue;
            }
            if link_name == "bubbles.json.bak" {
                std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
            }
            assert!(
                matches!(save(&s, &manual), Err(ApiError::Internal(_))),
                "{link_name}"
            );
            assert!(names(foreign.path()).is_empty(), "{link_name}");
            if link_name == "bubbles.json.bak" {
                assert_eq!(
                    std::fs::read(dir.path().join("bubbles.json")).unwrap(),
                    original
                );
            }
        }

        // (d) offset の逆転・収まらない大きさ → 422、何も変えない
        let (dir, s) = setup();
        std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
        let mut inv = bubble(100, 40, 120, 120);
        inv.start_offset_ms = 2_000; // 500+2000 = 2500 >= 終了 2000
        assert!(matches!(
            save(&s, &one("S01-L001", inv)),
            Err(ApiError::Unprocessable(_))
        ));
        assert!(matches!(
            save(&s, &one("S01-L001", bubble(10, 10, 10, 10))),
            Err(ApiError::Unprocessable(_))
        ));
        assert_eq!(
            std::fs::read(dir.path().join("bubbles.json")).unwrap(),
            original
        );
        assert_eq!(names(dir.path()), vec!["bubbles.json"]);
    }

    #[test]
    fn inverted_times_are_rejected_by_render_layout_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あ");
        let mut inv = bubble(100, 40, 120, 120);
        inv.start_offset_ms = 2_000; // 500+2000 >= 終了 2000
        let manual = one("S01-L001", inv);
        let r = render_png(&s, "S01-L001", &manual);
        assert!(matches!(r, Err(ApiError::Unprocessable(_))), "{r:?}");
        let r = layout_json(&s, &manual);
        assert!(matches!(r, Err(ApiError::Unprocessable(_))), "{r:?}");
        assert!(matches!(save(&s, &manual), Err(ApiError::Unprocessable(_))));
        assert!(layout_json(&s, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn repeated_save_replaces_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = session_in(dir.path(), "あ");
        // 最初に本体(A)と .bak(Z)の両方を置く
        std::fs::write(dir.path().join("bubbles.json"), b"AAA").unwrap();
        std::fs::write(dir.path().join("bubbles.json.bak"), b"ZZZ").unwrap();
        save(&s, &one("S01-L001", bubble(100, 40, 120, 120))).unwrap();
        let b = std::fs::read(dir.path().join("bubbles.json")).unwrap();
        assert_ne!(b, b"AAA");
        assert_eq!(
            std::fs::read(dir.path().join("bubbles.json.bak")).unwrap(),
            b"AAA"
        );
        save(&s, &one("S01-L001", bubble(20, 30, 120, 120))).unwrap();
        let c = std::fs::read(dir.path().join("bubbles.json")).unwrap();
        assert_ne!(b, c);
        assert_eq!(
            std::fs::read(dir.path().join("bubbles.json.bak")).unwrap(),
            b
        );
        assert!(names(dir.path()).iter().all(|n| !n.ends_with(".tmp")));
    }

    #[test]
    fn injected_failures_in_save_stages() {
        let original = b"ORIGINAL".to_vec();
        let manual = one("S01-L001", bubble(100, 40, 120, 120));
        let fail_on = |file: &'static str, stage: Stage| {
            move |p: &Path, st: Stage| {
                if st == stage && p.file_name().unwrap() == file {
                    Err(std::io::Error::other("注入した失敗"))
                } else {
                    Ok(())
                }
            }
        };
        for stage in [Stage::Write, Stage::Replace] {
            // (a) .bak の段階 → 何も変わらない
            let dir = tempfile::tempdir().unwrap();
            let s = session_in(dir.path(), "あ");
            std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
            std::fs::write(dir.path().join("bubbles.json.bak"), b"ZZZ").unwrap();
            let r = save_with(&s, &manual, &fail_on("bubbles.json.bak", stage));
            assert!(matches!(r, Err(ApiError::Internal(_))), "{stage:?}");
            assert_eq!(
                std::fs::read(dir.path().join("bubbles.json")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(dir.path().join("bubbles.json.bak")).unwrap(),
                b"ZZZ"
            );
            assert_eq!(
                names(dir.path()),
                vec!["bubbles.json", "bubbles.json.bak"],
                "{stage:?}"
            );

            // (b) 本体の段階 → bubbles.json は元のまま、.bak は新しくなる
            let dir = tempfile::tempdir().unwrap();
            let s = session_in(dir.path(), "あ");
            std::fs::write(dir.path().join("bubbles.json"), &original).unwrap();
            let r = save_with(&s, &manual, &fail_on("bubbles.json", stage));
            assert!(matches!(r, Err(ApiError::Internal(_))), "{stage:?}");
            assert_eq!(
                std::fs::read(dir.path().join("bubbles.json")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(dir.path().join("bubbles.json.bak")).unwrap(),
                original
            );
            assert_eq!(
                names(dir.path()),
                vec!["bubbles.json", "bubbles.json.bak"],
                "{stage:?}"
            );
        }
    }
}
