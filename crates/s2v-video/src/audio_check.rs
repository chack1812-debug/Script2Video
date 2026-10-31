//! 出力の音声の検証(設計書 §4.6)。純粋な計算だけで、プロセスは起動しない。

use crate::ffmpeg::PROTOCOL_WHITELIST;
use realfft::RealFftPlanner;

/// 出力の音声を PCM(f32le、48k、モノラル)に復号する ffmpeg の引数。
pub fn decode_args(path: &str) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-hide_banner", "-n", "-v", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(PROTOCOL_WHITELIST.iter().map(|s| s.to_string()));
    a.extend(
        [
            "-i", path, "-map", "0:a", "-ac", "1", "-ar", "48000", "-f", "f32le", "-",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    a
}

/// f32le のバイト列を読む(端数のバイトは捨てる)。
pub fn f32le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// 窓の位置 start から、decoded を ±search ずらして相互相関を取り、最も一致するずれを返す。
/// 窓の長さ + 2×search 以上の 2 のべき乗の長さの FFT で求める。
fn best_lag_at(window: &[f32], decoded: &[f32], start: usize, search: usize) -> i64 {
    let n = (window.len() + 2 * search).next_power_of_two().max(2);
    let mut a = vec![0.0f32; n];
    a[..window.len()].copy_from_slice(window);
    // b[j] = decoded[start + j - search](範囲外は 0)
    let mut b = vec![0.0f32; n];
    for (j, slot) in b.iter_mut().enumerate().take(window.len() + 2 * search) {
        let idx = (start + j) as i64 - search as i64;
        if idx >= 0 {
            if let Some(v) = decoded.get(idx as usize) {
                *slot = *v;
            }
        }
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut fa = fwd.make_output_vec();
    let mut fb = fwd.make_output_vec();
    fwd.process(&mut a, &mut fa).expect("FFT の長さが合わない");
    fwd.process(&mut b, &mut fb).expect("FFT の長さが合わない");
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * *y;
    }
    let mut c = inv.make_output_vec();
    inv.process(&mut fa, &mut c)
        .expect("逆 FFT の長さが合わない");
    // c[k] = Σ a[i]·b[i+k]。k = lag + search
    let mut best_k = search;
    let mut best_v = f32::NEG_INFINITY;
    for (k, v) in c.iter().enumerate().take(2 * search + 1) {
        if *v > best_v {
            best_v = *v;
            best_k = k;
        }
    }
    best_k as i64 - search as i64
}

/// 相互相関で最も一致するずれ(サンプル、正なら decoded が遅れている)。search は ±search サンプル。
pub fn best_lag(reference: &[f32], decoded: &[f32], search: usize) -> i64 {
    best_lag_at(reference, decoded, 0, search)
}

/// 無音とみなす閾値(-60 dBFS)。
fn silence_threshold() -> f32 {
    10f32.powf(-60.0 / 20.0)
}

/// 参照が全体として無音か(閾値以上のサンプルが 1 つも無い)。
pub fn is_silent(reference: &[f32]) -> bool {
    let th = silence_threshold();
    !reference.iter().any(|x| x.abs() >= th)
}

const SR: usize = 48_000;

/// 設計書 §4.6 の映像と音声の長さの検査。差が 1 コマ + 1024 サンプル以内であること(整数で判定)。
/// |frames/fps − samples/48000| ≤ 1/fps + 1024/48000 を 48000×fps 倍した形で比べる。
pub fn check_av_length(total_frames: i64, fps: u32, decoded_samples: usize) -> Vec<String> {
    let fps = fps as i128;
    let diff = (total_frames as i128 * SR as i128 - decoded_samples as i128 * fps).abs();
    let limit = SR as i128 + 1024 * fps;
    if diff > limit {
        vec![format!(
            "映像({total_frames} コマ、{fps}fps)と音声({decoded_samples} サンプル)の長さの差が 1 コマ + 1024 サンプルを超えています"
        )]
    } else {
        Vec::new()
    }
}

/// 設計書 §4.6 の音声の検査。reference は full_dialogue.wav の左右の平均。
/// 全体が無音なら検査せず空の Vec を返す(呼び出し側が is_silent で判定して Process.log に書く)。
pub fn check_audio(reference: &[f32], decoded: &[f32]) -> Vec<String> {
    let mut errs = Vec::new();
    let diff = (decoded.len() as i64 - reference.len() as i64).abs();
    if diff > 1024 {
        errs.push(format!(
            "音声の長さが参照と {diff} サンプル違います(許容 1024)"
        ));
    }
    if is_silent(reference) {
        return errs;
    }
    let th = silence_threshold();
    let head = &reference[..reference.len().min(10 * SR)];
    let rms =
        (head.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / head.len() as f64).sqrt();
    let start = if rms < th as f64 {
        reference.iter().position(|x| x.abs() >= th).unwrap_or(0)
    } else {
        0
    };
    let end = (start + 10 * SR).min(reference.len());
    let lag = best_lag_at(&reference[start..end], decoded, start, 2048);
    if lag.abs() > 2 {
        errs.push(format!("音声が参照とずれています({lag} サンプル、許容 ±2)"));
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5
            })
            .collect()
    }

    #[test]
    fn lag_is_found() {
        let r = noise(48_000, 7);
        let mut d = vec![0.0; 100];
        d.extend_from_slice(&r);
        assert_eq!(best_lag(&r, &d, 2048), 100);
        assert_eq!(best_lag(&r, &r, 2048), 0);
    }

    #[test]
    fn audio_check_accepts_aligned_and_rejects_shifted() {
        let r = noise(48_000 * 3, 9);
        assert!(check_audio(&r, &r).is_empty());
        let mut shifted = vec![0.0; 100];
        shifted.extend_from_slice(&r[..r.len() - 100]);
        assert!(!check_audio(&r, &shifted).is_empty()); // 100 サンプルのずれ
        assert!(!check_audio(&r, &r[..r.len() - 2000]).is_empty()); // 長さの差 > 1024
    }

    #[test]
    fn leading_silence_moves_the_window_and_all_silence_is_detected() {
        let mut r = vec![0.0; 48_000 * 12];
        r.extend(noise(48_000 * 3, 3));
        assert!(check_audio(&r, &r).is_empty());
        assert!(is_silent(&vec![0.0; 48_000]));
        assert!(!is_silent(&r));
    }

    #[test]
    fn decode_args_have_nostdin_hide_banner_and_n() {
        assert_eq!(
            &decode_args("o.mp4")[..8],
            [
                "-nostdin",
                "-hide_banner",
                "-n",
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-i"
            ]
        );
    }

    #[test]
    fn shifted_audio_after_leading_silence_is_detected() {
        let mut r = vec![0.0; 48_000 * 12];
        r.extend(noise(48_000 * 3, 3));
        let mut d = vec![0.0; 48_000 * 12 + 100];
        d.extend_from_slice(&r[48_000 * 12..r.len() - 100]);
        assert_eq!(d.len(), r.len());
        assert!(!check_audio(&r, &d).is_empty());
    }

    #[test]
    fn av_length_boundaries() {
        // 90 コマ(30fps)= 144000 サンプル。許容は 1 コマ(1600)+ 1024 サンプル。
        assert!(check_av_length(90, 30, 144_000).is_empty());
        assert!(check_av_length(90, 30, 144_000 + 1600 + 1024).is_empty());
        assert!(!check_av_length(90, 30, 144_000 + 1600 + 1025).is_empty());
        assert!(check_av_length(90, 30, 144_000 - 1600 - 1024).is_empty());
        assert!(!check_av_length(90, 30, 144_000 - 1600 - 1025).is_empty());
    }

    #[test]
    fn f32le_reads_little_endian() {
        let b: Vec<u8> = [1.0f32, -0.5]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        assert_eq!(f32le(&b), vec![1.0, -0.5]);
    }
}
