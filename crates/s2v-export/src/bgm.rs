//! BGM の区間とクロスフェード(流用元 compute_bgm_segments の移植)。

use std::path::{Path, PathBuf};

use s2v_core::placement::PlacedEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct BgmSegment {
    pub path: PathBuf,
    pub mix_start_s: f64,
    pub mix_duration_s: f64,
    pub fade_in_s: f64,
    pub fade_out_s: f64,
}

/// `#bgm_start` から次の `#bgm_start` / `#bgm_stop` / 終端までを1区間とする。
/// 区間の境目は `crossfade_s` でクロスフェードする(区間長の 1/3 を上限)。
pub fn compute_bgm_segments(
    events: &[PlacedEvent],
    total_s: f64,
    crossfade_s: f64,
) -> Vec<BgmSegment> {
    let half = crossfade_s / 2.0;
    struct Raw {
        path: PathBuf,
        start: f64,
        end: f64,
        /// 次の `#bgm_start` に切り替わって終わった(`#bgm_stop` や終端ではない)
        switched: bool,
    }
    let mut raw: Vec<Raw> = Vec::new();
    let mut pending: Option<(f64, PathBuf)> = None;
    let mut ordered: Vec<&PlacedEvent> = events.iter().collect();
    // 安定ソート: 同じ時刻のイベントは入力順のまま処理する
    ordered.sort_by_key(|e| e.start());
    for e in ordered {
        match e {
            PlacedEvent::BgmStart { path, start } => {
                let t = *start as f64 / 48_000.0;
                if let Some((s, p)) = pending.take() {
                    raw.push(Raw {
                        path: p,
                        start: s,
                        end: t,
                        switched: true,
                    });
                }
                pending = Some((t, PathBuf::from(path)));
            }
            PlacedEvent::BgmStop { start } => {
                if let Some((s, p)) = pending.take() {
                    raw.push(Raw {
                        path: p,
                        start: s,
                        end: *start as f64 / 48_000.0,
                        switched: false,
                    });
                }
            }
            PlacedEvent::Se { .. } => {}
        }
    }
    if let Some((s, p)) = pending {
        let mut end = total_s;
        if end <= s {
            let file_dur = wav_duration_s(&p);
            end = s + if file_dur > 0.0 { file_dur } else { 30.0 };
        }
        raw.push(Raw {
            path: p,
            start: s,
            end,
            switched: false,
        });
    }

    // 前の区間が切り替えで終わったときだけ、境目をクロスフェードする
    let switched: Vec<bool> = raw.iter().map(|r| r.switched).collect();
    raw.into_iter()
        .enumerate()
        .map(|(k, seg)| {
            let dur = (seg.end - seg.start).max(0.0);
            let clamp = dur / 3.0;
            let fi_half = if k > 0 && switched[k - 1] {
                half.min(clamp)
            } else {
                0.0
            };
            let fo_half = if switched[k] { half.min(clamp) } else { 0.0 };
            let mix_start = (seg.start - fi_half).max(0.0);
            let mix_end = seg.end + fo_half;
            BgmSegment {
                path: seg.path,
                mix_start_s: mix_start,
                mix_duration_s: (mix_end - mix_start).max(0.0),
                fade_in_s: fi_half * 2.0,
                fade_out_s: fo_half * 2.0,
            }
        })
        .collect()
}

fn wav_duration_s(path: &Path) -> f64 {
    let Ok(reader) = hound::WavReader::open(path) else {
        return 0.0;
    };
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return 0.0;
    }
    reader.duration() as f64 / spec.sample_rate as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_segments_crossfade_and_last_runs_to_end() {
        let events = vec![
            PlacedEvent::BgmStart {
                path: "a.wav".into(),
                start: 0,
            },
            PlacedEvent::BgmStart {
                path: "b.wav".into(),
                start: 10 * 48_000,
            },
        ];
        let segs = compute_bgm_segments(&events, 20.0, 3.0);
        assert_eq!(segs.len(), 2);
        assert_eq!(
            (segs[0].mix_start_s, segs[0].fade_in_s, segs[0].fade_out_s),
            (0.0, 0.0, 3.0)
        );
        assert!((segs[0].mix_duration_s - 11.5).abs() < 1e-9);
        assert_eq!(
            (segs[1].mix_start_s, segs[1].fade_in_s, segs[1].fade_out_s),
            (8.5, 3.0, 0.0)
        );
        assert!((segs[1].mix_duration_s - 11.5).abs() < 1e-9);
    }

    fn st(p: &str, s: i64) -> PlacedEvent {
        PlacedEvent::BgmStart {
            path: p.into(),
            start: s,
        }
    }

    fn sp(s: i64) -> PlacedEvent {
        PlacedEvent::BgmStop { start: s }
    }

    const S: i64 = 48_000;

    #[test]
    fn stop_then_restart_has_no_fades_and_no_overlap() {
        let segs = compute_bgm_segments(
            &[st("a.wav", 0), sp(10 * S), st("b.wav", 11 * S)],
            20.0,
            3.0,
        );
        assert_eq!(segs.len(), 2);
        assert_eq!(
            (
                segs[0].mix_start_s,
                segs[0].mix_duration_s,
                segs[0].fade_in_s,
                segs[0].fade_out_s
            ),
            (0.0, 10.0, 0.0, 0.0)
        );
        assert_eq!(
            (
                segs[1].mix_start_s,
                segs[1].mix_duration_s,
                segs[1].fade_in_s,
                segs[1].fade_out_s
            ),
            (11.0, 9.0, 0.0, 0.0)
        );
    }

    #[test]
    fn start_past_timeline_end_falls_back_to_30s_for_unreadable_file() {
        let segs = compute_bgm_segments(&[st("none.wav", 30 * S)], 20.0, 3.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            (
                segs[0].mix_start_s,
                segs[0].mix_duration_s,
                segs[0].fade_in_s,
                segs[0].fade_out_s
            ),
            (30.0, 30.0, 0.0, 0.0)
        );
    }

    #[test]
    fn stop_without_start_and_double_stop() {
        assert!(compute_bgm_segments(&[sp(5 * S)], 20.0, 3.0).is_empty());
        let segs = compute_bgm_segments(&[st("a.wav", 0), sp(5 * S), sp(8 * S)], 20.0, 3.0);
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].mix_duration_s, segs[0].fade_out_s), (5.0, 0.0));
    }

    #[test]
    fn equal_timestamps_keep_input_order() {
        // stop -> start: A は stop で終わる(フェードなし)、B は 5s から終端まで
        let segs =
            compute_bgm_segments(&[st("a.wav", 0), sp(5 * S), st("b.wav", 5 * S)], 10.0, 3.0);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].mix_duration_s, segs[0].fade_out_s), (5.0, 0.0));
        assert_eq!(
            (
                segs[1].mix_start_s,
                segs[1].mix_duration_s,
                segs[1].fade_in_s
            ),
            (5.0, 5.0, 0.0)
        );
        // start -> stop: A は B に切り替わる(クロスフェード)、B は長さ 0
        let segs =
            compute_bgm_segments(&[st("a.wav", 0), st("b.wav", 5 * S), sp(5 * S)], 10.0, 3.0);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].mix_duration_s, segs[0].fade_out_s), (6.5, 3.0));
        assert_eq!((segs[1].mix_duration_s, segs[1].fade_in_s), (0.0, 0.0));
    }
}
