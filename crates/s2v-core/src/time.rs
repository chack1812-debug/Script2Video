//! サンプルと時刻の規則(設計書 5.11)。内部は 48kHz のサンプル単位の整数で持つ。

pub const SAMPLE_RATE: u32 = 48_000;
pub const SAMPLES_PER_MS: i64 = 48;

/// ミリ秒の入力をサンプルにする(ms × 48)。
///
/// 入力は有限かつ非負を前提とする。範囲外は `as` の飽和変換に従い、NaN は 0 になる。
pub fn ms_to_samples(ms: f64) -> i64 {
    (ms * SAMPLES_PER_MS as f64).round() as i64
}

/// 開始の時刻: 切り捨て(負の値も -∞ 方向)。
pub fn floor_ms(samples: i64) -> i64 {
    samples.div_euclid(SAMPLES_PER_MS)
}

/// 終了の時刻・長さ: 切り上げ(負の値も +∞ 方向)。
pub fn ceil_ms(samples: i64) -> i64 {
    let q = samples.div_euclid(SAMPLES_PER_MS);
    if samples.rem_euclid(SAMPLES_PER_MS) != 0 {
        q + 1
    } else {
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_and_samples_round_trip_rules() {
        assert_eq!(ms_to_samples(1.0), 48);
        assert_eq!(ms_to_samples(30.0), 1_440);
        assert_eq!(floor_ms(0), 0);
        assert_eq!(ceil_ms(0), 0);
        assert_eq!(floor_ms(1), 0);
        assert_eq!(ceil_ms(1), 1);
        assert_eq!(floor_ms(48), 1);
        assert_eq!(ceil_ms(48), 1);
        assert_eq!(floor_ms(481), 10);
        assert_eq!(ceil_ms(481), 11);
    }

    #[test]
    fn negative_samples_round_toward_the_documented_direction() {
        assert_eq!(floor_ms(-1), -1);
        assert_eq!(ceil_ms(-1), 0);
        assert_eq!(floor_ms(-48), -1);
        assert_eq!(ceil_ms(-48), -1);
        assert_eq!(ceil_ms(-49), -1);
        assert_eq!(floor_ms(-49), -2);
    }

    fn floor_oracle(s: i64) -> i64 {
        (s as i128).div_euclid(48) as i64
    }

    fn ceil_oracle(s: i64) -> i64 {
        (-((-(s as i128)).div_euclid(48))) as i64
    }

    #[test]
    fn extreme_samples_match_i128_oracle() {
        for s in [i64::MIN, i64::MIN + 1, i64::MAX - 1, i64::MAX] {
            assert_eq!(floor_ms(s), floor_oracle(s), "floor {s}");
            assert_eq!(ceil_ms(s), ceil_oracle(s), "ceil {s}");
        }
    }

    #[test]
    fn rounding_matches_i128_oracle_around_multiples_of_48() {
        for s in -100..=100i64 {
            assert_eq!(floor_ms(s), floor_oracle(s), "floor {s}");
            assert_eq!(ceil_ms(s), ceil_oracle(s), "ceil {s}");
        }
        for k in [
            1i64,
            1_000,
            1_000_000,
            192_153_584_101_141_000,
            -192_153_584_101_141_000,
        ] {
            for d in -1..=1i64 {
                let s = k * 48 + d;
                assert_eq!(floor_ms(s), floor_oracle(s), "floor {s}");
                assert_eq!(ceil_ms(s), ceil_oracle(s), "ceil {s}");
            }
        }
    }

    #[test]
    fn ms_to_samples_saturates_and_nan_is_zero() {
        assert_eq!(ms_to_samples(f64::INFINITY), i64::MAX);
        assert_eq!(ms_to_samples(f64::NEG_INFINITY), i64::MIN);
        assert_eq!(ms_to_samples(f64::NAN), 0);
    }
}
