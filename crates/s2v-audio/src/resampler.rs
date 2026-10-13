use rubato::{FftFixedIn, Resampler};

/// モノラル f32 サンプル列を `from_rate` から `to_rate` にリサンプリングする。
/// rubato::FftFixedIn を使用 (SIMD 最適化)。
pub fn resample_mono(samples: &[f32], from_rate: u32, to_rate: u32) -> anyhow::Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let chunk_size = 1024_usize;

    let mut resampler =
        FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, chunk_size, 2, 1)?;

    // FFT リサンプラは出力が output_delay() サンプル遅れる（24k→48k・チャンク1024で512）。
    // 遅延分を先頭から捨て、入力が尽きた後は空入力の process_partial で残りを吐き出させる
    // （rubato の推奨手順）。これを怠ると全体が遅れ、末尾の遅延分が切り捨てられる。
    let delay = resampler.output_delay();
    let expected = (samples.len() as f64 * ratio).round() as usize;
    let mut output = Vec::with_capacity(delay + expected + chunk_size * 2);

    let mut chunks = samples.chunks_exact(chunk_size);
    for chunk in &mut chunks {
        let wave_out = resampler.process(&[chunk], None)?;
        output.extend_from_slice(&wave_out[0]);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let wave_out = resampler.process_partial(Some(&[rest]), None)?;
        output.extend_from_slice(&wave_out[0]);
    }
    while output.len() < delay + expected {
        let wave_out = resampler.process_partial::<&[f32]>(None, None)?;
        if wave_out[0].is_empty() {
            break;
        }
        output.extend_from_slice(&wave_out[0]);
    }

    let mut output = output.split_off(delay.min(output.len()));
    output.resize(expected, 0.0);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_wave(freq: f32, sample_rate: u32, duration_s: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_s) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn same_rate_returns_identical() {
        let samples = sine_wave(440.0, 24000, 0.1);
        let out = resample_mono(&samples, 24000, 24000).unwrap();
        assert_eq!(out.len(), samples.len());
        for (a, b) in samples.iter().zip(out.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = resample_mono(&[], 24000, 48000).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn upsample_produces_correct_length() {
        let samples = sine_wave(440.0, 24000, 0.5);
        let out = resample_mono(&samples, 24000, 48000).unwrap();
        let expected = (samples.len() as f64 * 2.0).round() as usize;
        // rubato の出力は ±chunk_size 以内に収まる
        assert!((out.len() as isize - expected as isize).unsigned_abs() <= 1024);
    }

    #[test]
    fn downsample_produces_correct_length() {
        let samples = sine_wave(440.0, 48000, 0.5);
        let out = resample_mono(&samples, 48000, 24000).unwrap();
        let expected = (samples.len() as f64 * 0.5).round() as usize;
        assert!((out.len() as isize - expected as isize).unsigned_abs() <= 1024);
    }

    /// s2v-18b.3: rubato の出力遅延(output_delay)を補正し、時間位置がずれないこと。
    /// 以前は 24kHz→48kHz で常に 512 サンプル遅れていた。
    #[test]
    fn upsample_preserves_impulse_position() {
        let mut samples = vec![0.0_f32; 4800];
        samples[1000] = 1.0;
        let out = resample_mono(&samples, 24000, 48000).unwrap();
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!(
            (peak as isize - 2000).abs() <= 1,
            "インパルスは 2000 付近にあるべき: {peak}"
        );
    }

    #[test]
    fn downsample_preserves_impulse_position() {
        let mut samples = vec![0.0_f32; 9600];
        samples[3000] = 1.0;
        let out = resample_mono(&samples, 48000, 24000).unwrap();
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!(
            (peak as isize - 1500).abs() <= 1,
            "インパルスは 1500 付近にあるべき: {peak}"
        );
    }

    /// 出力長は round(n·to/from) ちょうどで、末尾まで信号が残ること（遅延分が切り捨てられない）。
    #[test]
    fn output_has_exact_length_and_keeps_tail() {
        for &(n, from, to) in &[
            (2048usize, 24000u32, 48000u32),
            (12345, 24000, 48000),
            (10000, 22050, 48000),
            (9999, 48000, 24000),
        ] {
            let samples = sine_wave(440.0, from, n as f32 / from as f32);
            let samples = &samples[..n.min(samples.len())];
            let out = resample_mono(samples, from, to).unwrap();
            let expected = (samples.len() as f64 * to as f64 / from as f64).round() as usize;
            assert_eq!(out.len(), expected, "n={n} {from}->{to}");
            let tail = &out[out.len() - 256..];
            let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
            assert!(
                rms > 0.5,
                "末尾256サンプルにも正弦波が残ること: rms={rms} n={n}"
            );
        }
    }

    #[test]
    fn output_samples_are_finite() {
        let samples = sine_wave(1000.0, 22050, 0.2);
        let out = resample_mono(&samples, 22050, 48000).unwrap();
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
