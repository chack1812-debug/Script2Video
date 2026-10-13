//! 基準ゲインの較正(設計書 5.10 C)。基準の台詞を残響なしで処理したとき、
//! マスター前のステレオのラウドネスが入力のラウドネスと一致するゲインを求める。

use std::collections::HashMap;

use anyhow::Context;
use s2v_core::{Cast, SceneConfig};

use crate::filters::Biquad;
use crate::loudness::{apply_gain_db, measure_mono, measure_stereo, Loudness};
use crate::processor::AudioProcessor;

/// 較正に使う信号の種(固定)。
pub const CALIBRATION_SEED: u64 = 0x5C21_7B1D;

/// SplitMix64。較正信号の再現性を外部クレートの実装に依存させないための最小の乱数生成器。
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// [-1, 1) の一様乱数。
    fn next_signed_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// 較正信号: 100Hz〜8kHz に帯域制限したピンクノイズ 5 秒、-23 LUFS。`seed` が同じなら同じ信号。
pub fn calibration_signal(seed: u64) -> Vec<f32> {
    let n = 5 * 48_000;
    let mut rng = SplitMix64(seed);
    // Paul Kellet のピンクノイズフィルタ
    let mut b = [0.0f64; 7];
    let mut hp = Biquad::highpass(100.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut lp = Biquad::lowpass(8_000.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            let w: f64 = rng.next_signed_unit();
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

pub(crate) fn compute_calibration_gain(p: &AudioProcessor) -> anyhow::Result<f64> {
    let input = calibration_signal(CALIBRATION_SEED);
    let Loudness::Lufs(lin) = measure_mono(&input) else {
        anyhow::bail!("較正信号のラウドネスを測定できませんでした");
    };
    let out = p
        .render_with_gain(
            &input,
            &reference_cast(p.reference_dist()),
            &reference_scene(),
            1.0,
        )
        .with_context(|| {
            format!(
                "較正信号の処理に失敗しました(reference_dist={} などの設定を確認してください)",
                p.reference_dist()
            )
        })?;
    let Loudness::Lufs(lout) = measure_stereo(&out) else {
        anyhow::bail!(
            "較正信号の出力のラウドネスを測定できませんでした(reference_dist={} などの設定を確認してください)",
            p.reference_dist()
        );
    };
    let gain = 10f64.powf((lin - lout) / 20.0);
    if !(gain.is_finite() && gain > 0.0) {
        anyhow::bail!(
            "基準ゲインを求められませんでした(reference_dist={} などの設定を確認してください): {gain}",
            p.reference_dist()
        );
    }
    Ok(gain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::{measure_mono, measure_stereo, Loudness};
    use crate::processor::AudioProcessor;

    fn processor() -> AudioProcessor {
        AudioProcessor::new(s2v_core::Config::builtin().audio).unwrap()
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
    fn calibration_signal_is_pinned_to_the_inline_generator() {
        // 乱数生成器(SplitMix64)や較正信号の作り方が変わったら気づけるようにする
        let a = calibration_signal(1);
        let head = [
            0.001_740_654_f32,
            0.011_981_206,
            0.040_222_086,
            0.071_230_896,
        ];
        for (x, y) in a.iter().zip(head) {
            assert!((x - y).abs() < 1e-6, "{x} vs {y}");
        }
        assert!((a[1000] - 0.099_830_09).abs() < 1e-6);
        assert!((a[1001] - 0.098_063_365).abs() < 1e-6);
        assert!((lufs(measure_mono(&a)) + 23.0).abs() < 0.01);
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
