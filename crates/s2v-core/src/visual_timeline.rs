//! カットとト書きのミリ秒の区間(映像工程設計書 §3.4)。
//! 境目は開始も終了も floor_ms で変換し、カットを隙間なく並べる(最後だけ duration_ms)。

use crate::ids::{format_cut_id, format_note_id, format_scene_id};
use crate::placement::{PlacedScene, PlacedVisual};
use crate::time::floor_ms;

#[derive(Debug, Clone, PartialEq)]
pub struct CutSpan {
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub visual: Option<PlacedVisual>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteSpan {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneSpans {
    pub cuts: Vec<CutSpan>,
    pub notes: Vec<NoteSpan>,
}

/// シーンごとのカット・ト書きの区間と、警告(長さ 0 のシーン・カット・ト書き)。
pub fn layout_visuals(scenes: &[PlacedScene], duration_ms: i64) -> (Vec<SceneSpans>, Vec<String>) {
    let mut out = Vec::with_capacity(scenes.len());
    let mut warnings = Vec::new();
    let starts: Vec<i64> = scenes
        .iter()
        .enumerate()
        .map(|(i, s)| if i == 0 { 0 } else { floor_ms(s.start) })
        .collect();
    for (i, s) in scenes.iter().enumerate() {
        let vs = starts[i];
        let ve = starts.get(i + 1).copied().unwrap_or(duration_ms).max(vs);
        if vs == ve {
            warnings.push(format!(
                "{} は、次のシーンと同じ時刻に始まるので映像に現れません",
                format_scene_id(s.id)
            ));
        }
        let clamp = |samples: i64| floor_ms(samples).clamp(vs, ve);
        let cut_starts: Vec<i64> = s.cuts.iter().map(|c| c.at.map_or(vs, clamp)).collect();
        let mut cuts = Vec::with_capacity(s.cuts.len());
        for (k, c) in s.cuts.iter().enumerate() {
            let start = cut_starts[k];
            let end = cut_starts.get(k + 1).copied().unwrap_or(ve);
            let id = format_cut_id(s.id, c.number);
            // 映像のない C01 が空になるのは普通(#paragraph の直後の #cut)なので警告しない
            if start == end && !(c.number == 1 && c.visual.is_none()) && vs != ve {
                warnings.push(format!("{id} は長さが 0 なので映像に現れません"));
            }
            cuts.push(CutSpan {
                id,
                start_ms: start,
                end_ms: end,
                visual: c.visual.clone(),
            });
        }
        let note_starts: Vec<i64> = s.notes.iter().map(|n| clamp(n.at)).collect();
        let mut notes = Vec::with_capacity(s.notes.len());
        for (k, n) in s.notes.iter().enumerate() {
            let start = note_starts[k];
            let mut end = ve;
            if let Some(&next) = note_starts.get(k + 1) {
                end = end.min(next);
            }
            if let Some(&next_cut) = cut_starts.get(n.after_cut + 1) {
                end = end.min(next_cut);
            }
            let end = end.max(start);
            let id = format_note_id(s.id, n.number);
            if start == end && vs != ve {
                warnings.push(format!("{id} は長さが 0 なので表示されません"));
            }
            notes.push(NoteSpan {
                id,
                text: n.text.clone(),
                start_ms: start,
                end_ms: end,
            });
        }
        out.push(SceneSpans { cuts, notes });
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{PlacedCut, PlacedNote, PlacedScene};

    fn s48(ms: i64) -> i64 {
        ms * 48
    }

    fn scene(id: u32, start_ms: i64, cuts_at: &[i64], notes: &[(i64, usize)]) -> PlacedScene {
        let mut cuts = vec![PlacedCut {
            number: 1,
            at: None,
            visual: None,
        }];
        for (k, &a) in cuts_at.iter().enumerate() {
            cuts.push(PlacedCut {
                number: k as u32 + 2,
                at: Some(s48(a)),
                visual: None,
            });
        }
        PlacedScene {
            id,
            name: None,
            index: 0,
            silent: false,
            start: s48(start_ms),
            end: s48(start_ms),
            cuts,
            notes: notes
                .iter()
                .enumerate()
                .map(|(k, &(a, after))| PlacedNote {
                    number: k as u32 + 1,
                    at: s48(a),
                    text: format!("n{k}"),
                    after_cut: after,
                })
                .collect(),
        }
    }

    type Triples = Vec<(String, i64, i64)>;

    fn spans(s: &SceneSpans) -> (Triples, Triples) {
        (
            s.cuts
                .iter()
                .map(|c| (c.id.clone(), c.start_ms, c.end_ms))
                .collect(),
            s.notes
                .iter()
                .map(|n| (n.id.clone(), n.start_ms, n.end_ms))
                .collect(),
        )
    }

    #[test]
    fn video_ranges_tile_from_zero_to_duration() {
        let sc = vec![
            scene(1, 500, &[3000], &[(500, 0), (3000, 1)]),
            scene(2, 6000, &[], &[]),
        ];
        let (v, w) = layout_visuals(&sc, 9000);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            spans(&v[0]).0,
            vec![("S01-C01".into(), 0, 3000), ("S01-C02".into(), 3000, 6000)]
        );
        // N01 は次のカット(3000)で、N02 はシーンの終わり(6000)で終わる
        assert_eq!(
            spans(&v[0]).1,
            vec![
                ("S01-N01".into(), 500, 3000),
                ("S01-N02".into(), 3000, 6000)
            ]
        );
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 6000, 9000)]);
    }

    #[test]
    fn cut_before_first_line_makes_c01_empty_without_warning() {
        // #paragraph の直後の #cut: 現在位置は前のシーンの終わり(シーンの開始より前)→ シーンの始めに収める
        let sc = vec![scene(1, 0, &[], &[]), scene(2, 4000, &[3000], &[])];
        let (v, w) = layout_visuals(&sc, 8000);
        assert_eq!(
            spans(&v[1]).0,
            vec![
                ("S02-C01".into(), 4000, 4000),
                ("S02-C02".into(), 4000, 8000)
            ]
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn empty_c01_with_a_visual_warns() {
        let mut s2 = scene(2, 4000, &[3000], &[]);
        s2.cuts[0].visual = Some(crate::placement::PlacedVisual {
            text: "朝".into(),
            kind: None,
            looped: false,
        });
        let sc = vec![scene(1, 0, &[], &[]), s2];
        let (_, w) = layout_visuals(&sc, 8000);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("S02-C01"), "{w:?}");
    }

    #[test]
    fn same_position_cuts_and_notes_warn() {
        // #cut #cut #note #note(どちらのト書きも C03 の後)
        let sc = vec![scene(1, 0, &[1000, 1000], &[(1000, 2), (1000, 2)])];
        let (v, w) = layout_visuals(&sc, 3000);
        assert_eq!(spans(&v[0]).0[1], ("S01-C02".into(), 1000, 1000));
        assert_eq!(spans(&v[0]).1[0], ("S01-N01".into(), 1000, 1000));
        assert_eq!(spans(&v[0]).1[1], ("S01-N02".into(), 1000, 3000));
        assert_eq!(w.len(), 2, "{w:?}"); // C02 と N01 が長さ 0
    }

    #[test]
    fn note_after_last_line_without_pause_is_empty() {
        let sc = vec![scene(1, 0, &[], &[(3000, 0)])];
        let (v, w) = layout_visuals(&sc, 3000);
        assert_eq!(spans(&v[0]).1[0], ("S01-N01".into(), 3000, 3000));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn scenes_starting_at_the_same_time_warn() {
        let sc = vec![
            scene(1, 0, &[], &[]),
            scene(2, 2000, &[], &[]),
            scene(3, 2000, &[], &[]),
        ];
        let (v, w) = layout_visuals(&sc, 5000);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 2000, 2000)]);
        assert!(w.iter().any(|m| m.contains("S02")), "{w:?}");
    }

    #[test]
    fn sub_millisecond_starts_are_floored_consistently() {
        // シーンの開始 1000.5ms → 1000ms(scene.start_ms と同じ floor)
        let mut s2 = scene(2, 0, &[], &[]);
        s2.start = 48_024;
        let sc = vec![scene(1, 0, &[], &[]), s2];
        let (v, _) = layout_visuals(&sc, 2000);
        assert_eq!(spans(&v[0]).0, vec![("S01-C01".into(), 0, 1000)]);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 1000, 2000)]);
    }
}
