//! 項目(吹き出し・ト書き)の組み立て・配置・描画。`render` と `bubbles` が同じ関数を使う。

use std::collections::{BTreeMap, BTreeSet};

use s2v_core::config::RenderConfig;
use s2v_timeline::Timeline;

use crate::autolayout::{auto_layout, AutoParams, LayoutItem};
use crate::bubbles::{effective_times, expand_to_min, Canvas, Placed};
use crate::draw::{composite, render as render_item, render_clipped, ItemSpec, Overlay, Style};
use crate::font::FontFace;
use crate::layout::{fits, min_size, LayoutContext};
use crate::ItemKind;

/// 台本の項目(吹き出しまたはト書き)。時刻は `effective_times` を通した表示時刻。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 台本の並び(台詞が先、ト書きが後。同じ種類の中の順序だけが意味を持つ)
    pub order: usize,
}

/// timeline.json から項目を作り、`bubbles.json` の offset を反映する。
/// 長さ 0 の項目は警告して除く。`timeline` に無い手動配置の ID は警告する。
pub fn build_items(
    tl: &Timeline,
    manual: &BTreeMap<String, Placed>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Item>, String> {
    let mut raw: Vec<(&str, ItemKind, &str, i64, i64)> = Vec::new();
    for l in &tl.lines {
        raw.push((&l.id, ItemKind::Bubble, &l.text, l.start_ms, l.end_ms));
    }
    for sc in &tl.scenes {
        for n in &sc.notes {
            raw.push((&n.id, ItemKind::Note, &n.text, n.start_ms, n.end_ms));
        }
    }
    let mut items = Vec::new();
    for (order, (id, kind, text, s, e)) in raw.iter().enumerate() {
        let p = manual.get(*id);
        let has_offset = p.is_some_and(|p| p.start_offset_ms != 0 || p.end_offset_ms != 0);
        if s >= e && !has_offset {
            warnings.push(format!("{id}: 表示の長さが 0 なので映像に出しません"));
            continue;
        }
        let (start_ms, end_ms) =
            effective_times(*s, *e, p, tl.duration_ms).map_err(|m| format!("{id}: {m}"))?;
        items.push(Item {
            id: id.to_string(),
            kind: *kind,
            text: text.to_string(),
            start_ms,
            end_ms,
            order,
        });
    }
    let known: BTreeSet<&str> = raw.iter().map(|r| r.0).collect();
    for id in manual.keys() {
        if !known.contains(id.as_str()) {
            warnings.push(format!(
                "bubbles.json の {id} は timeline.json にありません(無視します)"
            ));
        }
    }
    Ok(items)
}

/// フォントに無い文字がある項目ごとの警告。
pub fn missing_glyph_warnings(face: &FontFace, items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|it| {
            let m = face.missing_chars(&it.text);
            (!m.is_empty()).then(|| {
                let chars: String = m.iter().collect();
                format!(
                    "{}: フォントに無い文字があります(代わりの字形で描きます): {chars}",
                    it.id
                )
            })
        })
        .collect()
}

/// 項目の種類ごとの組版の条件。
pub fn layout_ctx<'a>(face: &'a FontFace, cfg: &RenderConfig, kind: ItemKind) -> LayoutContext<'a> {
    let (padding, ratio) = match kind {
        ItemKind::Bubble => (cfg.bubble_padding, cfg.bubble_max_height_ratio),
        ItemKind::Note => (cfg.note_padding, cfg.note_max_height_ratio),
    };
    LayoutContext {
        face,
        font_size: cfg.font_size,
        line_spacing: cfg.line_spacing,
        padding,
        max_inner_h: (cfg.height as f32 * ratio).round() as u32,
    }
}

/// `place_items` の結果。`clipped` は画面の大きさに切り詰めた項目の ID。
pub struct Placement {
    pub placed: BTreeMap<String, Placed>,
    pub clipped: BTreeSet<String>,
}

