//! マスターのソフトリミッタ(設計書 5.4)。ステレオ連動・先読み 5ms・リリース 50ms。
//! 各サンプルに必要なゲインの「先読み区間の最小値」を同じ長さで移動平均するので、
//! ゲインはなめらかに下がり、かつ必要なゲイン以下になることが保証される。

use std::collections::VecDeque;

/// 先読み(48kHz で 5ms)。
pub const LOOKAHEAD: usize = 240;
/// リリースの時定数[秒]。
pub const RELEASE_S: f64 = 0.05;

/// 前提: 入力はすべて有限値であること(呼び出し側で保証する。NaN を渡してもパニックはしないが、
/// その位置の出力は NaN のままで、ゲイン計算は NaN を無視する)。
pub fn soft_limit(buf: &mut [[f32; 2]], ceiling_db: f64) {
    let n = buf.len();
    if n == 0 {
        return;
    }
    let ceiling = 10f64.powf(ceiling_db / 20.0);
    let req: Vec<f64> = buf
        .iter()
        .map(|f| {
            let p = f[0].abs().max(f[1].abs()) as f64;
            if p > ceiling {
                ceiling / p
            } else {
                1.0
            }
        })
        .collect();
    if req.iter().all(|&r| r >= 1.0) {
        return;
    }

    // m[k] = min(req[k .. k+L))
    let l = LOOKAHEAD;
    let mut m = vec![1.0f64; n];
    let mut dq: VecDeque<usize> = VecDeque::new();
    for k in (0..n).rev() {
        while let Some(&b) = dq.back() {
            if req[b] >= req[k] {
                dq.pop_back();
            } else {
                break;
            }
        }
        dq.push_back(k);
        while let Some(&f) = dq.front() {
            if f >= k + l {
                dq.pop_front();
            } else {
                break;
            }
        }
        m[k] = req[*dq.front().expect("直前に追加している")];
    }

    // g[i] = mean(m[i-L+1 ..= i]): 平均される m[k] はすべて i を区間に含むので g[i] <= req[i]
    let a = 1.0 - (-1.0 / (RELEASE_S * 48_000.0)).exp();
    let c = ceiling as f32;
    let mut sum = 0.0f64;
    let mut env = 1.0f64;
    for i in 0..n {
        sum += m[i];
        if i >= l {
            sum -= m[i - l];
        }
        let g = sum / (i + 1).min(l) as f64;
        // 下がるときは即座に、戻るときはリリースの時定数で
        env = if g < env { g } else { env + (g - env) * a };
        let gi = env as f32;
        buf[i][0] = (buf[i][0] * gi).clamp(-c, c);
        buf[i][1] = (buf[i][1] * gi).clamp(-c, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(n: usize, amp_l: f32, amp_r: f32) -> Vec<[f32; 2]> {
        (0..n)
            .map(|i| {
                let s = ((i as f32) * 0.01).sin();
                [amp_l * s, amp_r * s]
            })
            .collect()
    }

    #[test]
    fn output_never_exceeds_ceiling() {
        let mut x = stereo_sine(48_000, 2.0, 1.5);
        soft_limit(&mut x, -1.0);
        let c = 10f32.powf(-1.0 / 20.0);
        let peak = x
            .iter()
            .flat_map(|f| f.iter())
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= c + 1e-6, "{peak}");
        assert!(peak > c * 0.9, "{peak}");
    }

    #[test]
    fn signal_below_ceiling_is_untouched() {
        let mut x = stereo_sine(4_800, 0.5, 0.3);
        let orig = x.clone();
        soft_limit(&mut x, -1.0);
        assert_eq!(x, orig);
    }

    #[test]
    fn channels_are_linked() {
        let mut x = stereo_sine(48_000, 2.0, 0.5);
        soft_limit(&mut x, -1.0);
        for f in x.iter().filter(|f| f[0].abs() > 0.1) {
            assert!((f[1] / f[0] - 0.25).abs() < 1e-3);
        }
    }

    #[test]
    fn empty_buffer_is_fine() {
        let mut x: Vec<[f32; 2]> = Vec::new();
        soft_limit(&mut x, -1.0);
        assert!(x.is_empty());
    }

    /// 天井を超えるサンプルの手前でゲインが下がっていること(最後の clamp に頼らない)。
    /// 戻り値の代わりに「clamp なしで必要なゲイン以下か」を、入力 1.0 の DC で確かめる。
    fn gains_for(peaks: &[(usize, f32)], n: usize) -> Vec<f32> {
        let mut x = vec![[0.5f32, 0.5]; n];
        for &(i, p) in peaks {
            x[i] = [p, p];
        }
        soft_limit(&mut x, -1.0);
        // 元の値で割ってゲインを復元する(0.5 は天井以下なので 0.5 を基準にする)
        x.iter()
            .enumerate()
            .map(|(i, f)| {
                let orig = peaks
                    .iter()
                    .find(|(k, _)| *k == i)
                    .map(|(_, p)| *p)
                    .unwrap_or(0.5);
                f[0] / orig
            })
            .collect()
    }

    #[test]
    fn isolated_peak_is_attenuated_before_it_and_far_samples_are_untouched() {
        let c = 10f32.powf(-1.0 / 20.0);
        let n = 48_000;
        let at = 24_000;
        let g = gains_for(&[(at, 3.0)], n);
        // ピークそのものは天井以下、かつ clamp の寄与ではなくゲインで下がっている
        assert!(3.0 * g[at] <= c + 1e-6, "{}", 3.0 * g[at]);
        assert!(g[at] <= c / 3.0 + 1e-6, "gain at peak {}", g[at]);
        // ピークの手前(先読み区間内)ですでにゲインが下がり始めている
        assert!(g[at - 1] < 1.0 && g[at - 100] < 1.0);
        assert!(g[at - 239] < 1.0);
        // 先読みより十分前は触らない
        assert_eq!(g[at - 241], 1.0);
        assert_eq!(g[0], 1.0);
        // リリース(時定数 50ms=2400 サンプルの指数)で戻り、約 8 時定数後にはほぼ 1.0
        assert!((g[at + 20_000] - 1.0).abs() < 1e-3, "{}", g[at + 20_000]);
        assert!(g[at + 2_400] < g[at + 20_000]);
    }

    #[test]
    fn dense_burst_stays_under_ceiling_without_hard_clipping() {
        let c = 10f32.powf(-1.0 / 20.0);
        let n = 20_000;
        let mut x = vec![[0.2f32, -0.2]; n];
        for f in &mut x[5_000..5_500] {
            *f = [4.0, -4.0];
        }
        let orig = x.clone();
        soft_limit(&mut x, -1.0);
        let gain = |i: usize| x[i][0] / orig[i][0];
        for (o, y) in orig.iter().zip(&x) {
            assert!(y[0].abs() <= c + 1e-6 && y[1].abs() <= c + 1e-6);
            // ステレオ連動(符号以外は同じ比率)
            assert!((y[0] + y[1]).abs() < 1e-6 * o[0].abs().max(1.0));
        }
        // バースト直前の先読み区間でゲインが滑らかに(単調に)下がっている = clamp に頼らない
        assert_eq!(gain(5_000 - 241), 1.0);
        for i in (5_000 - 239)..5_000 {
            assert!(gain(i) < 1.0);
            assert!(gain(i) <= gain(i - 1) + 1e-7);
        }
        // バースト内はゲインだけで天井に収まる(必要ゲイン以下)
        for i in 5_000..5_500 {
            assert!(4.0 * gain(i) <= c + 1e-5, "{}", 4.0 * gain(i));
        }
    }

    #[test]
    fn peaks_at_buffer_start_and_end_are_limited() {
        let c = 10f32.powf(-1.0 / 20.0);
        let g = gains_for(&[(0, 3.0), (9_999, 3.0)], 10_000);
        assert!(3.0 * g[0] <= c + 1e-6 && 3.0 * g[9_999] <= c + 1e-6);
        assert!(g[0] < c / 3.0 + 1e-6 && g[9_999] < c / 3.0 + 1e-6);
        assert!(g[9_999 - 100] < 1.0);
    }

    #[test]
    fn nan_input_does_not_panic() {
        let mut x = vec![[0.5f32, 0.5]; 1_000];
        x[500] = [f32::NAN, 0.3];
        x[600] = [f32::NAN, f32::NAN];
        x[700] = [3.0, 0.1];
        soft_limit(&mut x, -1.0);
        assert_eq!(x.len(), 1_000);
        let mut y = vec![[f32::NAN; 2]; 10];
        soft_limit(&mut y, -1.0);
    }
}
