//! ITU-R BS.1770-4 のラウドネス測定と、台詞の正規化(設計書 5.10 B)。48kHz 専用。

use s2v_core::NormalizeConfig;

use crate::filters::Biquad;

/// 400ms ブロック(48kHz)。
const BLOCK: usize = 19_200;
/// 100ms(オーバーラップ 75%)。
const STEP: usize = 4_800;
/// 絶対ゲート[LUFS]。
const ABS_GATE: f64 = -70.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Loudness {
    Lufs(f64),
    /// 測定できない(reason: unmeasurable)
    Unmeasurable,
}

/// K 特性フィルタ(48kHz の係数。状態はゼロから始める)。
fn k_weight(samples: &[f32]) -> Vec<f64> {
    let mut shelf = Biquad::new(
        [1.53512485958697, -2.69169618940638, 1.19839281085285],
        [-1.69065929318241, 0.73248077421585],
    );
    let mut rlb = Biquad::new([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]);
    samples
        .iter()
        .map(|&x| rlb.process(shelf.process(x as f64)))
        .collect()
}

fn lufs_of(mean_square: f64) -> f64 {
    -0.691 + 10.0 * mean_square.log10()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// モノラルの台詞を測る。400ms 未満はブロックなし(ゲートなし)、400ms 以上はゲート付き。
pub fn measure_mono(samples: &[f32]) -> Loudness {
    let sq: Vec<f64> = k_weight(samples).iter().map(|v| v * v).collect();
    gated_or_short(&sq)
}

/// ステレオ(左右チャンネルの加算)を測る。基準ゲインの較正に使う。
pub fn measure_stereo(frames: &[[f32; 2]]) -> Loudness {
    let l: Vec<f32> = frames.iter().map(|f| f[0]).collect();
    let r: Vec<f32> = frames.iter().map(|f| f[1]).collect();
    let (yl, yr) = (k_weight(&l), k_weight(&r));
    let sq: Vec<f64> = yl.iter().zip(&yr).map(|(a, b)| a * a + b * b).collect();
    gated_or_short(&sq)
}

fn gated_or_short(sq: &[f64]) -> Loudness {
    if sq.is_empty() {
        return Loudness::Unmeasurable;
    }
    if sq.len() < BLOCK {
        return finite(lufs_of(mean(sq)));
    }
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + BLOCK <= sq.len() {
        blocks.push(mean(&sq[start..start + BLOCK]));
        start += STEP;
    }
    let above_abs: Vec<f64> = blocks
        .into_iter()
        .filter(|&z| lufs_of(z) > ABS_GATE)
        .collect();
    if above_abs.is_empty() {
        return Loudness::Unmeasurable;
    }
    let rel_gate = lufs_of(mean(&above_abs)) - 10.0;
    let kept: Vec<f64> = above_abs
        .into_iter()
        .filter(|&z| lufs_of(z) > rel_gate)
        .collect();
    if kept.is_empty() {
        return Loudness::Unmeasurable;
    }
    finite(lufs_of(mean(&kept)))
}

fn finite(v: f64) -> Loudness {
    if v.is_finite() {
        Loudness::Lufs(v)
    } else {
        Loudness::Unmeasurable
    }
}

/// 正規化のゲイン。`applied_db` は制限後に実際に掛ける値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainDecision {
    pub requested_db: f64,
    pub applied_db: f64,
    pub clamped: bool,
}

/// `measured_lufs` は有限値であること。呼び出し側は `Unmeasurable` を先に弾くため、
/// NaN/inf は渡されない(NaN に `clamp` を使うと NaN のままになる)。
pub fn decide_gain(measured_lufs: f64, cfg: &NormalizeConfig) -> GainDecision {
    let requested = cfg.target_lufs - measured_lufs;
    let applied = requested.clamp(-cfg.max_cut_db, cfg.max_boost_db);
    GainDecision {
        requested_db: requested,
        applied_db: applied,
        clamped: applied != requested,
    }
}

/// 1つの固定ゲインを掛ける(圧縮や自動ゲイン制御はしない)。
pub fn apply_gain_db(samples: &mut [f32], db: f64) {
    let g = 10f64.powf(db / 20.0) as f32;
    for s in samples {
        *s *= g;
    }
}

/// 5ms(48kHz で 240 サンプル)。
pub const FADE_SAMPLES: usize = 240;

