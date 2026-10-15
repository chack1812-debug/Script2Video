//! `bubbles.json`(吹き出し・ト書きの手動配置)の型と検証。

use crate::ItemKind;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubblesFile {
    pub version: u32,
    pub canvas: Canvas,
    pub items: BTreeMap<String, Placed>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    pub kind: ItemKind,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub tail: Option<Tail>,
    #[serde(default)]
    pub start_offset_ms: i64,
    #[serde(default)]
    pub end_offset_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tail {
    pub dx: i32,
    pub dy: i32,
}

pub const MAX_ITEMS: usize = 10_000;
pub const MAX_OFFSET_MS: i64 = 60_000;

/// ID が `S<数字>-<種別><数字>` の形式なら種別の文字を返す。
fn id_kind(id: &str) -> Option<char> {
    let rest = id.strip_prefix('S')?;
    let (a, b) = rest.split_once('-')?;
    let mut it = b.chars();
    let k = it.next()?;
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit());
    if digits(a) && (k == 'L' || k == 'N') && digits(it.as_str()) {
        Some(k)
    } else {
        None
    }
}

fn canvas_ok(c: Canvas) -> bool {
    (16..=7680).contains(&c.w)
        && c.w.is_multiple_of(2)
        && (16..=4320).contains(&c.h)
        && c.h.is_multiple_of(2)
}

/// 読み込みと検証(§5.2)。Err は理由の一覧(エラー 2)
pub fn parse_bubbles(json: &str) -> Result<BubblesFile, Vec<String>> {
    let file: BubblesFile = serde_json::from_str(json).map_err(|e| vec![e.to_string()])?;
    let errs = validate_file(&file);
    if errs.is_empty() {
        Ok(file)
    } else {
        Err(errs)
    }
}

/// 型への変換の後の検証(§5.2)。誤りの一覧(空なら正しい)。`bubbles` の保存でも同じ検証を使う。
pub fn validate_file(file: &BubblesFile) -> Vec<String> {
    let mut errs = Vec::new();
    if file.version != 1 {
        errs.push(format!(
            "version は 1 でなければなりません: {}",
            file.version
        ));
    }
    let c = file.canvas;
    if !canvas_ok(c) {
        errs.push(format!(
            "canvas が範囲外です(w は 16〜7680、h は 16〜4320 の偶数): {}x{}",
            c.w, c.h
        ));
    }
    if file.items.len() > MAX_ITEMS {
        errs.push(format!(
            "items が上限 {MAX_ITEMS} 件を超えています: {} 件",
            file.items.len()
        ));
    }
    for (id, p) in &file.items {
        let expect = match p.kind {
            ItemKind::Bubble => 'L',
            ItemKind::Note => 'N',
        };
        match id_kind(id) {
            None => errs.push(format!("{id}: ID の形式が正しくありません")),
            Some(k) if k != expect => errs.push(format!("{id}: ID の種別と kind が一致しません")),
            _ => {}
        }
        if p.kind == ItemKind::Note && p.tail.is_some() {
            errs.push(format!("{id}: note に tail は指定できません"));
        }
        if p.w == 0 || p.h == 0 {
            errs.push(format!("{id}: w と h は 1 以上でなければなりません"));
        }
        let (cw, ch) = (i64::from(c.w), i64::from(c.h));
        let (x, y) = (i64::from(p.x), i64::from(p.y));
        if x < 0 || y < 0 || x + i64::from(p.w) > cw || y + i64::from(p.h) > ch {
            errs.push(format!("{id}: 枠が画面の外にはみ出しています"));
        }
        if let Some(t) = p.tail {
            let tx = x + i64::from(p.w) / 2 + i64::from(t.dx);
            let ty = y + i64::from(p.h) / 2 + i64::from(t.dy);
            if tx < 0 || ty < 0 || tx > cw || ty > ch {
                errs.push(format!("{id}: 尻尾の先端が画面の外にはみ出しています"));
            }
        }
        if p.start_offset_ms.unsigned_abs() > MAX_OFFSET_MS as u64
            || p.end_offset_ms.unsigned_abs() > MAX_OFFSET_MS as u64
        {
            errs.push(format!(
                "{id}: offset の絶対値が {MAX_OFFSET_MS} ms を超えています"
            ));
        }
    }
    errs
}

/// `v * new / old` を四捨五入する(負の値は符号を分けて同様)。
fn scale_val(v: i64, new: u32, old: u32) -> i64 {
    let (n, o) = (i64::from(new), i64::from(old));
    let r = (v.abs() * n + o / 2) / o;
    if v < 0 {
        -r
    } else {
        r
    }
}

/// 枠の両端をそれぞれ比で四捨五入して (位置, 長さ) を拡大縮小する。
/// 元が内側なら結果も内側。長さが 0 になれば 1 にし、はみ出すなら位置を 1 戻す。
fn scale_span(pos: i32, len: u32, new: u32, old: u32) -> (i32, u32) {
    let a = scale_val(i64::from(pos), new, old);
    let b = scale_val(i64::from(pos) + i64::from(len), new, old);
    let mut l = (b - a).max(1);
    let mut a = a;
    if a + l > i64::from(new) {
        a -= 1;
    }
    l = l.min(i64::from(new));
    (a.max(0) as i32, l as u32)
}

