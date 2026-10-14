use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ordered_float::OrderedFloat;
use rand::{rngs::SmallRng, SeedableRng};
use rand_distr::{Distribution, StandardNormal};
use realfft::RealFftPlanner;

/// wet 信号の固定ゲイン。IR は生成時に L2 エネルギー 1 へ正規化済みなので、
/// 信号ごとの(ピーク依存の)再正規化は行わず、この定数だけを掛ける(処理を入力に対して線形に保つ)。
pub const REVERB_WET_GAIN: f32 = 0.4;

type IrKey = (OrderedFloat<f64>, usize);
type StereoIr = [Vec<f32>; 2];

/// dry と wet の混合比。`apply` と音量補償(processor)の両方がこの関数を使い、食い違いを防ぐ。
/// 0.01 刻みに量子化して [0, 0.9] に収める(補償のキーを有限個にし、測った値と掛ける値を一致させる)。
/// 非有限な入力は NaN を返す(呼び出し側が Err にする)。
pub(crate) fn actual_wet(
    reverb_wet: f64,
    wet_base: f64,
    avg_dist: f64,
    wet_distance_slope: f64,
) -> f64 {
    let raw = reverb_wet * wet_base * (1.0 + wet_distance_slope * avg_dist);
    if !raw.is_finite() {
        return f64::NAN;
    }
    ((raw.clamp(0.0, 0.9) * 100.0).round() / 100.0).min(0.9)
}

/// 量子化(4桁)した値。キャッシュのキーに使う。
pub(crate) fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn poisoned() -> anyhow::Error {
    anyhow::anyhow!("残響 IR のキャッシュが使えません(別スレッドの処理が異常終了しました)")
}

/// (rt60, pre_delay) をキーとした stereo IR キャッシュ ([ir_l, ir_r])。
/// IR は丸めた rt60(4桁)から生成する(キーと IR が一致する)。
pub struct IrCache {
    sample_rate: u32,
    cache: Mutex<HashMap<IrKey, Arc<StereoIr>>>,
}

impl IrCache {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn prewarm(&self, params: &[(f64, usize)]) -> anyhow::Result<()> {
        for &(rt60, pre_delay) in params {
            self.compute_if_needed(rt60, pre_delay)?;
        }
        Ok(())
    }

    /// IR を(無ければ)生成して返す。生成はロックの外で行う。
    fn get_or_build(&self, rt60: f64, pre_delay: usize) -> anyhow::Result<Arc<StereoIr>> {
        let key = (OrderedFloat(round4(rt60)), pre_delay);
        if let Some(ir) = self.cache.lock().map_err(|_| poisoned())?.get(&key) {
            return Ok(Arc::clone(ir));
        }
        let ir = Arc::new(build_ir(key.0.into_inner(), pre_delay, self.sample_rate));
        let mut cache = self.cache.lock().map_err(|_| poisoned())?;
        Ok(Arc::clone(cache.entry(key).or_insert(ir)))
    }

    pub fn compute_if_needed(&self, rt60: f64, pre_delay: usize) -> anyhow::Result<()> {
        self.get_or_build(rt60, pre_delay).map(|_| ())
    }

    /// IR を使って stereo バッファにリバーブをかける (in-place)
    #[allow(clippy::too_many_arguments)] // 移植元の署名を保つ(引数を束ねるリファクタは行わない)
    pub fn apply(
        &self,
        stereo: &mut [[f32; 2]],
        rt60: f64,
        pre_delay: usize,
        reverb_wet: f64,
        wet_base: f64,
        avg_dist: f64,
        wet_distance_slope: f64,
    ) -> anyhow::Result<()> {
        let actual_wet = actual_wet(reverb_wet, wet_base, avg_dist, wet_distance_slope);
        self.apply_with_wet(stereo, rt60, pre_delay, actual_wet as f32)
    }

    /// `actual_wet`(`actual_wet()` で求めた混合比)を直接指定して適用する (in-place)。
    /// IR は Arc を複製してロックを放してから畳み込む。
    pub(crate) fn apply_with_wet(
        &self,
        stereo: &mut [[f32; 2]],
        rt60: f64,
        pre_delay: usize,
        actual_wet: f32,
    ) -> anyhow::Result<()> {
        if !actual_wet.is_finite() || !(0.0..=0.9).contains(&actual_wet) {
            anyhow::bail!("残響の混合比が範囲外です: {actual_wet}");
        }
        if actual_wet <= 0.0 || stereo.is_empty() {
            return Ok(());
        }
        let ir = self.get_or_build(rt60, pre_delay)?;

        for ch in 0..2 {
            let dry: Vec<f32> = stereo.iter().map(|s| s[ch]).collect();
            let wet = fft_convolve(&dry, &ir[ch]);
            for (i, s) in stereo.iter_mut().enumerate() {
                s[ch] = (1.0 - actual_wet) * s[ch] + actual_wet * REVERB_WET_GAIN * wet[i];
            }
        }
        Ok(())
    }

