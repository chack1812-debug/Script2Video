//! 吹き出し・ト書きの描画と、画面の大きさの画像への合成(映像工程設計書 §6.2–6.3)。

use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder};
use s2v_core::config::RenderConfig;
use tiny_skia::{FillRule, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use crate::layout::{column_center_x, fits, layout, GlyphTransform, LayoutContext};
use crate::ItemKind;

/// 吹き出しの角の半径(短辺に対する比)
const CORNER_RATIO: f32 = 0.4;
/// 尻尾の根元の半幅(短辺に対する比)
const TAIL_HALF_RATIO: f32 = 0.12;
/// 円弧を3次ベジェで近似する係数
const ARC_K: f32 = 0.552_284_8;

#[derive(Debug, Clone)]
pub struct Style {
    pub fill: [u8; 3],
    pub stroke: [u8; 3],
    pub stroke_width: f32,
    pub text: [u8; 3],
}

impl Style {
    pub fn bubble(cfg: &RenderConfig) -> Style {
        Style {
            fill: RenderConfig::rgb(&cfg.bubble_fill).unwrap_or([255, 255, 255]),
            stroke: RenderConfig::rgb(&cfg.bubble_stroke).unwrap_or([0, 0, 0]),
            stroke_width: cfg.bubble_stroke_width,
            text: RenderConfig::rgb(&cfg.text_color).unwrap_or([0, 0, 0]),
        }
    }

    pub fn note(cfg: &RenderConfig) -> Style {
        Style {
            fill: RenderConfig::rgb(&cfg.note_fill).unwrap_or([255, 251, 230]),
            stroke: RenderConfig::rgb(&cfg.note_stroke).unwrap_or([0, 0, 0]),
            stroke_width: cfg.note_stroke_width,
            text: RenderConfig::rgb(&cfg.text_color).unwrap_or([0, 0, 0]),
        }
    }
}

pub struct ItemSpec<'a> {
    pub kind: ItemKind,
    pub text: &'a str,
    pub w: u32,
    pub h: u32,
    /// 枠の中心から尻尾の先端への相対位置(吹き出しだけ)
    pub tail: Option<(i32, i32)>,
}

pub struct Overlay {
    /// 画像の左上の、枠の左上からの相対位置(尻尾があれば負になりうる)
    pub dx: i32,
    pub dy: i32,
    pub pixmap: Pixmap,
}

fn paint(rgb: [u8; 3]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    p.anti_alias = true;
    p
}

