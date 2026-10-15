//! コマ・区間・重ね画像の一覧の計算(設計書 §4.2、§4.5)。プロセスは起動しない。

use std::collections::{BTreeMap, BTreeSet};

/// 時刻(ミリ秒)をコマ番号にする。`floor((t*fps + 500) / 1000)`(0.5 は切り上げ)。
pub fn frame(t_ms: i64, fps: u32) -> i64 {
    let v = t_ms as i128 * fps as i128 + 500;
    v.div_euclid(1000) as i64
}

/// 全体のコマ数。`ceil(duration_ms * fps / 1000)`。
pub fn total_frames(duration_ms: i64, fps: u32) -> i64 {
    let v = duration_ms as i128 * fps as i128;
    (v + 999).div_euclid(1000) as i64
}

/// 画面に表示される項目(ト書きまたは吹き出し)。
#[derive(Debug, Clone, PartialEq)]
pub struct ShownItem {
    pub id: String,
    pub is_note: bool,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 台本の並び順
    pub order: usize,
}

/// 重ね画像が同じになる区間。
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    /// カットの先頭からの相対コマ番号 r_k
    pub rel_start: i64,
    /// 表示中の項目の ID(重ねる順: ト書き → 吹き出し(表示開始の順、同時なら order))
    pub items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CutPlan {
    pub cut_id: String,
    pub start_frame: i64,
    pub frames: i64,
    pub intervals: Vec<Interval>,
}

/// 重ねる順の項目 ID(ト書き → 吹き出し。表示開始の順、同時なら order)。
pub fn stacking_order(items: &[ShownItem]) -> Vec<String> {
    let mut sorted: Vec<&ShownItem> = items.iter().collect();
    sorted.sort_by_key(|it| {
        (
            !it.is_note,
            if it.is_note { 0 } else { it.start_ms },
            it.order,
        )
    });
    sorted.into_iter().map(|it| it.id.clone()).collect()
}

/// 動画の素材の長さを出力の fps のコマ数にする(切り捨て)。time_base は "num/den"。
pub fn asset_frames(duration_ts: i64, time_base: &str, fps: u32) -> Option<i64> {
    let (num, den) = time_base.split_once('/')?;
    let (num, den): (i128, i128) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    if duration_ts <= 0 || num <= 0 || den <= 0 {
        return None;
    }
    let v = (duration_ts as i128 * num * fps as i128).div_euclid(den);
    (v > 0).then_some(v as i64)
}

/// カットのコマ n に表示する素材の中のコマ(最後のコマで止める、loop ならループする)。
pub fn asset_frame(n: i64, n0: i64, asset_frames: i64, looped: bool) -> i64 {
    if asset_frames <= 0 {
        return 0;
    }
    let d = (n - n0).max(0);
    if d < asset_frames {
        d
    } else if looped {
        d % asset_frames
    } else {
        asset_frames - 1
    }
}

/// cuts: (カット ID, start_ms, end_ms)(台本の並び、隙間なし)。コマ数 0 のカットは含めない。
pub fn plan_cuts(
    cuts: &[(String, i64, i64)],
    items: &[ShownItem],
    duration_ms: i64,
    fps: u32,
) -> Vec<CutPlan> {
    let total = total_frames(duration_ms, fps);
    // 重ねる順(ト書き → 吹き出し。表示開始の順、同時なら order)
    let order = stacking_order(items);
    let by_id: BTreeMap<&str, &ShownItem> = items.iter().map(|it| (it.id.as_str(), it)).collect();
    let spans: Vec<(i64, i64, &str)> = order
        .iter()
        .map(|id| {
            let it = by_id[id.as_str()];
            (
                frame(it.start_ms, fps),
                frame(it.end_ms, fps),
                it.id.as_str(),
            )
        })
        .collect();

    let mut plans = Vec::new();
    for (i, (id, start_ms, end_ms)) in cuts.iter().enumerate() {
        let start = frame(*start_ms, fps);
        let end = if i + 1 == cuts.len() {
            total
        } else {
            frame(*end_ms, fps)
        };
        let n = end - start;
        if n <= 0 {
            continue;
        }
        // 区間の境目: カットの始めと、内側にある項目の始め・終わり
        let mut bounds: BTreeSet<i64> = BTreeSet::new();
        bounds.insert(start);
        for &(s, e, _) in &spans {
            for f in [s, e] {
                if f > start && f < end {
                    bounds.insert(f);
                }
            }
        }
        let intervals = bounds
            .iter()
            .map(|&b| Interval {
                rel_start: b - start,
                items: spans
                    .iter()
                    .filter(|&&(s, e, _)| s <= b && b < e)
                    .map(|&(_, _, id)| id.to_string())
                    .collect(),
            })
            .collect();
        plans.push(CutPlan {
            cut_id: id.clone(),
            start_frame: start,
            frames: n,
            intervals,
        });
    }
    plans
}