    /// テスト用: 別スレッドでロックを握ったまま panic させ、キャッシュの Mutex を汚染する。
    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                let _g = self.cache.lock().unwrap();
                panic!("poison the IR cache");
            });
            assert!(h.join().is_err());
        });
    }
}

/// FFT 畳み込み (線形)
fn fft_convolve(signal: &[f32], kernel: &[f32]) -> Vec<f32> {
    let out_len = signal.len() + kernel.len() - 1;
    let fft_size = out_len.next_power_of_two();

    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);
    let c2r = planner.plan_fft_inverse(fft_size);

    let mut sig_buf = vec![0.0_f32; fft_size];
    sig_buf[..signal.len()].copy_from_slice(signal);
    let mut ker_buf = vec![0.0_f32; fft_size];
    ker_buf[..kernel.len()].copy_from_slice(kernel);

    let mut sig_spec = r2c.make_output_vec();
    let mut ker_spec = r2c.make_output_vec();

    r2c.process(&mut sig_buf, &mut sig_spec).unwrap();
    r2c.process(&mut ker_buf, &mut ker_spec).unwrap();

    let mut product: Vec<_> = sig_spec
        .iter()
        .zip(ker_spec.iter())
        .map(|(a, b)| a * b)
        .collect();

    let mut out_buf = vec![0.0_f32; fft_size];
    c2r.process(&mut product, &mut out_buf).unwrap();

    let scale = 1.0 / fft_size as f32;
    out_buf.iter_mut().for_each(|s| *s *= scale);
    out_buf.truncate(out_len);
    out_buf
}

/// 2次Butterworthローパスフィルタの SOS 係数 [b0,b1,b2,a0,a1,a2] (a0=1) を求める。
/// 双一次変換によるRBJ Audio EQ Cookbookの式 (Q = 1/sqrt(2)) は
/// scipy.signal.butter(2, cutoff_hz, 'lp', fs=sample_rate, output='sos') と
/// 浮動小数点誤差(1e-16オーダー)の範囲で一致する (Python版 audio_processor.py:36 相当)。
pub(crate) fn butterworth_lowpass_sos(cutoff_hz: f64, sample_rate: f64) -> [f64; 6] {
    let q = std::f64::consts::FRAC_1_SQRT_2;
    let w0 = 2.0 * std::f64::consts::PI * cutoff_hz / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = (1.0 - cos_w0) / 2.0;
    let b1 = 1.0 - cos_w0;
    let b2 = (1.0 - cos_w0) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;

    [b0 / a0, b1 / a0, b2 / a0, 1.0, a1 / a0, a2 / a0]
}

/// 単一セクションの SOS フィルタを Direct Form II Transposed で適用する
/// (scipy.signal.sosfilt と数値的に等価)。
pub(crate) fn sosfilt_single_section(sos: &[f64; 6], input: &[f64]) -> Vec<f64> {
    let [b0, b1, b2, _a0, a1, a2] = *sos;
    let mut z1 = 0.0_f64;
    let mut z2 = 0.0_f64;
    input
        .iter()
        .map(|&x| {
            let y = b0 * x + z1;
            z1 = b1 * x - a1 * y + z2;
            z2 = b2 * x - a2 * y;
            y
        })
        .collect()
}

