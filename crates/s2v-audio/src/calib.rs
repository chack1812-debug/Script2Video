//! 基準ゲインの較正(設計書 5.10 C)。基準の台詞を残響なしで処理したとき、
//! マスター前のステレオのラウドネスが入力のラウドネスと一致するゲインを求める。

use std::collections::HashMap;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use s2v_core::{Cast, SceneConfig};

use crate::filters::Biquad;
use crate::loudness::{apply_gain_db, measure_mono, measure_stereo, Loudness};
use crate::processor::AudioProcessor;

/// 較正に使う信号の種(固定)。
pub const CALIBRATION_SEED: u64 = 0x5C21_7B1D;

/// 較正信号: 100Hz〜8kHz に帯域制限したピンクノイズ 5 秒、-23 LUFS。`seed` が同じなら同じ信号。
pub fn calibration_signal(seed: u64) -> Vec<f32> {
    let n = 5 * 48_000;
    let mut rng = SmallRng::seed_from_u64(seed);
    // Paul Kellet のピンクノイズフィルタ
    let mut b = [0.0f64; 7];
    let mut hp = Biquad::highpass(100.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut lp = Biquad::lowpass(8_000.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            let w: f64 = rng.gen_range(-1.0..1.0);
            b[0] = 0.99886 * b[0] + w * 0.0555179;
            b[1] = 0.99332 * b[1] + w * 0.0750759;
            b[2] = 0.96900 * b[2] + w * 0.1538520;
            b[3] = 0.86650 * b[3] + w * 0.3104856;
            b[4] = 0.55000 * b[4] + w * 0.5329522;
            b[5] = -0.7616 * b[5] - w * 0.0168980;
            let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
            b[6] = w * 0.115926;
            lp.process(hp.process(pink)) as f32
        })
        .collect();
    if let Loudness::Lufs(l) = measure_mono(&out) {
        apply_gain_db(&mut out, -23.0 - l);
    }
    out
}

/// 基準の台詞: distance が reference_dist、pan 0、height 既定、volume 1.0。
pub fn reference_cast(reference_dist: f64) -> Cast {
    Cast {
        name: "基準".to_string(),
        speaker_name: String::new(),
        pan: 0.0,
        distance: reference_dist,
        volume: 1.0,
        params: HashMap::new(),
        height: None,
        height_offset: 0.0,
        appearance: None,
    }
}

/// 残響なしの場面(早期反射は設定どおり)。
pub fn reference_scene() -> SceneConfig {
    SceneConfig {
        reverb_wet: Some(0.0),
        ..SceneConfig::new("較正")
    }
}

pub(crate) fn compute_calibration_gain(p: &AudioProcessor) -> f64 {
    let input = calibration_signal(CALIBRATION_SEED);
    let Loudness::Lufs(lin) = measure_mono(&input) else {
        return 1.0;
    };
    let out = p
        .render_with_gain(
            &input,
            &reference_cast(p.reference_dist()),
            &reference_scene(),
            1.0,
        )
        .expect("較正信号の処理に失敗しました");
    let Loudness::Lufs(lout) = measure_stereo(&out) else {
        return 1.0;
    };
    10f64.powf((lin - lout) / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::{measure_mono, measure_stereo, Loudness};
    use crate::processor::AudioProcessor;

    fn processor() -> AudioProcessor {
        AudioProcessor::new(s2v_core::Config::builtin().audio)
    }

    fn lufs(l: Loudness) -> f64 {
        match l {
            Loudness::Lufs(v) => v,
            Loudness::Unmeasurable => panic!("測定できませんでした"),
        }
    }

    #[test]
    fn calibration_signal_is_minus_23_lufs_and_deterministic() {
        let a = calibration_signal(1);
        assert_eq!(a.len(), 240_000);
        assert!((lufs(measure_mono(&a)) + 23.0).abs() < 0.01);
        assert_eq!(a, calibration_signal(1));
        assert_ne!(a, calibration_signal(2));
    }

    #[test]
    fn reference_line_output_matches_input_loudness() {
        // 較正に使ったものと別の信号でも ±0.5 LU に入ること(設計書 5.10 C)
        let p = processor();
        let input = calibration_signal(99);
        let out = p
            .render(
                &input,
                &reference_cast(p.reference_dist()),
                &reference_scene(),
            )
            .unwrap();
        let diff = lufs(measure_stereo(&out)) - lufs(measure_mono(&input));
        assert!(diff.abs() < 0.5, "{diff}");
    }
}