/// 角丸の矩形の経路
fn rounded_rect(w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let k = r * ARC_K;
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(w - r, 0.0);
    pb.cubic_to(w - r + k, 0.0, w, r - k, w, r);
    pb.line_to(w, h - r);
    pb.cubic_to(w, h - r + k, w - r + k, h, w - r, h);
    pb.line_to(r, h);
    pb.cubic_to(r - k, h, 0.0, h - r + k, 0.0, h - r);
    pb.line_to(0.0, r);
    pb.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

/// 尻尾の三角形の経路。先端が中心と同じ位置なら None。
fn tail_path(w: f32, h: f32, tail: (i32, i32)) -> Option<tiny_skia::Path> {
    let (tx, ty) = (tail.0 as f32, tail.1 as f32);
    let len = (tx * tx + ty * ty).sqrt();
    if len == 0.0 {
        return None;
    }
    let half = w.min(h) * TAIL_HALF_RATIO;
    let (px, py) = (-ty / len * half, tx / len * half);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(cx + px, cy + py);
    pb.line_to(cx + tx, cy + ty);
    pb.line_to(cx - px, cy - py);
    pb.close();
    pb.finish()
}

/// 字形の輪郭を、座標変換しながら経路に写す。
struct GlyphPath<F: Fn(f32, f32) -> (f32, f32)> {
    pb: PathBuilder,
    map: F,
}

impl<F: Fn(f32, f32) -> (f32, f32)> OutlineBuilder for GlyphPath<F> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = (self.map)(x, y);
        self.pb.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = (self.map)(x, y);
        self.pb.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = (self.map)(x1, y1);
        let (x, y) = (self.map)(x, y);
        self.pb.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = (self.map)(x1, y1);
        let (x2, y2) = (self.map)(x2, y2);
        let (x, y) = (self.map)(x, y);
        self.pb.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

/// 文字を内側の枠に組んで塗る。`origin` は枠の左上の画像内の位置。
fn draw_text(
    pm: &mut Pixmap,
    origin: (f32, f32),
    ctx: &LayoutContext,
    style: &Style,
    spec: &ItemSpec,
    clip: bool,
) -> Result<(), String> {
    let pad = ctx.padding as f32;
    let inner_w = spec.w as f32 - 2.0 * pad;
    let inner_h = spec.h as f32 - 2.0 * pad;
    let l = layout(ctx, spec.text, inner_h);
    let face = ttf_parser::Face::parse(ctx.face.data(), ctx.face.index())
        .map_err(|e| format!("フォントを読めません: {e}"))?;
    let s = ctx.font_size / f32::from(ctx.face.units_per_em);
    // 全ての列は、最も高い列の高さで上下中央に揃えた上端から積む
    let top = origin.1 + pad + (inner_h - l.height) / 2.0;
    let text_paint = paint(style.text);
    // 切り詰め描画では、枠の内側の矩形の外へは塗らない
    let mask = if clip {
        let rect = tiny_skia::Rect::from_xywh(
            origin.0 + pad,
            origin.1 + pad,
            inner_w.max(0.0),
            inner_h.max(0.0),
        );
        let (Some(rect), Some(mut m)) = (rect, tiny_skia::Mask::new(pm.width(), pm.height()))
        else {
            return Ok(()); // 内側が無いので文字は描けない
        };
        m.fill_path(
            &PathBuilder::from_rect(rect),
            FillRule::Winding,
            false,
            Transform::identity(),
        );
        Some(m)
    } else {
        None
    };
    for (ci, col) in l.columns.iter().enumerate() {
        let left = origin.0 + pad + column_center_x(&l, ctx, ci, inner_w) - ctx.font_size / 2.0;
        let mut uy = top;
        for unit in &l.units[col.units.clone()] {
            for g in &unit.glyphs {
                let (ox, oy) = (left + g.x, uy + g.y);
                let mut gp = GlyphPath {
                    pb: PathBuilder::new(),
                    map: |px: f32, py: f32| match g.transform {
                        GlyphTransform::Upright => (ox + px * s, oy - py * s),
                        GlyphTransform::Tcy { scale_x } => (ox + px * s * scale_x, oy - py * s),
                        // 時計回りに 90°(y 下向きの座標で (a, b) -> (-b, a))
                        GlyphTransform::Rotated => (ox + py * s, oy + px * s),
                    },
                };
                if face.outline_glyph(GlyphId(g.glyph_id), &mut gp).is_some() {
                    if let Some(path) = gp.pb.finish() {
                        pm.fill_path(
                            &path,
                            &text_paint,
                            FillRule::Winding,
                            Transform::identity(),
                            mask.as_ref(),
                        );
                    }
                }
            }
            uy += unit.height;
        }
    }
    Ok(())
}

pub fn render(ctx: &LayoutContext, style: &Style, spec: &ItemSpec) -> Result<Overlay, String> {
    if !fits(ctx, spec.text, spec.w, spec.h) {
        return Err(format!(
            "文字が枠に収まりません(枠 {}x{}、文字「{}」)",
            spec.w, spec.h, spec.text
        ));
    }
    render_impl(ctx, style, spec, false)
}

/// 自動配置が画面の大きさに切り詰めた項目用。収まらなくても Err にせず、
/// 枠の内側に入る部分だけを描き、はみ出す文字は内側の矩形で切り落とす。
pub fn render_clipped(
    ctx: &LayoutContext,
    style: &Style,
    spec: &ItemSpec,
) -> Result<Overlay, String> {
    render_impl(ctx, style, spec, true)
}

fn render_impl(
    ctx: &LayoutContext,
    style: &Style,
    spec: &ItemSpec,
    clip: bool,
) -> Result<Overlay, String> {
    let (w, h) = (spec.w as f32, spec.h as f32);
    let sw = style.stroke_width.max(0.0);
    let tail = match spec.kind {
        ItemKind::Bubble => spec.tail,
        ItemKind::Note => None,
    };

    // 外接矩形(枠・尻尾の先端・線の太さ)
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, w, h);
    if spec.kind == ItemKind::Bubble {
        if let Some((tx, ty)) = tail {
            let (cx, cy) = (w / 2.0 + tx as f32, h / 2.0 + ty as f32);
            x0 = x0.min(cx);
            y0 = y0.min(cy);
            x1 = x1.max(cx);
            y1 = y1.max(cy);
        }
        x0 -= sw;
        y0 -= sw;
        x1 += sw;
        y1 += sw;
    }
    let (dx, dy) = (x0.floor() as i32, y0.floor() as i32);
    let (iw, ih) = (x1.ceil() as i32 - dx, y1.ceil() as i32 - dy);
    let mut pm = Pixmap::new(iw.max(0) as u32, ih.max(0) as u32)
        .ok_or_else(|| format!("画像の大きさが不正です({iw}x{ih})"))?;
    let shift = Transform::from_translate(-dx as f32, -dy as f32);

    match spec.kind {
        ItemKind::Bubble => {
            let r = w.min(h) * CORNER_RATIO;
            let mut shapes = Vec::new();
            shapes.extend(rounded_rect(w, h, r));
            if let Some(t) = tail {
                shapes.extend(tail_path(w, h, t));
            }
            // (1) 本体と尻尾を 2 倍の太さで縁取り、(2) 塗りで上書きする。
            // 外側にだけ sw の縁が残り、本体と尻尾の継ぎ目の線は消える。
            if sw > 0.0 {
                let stroke = Stroke {
                    width: 2.0 * sw,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                let sp = paint(style.stroke);
                for p in &shapes {
                    pm.stroke_path(p, &sp, &stroke, shift, None);
                }
            }
            let fp = paint(style.fill);
            for p in &shapes {
                pm.fill_path(p, &fp, FillRule::Winding, shift, None);
            }
        }
        ItemKind::Note => {
            let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h)
                .ok_or_else(|| "枠の大きさが不正です".to_string())?;
            pm.fill_rect(rect, &paint(style.fill), shift, None);
            if sw > 0.0 {
                // 線は内側に収める
                let inner = tiny_skia::Rect::from_xywh(
                    sw / 2.0,
                    sw / 2.0,
                    (w - sw).max(0.0),
                    (h - sw).max(0.0),
                );
                if let Some(inner) = inner {
                    let path = PathBuilder::from_rect(inner);
                    let stroke = Stroke {
                        width: sw,
                        line_join: LineJoin::Miter,
                        ..Stroke::default()
                    };
                    pm.stroke_path(&path, &paint(style.stroke), &stroke, shift, None);
                }
            }
        }
    }

    draw_text(&mut pm, (-dx as f32, -dy as f32), ctx, style, spec, clip)?;
    Ok(Overlay { dx, dy, pixmap: pm })
}