/// 画面の大きさ (w, h) に合わせる。縦横比が違えば Err(エラー 2)
pub fn scale_to(file: &BubblesFile, w: u32, h: u32) -> Result<BubblesFile, String> {
    let c = file.canvas;
    if !canvas_ok(Canvas { w, h }) {
        return Err(format!(
            "画面 {w}x{h} が範囲外です(w は 16〜7680、h は 16〜4320 の偶数)"
        ));
    }
    if u64::from(c.w) * u64::from(h) != u64::from(w) * u64::from(c.h) {
        return Err(format!(
            "bubbles.json の canvas {}x{} と画面 {w}x{h} の縦横比が違います",
            c.w, c.h
        ));
    }
    let items = file
        .items
        .iter()
        .map(|(id, p)| {
            let (nx, nw) = scale_span(p.x, p.w, w, c.w);
            let (ny, nh) = scale_span(p.y, p.h, h, c.h);
            let tail = p.tail.map(|t| Tail {
                dx: scale_val(i64::from(t.dx), w, c.w) as i32,
                dy: scale_val(i64::from(t.dy), h, c.h) as i32,
            });
            (
                id.clone(),
                Placed {
                    x: nx,
                    y: ny,
                    w: nw,
                    h: nh,
                    tail,
                    ..p.clone()
                },
            )
        })
        .collect();
    Ok(BubblesFile {
        version: file.version,
        canvas: Canvas { w, h },
        items,
    })
}

/// 表示時刻(元の時刻 + offset を [0, duration] に収める)。開始 >= 終了なら Err
pub fn effective_times(
    start_ms: i64,
    end_ms: i64,
    p: Option<&Placed>,
    duration_ms: i64,
) -> Result<(i64, i64), String> {
    let (so, eo) = p.map_or((0, 0), |p| (p.start_offset_ms, p.end_offset_ms));
    let s = start_ms.saturating_add(so).clamp(0, duration_ms);
    let e = end_ms.saturating_add(eo).clamp(0, duration_ms);
    if s >= e {
        return Err(format!("表示の開始 {s} ms が終了 {e} ms 以降になります"));
    }
    Ok((s, e))
}

