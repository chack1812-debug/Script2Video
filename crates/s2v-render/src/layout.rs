//! 縦書きの組版(映像工程設計書 §6.1–6.2)。単位の整形・縦中横・回転・最小の大きさ・収まりの判定。

use std::ops::Range;

use rustybuzz::{Direction, UnicodeBuffer};
use unicode_segmentation::UnicodeSegmentation;

use crate::font::FontFace;
use crate::kinsoku::{break_columns, is_no_end, is_no_start, UnitClass};

const EPS: f32 = 1e-3;

pub struct LayoutContext<'a> {
    pub face: &'a FontFace,
    pub font_size: f32,
    pub line_spacing: f32,
    pub padding: u32,
    /// 1列の高さの上限(px。自動配置と min_size に使う)
    pub max_inner_h: u32,
}

/// 1字形の配置(px。単位の枠の左上を原点に、x 右・y 下)
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedGlyph {
    pub glyph_id: u16,
    /// 字形の原点(フォントの横書きの原点。ベースライン上)の位置
    pub x: f32,
    pub y: f32,
    pub transform: GlyphTransform,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlyphTransform {
    /// そのまま(縦書きの字形)
    Upright,
    /// 縦中横: 横に scale_x 倍して置く
    Tcy { scale_x: f32 },
    /// 時計回りに 90° 回転
    Rotated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub text: String,
    pub height: f32,
    pub glyphs: Vec<PlacedGlyph>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub units: Range<usize>,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub units: Vec<Unit>,
    pub columns: Vec<Column>,
    /// 列の幅の合計(列の幅 = font_size、列の間 = font_size × line_spacing)
    pub width: f32,
    /// 最も高い列の高さ
    pub height: f32,
}

/// 連なりの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// 縦中横(ASCII の数字だけの 1〜2 文字)
    Tcy,
    /// 回転(それ以外の ASCII の図形文字・空白)
    Rotated,
    /// 縦書き
    Upright,
}

fn is_ascii_run_char(c: char) -> bool {
    c == ' ' || c.is_ascii_graphic()
}

/// 文字列を連なりに分ける(バイトの範囲)。
pub fn split_runs(text: &str) -> Vec<(RunKind, Range<usize>)> {
    let mut runs = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        let ascii = is_ascii_run_char(c);
        let mut end = start;
        while let Some(&(i, c2)) = chars.peek() {
            if is_ascii_run_char(c2) != ascii {
                break;
            }
            end = i + c2.len_utf8();
            chars.next();
        }
        let slice = &text[start..end];
        let kind = if !ascii {
            RunKind::Upright
        } else if slice.len() <= 2 && slice.bytes().all(|b| b.is_ascii_digit()) {
            RunKind::Tcy
        } else {
            RunKind::Rotated
        };
        runs.push((kind, start..end));
    }
    runs
}

pub(crate) struct Shaped {
    pub(crate) glyph_id: u16,
    pub(crate) cluster: usize,
    pub(crate) x_advance: i32,
    pub(crate) y_advance: i32,
    pub(crate) x_offset: i32,
    pub(crate) y_offset: i32,
}

pub(crate) fn shape_run(
    face: &rustybuzz::Face,
    features: &[rustybuzz::Feature],
    text: &str,
    dir: Direction,
) -> Vec<Shaped> {
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    buf.set_direction(dir);
    buf.guess_segment_properties();
    let out = rustybuzz::shape(face, features, buf);
    out.glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(i, p)| Shaped {
            glyph_id: i.glyph_id as u16,
            cluster: i.cluster as usize,
            x_advance: p.x_advance,
            y_advance: p.y_advance,
            x_offset: p.x_offset,
            y_offset: p.y_offset,
        })
        .collect()
}