/// 四捨五入(0.5 は切り上げ)した `num / den`。num は非負、den は正。
fn round_half_up(num: i128, den: i128) -> i128 {
    (2 * num + den).div_euclid(2 * den)
}

fn micros(u: i128) -> String {
    format!("{}.{:06}", u / 1_000_000, u % 1_000_000)
}

/// 重ね画像の一覧(ffconcat)。intervals: (r_k, PNG のファイル名)、n: カットのコマ数。
pub fn overlay_list(intervals: &[(i64, String)], n: i64, fps: u32) -> String {
    let fps = fps as i128;
    let mut out = String::from("ffconcat version 1.0\n");
    let m = intervals.len();
    // u_0 = 0、u_k = (r_k − 0.5)/fps、u_m = n/fps(マイクロ秒)
    let u = |k: usize| -> i128 {
        if k == 0 {
            0
        } else if k == m {
            round_half_up(n as i128 * 1_000_000, fps)
        } else {
            round_half_up((2 * intervals[k].0 as i128 - 1) * 1_000_000, 2 * fps)
        }
    };
    for (k, (_, name)) in intervals.iter().enumerate() {
        out.push_str(&format!(
            "file '{name}'\noption framerate {}\nduration {}\n",
            2 * fps,
            micros(u(k + 1) - u(k))
        ));
    }
    if let Some((_, last)) = intervals.last() {
        out.push_str(&format!("file '{last}'\noption framerate {}\n", 2 * fps));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_frame_freezes_or_loops() {
        // 素材 60 コマ、カットの先頭 n0 = 100
        assert_eq!(asset_frame(100, 100, 60, false), 0);
        assert_eq!(asset_frame(159, 100, 60, false), 59);
        assert_eq!(asset_frame(160, 100, 60, false), 59); // 最後のコマで止める
        assert_eq!(asset_frame(300, 100, 60, false), 59);
        assert_eq!(asset_frame(160, 100, 60, true), 0); // ループ
        assert_eq!(asset_frame(225, 100, 60, true), 5);
        assert_eq!(asset_frame(105, 100, 0, false), 0);
    }

    #[test]
    fn asset_frames_from_probe_values() {
        assert_eq!(asset_frames(180_000, "1/90000", 30), Some(60)); // 2 秒
        assert_eq!(asset_frames(1001, "1/30000", 30), Some(1)); // 0.0333… 秒
        assert_eq!(asset_frames(0, "1/90000", 30), None);
        assert_eq!(asset_frames(100, "x", 30), None);
        assert_eq!(asset_frames(100, "1/0", 30), None);
    }

    #[test]
    fn stacking_order_matches_plan_cuts() {
        let items = vec![
            ShownItem {
                id: "L2".into(),
                is_note: false,
                start_ms: 500,
                end_ms: 900,
                order: 1,
            },
            ShownItem {
                id: "N1".into(),
                is_note: true,
                start_ms: 0,
                end_ms: 900,
                order: 5,
            },
            ShownItem {
                id: "L1".into(),
                is_note: false,
                start_ms: 500,
                end_ms: 900,
                order: 0,
            },
            ShownItem {
                id: "L0".into(),
                is_note: false,
                start_ms: 100,
                end_ms: 900,
                order: 9,
            },
        ];
        assert_eq!(stacking_order(&items), ["N1", "L0", "L1", "L2"]);
        let plans = plan_cuts(&[("C".into(), 0, 1000)], &items, 1000, 30);
        // 全部が表示されている区間(最後の区間は全項目が終わった後で空になる)
        let all = plans[0].intervals.iter().find(|iv| iv.items.len() == 4);
        assert_eq!(all.unwrap().items, ["N1", "L0", "L1", "L2"]);
    }

    #[test]
    fn frame_rounding() {
        assert_eq!(frame(0, 30), 0);
        assert_eq!(frame(16, 30), 0); // 0.48 → 0
        assert_eq!(frame(17, 30), 1); // 0.51 → 1
        assert_eq!(frame(50, 30), 2); // 1.5 → 2(0.5 は切り上げ)
        assert_eq!(total_frames(1000, 30), 30);
        assert_eq!(total_frames(1001, 30), 31);
        assert_eq!(frame(86_400_000, 120), 10_368_000);
    }

    #[test]
    fn cuts_cover_all_frames_for_random_inputs() {
        // 境目の丸めが積み重ならないこと
        let mut seed = 12345u64;
        for _ in 0..200 {
            let mut cuts = Vec::new();
            let mut t = 0;
            let n = 1 + (seed % 30) as usize;
            for i in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let len = (seed >> 33) % 5000;
                cuts.push((format!("S01-C{:02}", i + 1), t, t + len as i64));
                t += len as i64;
            }
            let dur = t;
            let plans = plan_cuts(&cuts, &[], dur, 30);
            let total: i64 = plans.iter().map(|p| p.frames).sum();
            assert_eq!(total, total_frames(dur, 30));
            for w in plans.windows(2) {
                assert_eq!(w[0].start_frame + w[0].frames, w[1].start_frame);
            }
        }
    }

    #[test]
    fn intervals_split_at_item_boundaries_inside_the_cut() {
        let cuts = vec![
            ("S01-C01".to_string(), 0, 2000),
            ("S01-C02".to_string(), 2000, 4000),
        ];
        let items = vec![
            ShownItem {
                id: "S01-N01".into(),
                is_note: true,
                start_ms: 0,
                end_ms: 4000,
                order: 0,
            },
            ShownItem {
                id: "S01-L001".into(),
                is_note: false,
                start_ms: 500,
                end_ms: 2500,
                order: 1,
            },
        ];
        let p = plan_cuts(&cuts, &items, 4000, 30);
        assert_eq!(p[0].start_frame, 0);
        assert_eq!(p[0].frames, 60);
        assert_eq!(
            p[0].intervals,
            vec![
                Interval {
                    rel_start: 0,
                    items: vec!["S01-N01".into()]
                },
                Interval {
                    rel_start: 15,
                    items: vec!["S01-N01".into(), "S01-L001".into()]
                },
            ]
        );
        assert_eq!(
            p[1].intervals,
            vec![
                Interval {
                    rel_start: 0,
                    items: vec!["S01-N01".into(), "S01-L001".into()]
                },
                Interval {
                    rel_start: 15,
                    items: vec!["S01-N01".into()]
                },
            ]
        );
    }

    #[test]
    fn zero_frame_cuts_are_skipped() {
        let cuts = vec![
            ("S01-C01".to_string(), 0, 10),
            ("S01-C02".to_string(), 10, 1000),
        ];
        let p = plan_cuts(&cuts, &[], 1000, 30);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].cut_id, "S01-C02");
    }

    #[test]
    fn overlay_list_switches_half_a_frame_early_and_repeats_the_last() {
        let s = overlay_list(
            &[(0, "ov_0001.png".into()), (15, "ov_empty.png".into())],
            60,
            30,
        );
        assert_eq!(
            s,
            "ffconcat version 1.0\n\
file 'ov_0001.png'\noption framerate 60\nduration 0.483333\n\
file 'ov_empty.png'\noption framerate 60\nduration 1.516667\n\
file 'ov_empty.png'\noption framerate 60\n"
        );
        // 1つだけの区間
        let s = overlay_list(&[(0, "ov_empty.png".into())], 30, 30);
        assert!(s.contains("duration 1.000000\n"), "{s}");
    }

    #[test]
    fn overlay_list_durations_sum_to_cut_length() {
        let iv: Vec<(i64, String)> = (0..50).map(|k| (k * 7, format!("ov_{k:04}.png"))).collect();
        let s = overlay_list(&iv, 400, 30);
        let total: i64 = s
            .lines()
            .filter_map(|l| l.strip_prefix("duration "))
            .map(|d| {
                let (a, b) = d.split_once('.').unwrap();
                a.parse::<i64>().unwrap() * 1_000_000 + b.parse::<i64>().unwrap()
            })
            .sum();
        assert_eq!(total, 13_333_333); // round(400/30 秒)
    }
}