/// 画面の大きさの透明な画像に、(絶対位置 x, y, 画像) を順に重ねる
pub fn composite(canvas_w: u32, canvas_h: u32, layers: &[(i32, i32, &Pixmap)]) -> Pixmap {
    let mut out = Pixmap::new(canvas_w, canvas_h).expect("画面の大きさは 1 以上");
    for (x, y, p) in layers {
        out.draw_pixmap(
            *x,
            *y,
            p.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;
    use crate::layout::LayoutContext;

    fn face() -> FontFace {
        FontFace::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap()
    }
    fn style() -> Style {
        Style {
            fill: [255, 255, 255],
            stroke: [0, 0, 0],
            stroke_width: 4.0,
            text: [0, 0, 0],
        }
    }
    fn px(p: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn note_box_has_fill_inside_border_and_transparent_outside() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 20,
            max_inner_h: 400,
        };
        let o = render(
            &c,
            &style(),
            &ItemSpec {
                kind: ItemKind::Note,
                text: "翌朝",
                w: 100,
                h: 140,
                tail: None,
            },
        )
        .unwrap();
        assert_eq!((o.dx, o.dy), (0, 0));
        assert_eq!((o.pixmap.width(), o.pixmap.height()), (100, 140));
        assert_eq!(px(&o.pixmap, 1, 70), [0, 0, 0, 255]); // 縁
        assert_eq!(px(&o.pixmap, 10, 5 + 4), [255, 255, 255, 255]); // 塗り(文字の無い場所)
    }

    #[test]
    fn bubble_with_tail_extends_the_image_downwards() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 28,
            max_inner_h: 400,
        };
        let o = render(
            &c,
            &style(),
            &ItemSpec {
                kind: ItemKind::Bubble,
                text: "港",
                w: 120,
                h: 160,
                tail: Some((0, 160)),
            },
        )
        .unwrap();
        assert!(o.dy <= 0 && o.dx <= 0);
        // 先端(中心 (60,80) + (0,160) = (60,240))の近くに不透明な画素がある
        let tip_y = (240 - o.dy) as u32 - 3;
        assert!(px(&o.pixmap, (60 - o.dx) as u32, tip_y)[3] > 0);
        // 角(0,0)の付近は角丸なので透明
        assert_eq!(
            px(&o.pixmap, (0 - o.dx) as u32 + 1, (0 - o.dy) as u32 + 1)[3],
            0
        );
        // 本体と尻尾の継ぎ目(本体の下端の中央の少し内側)は塗りの色
        assert_eq!(
            px(&o.pixmap, (60 - o.dx) as u32, (158 - o.dy) as u32),
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn text_is_drawn_in_text_color() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 20,
            max_inner_h: 400,
        };
        let o = render(
            &c,
            &style(),
            &ItemSpec {
                kind: ItemKind::Note,
                text: "一",
                w: 80,
                h: 80,
                tail: None,
            },
        )
        .unwrap();
        // 「一」の横線は枠の中央の高さを通る
        let row: Vec<[u8; 4]> = (20..60).map(|x| px(&o.pixmap, x, 40)).collect();
        assert!(row.iter().any(|p| p[0] < 50 && p[3] == 255), "{row:?}");
    }

    #[test]
    fn text_that_does_not_fit_is_an_error() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 20,
            max_inner_h: 400,
        };
        assert!(render(
            &c,
            &style(),
            &ItemSpec {
                kind: ItemKind::Note,
                text: "一二三四五",
                w: 60,
                h: 60,
                tail: None
            }
        )
        .is_err());
    }

    #[test]
    fn clipped_render_keeps_ink_inside_the_inner_box() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 20,
            max_inner_h: 400,
        };
        let st = Style {
            stroke_width: 0.0,
            ..style()
        };
        let spec = ItemSpec {
            kind: ItemKind::Note,
            text: "一二三四五",
            w: 60,
            h: 60,
            tail: None,
        };
        assert!(render(&c, &st, &spec).is_err());
        let o = render_clipped(&c, &st, &spec).unwrap();
        assert_eq!((o.pixmap.width(), o.pixmap.height()), (60, 60));
        let mut inside_ink = 0;
        for y in 0..60 {
            for x in 0..60 {
                let dark = px(&o.pixmap, x, y)[0] < 128;
                if (20..40).contains(&x) && (20..40).contains(&y) {
                    inside_ink += u32::from(dark);
                } else {
                    assert!(!dark, "枠の内側の外にインクがあります: ({x}, {y})");
                }
            }
        }
        assert!(inside_ink > 0, "内側にも文字が描かれるはず");
    }

    #[test]
    fn rendering_is_deterministic() {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 28,
            max_inner_h: 400,
        };
        let spec = ItemSpec {
            kind: ItemKind::Bubble,
            text: "港の朝、12日。",
            w: 200,
            h: 300,
            tail: Some((-30, 200)),
        };
        let a = render(&c, &style(), &spec).unwrap();
        let b = render(&c, &style(), &spec).unwrap();
        assert_eq!(a.pixmap.data(), b.pixmap.data());
    }

    #[test]
    fn composite_layers_in_order() {
        let mut red = tiny_skia::Pixmap::new(10, 10).unwrap();
        red.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let mut blue = tiny_skia::Pixmap::new(10, 10).unwrap();
        blue.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let out = composite(30, 20, &[(0, 0, &red), (5, 5, &blue)]);
        assert_eq!(px(&out, 2, 2), [255, 0, 0, 255]);
        assert_eq!(px(&out, 7, 7), [0, 0, 255, 255]); // 後の層が上
        assert_eq!(px(&out, 25, 15)[3], 0); // 何もない所は透明
                                            // PNG は straight alpha で書ける(半透明の色が戻る)
        let mut half = tiny_skia::Pixmap::new(1, 1).unwrap();
        half.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 128));
        let png = composite(1, 1, &[(0, 0, &half)]).encode_png().unwrap();
        let back = tiny_skia::Pixmap::decode_png(&png).unwrap();
        let got = px(&back, 0, 0);
        for (g, e) in got.iter().zip([200u8, 100, 50, 128]) {
            assert!(g.abs_diff(e) <= 1, "{got:?}");
        }
    }

    fn ctx(f: &FontFace, padding: u32) -> LayoutContext<'_> {
        LayoutContext {
            face: f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding,
            max_inner_h: 400,
        }
    }

    /// 線なし・白塗りのト書きに描いた文字のインクの (外接矩形 x0,y0,x1,y1, 重心 x,y)
    fn ink(text: &str, w: u32, h: u32) -> (u32, u32, u32, u32, f32, f32) {
        let f = face();
        let c = LayoutContext {
            face: &f,
            font_size: 40.0,
            line_spacing: 0.4,
            padding: 20,
            max_inner_h: 400,
        };
        let mut st = style();
        st.stroke_width = 0.0;
        let o = render(
            &c,
            &st,
            &ItemSpec {
                kind: ItemKind::Note,
                text,
                w,
                h,
                tail: None,
            },
        )
        .unwrap();
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for y in 0..h {
            for x in 0..w {
                if px(&o.pixmap, x, y)[0] < 128 {
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
        assert!(n > 0.0, "インクが無い");
        (x0, y0, x1, y1, sx / n, sy / n)
    }

    #[test]
    fn rotated_ascii_is_vertical_and_clockwise() {
        // 1列(幅 80 = 40 + 余白 40)に収まる回転の連なり
        let (x0, y0, x1, y1, cx, cy) = ink("L", 80, 120);
        assert!(y1 - y0 > 0 && x1 - x0 > 0);
        // 列の中央(x = 40)付近
        assert!(((x0 + x1) as f32 / 2.0 - 40.0).abs() < 12.0, "{x0} {x1}");
        // 時計回り 90° の L は、上に横棒・左に縦棒(Γ形)で、重心は外接矩形の左上寄り
        let (bx, by) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
        assert!(cx < bx && cy < by, "重心 ({cx},{cy}) 中心 ({bx},{by})");
        // 縦の位置: インクは単位の縦の範囲(上端から単位の高さ)に収まり、上端より上へ出ない
        let f = face();
        let c = ctx(&f, 20);
        let unit_h = crate::layout::shape_units(&c, "L")[0].height;
        let top = 20.0 + (80.0 - unit_h) / 2.0;
        assert!(y0 as f32 >= top - 1.0, "上へはみ出し {y0} < {top}");
        assert!(
            (y1 + 1) as f32 <= top + unit_h + 1.0,
            "下へはみ出し {y1} > {}",
            top + unit_h
        );
        // 2字を回転で並べると縦長になる
        let (a0, b0, a1, b1, _, _) = ink("AB", 80, 160);
        assert!(b1 - b0 > a1 - a0, "{a0} {b0} {a1} {b1}");
    }

    #[test]
    fn tate_chu_yoko_stays_in_one_em_and_is_wide() {
        let (x0, y0, x1, y1, _, _) = ink("12", 80, 80);
        // 1文字分の枠(40 四方)に収まる
        assert!(x1 - x0 < 40 && y1 - y0 < 40, "{x0} {y0} {x1} {y1}");
        // 横に並んでいる
        assert!(x1 - x0 > y1 - y0, "{x0} {y0} {x1} {y1}");
        // 列の中央に寄っている
        assert!(((x0 + x1) as f32 / 2.0 - 40.0).abs() < 6.0, "{x0} {x1}");
    }

    #[test]
    fn tate_chu_yoko_is_shrunk_to_one_em_and_centered() {
        let f = face();
        let c = ctx(&f, 20);
        // 縮小しないと 1 文字分を超える数字の組(scale_x が最小のもの)を選ぶ
        let (pair, scale) = (0..100)
            .map(|n| format!("{n:02}"))
            .map(|t| {
                let u = &crate::layout::shape_units(&c, &t)[0];
                let sc = match u.glyphs[0].transform {
                    GlyphTransform::Tcy { scale_x } => scale_x,
                    _ => 1.0,
                };
                (t, sc)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(scale < 1.0, "縮小される数字の組がテスト用フォントに無い");
        let (x0, y0, x1, y1, _, _) = ink(&pair, 80, 80);
        assert!(x1 - x0 < 40, "{pair} 幅 {}", x1 - x0 + 1);
        // 輪郭の外接矩形から期待する(縮小後の)インクの幅。縮小を無視すると約 10% 広がる
        let u = &crate::layout::shape_units(&c, &pair)[0];
        let tf = ttf_parser::Face::parse(f.data(), f.index()).unwrap();
        let s = 40.0 / f32::from(f.units_per_em);
        let b0 = tf
            .glyph_bounding_box(GlyphId(u.glyphs[0].glyph_id))
            .unwrap();
        let bl = tf
            .glyph_bounding_box(GlyphId(u.glyphs.last().unwrap().glyph_id))
            .unwrap();
        let left = u.glyphs[0].x + f32::from(b0.x_min) * s * scale;
        let right = u.glyphs.last().unwrap().x + f32::from(bl.x_max) * s * scale;
        let measured = (x1 - x0 + 1) as f32;
        assert!(
            (measured - (right - left)).abs() <= 1.2,
            "{pair} 測定 {measured} 期待 {}",
            right - left
        );
        // 縦の中心は 1 文字分の枠(上端 20、高さ 40)の中央付近
        let cy = (y0 + y1) as f32 / 2.0;
        assert!((cy - 40.0).abs() <= 8.0, "{pair} 縦の中心 {cy}");
    }
}