/// 書記素ごとに字形を割り当てる。字形を持たない書記素は直前の単位にまとめる
/// (先頭の書記素が字形を持たないときは、その単位が先頭になる)。
pub(crate) fn group_by_grapheme<'a>(
    text: &'a str,
    glyphs: &'a [Shaped],
) -> Vec<(&'a str, Vec<&'a Shaped>)> {
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let mut per: Vec<Vec<&Shaped>> = vec![Vec::new(); graphemes.len()];
    for g in glyphs {
        let idx = match graphemes.binary_search_by(|&(s, _)| s.cmp(&g.cluster)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        per[idx].push(g);
    }
    // 単位ごとのバイト範囲 [開始, 終了)
    let mut spans: Vec<(Range<usize>, Vec<&Shaped>)> = Vec::new();
    for ((start, g), gl) in graphemes.iter().zip(per) {
        let end = start + g.len();
        match spans.last_mut() {
            Some(last) if gl.is_empty() => last.0.end = end,
            _ => spans.push((*start..end, gl)),
        }
    }
    spans.into_iter().map(|(r, gl)| (&text[r], gl)).collect()
}

pub fn shape_units(ctx: &LayoutContext, text: &str) -> Vec<Unit> {
    let face = rustybuzz::Face::from_slice(ctx.face.data(), ctx.face.index())
        .expect("FontFace は読み込み時に検査済み");
    let features = ctx.face.vertical_features();
    let s = ctx.font_size / f32::from(ctx.face.units_per_em);
    let asc_desc = (i32::from(ctx.face.ascender) + i32::from(ctx.face.descender)) as f32 * s;
    let fs = ctx.font_size;
    let mut units = Vec::new();
    for (kind, range) in split_runs(text) {
        let run = &text[range];
        match kind {
            RunKind::Upright => {
                let glyphs = shape_run(&face, &features, run, Direction::TopToBottom);
                for (g_text, gl) in group_by_grapheme(run, &glyphs) {
                    let mut pen = 0i32;
                    let mut placed = Vec::new();
                    for g in &gl {
                        placed.push(PlacedGlyph {
                            glyph_id: g.glyph_id,
                            x: fs / 2.0 + g.x_offset as f32 * s,
                            y: -((pen + g.y_offset) as f32) * s,
                            transform: GlyphTransform::Upright,
                        });
                        pen += g.y_advance;
                    }
                    let adv: i32 = gl.iter().map(|g| g.y_advance.abs()).sum();
                    units.push(Unit {
                        text: g_text.to_string(),
                        height: adv as f32 * s,
                        glyphs: placed,
                    });
                }
            }
            RunKind::Tcy => {
                let glyphs = shape_run(&face, &[], run, Direction::LeftToRight);
                let w_px = glyphs.iter().map(|g| g.x_advance).sum::<i32>() as f32 * s;
                let scale_x = if w_px > 0.0 {
                    (fs / w_px).min(1.0)
                } else {
                    1.0
                };
                let left = (fs - w_px * scale_x) / 2.0;
                let y = (fs + asc_desc) / 2.0;
                let mut pen = 0i32;
                let mut placed = Vec::new();
                for g in &glyphs {
                    placed.push(PlacedGlyph {
                        glyph_id: g.glyph_id,
                        x: left + (pen + g.x_offset) as f32 * s * scale_x,
                        y,
                        transform: GlyphTransform::Tcy { scale_x },
                    });
                    pen += g.x_advance;
                }
                units.push(Unit {
                    text: run.to_string(),
                    height: fs,
                    glyphs: placed,
                });
            }
            RunKind::Rotated => {
                let glyphs = shape_run(&face, &[], run, Direction::LeftToRight);
                for (g_text, gl) in group_by_grapheme(run, &glyphs) {
                    let mut pen = 0i32;
                    let mut placed = Vec::new();
                    for g in &gl {
                        placed.push(PlacedGlyph {
                            glyph_id: g.glyph_id,
                            x: fs / 2.0 - asc_desc / 2.0,
                            y: (pen + g.x_offset) as f32 * s,
                            transform: GlyphTransform::Rotated,
                        });
                        pen += g.x_advance;
                    }
                    units.push(Unit {
                        text: g_text.to_string(),
                        height: pen as f32 * s,
                        glyphs: placed,
                    });
                }
            }
        }
    }
    units
}

pub fn layout(ctx: &LayoutContext, text: &str, inner_h: f32) -> Layout {
    let units = shape_units(ctx, text);
    let classes: Vec<UnitClass> = units
        .iter()
        .map(|u| UnitClass {
            height: u.height,
            no_start: u.text.chars().next().is_some_and(is_no_start),
            no_end: u.text.chars().last().is_some_and(is_no_end),
        })
        .collect();
    let columns: Vec<Column> = break_columns(&classes, inner_h)
        .into_iter()
        .map(|r| {
            let height = units[r.clone()].iter().map(|u| u.height).sum::<f32>();
            Column { units: r, height }
        })
        .collect();
    let n = columns.len();
    let width = if n == 0 {
        0.0
    } else {
        n as f32 * ctx.font_size + (n - 1) as f32 * ctx.font_size * ctx.line_spacing
    };
    let height = columns.iter().map(|c| c.height).fold(0.0f32, f32::max);
    Layout {
        units,
        columns,
        width,
        height,
    }
}

/// max_inner_h のもとでの、余白を含む最小の大きさ(切り上げの整数 px)
pub fn min_size(ctx: &LayoutContext, text: &str) -> (u32, u32) {
    let l = layout(ctx, text, ctx.max_inner_h as f32);
    // padding が大きくてもあふれないよう u64 で足し、u32::MAX に飽和させる
    let pad = 2 * u64::from(ctx.padding);
    let add = |v: f32| u32::try_from(u64::from(v.ceil() as u32) + pad).unwrap_or(u32::MAX);
    if l.columns.is_empty() {
        let e = add(ctx.font_size);
        return (e, e);
    }
    (add(l.width), add(l.height))
}

/// 大きさ w×h の枠の内側で組版して収まるか(設計書 §6.2)
pub fn fits(ctx: &LayoutContext, text: &str, w: u32, h: u32) -> bool {
    let pad = 2 * i64::from(ctx.padding);
    let (iw, ih) = (i64::from(w) - pad, i64::from(h) - pad);
    if iw <= 0 || ih <= 0 {
        return false;
    }
    let (iw, ih) = (iw as f32, ih as f32);
    let l = layout(ctx, text, ih);
    l.width <= iw + EPS && l.columns.iter().all(|c| c.height <= ih + EPS)
}

/// 列 i の中心の x(内側の幅 inner_w の中で、列の束を中央に置き、右から左へ)
pub fn column_center_x(layout: &Layout, ctx: &LayoutContext, i: usize, inner_w: f32) -> f32 {
    let right = (inner_w + layout.width) / 2.0;
    right - i as f32 * ctx.font_size * (1.0 + ctx.line_spacing) - ctx.font_size / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;

    fn face() -> FontFace {
        FontFace::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap()
    }
    fn ctx(face: &FontFace) -> LayoutContext<'_> {
        LayoutContext {
            face,
            font_size: 40.0,
            line_spacing: 0.5,
            padding: 10,
            max_inner_h: 200,
        }
    }

    #[test]
    fn split_runs_classifies_by_byte_range() {
        // "12日345AB 港": 12=0..2, 日=2..5, "345AB "=5..11, 港=11..14
        assert_eq!(
            split_runs("12日345AB 港"),
            vec![
                (RunKind::Tcy, 0..2),
                (RunKind::Upright, 2..5),
                (RunKind::Rotated, 5..11),
                (RunKind::Upright, 11..14),
            ]
        );
    }

    #[test]
    fn kanji_units_are_one_em_high() {
        let f = face();
        let units = shape_units(&ctx(&f), "港朝");
        assert_eq!(units.len(), 2);
        for u in &units {
            assert!((u.height - 40.0).abs() < 1.0, "{}", u.height);
            assert_eq!(u.glyphs.len(), 1);
            assert_eq!(u.glyphs[0].transform, GlyphTransform::Upright);
        }
    }

    #[test]
    fn vertical_forms_are_substituted() {
        // 縦書きの「、」「ー」「（」は横書きと別の字形(vrt2/vert)になる
        let f = face();
        let c = ctx(&f);
        let rb = rustybuzz::Face::from_slice(f.data(), f.index()).unwrap();
        for ch in ['、', 'ー', '（', '「'] {
            let horizontal = rb.glyph_index(ch).unwrap().0;
            let u = &shape_units(&c, &ch.to_string())[0];
            assert_ne!(u.glyphs[0].glyph_id, horizontal, "{ch}");
        }
    }

    #[test]
    fn two_digits_are_tate_chu_yoko_three_are_rotated() {
        let f = face();
        let units = shape_units(&ctx(&f), "12日345");
        assert_eq!(units[0].text, "12");
        assert!(matches!(
            units[0].glyphs[0].transform,
            GlyphTransform::Tcy { .. }
        ));
        assert_eq!(units[0].glyphs.len(), 2);
        assert!((units[0].height - 40.0).abs() < 1e-3);
        assert_eq!(units[1].text, "日");
        let rotated: Vec<&Unit> = units[2..].iter().collect();
        assert_eq!(rotated.len(), 3);
        assert!(rotated
            .iter()
            .all(|u| u.glyphs[0].transform == GlyphTransform::Rotated));
    }

    /// フォントの表から、字形 id の横送り(px)と asc+desc(px)を求める(組版の実装とは別の経路)
    fn metrics(f: &FontFace) -> (impl Fn(u16) -> f32 + '_, f32) {
        let s = 40.0 / f32::from(f.units_per_em);
        let tf = rustybuzz::ttf_parser::Face::parse(f.data(), f.index()).unwrap();
        let adv = move |gid: u16| {
            f32::from(
                tf.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(gid))
                    .unwrap(),
            ) * s
        };
        let asc_desc = (i32::from(f.ascender) + i32::from(f.descender)) as f32 * s;
        (adv, asc_desc)
    }

    #[test]
    fn tate_chu_yoko_positions_scale_and_advance_follow_font_metrics() {
        let f = face();
        let c = ctx(&f);
        let (adv, asc_desc) = metrics(&f);
        // 00〜99 のすべてで、表から求めた期待値と一致することを調べる
        let mut shrunk = 0;
        let mut kept = 0;
        for n in 0..100 {
            let t = format!("{n:02}");
            let u = &shape_units(&c, &t)[0];
            assert_eq!(u.glyphs.len(), 2, "{t}");
            let (a0, a1) = (adv(u.glyphs[0].glyph_id), adv(u.glyphs[1].glyph_id));
            let w = a0 + a1;
            let want_scale = (40.0 / w).min(1.0);
            let GlyphTransform::Tcy { scale_x } = u.glyphs[0].transform else {
                panic!("{t}")
            };
            assert!(
                (scale_x - want_scale).abs() < 1e-4,
                "{t}: {scale_x} {want_scale}"
            );
            // 単位の高さは常に 1 em。組全体を 1 em に収めて左右中央に置く
            assert!((u.height - 40.0).abs() < 1e-3, "{t}");
            let left = (40.0 - w * want_scale) / 2.0;
            let want_y = (40.0 + asc_desc) / 2.0;
            assert!(
                (u.glyphs[0].x - left).abs() < 1e-3,
                "{t}: {}",
                u.glyphs[0].x
            );
            assert!(
                (u.glyphs[1].x - (left + a0 * want_scale)).abs() < 1e-3,
                "{t}: {}",
                u.glyphs[1].x
            );
            assert!((u.glyphs[0].y - want_y).abs() < 1e-3, "{t}");
            assert!((u.glyphs[1].y - want_y).abs() < 1e-3, "{t}");
            if want_scale < 1.0 {
                shrunk += 1;
                // 縮小後の幅はちょうど 1 em で、左端は 0
                assert!(u.glyphs[0].x.abs() < 1e-3, "{t}");
                assert!(
                    (a0 * want_scale + a1 * want_scale - 40.0).abs() < 1e-3,
                    "{t}"
                );
            } else {
                kept += 1;
            }
        }
        // テスト用フォントでは数字の組がすべて 1 em を超えるので縮小される
        assert!(shrunk > 0, "shrunk={shrunk} kept={kept}");
    }

    #[test]
    fn rotated_units_have_font_advance_height_and_fixed_offsets() {
        let f = face();
        let c = ctx(&f);
        let (adv, asc_desc) = metrics(&f);
        let units = shape_units(&c, "AB");
        assert_eq!(units.len(), 2);
        for u in &units {
            assert_eq!(u.glyphs.len(), 1);
            let g = &u.glyphs[0];
            assert_eq!(g.transform, GlyphTransform::Rotated);
            // 単位の高さは横送り、単位の中の原点は y = 0。x は列の中央から asc+desc の半分を引いた位置
            assert!((u.height - adv(g.glyph_id)).abs() < 1e-3, "{}", u.text);
            assert!(g.y.abs() < 1e-3, "{}", u.text);
            assert!((g.x - (20.0 - asc_desc / 2.0)).abs() < 1e-3, "{}", u.text);
        }
    }

    #[test]
    fn latin_word_is_rotated_per_grapheme() {
        let f = face();
        let units = shape_units(&ctx(&f), "AB港");
        assert_eq!(
            units.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
            vec!["A", "B", "港"]
        );
        assert!(units[0].height < 40.0);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        let f = face();
        // が = か + 結合濁点(U+3099)は1単位
        let units = shape_units(&ctx(&f), "か\u{3099}き");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].text, "か\u{3099}");
    }

    #[test]
    fn layout_breaks_into_columns_with_kinsoku() {
        let f = face();
        let c = ctx(&f);
        // 内側の高さ 120 = 3文字。「あいう、え」→ あい|う、え(、を列頭に置かない)
        let l = layout(&c, "あいう、え", 120.0);
        assert_eq!(
            l.columns
                .iter()
                .map(|c| c.units.clone())
                .collect::<Vec<_>>(),
            vec![0..2, 2..5]
        );
        assert!((l.width - (2.0 * 40.0 + 1.0 * 20.0)).abs() < 1e-3);
    }

    #[test]
    fn min_size_and_fits_agree() {
        let f = face();
        let c = ctx(&f);
        let text = "一二三四五六七八九十港朝";
        let (w, h) = min_size(&c, text);
        assert!(fits(&c, text, w, h));
        assert!(fits(&c, text, w + 50, h + 50));
        assert!(!fits(&c, text, w, h - 45)); // 背を低くすると列が増えて幅が足りない
                                             // 幅を広げて背を低くする調整はできる
        let wide = w + 60 + 20;
        assert!(fits(&c, text, wide, h - 40));
        assert!(!fits(&c, text, 15, 15)); // 内側が正でない
    }

    #[test]
    fn same_input_gives_identical_layout() {
        let f = face();
        let c = ctx(&f);
        assert_eq!(
            layout(&c, "港の朝、12日。AB", 160.0),
            layout(&c, "港の朝、12日。AB", 160.0)
        );
    }

    #[test]
    fn huge_padding_does_not_overflow() {
        let f = face();
        let mut c = ctx(&f);
        c.padding = u32::MAX;
        assert_eq!(min_size(&c, "港"), (u32::MAX, u32::MAX));
        assert_eq!(min_size(&c, ""), (u32::MAX, u32::MAX));
        assert!(!fits(&c, "港", u32::MAX, u32::MAX));
    }

    #[test]
    fn empty_text_has_minimal_size() {
        let f = face();
        assert_eq!(min_size(&ctx(&f), ""), (60, 60));
    }

    #[test]
    fn column_center_runs_right_to_left() {
        let f = face();
        let c = ctx(&f);
        let l = layout(&c, "あいうえお", 80.0); // 2文字/列 → 3列、幅 160
        assert_eq!(l.columns.len(), 3);
        let x0 = column_center_x(&l, &c, 0, 200.0);
        let x2 = column_center_x(&l, &c, 2, 200.0);
        assert!((x0 - 160.0).abs() < 1e-3, "{x0}");
        assert!((x2 - 40.0).abs() < 1e-3, "{x2}");
    }
}
