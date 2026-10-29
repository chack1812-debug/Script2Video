//! 項目(吹き出し・ト書き)の組み立て・配置・描画。`render` と `bubbles` が同じ関数を使う。

use std::collections::{BTreeMap, BTreeSet};

use s2v_core::config::{RenderConfig, RenderStyle};
use s2v_timeline::Timeline;

use crate::autolayout::{auto_layout, AutoParams, LayoutItem};
use crate::bubbles::{effective_times, expand_to_min, Canvas, Placed};
use crate::draw::{composite, render as render_item, render_clipped, ItemSpec, Overlay, Style};
use crate::font::FontFace;
use crate::layout::{fits, min_size, LayoutContext};
use crate::subtitle::{self, SUBTITLE_MAX_LINES};
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
    /// 発言者名(台詞の役名を trim したもの。空ならなし。ト書きは常になし)
    pub speaker: Option<String>,
}

/// timeline.json から項目を作り、`bubbles.json` の offset を反映する。
/// 長さ 0 の項目は警告して除く。`timeline` に無い手動配置の ID は警告する。
pub fn build_items(
    tl: &Timeline,
    manual: &BTreeMap<String, Placed>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Item>, String> {
    type Raw<'a> = (&'a str, ItemKind, &'a str, i64, i64, Option<&'a str>);
    let mut raw: Vec<Raw> = Vec::new();
    for l in &tl.lines {
        let cast = l.cast.trim();
        let speaker = (!cast.is_empty()).then_some(cast);
        raw.push((
            &l.id,
            ItemKind::Bubble,
            &l.text,
            l.start_ms,
            l.end_ms,
            speaker,
        ));
    }
    for sc in &tl.scenes {
        for n in &sc.notes {
            raw.push((&n.id, ItemKind::Note, &n.text, n.start_ms, n.end_ms, None));
        }
    }
    let mut items = Vec::new();
    for (order, (id, kind, text, s, e, speaker)) in raw.iter().enumerate() {
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
            speaker: speaker.map(str::to_string),
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

/// 字幕モードで描く役名にフォントに無い文字があるときの警告(吹き出しモードでは役名を描かないので空)。
pub fn missing_speaker_warnings(
    face: &FontFace,
    items: &[Item],
    cfg: &RenderConfig,
) -> Vec<String> {
    if cfg.style != RenderStyle::Subtitle {
        return Vec::new();
    }
    items
        .iter()
        .filter_map(|it| {
            let m = face.missing_chars(it.speaker.as_deref()?);
            (!m.is_empty()).then(|| {
                let chars: String = m.iter().collect();
                format!(
                    "{}: 役名にフォントに無い文字があります(代わりの字形で描きます): {chars}",
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

/// 字幕の問題(どちらも `render` を止める理由になる)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitleProblem {
    /// 本文が `SUBTITLE_MAX_LINES` 行を超える(`lines` は折り返し後の行数)
    TooManyLines { lines: usize },
    /// 同時に出る字幕を積むと画面の上端を超える
    Overflow,
}

/// 字幕の問題 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleIssue {
    pub id: String,
    pub speaker: Option<String>,
    pub text: String,
    pub problem: SubtitleProblem,
}

/// `place_items` の結果。`clipped` は画面の大きさに切り詰めた項目の ID。
/// `subtitle_issues` は字幕モードの問題(吹き出しモードでは常に空)。
pub struct Placement {
    pub placed: BTreeMap<String, Placed>,
    pub clipped: BTreeSet<String>,
    pub subtitle_issues: Vec<SubtitleIssue>,
}

/// `bubbles` 画面で編集する項目。字幕モードでは台詞(字幕)を除き、ト書きだけにする。
pub fn editable_items(items: Vec<Item>, cfg: &RenderConfig) -> Vec<Item> {
    if cfg.style == RenderStyle::Subtitle {
        items
            .into_iter()
            .filter(|i| i.kind != ItemKind::Bubble)
            .collect()
    } else {
        items
    }
}

/// 字幕として描く台詞か(字幕モードの台詞)
fn is_subtitle(item: &Item, cfg: &RenderConfig) -> bool {
    cfg.style == RenderStyle::Subtitle && item.kind == ItemKind::Bubble
}

/// 時間の重なり(半開区間)
fn times_overlap(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// 字幕を最下段に置く。同時に出る台詞は `(start_ms, order)` の後のものを上へ積む。
/// 本文が空の台詞(0 行)は字幕にしない(高さ 0 で最下段に置き、積み上げに数えない)。
fn place_subtitles(
    lines: &[&Item],
    cfg: &RenderConfig,
    face: &FontFace,
    issues: &mut Vec<SubtitleIssue>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Placed> {
    let x = cfg.margin as i32;
    let w = subtitle::band_width(cfg);
    let bottom = cfg.height as i64 - i64::from(cfg.subtitle_margin_bottom);
    let gap = i64::from(cfg.bubble_gap);
    let mut order: Vec<&Item> = lines.to_vec();
    order.sort_by_key(|i| (i.start_ms, i.order));
    // 配置済みの字幕: (表示時間, ラベルを含む上端, 下端)
    let mut stacked: Vec<((i64, i64), i64, i64)> = Vec::new();
    let mut out = BTreeMap::new();
    for it in order {
        let m = subtitle::measure(face, cfg, &it.text, it.speaker.as_deref());
        let issue = |problem| SubtitleIssue {
            id: it.id.clone(),
            speaker: it.speaker.clone(),
            text: it.text.clone(),
            problem,
        };
        let placed_at = |y: i64, h: u32| Placed {
            kind: ItemKind::Bubble,
            x,
            y: y as i32,
            w,
            h,
            tail: None,
            start_offset_ms: 0,
            end_offset_ms: 0,
        };
        if m.lines == 0 {
            out.insert(it.id.clone(), placed_at(bottom, 0));
            continue;
        }
        // 四行以上の台詞は 1 件だけ報告する(Overflow は重ねない)
        let too_many = m.lines > SUBTITLE_MAX_LINES;
        if too_many {
            issues.push(issue(SubtitleProblem::TooManyLines { lines: m.lines }));
        }
        if i64::from(x) + i64::from(w.max(m.label_w)) > i64::from(cfg.width) {
            warnings.push(format!(
                "{}: 役名が長すぎて字幕の左上のラベルが画面の右端で切れます",
                it.id
            ));
        }
        let (h, label) = (i64::from(m.band_h), i64::from(m.label_h));
        let times = (it.start_ms, it.end_ms);
        let others: Vec<(i64, i64)> = stacked
            .iter()
            .filter(|s| times_overlap(times, s.0))
            .map(|s| (s.1, s.2))
            .collect();
        // 候補(帯の上端): 最下段と、重なる字幕のすぐ上。低い順に試す
        let mut cands: Vec<i64> = vec![bottom - h];
        cands.extend(
            others
                .iter()
                .map(|o| o.0 - gap - h)
                .filter(|&y| y < bottom - h),
        );
        cands.sort_unstable_by(|a, b| b.cmp(a));
        cands.dedup();
        let free = cands.iter().copied().find(|&y| {
            !others
                .iter()
                .any(|&(top, bot)| y - label - gap < bot && top - gap < y + h)
        });
        let y = match free {
            Some(y) if y - label >= 0 => y,
            _ => {
                if !too_many {
                    issues.push(issue(SubtitleProblem::Overflow));
                }
                // 報告のための位置: 画面内に収まる最後の候補(無ければ上端に寄せる)
                cands
                    .iter()
                    .copied()
                    .rfind(|&y| y - label >= 0)
                    .unwrap_or(label)
            }
        };
        stacked.push((times, y - label, y + h));
        out.insert(it.id.clone(), placed_at(y, m.band_h));
    }
    out
}

/// 字幕とト書きが時間も位置も重なれば警告する(ト書きは縮めない)。
/// 帯が見える(塗りが透明でない、または枠線がある)ときは帯とラベルの範囲で、
/// 帯が見えないときは画面に出る範囲(ラベルと文字)で判定する。
fn warn_subtitle_note_overlap(
    items: &[Item],
    placed: &BTreeMap<String, Placed>,
    cfg: &RenderConfig,
    face: &FontFace,
    warnings: &mut Vec<String>,
) {
    let band_visible = cfg.subtitle_band_opacity > 0.0 || cfg.subtitle_band_stroke_width > 0.0;
    let notes: Vec<&Item> = items.iter().filter(|i| i.kind == ItemKind::Note).collect();
    for line in items.iter().filter(|i| is_subtitle(i, cfg)) {
        let Some(lp) = placed.get(&line.id).filter(|p| p.h > 0) else {
            continue;
        };
        let m = subtitle::measure(face, cfg, &line.text, line.speaker.as_deref());
        let (x, y, w, h) = (
            i64::from(lp.x),
            i64::from(lp.y),
            i64::from(lp.w),
            i64::from(lp.h),
        );
        let label = i64::from(m.label_h);
        // (左, 上, 右, 下) の矩形
        let rects: Vec<(i64, i64, i64, i64)> = if band_visible {
            vec![(x, y - label, x + w, y + h)]
        } else {
            let pad = i64::from(cfg.subtitle_padding);
            // 縁取りは文字の外へ半分の太さだけはみ出す
            let out = cfg.subtitle_outline_width.max(0.0).ceil() as i64;
            let mut v = vec![(
                x + pad - out,
                y + pad - out,
                x + pad + i64::from(m.text_w) + out,
                y + h - pad + out,
            )];
            if label > 0 {
                v.push((x, y - label, x + i64::from(m.label_w), y));
            }
            v
        };
        for note in &notes {
            let Some(np) = placed.get(&note.id) else {
                continue;
            };
            let (n0x, n0y) = (i64::from(np.x), i64::from(np.y));
            let (n1x, n1y) = (n0x + i64::from(np.w), n0y + i64::from(np.h));
            let hit = rects
                .iter()
                .any(|&(l0x, l0y, l1x, l1y)| l0x < n1x && n0x < l1x && l0y < n1y && n0y < l1y);
            if hit && times_overlap((line.start_ms, line.end_ms), (note.start_ms, note.end_ms)) {
                warnings.push(format!(
                    "{} の字幕と {} のト書きが重なっています",
                    line.id, note.id
                ));
            }
        }
    }
}

/// 項目の位置と大きさを決める。手動の項目で枠に収まらなければ広げて警告し、残りは自動配置する。
/// 字幕モードでは、台詞は手動配置を使わず最下段に字幕として置き、ト書きだけを今までどおり置く。
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
    for it in items.iter().filter(|i| !is_subtitle(i, cfg)) {
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
    let mut subtitle_issues = Vec::new();
    if cfg.style == RenderStyle::Subtitle {
        let lines: Vec<&Item> = items.iter().filter(|i| is_subtitle(i, cfg)).collect();
        placed.extend(place_subtitles(
            &lines,
            cfg,
            face,
            &mut subtitle_issues,
            warnings,
        ));
        warn_subtitle_note_overlap(items, &placed, cfg, face, warnings);
    }
    Placement {
        placed,
        clipped,
        subtitle_issues,
    }
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
    if is_subtitle(item, cfg) {
        // 本文が空の台詞は字幕にしない(透明な 1 画素を返す)
        if placed.h == 0 || subtitle::measure(face, cfg, &item.text, None).lines == 0 {
            let pixmap = tiny_skia::Pixmap::new(1, 1).ok_or("画像を作れません")?;
            return Ok(Overlay {
                dx: 0,
                dy: 0,
                pixmap,
            });
        }
        return subtitle::draw_subtitle(
            face,
            cfg,
            &item.text,
            item.speaker.as_deref(),
            placed.w,
            placed.h,
        );
    }
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
    use s2v_core::config::RenderStyle;
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
            speaker: None,
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
            speaker: None,
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
    fn missing_speaker_glyphs_warn_only_in_subtitle_style() {
        let face = test_face();
        let mut it = sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0);
        it.speaker = Some("龘ゑ".into());
        let items = vec![it];
        let w = missing_speaker_warnings(&face, &items, &sub_cfg(RenderStyle::Subtitle));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("S01-L001")
                && w[0].contains("役名")
                && w[0].contains('龘')
                && w[0].contains('ゑ'),
            "{w:?}"
        );
        assert!(missing_speaker_warnings(&face, &items, &sub_cfg(RenderStyle::Bubble)).is_empty());
        // 通常の役名は警告しない
        let ok = vec![sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0)];
        assert!(missing_speaker_warnings(&face, &ok, &sub_cfg(RenderStyle::Subtitle)).is_empty());
    }

    #[test]
    fn too_long_speaker_label_warns_about_right_edge() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        // 帯は 5 文字分。役名 8 文字なら右端を超える
        let mut long = sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0);
        long.speaker = Some("港朝港朝港朝港朝".into());
        let short = sub_item("S01-L002", ItemKind::Bubble, "港朝", 2000, 3000, 1);
        let mut w = Vec::new();
        place_items(&[long, short], &BTreeMap::new(), &cfg, &face, &mut w);
        let hits: Vec<&String> = w.iter().filter(|m| m.contains("役名が長すぎて")).collect();
        assert_eq!(hits.len(), 1, "{w:?}");
        assert!(hits[0].starts_with("S01-L001:"), "{w:?}");
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
            speaker: None,
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

    // ---- 字幕モード ----

    fn test_face() -> FontFace {
        FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap()
    }

    /// 帯の内側がちょうど 5 文字分(15 文字で 3 行、16 文字で 4 行)になる設定
    fn sub_cfg(style: RenderStyle) -> RenderConfig {
        RenderConfig {
            style,
            font_size: 40.0,
            margin: 10,
            subtitle_padding: 20,
            width: 2 * 10 + 2 * 20 + 5 * 40 + 2,
            height: 360,
            ..RenderConfig::default()
        }
    }

    fn chars(n: usize) -> String {
        "港朝".chars().cycle().take(n).collect()
    }

    fn sub_item(id: &str, kind: ItemKind, text: &str, s: i64, e: i64, order: usize) -> Item {
        Item {
            id: id.into(),
            kind,
            text: text.into(),
            start_ms: s,
            end_ms: e,
            order,
            speaker: (kind == ItemKind::Bubble).then(|| "A".to_string()),
        }
    }

    fn bottom(cfg: &RenderConfig) -> i32 {
        (cfg.height - cfg.subtitle_margin_bottom) as i32
    }

    #[test]
    fn build_items_sets_trimmed_speaker_for_lines_only() {
        let mut tl = timeline();
        tl.lines[0].cast = "  田中 ".into();
        tl.lines[1].end_ms = 2500;
        tl.lines[1].cast = "  ".into();
        let items = build_items(&tl, &BTreeMap::new(), &mut Vec::new()).unwrap();
        let by: BTreeMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
        assert_eq!(by["S01-L001"].speaker.as_deref(), Some("田中"));
        assert_eq!(by["S01-L002"].speaker, None);
        assert_eq!(by["S01-N01"].speaker, None);
    }

    #[test]
    fn subtitle_style_places_lines_at_bottom_and_notes_as_before() {
        let face = test_face();
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0),
            sub_item("S01-N01", ItemKind::Note, "翌朝", 0, 1000, 1),
        ];
        // 台詞の手動配置は字幕モードでは無視する
        let mut manual = BTreeMap::new();
        manual.insert("S01-L001".to_string(), placed(0, 0));
        let cfg = sub_cfg(RenderStyle::Subtitle);
        let mut w = Vec::new();
        let p = place_items(&items, &manual, &cfg, &face, &mut w);
        let l = &p.placed["S01-L001"];
        assert_eq!(l.x, cfg.margin as i32);
        assert_eq!(l.w, cfg.width - 2 * cfg.margin);
        assert_eq!(l.y + l.h as i32, bottom(&cfg));
        let m = crate::subtitle::measure(&face, &cfg, "港朝", Some("A"));
        assert_eq!(l.h, m.band_h);
        assert_eq!(l.tail, None);
        assert!(p.subtitle_issues.is_empty());
        assert!(p.clipped.is_empty());

        // 比べる相手: 台詞を除いた項目を吹き出しモードで置いたときのト書き
        let notes_only: Vec<Item> = items
            .iter()
            .filter(|i| i.kind == ItemKind::Note)
            .cloned()
            .collect();
        let bubble = place_items(
            &notes_only,
            &BTreeMap::new(),
            &sub_cfg(RenderStyle::Bubble),
            &face,
            &mut Vec::new(),
        );
        assert_eq!(p.placed["S01-N01"], bubble.placed["S01-N01"]);
    }

    #[test]
    fn earlier_start_is_lower_even_if_order_is_later() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        // L001 は台本で先だが 500ms 後に始まる。先に始まる L002 が下
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 500, 1500, 0),
            sub_item("S01-L002", ItemKind::Bubble, "港朝", 0, 1000, 1),
        ];
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        let (a, b) = (&p.placed["S01-L001"], &p.placed["S01-L002"]);
        assert_eq!(b.y + b.h as i32, bottom(&cfg));
        assert!(a.y < b.y, "{a:?} {b:?}");
    }

    #[test]
    fn overflow_boundary_is_exact() {
        let face = test_face();
        let base = sub_cfg(RenderStyle::Subtitle);
        let m = crate::subtitle::measure(&face, &base, "港朝", Some("A"));
        // 2 段積んだときの上端(ラベル込み)がちょうど 0 になる高さ
        let exact = base.subtitle_margin_bottom + 2 * (m.band_h + m.label_h) + base.bubble_gap;
        assert_eq!(exact, 344, "想定の値(帯 80・ラベル 60・間 24・下 40)");
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0),
            sub_item("S01-L002", ItemKind::Bubble, "港朝", 0, 1000, 1),
        ];
        let at = |height: u32| {
            let cfg = RenderConfig {
                height,
                ..sub_cfg(RenderStyle::Subtitle)
            };
            place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new())
        };
        let ok = at(exact);
        assert!(ok.subtitle_issues.is_empty(), "{:?}", ok.subtitle_issues);
        assert_eq!(ok.placed["S01-L002"].y - m.label_h as i32, 0);
        let over = at(exact - 1);
        assert_eq!(
            over.subtitle_issues,
            vec![SubtitleIssue {
                id: "S01-L002".into(),
                speaker: Some("A".into()),
                text: "港朝".into(),
                problem: SubtitleProblem::Overflow,
            }]
        );
    }

    #[test]
    fn overlapping_lines_stack_upward_in_order() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        // L002 は台本の後ろだが同時に始まる。order の早い L001 が下
        let items = vec![
            sub_item("S01-L002", ItemKind::Bubble, "港朝", 0, 1000, 1),
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0),
            sub_item("S01-L003", ItemKind::Bubble, "港朝", 1000, 2000, 2),
        ];
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        let m = crate::subtitle::measure(&face, &cfg, "港朝", Some("A"));
        let (a, b, c) = (
            &p.placed["S01-L001"],
            &p.placed["S01-L002"],
            &p.placed["S01-L003"],
        );
        assert_eq!(a.y + a.h as i32, bottom(&cfg));
        // 上の台詞の下端 = 下の台詞のラベルの上端 - bubble_gap
        assert_eq!(
            b.y + b.h as i32,
            a.y - m.label_h as i32 - cfg.bubble_gap as i32
        );
        // 時間が重ならない台詞は最下段
        assert_eq!((c.x, c.y), (a.x, a.y));
        assert!(p.subtitle_issues.is_empty());
    }

    #[test]
    fn four_line_text_is_reported() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, &chars(16), 0, 1000, 0),
            sub_item("S01-L002", ItemKind::Bubble, &chars(15), 2000, 3000, 1),
        ];
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        assert_eq!(
            p.subtitle_issues,
            vec![SubtitleIssue {
                id: "S01-L001".into(),
                speaker: Some("A".into()),
                text: chars(16),
                problem: SubtitleProblem::TooManyLines { lines: 4 },
            }]
        );
        // 4 行の台詞も測った高さのまま置く
        let m = crate::subtitle::measure(&face, &cfg, &chars(16), Some("A"));
        assert_eq!(p.placed["S01-L001"].h, m.band_h);

        let three = vec![sub_item(
            "S01-L002",
            ItemKind::Bubble,
            &chars(15),
            0,
            1000,
            0,
        )];
        let p = place_items(&three, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        assert!(p.subtitle_issues.is_empty(), "{:?}", p.subtitle_issues);
    }

    #[test]
    fn too_many_lines_is_not_also_reported_as_overflow() {
        let face = test_face();
        let cfg = RenderConfig {
            height: 200,
            ..sub_cfg(RenderStyle::Subtitle)
        };
        let items = vec![sub_item(
            "S01-L001",
            ItemKind::Bubble,
            &chars(16),
            0,
            1000,
            0,
        )];
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        assert_eq!(
            p.subtitle_issues,
            vec![SubtitleIssue {
                id: "S01-L001".into(),
                speaker: Some("A".into()),
                text: chars(16),
                problem: SubtitleProblem::TooManyLines { lines: 4 },
            }]
        );
    }

    #[test]
    fn stack_beyond_screen_is_reported_as_overflow() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        // 1 段 = 帯 80 + ラベル 60 + 間 24。高さ 360・下の余白 40 では 2 段まで
        let items: Vec<Item> = (0..4)
            .map(|i| {
                sub_item(
                    &format!("S01-L00{}", i + 1),
                    ItemKind::Bubble,
                    "港朝",
                    0,
                    1000,
                    i,
                )
            })
            .collect();
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut Vec::new());
        let over: Vec<&str> = p
            .subtitle_issues
            .iter()
            .filter(|i| i.problem == SubtitleProblem::Overflow)
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(over, vec!["S01-L003", "S01-L004"]);
        let m = crate::subtitle::measure(&face, &cfg, "港朝", Some("A"));
        for id in ["S01-L003", "S01-L004"] {
            let q = &p.placed[id];
            assert!(q.y - m.label_h as i32 >= 0, "{id}: {q:?}");
        }
    }

    #[test]
    fn line_note_overlap_warns() {
        let face = test_face();
        let cfg = RenderConfig {
            subtitle_band_opacity: 0.6,
            ..sub_cfg(RenderStyle::Subtitle)
        };
        // 下のほうに手動で置いたト書き(字幕の帯と重なる)
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0),
            sub_item("S01-N01", ItemKind::Note, "翌", 0, 1000, 1),
            sub_item("S01-L002", ItemKind::Bubble, "港朝", 2000, 3000, 2),
        ];
        let mut manual = BTreeMap::new();
        manual.insert(
            "S01-N01".to_string(),
            Placed {
                kind: ItemKind::Note,
                x: 20,
                y: 220,
                w: 100,
                h: 100,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        let mut w = Vec::new();
        place_items(&items, &manual, &cfg, &face, &mut w);
        let hits: Vec<&String> = w.iter().filter(|m| m.contains("重なっています")).collect();
        assert_eq!(hits.len(), 1, "{w:?}");
        assert!(hits[0].contains("S01-L001") && hits[0].contains("S01-N01"));
    }

    /// 字幕 1 行("港朝"、左寄せ)と、指定の位置に手動で置いたト書きから、重なりの警告を集める。
    fn overlap_warnings(cfg: &RenderConfig, note_x: i32) -> Vec<String> {
        let face = test_face();
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0),
            sub_item("S01-N01", ItemKind::Note, "翌", 0, 1000, 1),
        ];
        let mut manual = BTreeMap::new();
        manual.insert(
            "S01-N01".to_string(),
            Placed {
                kind: ItemKind::Note,
                x: note_x,
                y: 220,
                w: 50,
                h: 100,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        let mut w = Vec::new();
        place_items(&items, &manual, cfg, &face, &mut w);
        w.into_iter()
            .filter(|m| m.contains("重なっています"))
            .collect()
    }

    #[test]
    fn invisible_band_judges_overlap_by_text_extent() {
        let cfg = sub_cfg(RenderStyle::Subtitle);
        assert_eq!(cfg.subtitle_band_opacity, 0.0);
        assert_eq!(cfg.subtitle_band_stroke_width, 0.0);
        // 帯の範囲の右端(文字の右側の空き)に置いたト書き: 帯は見えないので警告しない
        assert!(overlap_warnings(&cfg, 200).is_empty());
        // 文字の上に置いたト書き: 警告する
        assert_eq!(overlap_warnings(&cfg, 40).len(), 1);
        // 帯が見えるなら、同じ位置でも帯と重なるので警告する
        let visible = RenderConfig {
            subtitle_band_opacity: 0.6,
            ..cfg.clone()
        };
        assert_eq!(overlap_warnings(&visible, 200).len(), 1);
        let framed = RenderConfig {
            subtitle_band_stroke_width: 2.0,
            ..cfg
        };
        assert_eq!(overlap_warnings(&framed, 200).len(), 1);
    }

    #[test]
    fn bubble_style_is_unchanged() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Bubble);
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, &chars(16), 0, 1000, 0),
            sub_item("S01-L002", ItemKind::Bubble, "港朝", 0, 1000, 1),
            sub_item("S01-N01", ItemKind::Note, "翌朝", 0, 1000, 2),
        ];
        let mut w = Vec::new();
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut w);
        assert!(p.subtitle_issues.is_empty());
        // 吹き出しは上の余白に置かれ、尻尾がある(字幕の経路を通らない)
        for id in ["S01-L001", "S01-L002"] {
            assert_eq!(p.placed[id].y, cfg.margin as i32, "{id}");
            assert!(p.placed[id].tail.is_some(), "{id}");
        }
    }

    #[test]
    fn editable_items_drops_lines_only_in_subtitle_style() {
        let items = vec![
            sub_item("S01-L001", ItemKind::Bubble, "港", 0, 1000, 0),
            sub_item("S01-N01", ItemKind::Note, "朝", 0, 1000, 1),
        ];
        let sub = editable_items(items.clone(), &sub_cfg(RenderStyle::Subtitle));
        assert_eq!(
            sub.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["S01-N01"]
        );
        assert_eq!(
            editable_items(items.clone(), &sub_cfg(RenderStyle::Bubble)),
            items
        );
    }

    #[test]
    fn draw_item_uses_subtitle_in_subtitle_style() {
        let face = test_face();
        let it = sub_item("S01-L001", ItemKind::Bubble, "港朝", 0, 1000, 0);
        let cfg = sub_cfg(RenderStyle::Subtitle);
        let p = place_items(
            std::slice::from_ref(&it),
            &BTreeMap::new(),
            &cfg,
            &face,
            &mut Vec::new(),
        );
        let pl = &p.placed["S01-L001"];
        let sub = draw_item(&it, pl, false, &cfg, &face).unwrap();
        assert!(sub.dy < 0, "ラベルの分だけ上へずれるはず");
        assert_eq!(sub.pixmap.width(), pl.w);
        let bcfg = sub_cfg(RenderStyle::Bubble);
        let bp = place_items(
            std::slice::from_ref(&it),
            &BTreeMap::new(),
            &bcfg,
            &face,
            &mut Vec::new(),
        );
        let bub = draw_item(&it, &bp.placed["S01-L001"], false, &bcfg, &face).unwrap();
        assert_ne!(
            (sub.pixmap.width(), sub.pixmap.height()),
            (bub.pixmap.width(), bub.pixmap.height())
        );
    }

    #[test]
    fn empty_line_text_does_not_break_subtitle_render() {
        let face = test_face();
        let cfg = sub_cfg(RenderStyle::Subtitle);
        let mut tl = timeline();
        tl.lines[0].text = String::new();
        let items = build_items(&tl, &BTreeMap::new(), &mut Vec::new()).unwrap();
        let mut w = Vec::new();
        let p = place_items(&items, &BTreeMap::new(), &cfg, &face, &mut w);
        assert!(p.subtitle_issues.is_empty(), "{:?}", p.subtitle_issues);
        for it in &items {
            let ov = draw_item(it, &p.placed[&it.id], false, &cfg, &face).unwrap();
            if it.kind == ItemKind::Bubble {
                // 字幕にしない(何も描かない)
                assert!(ov.pixmap.pixels().iter().all(|px| px.alpha() == 0));
            }
        }
    }
}