/// 両端に半コサインのフェードを掛ける。台詞が 10ms 未満なら長さの半分。
pub fn apply_edge_fades(samples: &mut [f32]) {
    let len = samples.len();
    let n = FADE_SAMPLES.min(len / 2);
    for i in 0..n {
        let g = (0.5 - 0.5 * (std::f64::consts::PI * i as f64 / n as f64).cos()) as f32;
        samples[i] *= g;
        samples[len - 1 - i] *= g;
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    fn lufs(l: Loudness) -> f64 {
        match l {
            Loudness::Lufs(v) => v,
            Loudness::Unmeasurable => panic!("測定できませんでした"),
        }
    }

    #[test]
    fn full_scale_997hz_sine_reads_minus_3_01() {
        // BS.1770 の基準: 997Hz・振幅1.0 の正弦波(1チャンネル)は -3.01 LUFS
        assert!((lufs(measure_mono(&sine(997.0, 1.0, 96_000))) + 3.01).abs() < 0.1);
        assert!((lufs(measure_mono(&sine(997.0, 0.1, 96_000))) + 23.01).abs() < 0.1);
    }

    #[test]
    fn short_line_is_measured_without_gating() {
        // 300ms(400ms 未満)
        assert!((lufs(measure_mono(&sine(997.0, 0.1, 14_400))) + 23.01).abs() < 0.3);
    }

    #[test]
    fn method_switches_at_400ms() {
        // -80 dBFS の音: ゲートなしなら測れる、ゲート付きなら絶対ゲート(-70)で測定不能
        let amp = 10f64.powf(-80.0 / 20.0) * 2f64.sqrt();
        assert!(matches!(
            measure_mono(&sine(997.0, amp, 19_199)),
            Loudness::Lufs(_)
        ));
        assert_eq!(
            measure_mono(&sine(997.0, amp, 19_200)),
            Loudness::Unmeasurable
        );
        assert_eq!(
            measure_mono(&sine(997.0, amp, 19_201)),
            Loudness::Unmeasurable
        );
    }

    #[test]
    fn silence_and_empty_are_unmeasurable() {
        assert_eq!(measure_mono(&[]), Loudness::Unmeasurable);
        assert_eq!(measure_mono(&vec![0.0; 1_000]), Loudness::Unmeasurable);
        assert_eq!(measure_mono(&vec![0.0; 96_000]), Loudness::Unmeasurable);
    }

    #[test]
    fn relative_gate_drops_quiet_parts() {
        let mut x = sine(997.0, 0.1, 96_000);
        x.extend(sine(997.0, 0.001, 96_000));
        assert!((lufs(measure_mono(&x)) + 23.0).abs() < 0.5);
    }

    #[test]
    fn stereo_sums_channels() {
        let m = sine(997.0, 0.1, 96_000);
        let st: Vec<[f32; 2]> = m.iter().map(|&v| [v, v]).collect();
        // 2チャンネル分のエネルギー: モノラルより +3.01 dB
        assert!((lufs(measure_stereo(&st)) - lufs(measure_mono(&m)) - 3.01).abs() < 0.05);
    }

    #[test]
    fn different_recording_levels_end_at_the_same_target() {
        let cfg = s2v_core::NormalizeConfig::default();
        for amp in [0.01, 0.1] {
            let mut x = sine(997.0, amp, 96_000);
            let g = decide_gain(lufs(measure_mono(&x)), &cfg);
            apply_gain_db(&mut x, g.applied_db);
            assert!((lufs(measure_mono(&x)) + 23.0).abs() < 0.05, "{amp}");
        }
    }

    #[test]
    fn late_short_voice_in_a_450ms_file_is_measurable() {
        // 単独ファイルで、最初の 400ms が無音・末尾 50ms だけ発声する 450ms の波形
        let mut x = vec![0.0f32; 19_200];
        x.extend(sine(997.0, 0.1, 2_400));
        let r = crate::detect::single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(r.measure.start, 19_200);
        assert!(matches!(
            measure_mono(&x[r.measure.start..r.measure.end]),
            Loudness::Lufs(_)
        ));
    }

    #[test]
    fn gain_is_clamped_and_recorded() {
        let cfg = s2v_core::NormalizeConfig::default();
        let g = decide_gain(-31.0, &cfg);
        assert_eq!(
            g,
            GainDecision {
                requested_db: 8.0,
                applied_db: 8.0,
                clamped: false
            }
        );
        let g = decide_gain(-55.0, &cfg);
        assert_eq!(
            g,
            GainDecision {
                requested_db: 32.0,
                applied_db: 24.0,
                clamped: true
            }
        );
        let g = decide_gain(5.0, &cfg);
        assert_eq!(
            g,
            GainDecision {
                requested_db: -28.0,
                applied_db: -24.0,
                clamped: true
            }
        );
    }

    #[test]
    fn gain_is_a_single_fixed_factor() {
        let mut x = vec![0.1f32, -0.5, 0.25];
        apply_gain_db(&mut x, 20.0);
        assert!(
            (x[0] - 1.0).abs() < 1e-6 && (x[1] + 5.0).abs() < 1e-5 && (x[2] - 2.5).abs() < 1e-5
        );
    }

    #[test]
    fn edge_fades_are_half_cosine_and_short_lines_use_half_length() {
        let mut long = vec![1.0f32; 2_000];
        apply_edge_fades(&mut long);
        assert_eq!(long[0], 0.0);
        assert_eq!(long[1_999], 0.0);
        assert!((long[120] - 0.5).abs() < 1e-3);
        assert_eq!(long[240], 1.0);
        assert_eq!(long[1_000], 1.0);

        let mut short = vec![1.0f32; 10];
        apply_edge_fades(&mut short);
        assert_eq!(short[0], 0.0);
        assert_eq!(short[9], 0.0);
        assert!(short[4] > 0.0 && short[4] < 1.0);
    }

    #[test]
    fn nan_input_is_unmeasurable() {
        assert_eq!(measure_mono(&[f32::NAN; 1_000]), Loudness::Unmeasurable);
        assert_eq!(
            measure_mono(&vec![f32::NAN; 96_000]),
            Loudness::Unmeasurable
        );
        assert_eq!(
            measure_stereo(&[[f32::NAN, 0.0]; 1_000]),
            Loudness::Unmeasurable
        );
    }

    #[test]
    fn fades_and_gain_do_not_panic_on_tiny_slices() {
        let mut e: [f32; 0] = [];
        apply_edge_fades(&mut e);
        apply_gain_db(&mut e, 6.0);
        let mut one = [1.0f32];
        apply_edge_fades(&mut one);
        apply_gain_db(&mut one, 6.0);
        assert!(one[0].is_finite());
    }
}
