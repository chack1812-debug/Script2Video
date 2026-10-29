//! 字幕(最下段・横書き・最大 3 行)の測定と描画(字幕モード設計書 §5.2・§5.3)。

use rustybuzz::ttf_parser::{self, GlyphId};
use s2v_core::config::RenderConfig;
use tiny_skia::{FillRule, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::draw::{GlyphPath, Overlay};
use crate::font::FontFace;
use crate::hlayout::{hlayout, HLayout};

/// 字幕の本文の最大行数(設定項目にしない)
pub const SUBTITLE_MAX_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtitleMeasure {
    /// 本文の行数(ラベルは数えない)
    pub lines: usize,
    pub band_h: u32,
    /// 帯の上辺より上に出るラベルの高さ(ラベルなしなら 0)
    pub label_h: u32,
    /// ラベルの幅(ラベルなしなら 0)。帯より広いと画像の幅がこれまで広がる
    pub label_w: u32,
    /// 本文の文字が占める幅(最長の行の幅。余白・縁取りは含まない)
    pub text_w: u32,
}

pub fn band_width(cfg: &RenderConfig) -> u32 {
    cfg.width.saturating_sub(2 * cfg.margin)
}

/// 本文を整形する。改行文字は呼び出し側で除く前提だが、残っていても空白として扱う。
fn body_layout(face: &FontFace, cfg: &RenderConfig, text: &str, band_w: u32) -> HLayout {
    let inner_w = band_w.saturating_sub(2 * cfg.subtitle_padding) as f32;
    hlayout(face, cfg.font_size, &text.replace('\n', " "), inner_w)
}

/// 発言者名として描く文字列(空ならラベルなし)
fn speaker_name(speaker: Option<&str>) -> Option<&str> {
    speaker.filter(|s| !s.is_empty())
}

fn label_height(cfg: &RenderConfig, speaker: Option<&str>) -> u32 {
    if speaker_name(speaker).is_some() {
        cfg.font_size.ceil() as u32 + 2 * cfg.speaker_padding
    } else {
        0
    }
}

pub fn measure(
    face: &FontFace,
    cfg: &RenderConfig,
    text: &str,
    speaker: Option<&str>,
) -> SubtitleMeasure {
    let body_l = body_layout(face, cfg, text, band_width(cfg));
    let lines = body_l.lines.len();
    let text_w = body_l.lines.iter().map(|x| x.width).fold(0.0, f32::max);
    let fs = cfg.font_size;
    let body = if lines == 0 {
        0.0
    } else {
        lines as f32 * fs + (lines - 1) as f32 * fs * cfg.line_spacing
    };
    SubtitleMeasure {
        lines,
        band_h: body.ceil() as u32 + 2 * cfg.subtitle_padding,
        label_h: label_height(cfg, speaker),
        label_w: speaker_name(speaker).map_or(0, |n| label_width(face, cfg, n)),
        text_w: text_w.ceil() as u32,
    }
}

/// ラベルの幅(役名の幅 + 左右の余白)
fn label_width(face: &FontFace, cfg: &RenderConfig, name: &str) -> u32 {
    let l = hlayout(
        face,
        cfg.font_size,
        &name.replace('\n', " "),
        f32::MAX / 4.0,
    );
    let w = l.lines.iter().map(|x| x.width).fold(0.0, f32::max);
    (w + 2.0 * cfg.speaker_padding as f32).ceil() as u32
}

fn solid(rgb: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    p.set_color_rgba8(rgb[0], rgb[1], rgb[2], a);
    p.anti_alias = true;
    p
}

/// 矩形を塗り、`stroke_w > 0` なら内側に収まる枠線を描く。
fn draw_box(
    pm: &mut Pixmap,
    (x, y, w, h): (f32, f32, f32, f32),
    fill: ([u8; 3], f32),
    stroke: ([u8; 3], f32),
) {
    let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else {
        return;
    };
    pm.fill_rect(rect, &solid(fill.0, fill.1), Transform::identity(), None);
    let sw = stroke.1;
    if sw > 0.0 {
        if let Some(inner) = tiny_skia::Rect::from_xywh(
            x + sw / 2.0,
            y + sw / 2.0,
            (w - sw).max(0.0),
            (h - sw).max(0.0),
        ) {
            let st = Stroke {
                width: sw,
                line_join: LineJoin::Miter,
                ..Stroke::default()
            };
            pm.stroke_path(
                &PathBuilder::from_rect(inner),
                &solid(stroke.0, 1.0),
                &st,
                Transform::identity(),
                None,
            );
        }
    }
}

/// 字形を描く方法
#[derive(Clone, Copy)]
enum Pass {
    /// 色と幅で縁取る(文字の外側にだけ縁が残るよう、後で文字を重ねる)
    Outline([u8; 3], f32),
    /// 文字色で塗る
    Fill([u8; 3]),
}

/// 1 行分の字形を描く。`left` は行の左端、`baseline` はベースラインの y。
fn draw_line(
    pm: &mut Pixmap,
    face: &FontFace,
    ttf: &ttf_parser::Face,
    (layout, line): (&HLayout, usize),
    (left, baseline): (f32, f32),
    font_size: f32,
    pass: Pass,
) {
    let s = font_size / f32::from(face.units_per_em);
    let mut pen = left;
    for unit in &layout.units[layout.lines[line].units.clone()] {
        for g in &unit.glyphs {
            let (ox, oy) = (pen + g.x, baseline + g.y);
            let mut gp = GlyphPath {
                pb: PathBuilder::new(),
                map: |px: f32, py: f32| (ox + px * s, oy - py * s),
            };
            if ttf.outline_glyph(GlyphId(g.glyph_id), &mut gp).is_none() {
                continue;
            }
            let Some(path) = gp.pb.finish() else {
                continue;
            };
            match pass {
                Pass::Outline(color, w) if w > 0.0 => {
                    let st = Stroke {
                        width: 2.0 * w,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    pm.stroke_path(&path, &solid(color, 1.0), &st, Transform::identity(), None);
                }
                Pass::Outline(..) => {}
                Pass::Fill(color) => pm.fill_path(
                    &path,
                    &solid(color, 1.0),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                ),
            }
        }
        pen += unit.width;
    }
}

/// 行ごとのベースライン(行の箱は高さ `font_size`、アセント・ディセントで上下中央)
fn baseline_in_line(face: &FontFace, font_size: f32, line_top: f32) -> f32 {
    let s = font_size / f32::from(face.units_per_em);
    let content = (i32::from(face.ascender) - i32::from(face.descender)) as f32 * s;
    line_top + (font_size - content) / 2.0 + f32::from(face.ascender) * s
}

/// `band_w` は通常 `band_width(cfg)`、`band_h` は `measure` の `band_h`。
/// 本文は `band_w` から余白を引いた幅で折り返す。
pub fn draw_subtitle(
    face: &FontFace,
    cfg: &RenderConfig,
    text: &str,
    speaker: Option<&str>,
    band_w: u32,
    band_h: u32,
) -> Result<Overlay, String> {
    let ttf = ttf_parser::Face::parse(face.data(), face.index())
        .map_err(|e| format!("フォントを読めません: {e}"))?;
    let fs = cfg.font_size;
    let label_h = label_height(cfg, speaker);
    let name = speaker_name(speaker);

    // ラベルの文字の整形と幅
    let name_layout = name.map(|n| hlayout(face, fs, &n.replace('\n', " "), f32::MAX / 4.0));
    let name_w = name_layout
        .as_ref()
        .map_or(0.0, |l| l.lines.iter().map(|x| x.width).fold(0.0, f32::max));
    let label_w = if name.is_some() {
        (name_w + 2.0 * cfg.speaker_padding as f32).ceil() as u32
    } else {
        0
    };

    let (iw, ih) = (band_w.max(label_w), band_h + label_h);
    let mut pm = Pixmap::new(iw, ih).ok_or_else(|| format!("画像の大きさが不正です({iw}x{ih})"))?;

    let rgb = |s: &str, d: [u8; 3]| RenderConfig::rgb(s).unwrap_or(d);
    let top = label_h as f32;

    // 帯
    draw_box(
        &mut pm,
        (0.0, top, band_w as f32, band_h as f32),
        (
            rgb(&cfg.subtitle_band_fill, [0, 0, 0]),
            cfg.subtitle_band_opacity,
        ),
        (
            rgb(&cfg.subtitle_band_stroke, [255, 255, 255]),
            cfg.subtitle_band_stroke_width.max(0.0),
        ),
    );

    // 本文(左揃え、行ごとに上から)
    let body = body_layout(face, cfg, text, band_w);
    let text_color = rgb(&cfg.subtitle_text_color, [255, 255, 255]);
    let outline = (
        rgb(&cfg.subtitle_outline_color, [0, 0, 0]),
        cfg.subtitle_outline_width.max(0.0),
    );
    let pitch = fs * (1.0 + cfg.line_spacing);
    let pad = cfg.subtitle_padding as f32;
    let bases: Vec<f32> = (0..body.lines.len())
        .map(|i| baseline_in_line(face, fs, top + pad + i as f32 * pitch))
        .collect();
    // 縁取りを全行・全字で先に描き、その上に文字色を重ねる(隣の字の縁が文字を隠さない)
    for pass in [Pass::Outline(outline.0, outline.1), Pass::Fill(text_color)] {
        for (i, base) in bases.iter().enumerate() {
            draw_line(&mut pm, face, &ttf, (&body, i), (pad, *base), fs, pass);
        }
    }

    // ラベル(帯の左上に接し、下辺が帯の上辺)
    if let Some(nl) = &name_layout {
        let sp = cfg.speaker_padding as f32;
        draw_box(
            &mut pm,
            (0.0, 0.0, label_w as f32, label_h as f32),
            (rgb(&cfg.speaker_fill, [51, 51, 51]), 1.0),
            (
                rgb(&cfg.speaker_stroke, [255, 255, 255]),
                cfg.speaker_stroke_width.max(0.0),
            ),
        );
        let base = baseline_in_line(face, fs, sp);
        let tc = rgb(&cfg.speaker_text_color, [255, 255, 255]);
        if !nl.lines.is_empty() {
            draw_line(&mut pm, face, &ttf, (nl, 0), (sp, base), fs, Pass::Fill(tc));
        }
    }

    Ok(Overlay {
        dx: 0,
        dy: -(label_h as i32),
        pixmap: pm,
    })
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
    fn cfg() -> RenderConfig {
        RenderConfig {
            font_size: 40.0,
            ..RenderConfig::default()
        }
    }
    fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }
    /// 帯の内側がちょうど `n` 文字分になる設定
    fn cfg_n_chars(n: u32) -> RenderConfig {
        let mut c = cfg();
        c.margin = 10;
        c.subtitle_padding = 20;
        c.width = 2 * c.margin + 2 * c.subtitle_padding + n * 40 + 2;
        c
    }
    fn chars(n: usize) -> String {
        "港朝".chars().cycle().take(n).collect()
    }

    #[test]
    fn measure_counts_body_lines_only() {
        let (f, c) = (face(), cfg_n_chars(5));
        let m = measure(&f, &c, &chars(15), None);
        assert_eq!(m.lines, 3);
        assert_eq!(m.label_h, 0);
        let m2 = measure(&f, &c, &chars(15), Some("A"));
        assert_eq!(m2.lines, 3);
        assert!(m2.label_h > 0);
        assert_eq!(m2.label_h, 40 + 2 * c.speaker_padding);
        assert_eq!(measure(&f, &c, &chars(15), Some("")).label_h, 0);
        // 帯の高さ = 3行 + 行間2つ + 余白
        assert_eq!(
            m.band_h,
            (3.0f32 * 40.0 + 2.0 * 40.0 * c.line_spacing).ceil() as u32 + 40
        );
        assert_eq!(band_width(&c), c.width - 2 * c.margin);
    }

    #[test]
    fn measure_boundary_three_vs_four_lines() {
        let (f, c) = (face(), cfg_n_chars(5));
        assert_eq!(measure(&f, &c, &chars(15), None).lines, 3);
        assert_eq!(measure(&f, &c, &chars(16), None).lines, 4);
    }

    #[test]
    fn newline_in_body_is_treated_as_space() {
        let (f, c) = (face(), cfg_n_chars(5));
        assert_eq!(
            measure(&f, &c, "港\n朝", None),
            measure(&f, &c, "港 朝", None)
        );
    }

    #[test]
    fn draw_places_band_and_label() {
        let (f, mut c) = (face(), cfg_n_chars(5));
        c.subtitle_band_opacity = 1.0;
        c.speaker_stroke_width = 2.0;
        let m = measure(&f, &c, "港朝", Some("A"));
        let bw = band_width(&c);
        let o = draw_subtitle(&f, &c, "港朝", Some("A"), bw, m.band_h).unwrap();
        assert_eq!(o.dx, 0);
        assert_eq!(o.dy, -(m.label_h as i32));
        assert_eq!(
            (o.pixmap.width(), o.pixmap.height()),
            (bw, m.band_h + m.label_h)
        );
        // 帯の内側(右寄りの文字の無い所)は不透明な帯の色
        assert_eq!(
            px(&o.pixmap, bw - 30, m.label_h + m.band_h / 2),
            [0, 0, 0, 255]
        );
        // ラベルの左上の塗り(縁の内側・文字の外)
        assert_eq!(px(&o.pixmap, 4, 4), [0x33, 0x33, 0x33, 255]);
        // 帯の上辺のすぐ上(ラベルの外)は透明
        assert_eq!(px(&o.pixmap, bw - 30, m.label_h - 1)[3], 0);

        c.subtitle_band_opacity = 0.5;
        let o = draw_subtitle(&f, &c, "港朝", Some("A"), bw, m.band_h).unwrap();
        let a = px(&o.pixmap, bw - 30, m.label_h + m.band_h / 2)[3];
        assert!(a.abs_diff(128) <= 1, "{a}");
    }

    #[test]
    fn first_glyph_is_inside_top_left_padding_and_upright() {
        let (f, mut c) = (face(), cfg_n_chars(5));
        c.subtitle_band_opacity = 1.0;
        c.subtitle_outline_width = 0.0;
        let m = measure(&f, &c, "L", None);
        let o = draw_subtitle(&f, &c, "L", None, band_width(&c), m.band_h).unwrap();
        let pad = c.subtitle_padding;
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for y in 0..o.pixmap.height() {
            for x in 0..o.pixmap.width() {
                if px(&o.pixmap, x, y)[0] > 128 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                    sx += x as f32;
                    sy += y as f32;
                    n += 1.0;
                }
            }
        }
        assert!(n > 0.0, "文字色の画素が無い");
        // 1 文字目は padding の内側の 1 文字分の枠(40 四方)に収まる
        assert!(x0 >= pad && y0 >= pad, "{x0} {y0}");
        assert!(x1 < pad + 40 && y1 < pad + 40, "{x1} {y1}");
        // 左上寄りの位置に実際に塗られている
        assert!(x0 < pad + 20 && y0 < pad + 30, "{x0} {y0}");
        // L は縦棒が左・横棒が下(重心が外接矩形の左下寄り)= 向きが正しい
        let (bx, by) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
        assert!(
            sx / n < bx && sy / n > by,
            "重心 ({},{}) 中心 ({bx},{by})",
            sx / n,
            sy / n
        );
    }

    #[test]
    fn outline_width_zero_draws_no_outline() {
        let (f, mut c) = (face(), cfg_n_chars(5));
        let m = measure(&f, &c, "港朝", None);
        let bw = band_width(&c);
        c.subtitle_outline_width = 0.0;
        let a = draw_subtitle(&f, &c, "港朝", None, bw, m.band_h).unwrap();
        c.subtitle_outline_width = 3.0;
        let b = draw_subtitle(&f, &c, "港朝", None, bw, m.band_h).unwrap();
        assert_ne!(a.pixmap.data(), b.pixmap.data());
    }

    #[test]
    fn draw_wraps_by_the_given_band_width() {
        let (f, mut c) = (face(), cfg_n_chars(5));
        c.subtitle_band_opacity = 1.0;
        c.subtitle_outline_width = 0.0;
        let text = chars(5);
        let bw = band_width(&c);
        let m = measure(&f, &c, &text, None);
        assert_eq!(m.lines, 1);
        let inked_rows = |o: &Overlay| {
            (0..o.pixmap.height())
                .filter(|&y| (0..o.pixmap.width()).any(|x| px(&o.pixmap, x, y)[0] > 128))
                .count()
        };
        let wide = draw_subtitle(&f, &c, &text, None, bw, m.band_h).unwrap();
        // 3 文字分の幅に狭めると 2 行になり、2 行目にも描かれる
        let narrow_w = 2 * c.subtitle_padding + 3 * 40 + 2;
        let narrow_h = m.band_h + (40.0 * (1.0 + c.line_spacing)).ceil() as u32;
        let narrow = draw_subtitle(&f, &c, &text, None, narrow_w, narrow_h).unwrap();
        assert!(inked_rows(&narrow) > inked_rows(&wide));
        let second_line_top = c.subtitle_padding + (40.0 * (1.0 + c.line_spacing)) as u32;
        assert!((second_line_top..narrow_h - c.subtitle_padding)
            .any(|y| (0..narrow_w).any(|x| px(&narrow.pixmap, x, y)[0] > 128)));
    }
}
