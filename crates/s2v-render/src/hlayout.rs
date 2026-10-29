//! 横書きの折り返し(字幕用)。縦書きの `layout` と同じ書記素の単位・禁則を、幅で行に分ける。

use std::ops::Range;

use rustybuzz::Direction;

use crate::font::FontFace;
use crate::kinsoku::{break_columns, is_no_end, is_no_start, UnitClass};
use crate::layout::{group_by_grapheme, shape_run};

/// 1字形の配置(px。単位の左端・ベースラインからの相対)
#[derive(Debug, Clone, PartialEq)]
pub struct HGlyph {
    pub glyph_id: u16,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HUnit {
    pub text: String,
    pub width: f32,
    pub glyphs: Vec<HGlyph>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HLine {
    pub units: Range<usize>,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HLayout {
    pub units: Vec<HUnit>,
    pub lines: Vec<HLine>,
}

/// 横書きで整形し、`inner_w` を上限に行へ分ける。行の高さは `font_size`(行間は呼び出し側)。
pub fn hlayout(face: &FontFace, font_size: f32, text: &str, inner_w: f32) -> HLayout {
    let rb = rustybuzz::Face::from_slice(face.data(), face.index())
        .expect("FontFace は読み込み時に検査済み");
    let s = font_size / f32::from(face.units_per_em);
    let shaped = shape_run(&rb, &[], text, Direction::LeftToRight);
    let mut units = Vec::new();
    for (g_text, gl) in group_by_grapheme(text, &shaped) {
        let mut pen = 0i32;
        let mut glyphs = Vec::new();
        for g in &gl {
            glyphs.push(HGlyph {
                glyph_id: g.glyph_id,
                x: (pen + g.x_offset) as f32 * s,
                y: -(g.y_offset as f32) * s,
            });
            pen += g.x_advance;
        }
        units.push(HUnit {
            text: g_text.to_string(),
            width: pen as f32 * s,
            glyphs,
        });
    }
    let classes: Vec<UnitClass> = units
        .iter()
        .map(|u| UnitClass {
            height: u.width,
            no_start: u.text.chars().next().is_some_and(is_no_start),
            no_end: u.text.chars().last().is_some_and(is_no_end),
        })
        .collect();
    let lines = break_columns(&classes, inner_w)
        .into_iter()
        .map(|r| {
            let width = units[r.clone()].iter().map(|u| u.width).sum();
            HLine { units: r, width }
        })
        .collect();
    HLayout { units, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face() -> FontFace {
        FontFace::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap()
    }

    #[test]
    fn short_text_is_one_line() {
        let l = hlayout(&face(), 40.0, "港朝", 400.0);
        assert_eq!(l.lines.len(), 1);
        assert!(
            (l.lines[0].width - 80.0).abs() < 2.0,
            "{}",
            l.lines[0].width
        );
    }

    #[test]
    fn wraps_at_inner_width() {
        let l = hlayout(&face(), 40.0, "港朝港朝港", 100.0);
        let lens: Vec<usize> = l.lines.iter().map(|x| x.units.len()).collect();
        assert_eq!(lens, vec![2, 2, 1]);
    }

    #[test]
    fn line_start_prohibited_char_is_pulled_back() {
        let l = hlayout(&face(), 40.0, "港朝、", 80.0);
        assert!(l.lines.len() >= 2);
        assert_ne!(l.units[l.lines[1].units.start].text, "、");
    }

    #[test]
    fn long_unbreakable_run_does_not_hang() {
        let t = "港".repeat(400);
        let l = hlayout(&face(), 40.0, &t, 100.0);
        assert!(l.lines.len() >= 100, "{}", l.lines.len());
    }

    #[test]
    fn empty_text_has_no_lines() {
        assert!(hlayout(&face(), 40.0, "", 100.0).lines.is_empty());
    }
}