/// シード固定の乱数でリバーブ IR を生成する
fn build_ir(rt60: f64, pre_delay: usize, sample_rate: u32) -> [Vec<f32>; 2] {
    let fs = sample_rate as f64;
    let rv_time = round4(rt60);
    let n = (fs * rv_time) as usize;

    let seed = (round4(rt60) * 10000.0) as u64 & 0xFFFF_FFFF;
    let mut rng = SmallRng::seed_from_u64(seed);

    let sos = butterworth_lowpass_sos(1800.0, fs);

    let decay: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / fs;
            (-6.91 * t / rv_time).exp()
        })
        .collect();

    let mut irs: [Vec<f32>; 2] = std::array::from_fn(|_| {
        let noise: Vec<f64> = StandardNormal.sample_iter(&mut rng).take(n).collect();
        let filtered = sosfilt_single_section(&sos, &noise);
        let mut ir: Vec<f32> = vec![0.0; pre_delay];
        ir.extend(
            filtered
                .iter()
                .zip(decay.iter())
                .map(|(s, d)| (s * d) as f32),
        );
        ir
    });
    // 各チャンネルを L2 エネルギー 1 に正規化する(生成時に1回だけ)
    for ir in &mut irs {
        let energy: f64 = ir.iter().map(|&v| (v as f64) * (v as f64)).sum();
        if energy > 0.0 {
            let scale = (1.0 / energy.sqrt()) as f32;
            ir.iter_mut().for_each(|v| *v *= scale);
        }
    }
    irs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_cache_builds_entry() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();
        let guard = cache.cache.lock().expect("テスト内でロックは汚染されない");
        assert!(guard.contains_key(&(OrderedFloat(round4(1.0)), 240)));
    }

    #[test]
    fn prewarm_fills_multiple_entries() {
        let cache = IrCache::new(48000);
        cache
            .prewarm(&[(0.5, 240), (1.0, 240), (2.0, 480)])
            .unwrap();
        let guard = cache.cache.lock().expect("テスト内でロックは汚染されない");
        assert_eq!(guard.len(), 3);
    }

    #[test]
    fn ir_has_two_channels() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();
        let guard = cache.cache.lock().expect("テスト内でロックは汚染されない");
        let ir = &guard[&(OrderedFloat(round4(1.0)), 240)];
        assert!(!ir[0].is_empty());
        assert!(!ir[1].is_empty());
    }

    #[test]
    fn apply_with_zero_wet_leaves_signal_unchanged() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();
        let original: Vec<[f32; 2]> = (0..100)
            .map(|i| [i as f32 * 0.01, i as f32 * 0.01])
            .collect();
        let mut signal = original.clone();
        cache
            .apply(&mut signal, 1.0, 240, 0.0, 0.8, 1.0, 0.1)
            .unwrap();
        for (a, b) in original.iter().zip(signal.iter()) {
            assert!((a[0] - b[0]).abs() < 1e-6);
        }
    }

    #[test]
    fn apply_with_wet_modifies_signal() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();
        let n = 2400_usize;
        let mut signal: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let v = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.5;
                [v, v]
            })
            .collect();
        let before = signal[100][0];
        cache
            .apply(&mut signal, 1.0, 240, 1.0, 0.8, 1.0, 0.1)
            .unwrap();
        // リバーブ後は少なくとも何らかの変化があるはず
        let changed = signal.iter().any(|s| (s[0] - before).abs() > 1e-4);
        assert!(changed, "apply should modify signal");
    }

    #[test]
    fn fft_convolve_delta_returns_signal() {
        let signal: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let delta = vec![1.0_f32];
        let result = fft_convolve(&signal, &delta);
        for (a, b) in signal.iter().zip(result.iter()) {
            assert!((a - b).abs() < 1e-3, "a={a}, b={b}");
        }
    }

    #[test]
    fn butterworth_lowpass_sos_matches_scipy_butter_2_1800hz() {
        // 参照値: scipy.signal.butter(2, 1800, 'lp', fs=48000, output='sos')[0]
        // (Python版 audio_processor.py:36 self._reverb_sos の生成と等価)
        let sos = butterworth_lowpass_sos(1800.0, 48000.0);
        let expected = [
            0.011857682643241158,
            0.023715365286482316,
            0.011857682643241158,
            1.0,
            -1.6692031429311929,
            0.7166338735041575,
        ];
        for (a, b) in sos.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-12, "got {sos:?}, expected {expected:?}");
        }
    }

    #[test]
    fn sosfilt_single_section_matches_scipy_sosfilt_impulse_response() {
        // 参照値: scipy.signal.sosfilt(sos, [1,0,0,0,0,0,0,0]) (fs=48000)
        let sos = butterworth_lowpass_sos(1800.0, 48000.0);
        let mut impulse = vec![0.0_f64; 8];
        impulse[0] = 1.0;
        let y = sosfilt_single_section(&sos, &impulse);
        let expected = [
            0.011857682643241158,
            0.04350824642246111,
            0.07598416727162914,
            0.09565352765971114,
            0.10521234088519019,
            0.10707221203959165,
            0.1033265454680878,
            0.0957414203849639,
        ];
        for (a, b) in y.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-12, "got {y:?}, expected {expected:?}");
        }
    }

    #[test]
    fn build_ir_is_deterministic() {
        let ir1 = build_ir(1.0, 240, 48000);
        let ir2 = build_ir(1.0, 240, 48000);
        assert_eq!(ir1[0].len(), ir2[0].len());
        for (a, b) in ir1[0].iter().zip(ir2[0].iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn apply_uses_distance_slope_for_wet_amount() {
        // slope を大きくすると遠距離での wet 比が増える → 出力の変化量が増える
        let make_signal = || -> Vec<[f32; 2]> {
            (0..2400)
                .map(|i| {
                    let v = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.5;
                    [v, v]
                })
                .collect()
        };
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();

        let dry = make_signal();
        let mut s_small = make_signal();
        let mut s_large = make_signal();
        cache
            .apply(&mut s_small, 1.0, 240, 0.3, 0.8, 5.0, 0.1)
            .unwrap();
        cache
            .apply(&mut s_large, 1.0, 240, 0.3, 0.8, 5.0, 0.5)
            .unwrap();

        let dev = |sig: &Vec<[f32; 2]>| -> f64 {
            sig.iter()
                .zip(dry.iter())
                .map(|(a, b)| ((a[0] - b[0]) as f64).powi(2))
                .sum()
        };
        assert!(
            dev(&s_large) > dev(&s_small),
            "slope大の方がwet寄与が大きいこと"
        );
    }

    #[test]
    fn apply_increases_wet_with_distance() {
        // 距離が増えると wet 比が増える(D/Rが下がる)。slope固定で avg_dist を変えて検証(spec §6)。
        let make_signal = || -> Vec<[f32; 2]> {
            (0..2400)
                .map(|i| {
                    let v = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.5;
                    [v, v]
                })
                .collect()
        };
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();

        let dry = make_signal();
        let mut s_near = make_signal();
        let mut s_far = make_signal();
        cache
            .apply(&mut s_near, 1.0, 240, 0.3, 0.8, 1.0, 0.1)
            .unwrap();
        cache
            .apply(&mut s_far, 1.0, 240, 0.3, 0.8, 5.0, 0.1)
            .unwrap();

        let dev = |sig: &Vec<[f32; 2]>| -> f64 {
            sig.iter()
                .zip(dry.iter())
                .map(|(a, b)| ((a[0] - b[0]) as f64).powi(2))
                .sum()
        };
        assert!(
            dev(&s_far) > dev(&s_near),
            "遠い音源ほど wet 寄与が大きいこと"
        );
    }

    #[test]
    fn compute_and_apply_with_rt60_predelay_key() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.5, 480).unwrap();
        let mut signal: Vec<[f32; 2]> = (0..4800)
            .map(|i| {
                let v = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.5;
                [v, v]
            })
            .collect();
        let before = signal[2000][0];
        cache
            .apply(&mut signal, 1.5, 480, 1.0, 0.8, 1.0, 0.1)
            .unwrap();
        assert!(
            signal.iter().any(|s| (s[0] - before).abs() > 1e-4),
            "apply は信号を変化させること"
        );
    }

    #[test]
    fn apply_wet_base_zero_leaves_signal_unchanged() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(1.0, 240).unwrap();
        let original: Vec<[f32; 2]> = (0..200)
            .map(|i| [i as f32 * 0.01, i as f32 * 0.01])
            .collect();
        let mut signal = original.clone();
        cache
            .apply(&mut signal, 1.0, 240, 1.0, 0.0, 1.0, 0.1)
            .unwrap();
        for (a, b) in original.iter().zip(signal.iter()) {
            assert!((a[0] - b[0]).abs() < 1e-6);
        }
    }

    #[test]
    fn cached_ir_channels_have_unit_energy() {
        let cache = IrCache::new(48000);
        cache
            .prewarm(&[(0.3, 100), (1.0, 240), (2.5, 480)])
            .unwrap();
        let guard = cache.cache.lock().expect("テスト内でロックは汚染されない");
        assert_eq!(guard.len(), 3);
        for ir in guard.values() {
            for ch in ir.iter() {
                let e: f64 = ch.iter().map(|&v| (v as f64) * (v as f64)).sum();
                assert!((e - 1.0).abs() < 1e-3, "IR エネルギー={e}");
            }
        }
    }

    #[test]
    fn apply_is_linear_in_the_signal() {
        let cache = IrCache::new(48000);
        cache.compute_if_needed(0.5, 100).unwrap();
        let mk = |f: f32| -> Vec<[f32; 2]> {
            (0..3000)
                .map(|i| {
                    let v = (i as f32 * f).sin() * 0.3;
                    [v, v * 0.5]
                })
                .collect()
        };
        let (x, y) = (mk(0.05), mk(0.31));
        let mut sum: Vec<[f32; 2]> = x
            .iter()
            .zip(&y)
            .map(|(a, b)| [a[0] + b[0], a[1] + b[1]])
            .collect();
        let (mut rx, mut ry) = (x.clone(), y.clone());
        cache.apply(&mut sum, 0.5, 100, 0.6, 0.8, 1.0, 0.1).unwrap();
        cache.apply(&mut rx, 0.5, 100, 0.6, 0.8, 1.0, 0.1).unwrap();
        cache.apply(&mut ry, 0.5, 100, 0.6, 0.8, 1.0, 0.1).unwrap();
        for i in 0..sum.len() {
            assert!((sum[i][0] - rx[i][0] - ry[i][0]).abs() < 1e-4);
            assert!((sum[i][1] - rx[i][1] - ry[i][1]).abs() < 1e-4);
        }
    }
}
