//! 10ms RMS 窓による音の範囲の検出と、単独ファイルの使用範囲(設計書 5.3、5.11)。

/// 10ms 窓(48kHz で 480 サンプル)。ホップも同じで重ならない。
pub const RMS_WINDOW: usize = 480;

/// サンプル位置の半開区間 [start, end)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub start: usize,
    pub end: usize,
}

impl Extent {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// しきい値を超えた最初の窓の開始から、最後の窓の終了まで。音が無ければ None。
pub fn sound_extent(samples: &[f32], threshold_db: f64) -> Option<Extent> {
    let mut first = None;
    let mut last_end = 0;
    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + RMS_WINDOW).min(samples.len());
        if window_db(&samples[pos..end]) > threshold_db {
            if first.is_none() {
                first = Some(pos);
            }
            last_end = end;
        }
        pos = end;
    }
    first.map(|start| Extent {
        start,
        end: last_end,
    })
}

/// 窓の RMS(dBFS)。全サンプル 0 なら -inf、NaN を含めば NaN(どのしきい値とも `>` が偽)。
fn window_db(w: &[f32]) -> f64 {
    let ms = w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / w.len() as f64;
    if ms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * ms.log10()
    }
}

/// 単独ファイルの範囲。`used` は音声として使う範囲(マージン込み)、`measure` はラウドネスの測定範囲(マージンを含めない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsedRange {
    pub used: Extent,
    pub measure: Extent,
}

/// 単独ファイルの使用範囲(設計書 5.0)。音が無ければ None(→ `silent`)。
pub fn single_used_range(
    samples: &[f32],
    threshold_db: f64,
    head_margin_ms: u32,
    tail_margin_ms: u32,
) -> Option<UsedRange> {
    let m = sound_extent(samples, threshold_db)?;
    let head = (head_margin_ms as usize).saturating_mul(48);
    let tail = (tail_margin_ms as usize).saturating_mul(48);
    Some(UsedRange {
        used: Extent {
            start: m.start.saturating_sub(head),
            end: m.end.saturating_add(tail).min(samples.len()),
        },
        measure: m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, from: usize, to: usize, amp: f32) -> Vec<f32> {
        (0..len)
            .map(|i| {
                if (from..to).contains(&i) {
                    amp * ((i as f32) * 0.3).sin()
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn silence_has_no_extent() {
        assert_eq!(sound_extent(&vec![0.0; 10_000], -50.0), None);
        assert_eq!(single_used_range(&vec![0.0; 10_000], -50.0, 30, 80), None);
    }

    #[test]
    fn empty_input_has_no_extent() {
        assert_eq!(sound_extent(&[], -50.0), None);
        assert_eq!(sound_extent(&[], f64::NEG_INFINITY), None);
        assert_eq!(single_used_range(&[], -50.0, 30, 80), None);
    }

    #[test]
    fn neg_infinity_threshold_still_ignores_digital_silence() {
        assert_eq!(sound_extent(&vec![0.0; 1_000], f64::NEG_INFINITY), None);
    }

    #[test]
    fn nan_window_does_not_count_as_sound() {
        assert_eq!(sound_extent(&vec![f32::NAN; 960], -50.0), None);
    }

    #[test]
    fn extent_snaps_to_windows_and_margins_are_added() {
        // 200ms の無音 + 1000ms の音 + 300ms の無音(48kHz)
        let x = tone(72_000, 9_600, 57_600, 0.1);
        assert_eq!(
            sound_extent(&x, -50.0),
            Some(Extent {
                start: 9_600,
                end: 57_600
            })
        );
        let r = single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(
            r.measure,
            Extent {
                start: 9_600,
                end: 57_600
            }
        );
        assert_eq!(
            r.used,
            Extent {
                start: 8_160,
                end: 61_440
            }
        );
    }

    #[test]
    fn margins_are_clamped_to_file_edges() {
        let x = tone(4_800, 0, 4_800, 0.1);
        let r = single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(
            r.used,
            Extent {
                start: 0,
                end: 4_800
            }
        );
    }

    #[test]
    fn huge_margins_do_not_overflow() {
        let x = tone(4_800, 0, 4_800, 0.1);
        let r = single_used_range(&x, -50.0, u32::MAX, u32::MAX).unwrap();
        assert_eq!(
            r.used,
            Extent {
                start: 0,
                end: 4_800
            }
        );
    }

    #[test]
    fn partial_last_window_is_evaluated_with_its_own_length() {
        // 1000 サンプル: 最後の窓は 40 サンプル(960..1000)だけで、そこにだけ音がある
        let x = tone(1_000, 960, 1_000, 0.5);
        assert_eq!(
            sound_extent(&x, -50.0),
            Some(Extent {
                start: 960,
                end: 1_000
            })
        );
    }

    #[test]
    fn quiet_minus_45_dbfs_voice_is_still_used() {
        // 設計書 §10 項目2e: 単独ファイルは take_gate_db(-40)ではなく trim_threshold_db(-50)で判定する
        let amp = 10f32.powf(-45.0 / 20.0) * 2f32.sqrt();
        let x = tone(48_000, 4_800, 43_200, amp);
        assert!(single_used_range(&x, -50.0, 30, 80).is_some());
    }

    #[test]
    fn single_sample_just_above_and_below_threshold() {
        // 480 サンプル窓に1サンプルだけ値 a: RMS = a / sqrt(480)
        let a_at_threshold = 10f64.powf(-50.0 / 20.0) * (480f64).sqrt();
        let mut above = vec![0.0f32; 960];
        above[500] = (a_at_threshold * 1.01) as f32;
        assert_eq!(
            sound_extent(&above, -50.0),
            Some(Extent {
                start: 480,
                end: 960
            })
        );
        let mut below = vec![0.0f32; 960];
        below[500] = (a_at_threshold * 0.99) as f32;
        assert_eq!(sound_extent(&below, -50.0), None);
    }
}
