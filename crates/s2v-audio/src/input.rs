//! 収録音声の取り込み(設計書 5.10 A)。モノラルであることを確認し、48kHz に変換する。レベルは変えない。

use std::fmt;
use std::path::Path;

use crate::resampler::resample_mono;

/// 作業サンプルレート(固定)。
pub const WORK_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    /// 非モノラル・非有限値・壊れたヘッダ・未対応形式(reason: bad_format)
    BadFormat(String),
    /// ファイルを読めない(reason: unreadable)
    Unreadable(String),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::BadFormat(m) => write!(f, "形式が不正です: {m}"),
            InputError::Unreadable(m) => write!(f, "読み込めません: {m}"),
        }
    }
}

/// 収録ファイルを読み、モノラル・48kHz・f32 で返す。
pub fn read_mono_48k(path: &Path) -> Result<Vec<f32>, InputError> {
    if !path.is_file() {
        return Err(InputError::Unreadable(format!(
            "ファイルがありません: {}",
            path.display()
        )));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (samples, rate) = if ext == "flac" {
        read_flac(path)?
    } else {
        read_wav(path)?
    };
    if samples.is_empty() {
        return Err(InputError::BadFormat("サンプルがありません".to_string()));
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(InputError::BadFormat(
            "非有限の値(NaN/Inf)を含みます".to_string(),
        ));
    }
    if rate == WORK_SAMPLE_RATE {
        return Ok(samples);
    }
    let out = resample_mono(&samples, rate, WORK_SAMPLE_RATE)
        .map_err(|e| InputError::BadFormat(format!("リサンプルできません: {e}")))?;
    if out.is_empty() {
        return Err(InputError::BadFormat(
            "音声が短すぎます（48kHz に変換すると空になります）".to_string(),
        ));
    }
    if out.iter().any(|s| !s.is_finite()) {
        return Err(InputError::BadFormat(
            "リサンプル結果に非有限の値(NaN/Inf)を含みます".to_string(),
        ));
    }
    Ok(out)
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), InputError> {
    let mut reader = hound::WavReader::open(path).map_err(map_hound)?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(InputError::BadFormat(format!(
            "モノラルではありません（チャンネル数 {}）",
            spec.channels
        )));
    }
    if spec.sample_rate == 0 {
        return Err(InputError::BadFormat("サンプルレートが 0 です".to_string()));
    }
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16 | 24 | 32) => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .map_err(map_hound)?
        }
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(map_hound)?,
        (f, b) => {
            return Err(InputError::BadFormat(format!(
                "対応していない形式です（{f:?} {b}bit。16/24/32bit 整数、32bit 浮動小数に対応）"
            )))
        }
    };
    Ok((samples, spec.sample_rate))
}

fn map_hound(e: hound::Error) -> InputError {
    match e {
        hound::Error::IoError(io) if io.kind() != std::io::ErrorKind::UnexpectedEof => {
            InputError::Unreadable(io.to_string())
        }
        other => InputError::BadFormat(other.to_string()),
    }
}

fn read_flac(path: &Path) -> Result<(Vec<f32>, u32), InputError> {
    let mut reader = claxon::FlacReader::open(path).map_err(map_claxon)?;
    let info = reader.streaminfo();
    if info.channels != 1 {
        return Err(InputError::BadFormat(format!(
            "モノラルではありません（チャンネル数 {}）",
            info.channels
        )));
    }
    if info.sample_rate == 0 {
        return Err(InputError::BadFormat("サンプルレートが 0 です".to_string()));
    }
    if info.bits_per_sample == 0 || info.bits_per_sample > 32 {
        return Err(InputError::BadFormat(format!(
            "ビット深度が不正です（{}bit）",
            info.bits_per_sample
        )));
    }
    let max = (1i64 << (info.bits_per_sample - 1)) as f32;
    let samples = reader
        .samples()
        .map(|s| s.map(|v| v as f32 / max))
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_claxon)?;
    Ok((samples, info.sample_rate))
}