/// 項目の位置と大きさを決める。手動の項目で枠に収まらなければ広げて警告し、残りは自動配置する。
pub fn place_items(
    items: &[Item],
    manual: &BTreeMap<String, Placed>,
    cfg: &RenderConfig,
    face: &FontFace,
    warnings: &mut Vec<String>,
) -> Placement {
    let canvas = Canvas {
        w: cfg.width,
        h: cfg.height,
    };
    let mut placed: BTreeMap<String, Placed> = BTreeMap::new();
    let mut manual_clipped: BTreeSet<String> = BTreeSet::new();
    let mut layout_items = Vec::new();
    for it in items {
        let ctx = layout_ctx(face, cfg, it.kind);
        let size = match manual.get(&it.id) {
            Some(p) => {
                let p = if fits(&ctx, &it.text, p.w, p.h) {
                    p.clone()
                } else {
                    let min = min_size(&ctx, &it.text);
                    warnings.push(format!(
                        "{}: 文字が枠に収まらないので {}x{} に広げます(元は {}x{})",
                        it.id,
                        p.w.max(min.0),
                        p.h.max(min.1),
                        p.w,
                        p.h
                    ));
                    let mut q = expand_to_min(p, min, canvas);
                    // 広げても画面に入らないときは画面の大きさに切り詰め、枠の中で切って描く
                    if q.w > canvas.w || q.h > canvas.h {
                        q.w = q.w.min(canvas.w);
                        q.h = q.h.min(canvas.h);
                        q.x = q.x.clamp(0, (canvas.w - q.w) as i32);
                        q.y = q.y.clamp(0, (canvas.h - q.h) as i32);
                        warnings.push(format!(
                            "{}: 文字が画面に収まらないので、画面の大きさ {}x{} に切り詰めます",
                            it.id, q.w, q.h
                        ));
                        manual_clipped.insert(it.id.clone());
                    }
                    q
                };
                let size = (p.w, p.h);
                placed.insert(it.id.clone(), p);
                size
            }
            None => min_size(&ctx, &it.text),
        };
        layout_items.push(LayoutItem {
            id: it.id.clone(),
            kind: it.kind,
            start_ms: it.start_ms,
            end_ms: it.end_ms,
            order: it.order,
            size,
        });
    }
    // 時刻は effective_times で offset を反映済みなので、障害物の計算で二重に足さないよう 0 にする
    let obstacles: BTreeMap<String, Placed> = placed
        .iter()
        .map(|(id, p)| {
            (
                id.clone(),
                Placed {
                    start_offset_ms: 0,
                    end_offset_ms: 0,
                    ..p.clone()
                },
            )
        })
        .collect();
    let (auto, auto_warnings) = auto_layout(
        &layout_items,
        &obstacles,
        &AutoParams {
            canvas,
            margin: cfg.margin,
            bubble_gap: cfg.bubble_gap,
            tail_length: cfg.tail_length,
        },
    );
    warnings.extend(auto_warnings);
    // 自動配置が画面の大きさに切り詰めた項目(min_size より小さく置かれたもの)
    let mut clipped: BTreeSet<String> = layout_items
        .iter()
        .filter(|li| {
            auto.get(&li.id)
                .is_some_and(|a| a.w < li.size.0 || a.h < li.size.1)
        })
        .map(|li| li.id.clone())
        .collect();
    clipped.extend(manual_clipped);
    placed.extend(auto);
    Placement { placed, clipped }
}

/// 項目を 1 つ描く。切り詰めた自動項目(`clipped`)だけ、収まらなくても入る部分を描く。
/// それ以外は枠に収まらなければ Err。
fn draw_one(
    ctx: &LayoutContext,
    style: &Style,
    spec: &ItemSpec,
    clipped: bool,
) -> Result<Overlay, String> {
    if clipped {
        render_clipped(ctx, style, spec)
    } else {
        render_item(ctx, style, spec)
    }
}