/// 本文が変わって収まらない手動の大きさを広げる: 幅と高さを max(保存値, min) にし、画面の内側に寄せる
pub fn expand_to_min(p: &Placed, min: (u32, u32), canvas: Canvas) -> Placed {
    let w = p.w.max(min.0);
    let h = p.h.max(min.1);
    let max_x = (i64::from(canvas.w) - i64::from(w)).max(0);
    let max_y = (i64::from(canvas.h) - i64::from(h)).max(0);
    Placed {
        x: i64::from(p.x).clamp(0, max_x) as i32,
        y: i64::from(p.y).clamp(0, max_y) as i32,
        w,
        h,
        ..p.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_file_reports_the_same_errors_as_parse() {
        let ok = r#"{"version":1,"canvas":{"w":1920,"h":1080},"items":{}}"#;
        let f = parse_bubbles(ok).unwrap();
        assert!(validate_file(&f).is_empty());
        let mut bad = f.clone();
        bad.items.insert(
            "S01-L001".into(),
            Placed {
                kind: ItemKind::Note,
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        bad.items.insert(
            "S01-L002".into(),
            Placed {
                kind: ItemKind::Bubble,
                x: 1915,
                y: 0,
                w: 10,
                h: 10,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        let errs = validate_file(&bad);
        assert!(errs.iter().any(|e| e.contains("S01-L001")), "{errs:?}"); // 種類の不一致
        assert!(errs.iter().any(|e| e.contains("S01-L002")), "{errs:?}"); // 画面の外
    }

    const OK: &str = r#"{"version":1,"canvas":{"w":1920,"h":1080},"items":{
        "S01-L003":{"kind":"bubble","x":1200,"y":80,"w":220,"h":420,"tail":{"dx":0,"dy":290},"start_offset_ms":0,"end_offset_ms":0},
        "S01-N01":{"kind":"note","x":1700,"y":40,"w":160,"h":380}}}"#;

    #[test]
    fn valid_file_parses_with_defaults() {
        let f = parse_bubbles(OK).unwrap();
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items["S01-N01"].start_offset_ms, 0);
        assert_eq!(f.items["S01-N01"].tail, None);
    }

    #[test]
    fn unknown_key_and_broken_json_are_errors() {
        // Review Focus 4
        let e = parse_bubbles(&OK.replace("\"w\":220", "\"w\":220,\"color\":\"red\"")).unwrap_err();
        assert!(e[0].contains("color"), "{e:?}");
        assert!(!parse_bubbles("{").unwrap_err().is_empty());
        assert!(
            !parse_bubbles(&OK.replace("\"version\":1", "\"version\":2"))
                .unwrap_err()
                .is_empty()
        );
    }

    #[test]
    fn each_limit_is_checked() {
        let cases = [
            OK.replace("\"x\":1200", "\"x\":1800"), // 枠が右にはみ出す
            OK.replace("\"y\":80", "\"y\":-1"),
            OK.replace("\"dy\":290", "\"dy\":900"), // 尻尾の先端が下にはみ出す
            OK.replace("\"start_offset_ms\":0", "\"start_offset_ms\":60001"),
            OK.replace("\"kind\":\"note\"", "\"kind\":\"bubble\""), // N の ID に bubble
            OK.replace(
                "\"S01-N01\":{\"kind\":\"note\",",
                "\"S01-N01\":{\"kind\":\"note\",\"tail\":{\"dx\":0,\"dy\":1},",
            ),
            OK.replace("\"w\":220", "\"w\":0"),
            OK.replace("\"w\":1920", "\"w\":1921"),
            OK.replace("\"S01-L003\"", "\"X01\""),
        ];
        for c in cases {
            assert!(parse_bubbles(&c).is_err(), "{c}");
        }
    }

    #[test]
    fn too_many_items_is_an_error() {
        let items: Vec<String> = (1..=10_001)
            .map(|i| {
                format!("\"S01-L{i:05}\":{{\"kind\":\"bubble\",\"x\":0,\"y\":0,\"w\":10,\"h\":10}}")
            })
            .collect();
        let json = format!(
            "{{\"version\":1,\"canvas\":{{\"w\":1920,\"h\":1080}},\"items\":{{{}}}}}",
            items.join(",")
        );
        let e = parse_bubbles(&json).unwrap_err();
        assert!(e.iter().any(|m| m.contains("10000")), "{e:?}");
    }

    #[test]
    fn scaling_keeps_aspect_and_rejects_other_ratios() {
        let f = parse_bubbles(OK).unwrap();
        let s = scale_to(&f, 1280, 720).unwrap();
        let b = &s.items["S01-L003"];
        assert_eq!((b.x, b.y, b.w, b.h), (800, 53, 147, 280)); // 2/3 倍で四捨五入
        assert_eq!(b.tail, Some(Tail { dx: 0, dy: 193 }));
        assert_eq!(s.canvas, Canvas { w: 1280, h: 720 });
        assert!(scale_to(&f, 1080, 1080).is_err());
    }

    #[test]
    fn effective_times_clamp_and_reject_inversion() {
        let p = parse_bubbles(OK).unwrap().items["S01-L003"].clone();
        assert_eq!(
            effective_times(
                1000,
                2000,
                Some(&Placed {
                    start_offset_ms: -2000,
                    end_offset_ms: 500,
                    ..p.clone()
                }),
                2300
            ),
            Ok((0, 2300))
        );
        assert!(effective_times(
            1000,
            2000,
            Some(&Placed {
                start_offset_ms: 1500,
                ..p.clone()
            }),
            5000
        )
        .is_err());
        assert_eq!(effective_times(1000, 2000, None, 5000), Ok((1000, 2000)));
    }

    #[test]
    fn expand_to_min_grows_and_moves_inside() {
        let p = Placed {
            kind: ItemKind::Bubble,
            x: 1800,
            y: 900,
            w: 100,
            h: 100,
            tail: None,
            start_offset_ms: 0,
            end_offset_ms: 0,
        };
        let q = expand_to_min(&p, (200, 300), Canvas { w: 1920, h: 1080 });
        assert_eq!((q.w, q.h), (200, 300));
        assert_eq!((q.x, q.y), (1720, 780)); // 右下へ広げ、はみ出す分を内側へ寄せる
        let r = expand_to_min(&p, (50, 50), Canvas { w: 1920, h: 1080 });
        assert_eq!((r.w, r.h, r.x, r.y), (100, 100, 1800, 900));
    }

    #[test]
    fn extreme_offsets_do_not_panic() {
        let e = parse_bubbles(&OK.replace(
            "\"start_offset_ms\":0",
            "\"start_offset_ms\":-9223372036854775808",
        ))
        .unwrap_err();
        assert!(!e.is_empty());
        let p = Placed {
            start_offset_ms: 1,
            ..parse_bubbles(OK).unwrap().items["S01-L003"].clone()
        };
        let _ = effective_times(i64::MAX, i64::MAX, Some(&p), 5000);
        let _ = effective_times(
            i64::MIN,
            i64::MIN,
            Some(&Placed {
                start_offset_ms: -1,
                ..p
            }),
            5000,
        );
    }

    #[test]
    fn scaling_keeps_edges_inside_and_rejects_bad_size() {
        let mut f = parse_bubbles(OK).unwrap();
        f.items.insert(
            "S01-L004".into(),
            Placed {
                kind: ItemKind::Bubble,
                x: 1,
                y: 0,
                w: 1919,
                h: 10,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        );
        let s = scale_to(&f, 960, 540).unwrap();
        let b = &s.items["S01-L004"];
        assert_eq!((b.x, b.w), (1, 959));
        assert!(scale_to(&f, 0, 0).is_err());
        assert!(scale_to(&f, 1921, 1081).is_err());
    }
}