fn map_claxon(e: claxon::Error) -> InputError {
    match e {
        claxon::Error::IoError(io) if io.kind() != std::io::ErrorKind::UnexpectedEof => {
            InputError::Unreadable(io.to_string())
        }
        other => InputError::BadFormat(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sine(freq: f64, amp: f64, rate: u32, secs: f64) -> Vec<f64> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin())
            .collect()
    }

    fn write_int(
        dir: &Path,
        name: &str,
        bits: u16,
        rate: u32,
        channels: u16,
        data: &[f64],
    ) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        let max = ((1i64 << (bits - 1)) - 1) as f64;
        for &v in data {
            for _ in 0..channels {
                w.write_sample((v * max).round() as i32).unwrap();
            }
        }
        w.finalize().unwrap();
        path
    }

    fn write_float(dir: &Path, name: &str, data: &[f32]) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for &v in data {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    fn rms_db(x: &[f32]) -> f64 {
        let ms = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64;
        10.0 * ms.log10()
    }

    #[test]
    fn reads_16_24_32_bit_integer_and_float() {
        let tmp = tempfile::tempdir().unwrap();
        let data = sine(1000.0, 0.5, 48_000, 0.1);
        for bits in [16u16, 24, 32] {
            let p = write_int(tmp.path(), &format!("i{bits}.wav"), bits, 48_000, 1, &data);
            let got = read_mono_48k(&p).unwrap();
            assert_eq!(got.len(), data.len());
            assert!((got[100] as f64 - data[100]).abs() < 1e-3, "{bits}bit");
        }
        let f: Vec<f32> = data.iter().map(|&v| v as f32).collect();
        let p = write_float(tmp.path(), "f32.wav", &f);
        assert_eq!(read_mono_48k(&p).unwrap(), f);
    }

    #[test]
    fn resamples_to_48k_without_changing_level() {
        let tmp = tempfile::tempdir().unwrap();
        for rate in [44_100u32, 96_000] {
            let data = sine(1000.0, 0.5, rate, 1.0);
            let p = write_int(tmp.path(), &format!("r{rate}.wav"), 24, rate, 1, &data);
            let got = read_mono_48k(&p).unwrap();
            assert!(
                (got.len() as i64 - 48_000).abs() <= 48,
                "{rate}: {}",
                got.len()
            );
            let mid = &got[4_800..43_200];
            let expected = 20.0 * (0.5f64 / 2f64.sqrt()).log10();
            assert!(
                (rms_db(mid) - expected).abs() < 0.1,
                "{rate}: {}",
                rms_db(mid)
            );
        }
    }

    #[test]
    fn stereo_is_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_int(
            tmp.path(),
            "st.wav",
            16,
            48_000,
            2,
            &sine(440.0, 0.1, 48_000, 0.05),
        );
        match read_mono_48k(&p) {
            Err(InputError::BadFormat(m)) => assert!(m.contains("モノラル"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn non_finite_and_8bit_and_broken_header_are_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_float(tmp.path(), "nan.wav", &[0.0, f32::NAN, 0.0]);
        assert!(matches!(read_mono_48k(&p), Err(InputError::BadFormat(_))));
        let p8 = write_int(tmp.path(), "b8.wav", 8, 48_000, 1, &[0.0, 0.5]);
        assert!(matches!(read_mono_48k(&p8), Err(InputError::BadFormat(_))));
        let broken = tmp.path().join("broken.wav");
        std::fs::write(&broken, b"RIFF....not a wave").unwrap();
        assert!(matches!(
            read_mono_48k(&broken),
            Err(InputError::BadFormat(_))
        ));
        let empty = write_float(tmp.path(), "empty.wav", &[]);
        assert!(matches!(
            read_mono_48k(&empty),
            Err(InputError::BadFormat(_))
        ));
    }

    #[test]
    fn missing_file_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_mono_48k(&tmp.path().join("none.wav")),
            Err(InputError::Unreadable(_))
        ));
    }

    /// FLAC のエンコーダは依存に入れないため、ffmpeg がある環境でだけ確認する。
    #[test]
    fn reads_mono_flac_when_ffmpeg_is_available() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            eprintln!("ffmpeg が無いため FLAC のテストを省略します");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wav = write_int(
            tmp.path(),
            "src.wav",
            16,
            44_100,
            1,
            &sine(1000.0, 0.5, 44_100, 0.5),
        );
        let flac = tmp.path().join("src.flac");
        let st = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&wav)
            .arg(&flac)
            .status()
            .unwrap();
        assert!(st.success());
        let got = read_mono_48k(&flac).unwrap();
        assert!((got.len() as i64 - 24_000).abs() <= 48);
    }

    #[test]
    fn too_short_after_resampling_is_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_int(tmp.path(), "one.wav", 16, 192_000, 1, &[0.5]);
        match read_mono_48k(&p) {
            Err(InputError::BadFormat(m)) => assert!(m.contains("短すぎ"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// STREAMINFO だけを持つ最小の FLAC バイト列を作る。
    fn flac_streaminfo(rate: u32, channels: u32, bits: u32) -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        v.extend([0x80, 0, 0, 34]);
        v.extend(4096u16.to_be_bytes());
        v.extend(4096u16.to_be_bytes());
        v.extend([0u8; 6]);
        let packed: u64 = ((rate as u64 & 0xF_FFFF) << 44)
            | (((channels as u64 - 1) & 0x7) << 41)
            | (((bits as u64).wrapping_sub(1) & 0x1F) << 36);
        v.extend(packed.to_be_bytes());
        v.extend([0u8; 16]);
        v
    }

    fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn truncated_flac_is_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_bytes(tmp.path(), "garbage.flac", b"fLaC garbage");
        assert!(matches!(read_mono_48k(&p), Err(InputError::BadFormat(_))));
        let p = write_bytes(
            tmp.path(),
            "trunc.flac",
            &flac_streaminfo(48_000, 1, 16)[..20],
        );
        assert!(matches!(read_mono_48k(&p), Err(InputError::BadFormat(_))));
    }

    #[test]
    fn stereo_flac_is_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_bytes(tmp.path(), "st.flac", &flac_streaminfo(48_000, 2, 16));
        match read_mono_48k(&p) {
            Err(InputError::BadFormat(m)) => assert!(m.contains("モノラル"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn flac_with_zero_sample_rate_is_bad_format() {
        // claxon がヘッダ段階で拒否する。ビット深度は 5bit フィールドが 1..=32 しか表せないため到達不能。
        let tmp = tempfile::tempdir().unwrap();
        let p = write_bytes(tmp.path(), "r0.flac", &flac_streaminfo(0, 1, 16));
        assert!(matches!(read_mono_48k(&p), Err(InputError::BadFormat(_))));
    }
}