/// 項目を 1 つ描く(種類ごとの Style・組版の条件・枠の大きさ・尻尾を `render` と `bubbles` で共有する)。
/// `clipped` の項目だけ、収まらなくても入る部分を描く。それ以外は枠に収まらなければ Err。
pub fn draw_item(
    item: &Item,
    placed: &Placed,
    clipped: bool,
    cfg: &RenderConfig,
    face: &FontFace,
) -> Result<Overlay, String> {
    let style = match item.kind {
        ItemKind::Bubble => Style::bubble(cfg),
        ItemKind::Note => Style::note(cfg),
    };
    let ctx = layout_ctx(face, cfg, item.kind);
    let spec = ItemSpec {
        kind: item.kind,
        text: &item.text,
        w: placed.w,
        h: placed.h,
        tail: placed.tail.map(|t| (t.dx, t.dy)),
    };
    draw_one(&ctx, &style, &spec, clipped)
}

/// 描いた項目を、配置の左上 + `Overlay` の原点(dx・dy)に、与えた順に重ねる。
pub fn compose(cfg: &RenderConfig, layers: &[(&Placed, &Overlay)]) -> tiny_skia::Pixmap {
    let layers: Vec<(i32, i32, &tiny_skia::Pixmap)> = layers
        .iter()
        .map(|(p, ov)| (p.x + ov.dx, p.y + ov.dy, &ov.pixmap))
        .collect();
    composite(cfg.width, cfg.height, &layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_timeline::{CutEntry, LineEntry, Normalize, NoteEntry, SceneEntry};
    use std::path::PathBuf;

    fn line(id: &str, text: &str, s: i64, e: i64) -> LineEntry {
        LineEntry {
            id: id.into(),
            scene: "S01".into(),
            cast: "A".into(),
            text: text.into(),
            start_ms: s,
            end_ms: e,
            take: "S01-L001".into(),
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

    fn timeline() -> Timeline {
        Timeline {
            schema_version: 3,
            script: "s.txt".into(),
            script_sha256: String::new(),
            audio_sha256: String::new(),
            sample_rate: 48000,
            duration_ms: 5000,
            normalize: Normalize { target_lufs: -16.0 },
            cast: vec![],
            scenes: vec![SceneEntry {
                id: "S01".into(),
                name: None,
                index: 1,
                silent: false,
                start_ms: 0,
                end_ms: 5000,
                cuts: vec![CutEntry {
                    id: "S01-C01".into(),
                    start_ms: 0,
                    end_ms: 5000,
                    visual: None,
                }],
                notes: vec![NoteEntry {
                    id: "S01-N01".into(),
                    text: "翌朝、港。".into(),
                    start_ms: 0,
                    end_ms: 2000,
                }],
            }],
            takes: vec![],
            lines: vec![
                line("S01-L001", "おはよう。", 500, 1500),
                line("S01-L002", "さようなら。", 1500, 1500),
            ],
            events: vec![],
        }
    }

    fn placed(start_off: i64, end_off: i64) -> Placed {
        Placed {
            kind: ItemKind::Bubble,
            x: 10,
            y: 10,
            w: 50,
            h: 80,
            tail: None,
            start_offset_ms: start_off,
            end_offset_ms: end_off,
        }
    }

    #[test]
    fn items_are_built_from_timeline() {
        let mut manual = BTreeMap::new();
        manual.insert("S01-L001".to_string(), placed(-200, 300));
        manual.insert("S01-L099".to_string(), placed(0, 0));
        let mut warnings = Vec::new();
        let items = build_items(&timeline(), &manual, &mut warnings).unwrap();
        // 台詞 L001(offset 反映)とト書き N01。L002 は長さ 0 なので除かれる
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["S01-L001", "S01-N01"]);
        assert_eq!(items[0].kind, ItemKind::Bubble);
        assert_eq!(items[0].text, "おはよう。");
        assert_eq!((items[0].start_ms, items[0].end_ms), (300, 1800));
        assert_eq!(items[1].kind, ItemKind::Note);
        assert_eq!((items[1].start_ms, items[1].end_ms), (0, 2000));
        assert!(
            warnings.iter().any(|w| w.contains("S01-L002")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("S01-L099")),
            "{warnings:?}"
        );
    }

    #[test]
    fn manual_times_are_clamped_and_empty_result_is_an_error() {
        let mut manual = BTreeMap::new();
        manual.insert("S01-L001".to_string(), placed(-60_000, 60_000));
        let items = build_items(&timeline(), &manual, &mut Vec::new()).unwrap();
        assert_eq!((items[0].start_ms, items[0].end_ms), (0, 5000)); // [0, duration] に収める
        manual.insert("S01-L001".to_string(), placed(2000, -400));
        let e = build_items(&timeline(), &manual, &mut Vec::new()).unwrap_err();
        assert!(e.contains("S01-L001"), "{e}");
    }

    #[test]
    fn only_clipped_items_may_overflow_the_frame() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = RenderConfig::default();
        let ctx = layout_ctx(&face, &cfg, ItemKind::Note);
        let style = Style::note(&cfg);
        let spec = ItemSpec {
            kind: ItemKind::Note,
            text: "一二三四五",
            w: 60,
            h: 60,
            tail: None,
        };
        // 切り詰めた印が無ければ、収まらない項目は Err(終了コード 2)
        assert!(draw_one(&ctx, &style, &spec, false).is_err());
        assert!(draw_one(&ctx, &style, &spec, true).is_ok());
    }

    #[test]
    fn place_items_marks_only_items_shrunk_to_the_screen() {
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
        let item = |id: &str, text: &str| Item {
            id: id.into(),
            kind: ItemKind::Bubble,
            text: text.into(),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        };
        let items = vec![item("S01-L001", "あ"), item("S01-L002", &"あ".repeat(500))];
        let mut w = Vec::new();
        let Placement { clipped, .. } = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut w);
        assert_eq!(clipped.into_iter().collect::<Vec<_>>(), vec!["S01-L002"]);
    }

    #[test]
    fn missing_glyphs_are_warned_per_item() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let item = |id: &str, text: &str| Item {
            id: id.into(),
            kind: ItemKind::Bubble,
            text: text.into(),
            start_ms: 0,
            end_ms: 1,
            order: 0,
        };
        let w = missing_glyph_warnings(
            &face,
            &[item("S01-L001", "おはよう。"), item("S01-L002", "龘ゑ")],
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("S01-L002") && w[0].contains('龘') && w[0].contains('ゑ'),
            "{w:?}"
        );
    }

    #[test]
    fn compose_places_overlay_at_placed_plus_origin() {
        let face = FontFace::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = s2v_core::config::RenderConfig {
            width: 320,
            height: 180,
            ..Default::default()
        };
        let item = Item {
            id: "S01-L001".into(),
            kind: crate::ItemKind::Bubble,
            text: "あ".into(),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        };
        let placed = crate::bubbles::Placed {
            kind: crate::ItemKind::Bubble,
            x: 140,
            y: 50,
            w: 120,
            h: 120,
            tail: Some(crate::bubbles::Tail { dx: -50, dy: -40 }),
            start_offset_ms: 0,
            end_offset_ms: 0,
        };
        let ov = draw_item(&item, &placed, false, &cfg, &face).unwrap();
        assert!(ov.dx != 0 || ov.dy != 0, "尻尾で原点がずれるはず");
        let img = compose(&cfg, &[(&placed, &ov)]);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..img.height() {
            for x in 0..img.width() {
                if img.pixel(x, y).unwrap().alpha() > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        assert!(x1 >= x0, "何も描かれていない");
        // 重ねの置き場所は placed.x + dx(画面の外にはみ出さない例なので外接矩形の左上がそれ以上)
        assert!(x0 as i32 >= placed.x + ov.dx, "{x0} {}", placed.x + ov.dx);
        assert!(y0 as i32 >= placed.y + ov.dy, "{y0} {}", placed.y + ov.dy);
        assert!(x1 as i32 <= placed.x + ov.dx + ov.pixmap.width() as i32);
        assert!(y1 as i32 <= placed.y + ov.dy + ov.pixmap.height() as i32);
    }
}
