//! BGM/SE の読み込みと、トラック・ミックスの書き出し。

use std::path::Path;

/// ヘッダーだけを読んで、48kHz に換算した長さ[サンプル]を返す(デコードしない)。
pub fn duration_48k_samples(path: &Path) -> anyhow::Result<u64> {
    let reader = hound::WavReader::open(path)?;
    let rate = reader.spec().sample_rate;
    if rate == 0 {
        anyhow::bail!("サンプルレートが 0 です");
    }
    Ok((reader.duration() as u128 * 48_000 / rate as u128) as u64)
}

/// BGM/SE を読む(モノラル・ステレオ可)。48kHz でなければ左右別々にリサンプルする。
/// 空のファイルや、NaN/無限大を含むファイル(リサンプル後も含む)は Err。
pub fn read_stereo_48k(path: &Path) -> anyhow::Result<Vec<[f32; 2]>> {
    let out = read_stereo_48k_unchecked(path)?;
    if out.iter().any(|f| !f[0].is_finite() || !f[1].is_finite()) {
        anyhow::bail!("音声に NaN/無限大が含まれています: {}", path.display());
    }
    Ok(out)
}

fn read_stereo_48k_unchecked(path: &Path) -> anyhow::Result<Vec<[f32; 2]>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
        anyhow::bail!("未対応のビット深度です: {}", spec.bits_per_sample);
    }
    if spec.sample_rate == 0 {
        anyhow::bail!("サンプルレートが 0 です");
    }
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()?
        }
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };
    let ch = spec.channels.max(1) as usize;
    let stereo: Vec<[f32; 2]> = if ch == 1 {
        raw.iter().map(|&s| [s, s]).collect()
    } else {
        raw.chunks_exact(ch).map(|c| [c[0], c[1]]).collect()
    };
    if stereo.is_empty() {
        anyhow::bail!("空の音声ファイル");
    }
    if spec.sample_rate == 48_000 {
        return Ok(stereo);
    }
    let left: Vec<f32> = stereo.iter().map(|s| s[0]).collect();
    let right: Vec<f32> = stereo.iter().map(|s| s[1]).collect();
    let left = s2v_audio::resample_mono(&left, spec.sample_rate, 48_000)?;
    let right = s2v_audio::resample_mono(&right, spec.sample_rate, 48_000)?;
    Ok(left.into_iter().zip(right).map(|(l, r)| [l, r]).collect())
}

/// 32bit 浮動小数ステレオ(クリップしない)。`tracks/*.wav` 用。
pub fn write_f32_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for f in buf {
        w.write_sample(f[0])?;
        w.write_sample(f[1])?;
    }
    w.finalize()?;
    Ok(())
}

/// 24bit 整数ステレオ。`full_dialogue.wav` 用(マスターのリミッタ後なので ±1 を超えない)。
pub fn write_i24_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    const MAX: f32 = 8_388_607.0;
    for f in buf {
        for v in f {
            w.write_sample((v.clamp(-1.0, 1.0) * MAX).round() as i32)?;
        }
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_raw(path: &Path, bits: u16, rate: u32, ch: u16, samples: &[i32]) {
        let spec = hound::WavSpec {
            channels: ch,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn empty_wav_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.wav");
        write_raw(&p, 16, 48_000, 1, &[]);
        assert!(read_stereo_48k(&p).is_err());
    }

    #[test]
    fn truncated_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.wav");
        write_raw(&p, 16, 48_000, 2, &vec![1000; 2_000]);
        let bytes = std::fs::read(&p).unwrap();
        std::fs::write(&p, &bytes[..bytes.len() - 1_001]).unwrap();
        assert!(read_stereo_48k(&p).is_err());
        assert!(read_stereo_48k(&tmp.path().join("missing.wav")).is_err());
    }

    #[test]
    fn eight_bit_mono_is_duplicated_to_stereo() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("b8.wav");
        write_raw(&p, 8, 48_000, 1, &[64, -64]);
        let v = read_stereo_48k(&p).unwrap();
        assert_eq!(v, vec![[0.5, 0.5], [-0.5, -0.5]]);
    }

    fn write_float(path: &Path, rate: u32, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn non_finite_samples_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, bad) in [
            ("nan.wav", f32::NAN),
            ("inf.wav", f32::INFINITY),
            ("ninf.wav", f32::NEG_INFINITY),
        ] {
            let p = tmp.path().join(name);
            write_float(&p, 48_000, &[0.1, bad, 0.1]);
            let err = read_stereo_48k(&p).unwrap_err().to_string();
            assert!(err.contains(name), "{err}");
        }
        let p = tmp.path().join("ok.wav");
        write_float(&p, 48_000, &[0.1, -0.2]);
        assert!(read_stereo_48k(&p).is_ok());
    }
}
