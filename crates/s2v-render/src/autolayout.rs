//! 吹き出しとト書きの自動配置(設計書 §4.4)。

use crate::bubbles::{Canvas, Placed, Tail};
use crate::ItemKind;
use std::collections::BTreeMap;

/// 自動配置の入力項目。
#[derive(Debug, Clone)]
pub struct LayoutItem {
    pub id: String,
    pub kind: ItemKind,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 台本の並び
    pub order: usize,
    /// min_size の結果
    pub size: (u32, u32),
}

/// 自動配置の設定。
pub struct AutoParams {
    pub canvas: Canvas,
    pub margin: u32,
    pub bubble_gap: u32,
    pub tail_length: u32,
}

/// 時間の重なり(半開区間)
fn times_overlap(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// 障害物(配置済みの枠と表示時間)
struct Obstacle {
    rect: (i64, i64, i64, i64), // x, y, w, h
    times: (i64, i64),
}

/// `gap` だけ広げた枠どうしが重なるか
fn rects_collide(a: (i64, i64, i64, i64), b: (i64, i64, i64, i64), gap: i64) -> bool {
    a.0 - gap < b.0 + b.2 && b.0 - gap < a.0 + a.2 && a.1 - gap < b.1 + b.3 && b.1 - gap < a.1 + a.3
}

/// 画面より大きい項目は、余白を引いた大きさに縮める
fn fit(it: &LayoutItem, max_w: i64, max_h: i64, warnings: &mut Vec<String>) -> (i64, i64) {
    let (w, h) = (i64::from(it.size.0), i64::from(it.size.1));
    if w > max_w || h > max_h {
        warnings.push(format!("{} の文字が多く、画面に収まりません", it.id));
    }
    (w.min(max_w), h.min(max_h))
}

/// manual に無い項目の配置を決める。戻り値: (id → 配置, 警告)
pub fn auto_layout(
    items: &[LayoutItem],
    manual: &BTreeMap<String, Placed>,
    p: &AutoParams,
) -> (BTreeMap<String, Placed>, Vec<String>) {
    let cw = i64::from(p.canvas.w);
    let ch = i64::from(p.canvas.h);
    let margin = i64::from(p.margin);
    let gap = i64::from(p.bubble_gap);
    let max_w = (cw - 2 * margin).max(1);
    let max_h = (ch - 2 * margin).max(1);

    let mut out = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut obstacles: Vec<Obstacle> = Vec::new();

    // 手動の項目は障害物にだけ使う(戻り値には含めない)
    for it in items {
        if let Some(m) = manual.get(&it.id) {
            obstacles.push(Obstacle {
                rect: (
                    i64::from(m.x),
                    i64::from(m.y),
                    i64::from(m.w),
                    i64::from(m.h),
                ),
                times: (it.start_ms + m.start_offset_ms, it.end_ms + m.end_offset_ms),
            });
        }
    }

    // 画面より大きい項目は、余白を引いた大きさに縮める
    // ト書き: 右上、尻尾なし
    for it in items
        .iter()
        .filter(|i| i.kind == ItemKind::Note && !manual.contains_key(&i.id))
    {
        let (w, h) = fit(it, max_w, max_h, &mut warnings);
        let x = cw - margin - w;
        obstacles.push(Obstacle {
            rect: (x, margin, w, h),
            times: (it.start_ms, it.end_ms),
        });
        out.insert(
            it.id.clone(),
            Placed {
                kind: ItemKind::Note,
                x: x as i32,
                y: margin as i32,
                w: w as u32,
                h: h as u32,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
    }

    // 吹き出し: (start_ms, order) の順に、右から左へ空きを探す
    let mut bubbles: Vec<&LayoutItem> = items
        .iter()
        .filter(|i| i.kind == ItemKind::Bubble && !manual.contains_key(&i.id))
        .collect();
    bubbles.sort_by_key(|i| (i.start_ms, i.order));
    for it in bubbles {
        let (w, h) = fit(it, max_w, max_h, &mut warnings);
        let first = cw - margin - w;
        let mut candidates = Vec::new();
        let mut x = first;
        while x >= margin {
            candidates.push(x);
            x -= gap.max(1);
        }
        if candidates.last() != Some(&margin) {
            candidates.push(margin);
        }
        let times = (it.start_ms, it.end_ms);
        let free = candidates.iter().copied().find(|&x| {
            !obstacles.iter().any(|o| {
                times_overlap(times, o.times) && rects_collide((x, margin, w, h), o.rect, gap)
            })
        });
        let x = free.unwrap_or_else(|| {
            warnings.push(format!("{} を重ならずに置けませんでした", it.id));
            first
        });
        obstacles.push(Obstacle {
            rect: (x, margin, w, h),
            times,
        });
        // 尻尾: 下へ伸ばし、画面の下端を超えるなら縮める(0 未満なら無し)
        let center = margin + h / 2;
        let mut dy = h / 2 + i64::from(p.tail_length);
        if center + dy > ch - 1 {
            dy = ch - 1 - center;
        }
        let tail = (dy >= 0).then_some(Tail {
            dx: 0,
            dy: dy as i32,
        });
        out.insert(
            it.id.clone(),
            Placed {
                kind: ItemKind::Bubble,
                x: x as i32,
                y: margin as i32,
                w: w as u32,
                h: h as u32,
                tail,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn item(id: &str, kind: ItemKind, s: i64, e: i64, order: usize) -> LayoutItem {
        LayoutItem {
            id: id.into(),
            kind,
            start_ms: s,
            end_ms: e,
            order,
            size: (200, 300),
        }
    }
    fn params() -> AutoParams {
        AutoParams {
            canvas: Canvas { w: 1920, h: 1080 },
            margin: 40,
            bubble_gap: 24,
            tail_length: 80,
        }
    }

    #[test]
    fn note_goes_top_right_and_bubbles_go_left_of_it() {
        let items = vec![
            item("S01-N01", ItemKind::Note, 0, 5000, 0),
            item("S01-L001", ItemKind::Bubble, 0, 2000, 1),
            item("S01-L002", ItemKind::Bubble, 1000, 3000, 2),
        ];
        let (out, w) = auto_layout(&items, &BTreeMap::new(), &params());
        assert!(w.is_empty(), "{w:?}");
        let n = &out["S01-N01"];
        assert_eq!((n.x, n.y, n.tail), (1920 - 40 - 200, 40, None));
        let b1 = &out["S01-L001"];
        let b2 = &out["S01-L002"];
        assert!(b1.x + 200 + 24 <= n.x, "{b1:?} {n:?}");
        assert!(b2.x + 200 + 24 <= b1.x, "{b2:?} {b1:?}");
        assert_eq!(
            b1.tail,
            Some(Tail {
                dx: 0,
                dy: 150 + 80
            })
        );
    }

    #[test]
    fn non_overlapping_times_reuse_the_same_slot() {
        let items = vec![
            item("S01-L001", ItemKind::Bubble, 0, 1000, 0),
            item("S01-L002", ItemKind::Bubble, 1000, 2000, 1),
        ];
        let (out, _) = auto_layout(&items, &BTreeMap::new(), &params());
        assert_eq!(out["S01-L001"].x, out["S01-L002"].x);
    }

    #[test]
    fn manual_items_are_obstacles_and_not_returned() {
        let mut manual = BTreeMap::new();
        manual.insert(
            "S01-L001".to_string(),
            Placed {
                kind: ItemKind::Bubble,
                x: 1680,
                y: 40,
                w: 200,
                h: 300,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        let items = vec![
            item("S01-L001", ItemKind::Bubble, 0, 2000, 0),
            item("S01-L002", ItemKind::Bubble, 0, 2000, 1),
        ];
        let (out, _) = auto_layout(&items, &manual, &params());
        assert!(!out.contains_key("S01-L001"));
        assert!(out["S01-L002"].x + 200 + 24 <= 1680);
    }

    #[test]
    fn no_room_falls_back_to_the_right_with_warning() {
        let items: Vec<LayoutItem> = (0..10)
            .map(|i| item(&format!("S01-L{:03}", i + 1), ItemKind::Bubble, 0, 1000, i))
            .collect();
        let (out, w) = auto_layout(&items, &BTreeMap::new(), &params());
        assert_eq!(out.len(), 10);
        assert!(!w.is_empty());
        assert_eq!(out["S01-L010"].x, 1920 - 40 - 200);
    }

    #[test]
    fn oversized_item_is_clamped_with_warning() {
        // Review Focus 1
        let mut big = item("S01-L001", ItemKind::Bubble, 0, 1000, 0);
        big.size = (3000, 2000);
        let (out, w) = auto_layout(&[big], &BTreeMap::new(), &params());
        let b = &out["S01-L001"];
        assert!(
            b.x >= 0 && b.y >= 0 && b.x as u32 + b.w <= 1920 && b.y as u32 + b.h <= 1080,
            "{b:?}"
        );
        assert!(w.iter().any(|m| m.contains("S01-L001")), "{w:?}");
    }

    #[test]
    fn tail_is_shortened_at_the_bottom() {
        let mut tall = item("S01-L001", ItemKind::Bubble, 0, 1000, 0);
        tall.size = (200, 1000);
        let (out, _) = auto_layout(&[tall], &BTreeMap::new(), &params());
        let b = &out["S01-L001"];
        let tip_y = b.y + b.h as i32 / 2 + b.tail.map_or(0, |t| t.dy);
        assert!(tip_y <= 1079);
    }
}
