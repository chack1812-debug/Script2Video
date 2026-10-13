use s2v_core::{AudioConfig, Cast, SceneConfig};

use crate::acoustics::{compute_reverb_params, resolve_room_geometry, ReverbParams, RoomGeometry};
use crate::early::build_early_taps;
use crate::geometry::{calc_geometry, directivity_pattern};
use crate::reverb::IrCache;

/// room_size/reverb_wet の実効値を決定する (Cast > Scene > AudioConfig の優先順)。
/// Python版 audio_processor.py:81-88 (`c_room_size if ... else (s_room_size if ... else self.default_room_size)`) 相当。
fn resolve_reverb_params(
    cast: &Cast,
    scene: &SceneConfig,
    default_room_size: f64,
    default_reverb_wet: f64,
) -> (f64, f64) {
    let room_size = cast
        .params
        .get("room_size")
        .and_then(|v| v.as_f64())
        .or(scene.room_size)
        .unwrap_or(default_room_size);
    let reverb_wet = cast
        .params
        .get("reverb_wet")
        .and_then(|v| v.as_f64())
        .or(scene.reverb_wet)
        .unwrap_or(default_reverb_wet);
    (room_size, reverb_wet)
}

pub struct AudioProcessor {
    config: AudioConfig,
    ir_cache: IrCache,
    /// 基準ゲイン(設計書 5.10 C)。生成時に較正する。
    calib_gain: f64,
}

impl AudioProcessor {
    pub fn new(config: AudioConfig) -> Self {
        let sample_rate = config.sample_rate;
        let mut p = Self {
            config,
            ir_cache: IrCache::new(sample_rate),
            calib_gain: 1.0,
        };
        p.calib_gain = crate::calib::compute_calibration_gain(&p);
        p
    }

    /// 較正で求めた基準ゲイン。
    pub fn calibration_gain(&self) -> f64 {
        self.calib_gain
    }

    pub fn reference_dist(&self) -> f64 {
        self.config.reference_dist
    }

    pub fn config_sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn config_room_size(&self) -> f64 {
        self.config.room_size
    }

    /// scene と解決済み room_size から拡散リバーブの (rt60, pre_delay) を算出する。
    pub fn reverb_params_for(&self, scene: &SceneConfig, fallback_room_size: f64) -> (f64, usize) {
        let geo = resolve_room_geometry(scene, &self.config.early_reflections, fallback_room_size);
        let rp = compute_reverb_params(
            geo.dims,
            &self.config.early_reflections,
            self.config.sound_speed,
            self.config.sample_rate,
        );
        (rp.rt60, rp.pre_delay)
    }

    /// (rt60, pre_delay) の集合で IR キャッシュを事前計算する。
    pub fn prewarm_reverb(&self, params: &[(f64, usize)]) {
        self.ir_cache.prewarm(params);
    }

    /// 正規化済みのモノラル(48kHz)に、定位・距離・早期反射・残響を掛けてステレオにする。
    /// ピーク正規化と台詞ごとのリミッタは行わない(設計書 5.10 C)。
    pub fn render(
        &self,
        mono: &[f32],
        cast: &Cast,
        scene: &SceneConfig,
    ) -> anyhow::Result<Vec<[f32; 2]>> {
        self.render_with_gain(mono, cast, scene, self.calib_gain)
    }

    pub(crate) fn render_with_gain(
        &self,
        mono: &[f32],
        cast: &Cast,
        scene: &SceneConfig,
        base_gain: f64,
    ) -> anyhow::Result<Vec<[f32; 2]>> {
        // 空の音声は、この先の apply_air_absorption 等が先頭要素に直接アクセスするため Err にする
        if mono.is_empty() {
            anyhow::bail!("音声データが空です(0サンプル)");
        }

        // --- パラメータ決定 (Cast > Scene > AudioConfig デフォルト) ---
        let (room_size, reverb_wet) =
            resolve_reverb_params(cast, scene, self.config.room_size, self.config.reverb_wet);
        let room_geo: RoomGeometry =
            resolve_room_geometry(scene, &self.config.early_reflections, room_size);
        let rp: ReverbParams = compute_reverb_params(
            room_geo.dims,
            &self.config.early_reflections,
            self.config.sound_speed,
            self.config.sample_rate,
        );
        let reverb_active = reverb_wet > 0.0 && rp.wet_base > 0.0;
        if reverb_active {
            self.ir_cache.compute_if_needed(rp.rt60, rp.pre_delay);
        }

        // --- 幾何学計算 ---
        let pan_rad = cast.pan.to_radians();
        let geo = calc_geometry(self.config.microphone_spacing, cast.distance, pan_rad);

        // 空気吸収フィルター
        let data_l = apply_air_absorption(
            mono,
            geo.dist_l,
            self.config.sample_rate,
            self.config.air_absorption_coeff,
        );
        let data_r = apply_air_absorption(
            mono,
            geo.dist_r,
            self.config.sample_rate,
            self.config.air_absorption_coeff,
        );

        // ITD
        let delay_l =
            ((geo.dist_l / self.config.sound_speed) * self.config.sample_rate as f64) as usize;
        let delay_r =
            ((geo.dist_r / self.config.sound_speed) * self.config.sample_rate as f64) as usize;
        let min_delay = delay_l.min(delay_r);
        let rel_l = delay_l - min_delay;
        let rel_r = delay_r - min_delay;

        // ILD + 基準ゲイン
        let mic_angle_rad = self.config.mic_angle.to_radians();
        let k = self.config.mic_directivity;
        let nominal_pat = (1.0 - k) + k * (0.0_f64 - mic_angle_rad).cos();
        let vol_factor = base_gain / nominal_pat.max(1e-6) * cast.volume;

        let dist_gain_l = self.config.reference_dist / geo.dist_l.max(0.1);
        let dist_gain_r = self.config.reference_dist / geo.dist_r.max(0.1);
        // Lマイクは外側 (-mic_angle / 左向き)、Rマイクは外側 (+mic_angle / 右向き) を向く
        // ORTF的な「外開き」配置。distance/ITDによる左右差と指向性パターンによる左右差が
        // 同じ方向を強め合うようにする (符号が逆だと両者が打ち消し合い、定位が反転して聞こえる)。
        let pat_l = directivity_pattern(geo.angle_l, k, mic_angle_rad);
        let pat_r = directivity_pattern(geo.angle_r, k, -mic_angle_rad);

        let gain_l = (vol_factor * dist_gain_l * pat_l) as f32;
        let gain_r = (vol_factor * dist_gain_r * pat_r) as f32;

        // --- 早期反射タップ（イメージソース法）---
        // 話者の実効高さ = 基準(@cast、未指定=聴取者高さ) + 行内臨時パラメータの加算
        let source_height = cast.height.unwrap_or(room_geo.listener_height) + cast.height_offset;
        let early_taps = build_early_taps(
            mono,
            cast.distance,
            pan_rad,
            vol_factor,
            &self.config,
            &self.config.early_reflections,
            &room_geo,
            source_height,
            self.config.sample_rate,
            min_delay,
        );
        let early_max_rel = early_taps
            .iter()
            .map(|t| t.rel_l.max(t.rel_r))
            .max()
            .unwrap_or(0);

        // --- ステレオバッファ構築 ---
        let rv_samples = if reverb_active {
            (self.config.sample_rate as f64 * rp.rt60) as usize + rp.pre_delay
        } else {
            0
        };
        let out_len = mono.len() + rel_l.max(rel_r).max(early_max_rel) + rv_samples;
        let mut stereo: Vec<[f32; 2]> = vec![[0.0, 0.0]; out_len];

        for (i, (&sl, &sr)) in data_l.iter().zip(data_r.iter()).enumerate() {
            stereo[rel_l + i][0] = sl * gain_l;
            stereo[rel_r + i][1] = sr * gain_r;
        }

        // 早期反射を加算
        for tap in &early_taps {
            for (i, &s) in tap.sig.iter().enumerate() {
                stereo[tap.rel_l + i][0] += s * tap.gain_l;
                stereo[tap.rel_r + i][1] += s * tap.gain_r;
            }
        }

        // リバーブ
        self.ir_cache.apply(
            &mut stereo,
            rp.rt60,
            rp.pre_delay,
            reverb_wet,
            rp.wet_base,
            cast.distance,
            self.config.early_reflections.wet_distance_slope,
        );
        Ok(stereo)
    }
}

/// 簡易一次 IIR ローパスで空気吸収をシミュレート
pub(crate) fn apply_air_absorption(
    samples: &[f32],
    dist: f64,
    sample_rate: u32,
    air_coeff: f64,
) -> Vec<f32> {
    if air_coeff <= 0.0 {
        return samples.to_vec();
    }
    let nyquist = sample_rate as f64 / 2.0;
    let cutoff = (nyquist / (1.0 + air_coeff * dist)).min(nyquist - 100.0);
    // 一次 IIR ローパス: alpha = cutoff / (cutoff + fs/2π) 近似
    let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
    let dt = 1.0 / sample_rate as f64;
    let alpha = (dt / (rc + dt)) as f32;
    let mut out = vec![0.0_f32; samples.len()];
    out[0] = alpha * samples[0];
    for i in 1..samples.len() {
        out[i] = out[i - 1] + alpha * (samples[i] - out[i - 1]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn default_audio_config() -> AudioConfig {
        AudioConfig {
            sample_rate: 48000,
            microphone_spacing: 0.2,
            sound_speed: 340.0,
            air_absorption_coeff: 0.05,
            room_size: 0.1,
            reverb_wet: 0.3,
            reference_dist: 1.0,
            mic_directivity: 0.5,
            mic_angle: 45.0,
            early_reflections: s2v_core::EarlyConfig::default(),
        }
    }

    fn dummy_cast(pan: f64, distance: f64) -> Cast {
        Cast {
            name: "テスト".to_string(),
            speaker_name: "ずんだもん".to_string(),
            pan,
            distance,
            volume: 1.0,
            params: HashMap::new(),
            height: None,
            height_offset: 0.0,
            appearance: None,
        }
    }

    fn default_scene() -> SceneConfig {
        SceneConfig {
            room_size: Some(0.1),
            reverb_wet: Some(0.3),
            ..SceneConfig::new("テスト")
        }
    }

    #[test]
    fn resolve_reverb_params_falls_back_to_audio_config_when_scene_omits() {
        // s2v-57z: シーン側でroom_size/reverb_wetが省略された場合、AudioConfigの値にフォールバックする
        // (Python版 audio_processor.py:87-88 self.default_room_size/self.default_base_wet 相当)
        let cast = dummy_cast(0.0, 1.0);
        let scene = SceneConfig {
            room_size: None,
            reverb_wet: None,
            ..SceneConfig::new("テスト")
        };
        let (room_size, reverb_wet) = resolve_reverb_params(&cast, &scene, 0.42, 0.55);
        assert!(
            (room_size - 0.42).abs() < 1e-10,
            "config値にフォールバックするはず, got {room_size}"
        );
        assert!(
            (reverb_wet - 0.55).abs() < 1e-10,
            "config値にフォールバックするはず, got {reverb_wet}"
        );
    }

    #[test]
    fn resolve_reverb_params_prefers_scene_over_config_default() {
        let cast = dummy_cast(0.0, 1.0);
        let scene = SceneConfig {
            room_size: Some(0.8),
            reverb_wet: Some(0.2),
            ..SceneConfig::new("テスト")
        };
        let (room_size, reverb_wet) = resolve_reverb_params(&cast, &scene, 0.42, 0.55);
        assert!((room_size - 0.8).abs() < 1e-10);
        assert!((reverb_wet - 0.2).abs() < 1e-10);
    }

    #[test]
    fn resolve_reverb_params_prefers_cast_over_scene_and_config() {
        let mut cast = dummy_cast(0.0, 1.0);
        cast.params
            .insert("room_size".to_string(), serde_json::json!(0.9));
        cast.params
            .insert("reverb_wet".to_string(), serde_json::json!(0.1));
        let scene = SceneConfig {
            room_size: Some(0.8),
            reverb_wet: Some(0.2),
            ..SceneConfig::new("テスト")
        };
        let (room_size, reverb_wet) = resolve_reverb_params(&cast, &scene, 0.42, 0.55);
        assert!((room_size - 0.9).abs() < 1e-10);
        assert!((reverb_wet - 0.1).abs() < 1e-10);
    }

    /// 決定的な広帯域ノイズ(テスト再現性のため固定シードLCG)。
    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut state: u32 = 0x12345678;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                ((state >> 8) as f32 / 16_777_216.0 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    #[test]
    fn render_panning_creates_stereo_asymmetry() {
        // 外開き(ORTF的)マイク配置: Lマイクは-45°(左向き)、Rマイクは+45°(右向き)。
        // pan は + で右、- で左から聞こえる仕様 (台本仕様.txt) なので、
        // 左パン(pan=-45)では音源がLマイクに近く・正面に来てLchが大きくなり、
        // 右パン(pan=+45)では音源がRマイクに近く・正面に来てRchが大きくなる。
        let proc = AudioProcessor::new(default_audio_config());
        let input = tone(9_600, 0.5);

        let rms = |out: &[[f32; 2]], ch: usize| -> f64 {
            let sum_sq: f64 = out.iter().map(|f| (f[ch] as f64) * (f[ch] as f64)).sum();
            (sum_sq / out.len() as f64).sqrt()
        };
        // 小さな絶対値を避けるため振幅を 32767 倍相当で見る代わりに、比の分母へ微小量を足す
        let ratio = |out: &[[f32; 2]]| -> f64 {
            let (l, r) = (rms(out, 0), rms(out, 1));
            (l - r).abs() / (l + r + 1e-6)
        };

        // 中央: L≈R
        let out_center = proc
            .render(&input, &dummy_cast(0.0, 1.0), &default_scene())
            .unwrap();
        assert!(
            ratio(&out_center) < 0.1,
            "center ratio={:.3}",
            ratio(&out_center)
        );

        // 左パン(pan=-45): Lchが大きくなるはず (台本仕様.txt: -=左)
        let out_left = proc
            .render(&input, &dummy_cast(-45.0, 1.0), &default_scene())
            .unwrap();
        let (l_left, r_left) = (rms(&out_left, 0), rms(&out_left, 1));
        assert!(
            ratio(&out_left) > 0.05,
            "left ratio={:.3}",
            ratio(&out_left)
        );
        assert!(
            l_left > r_left,
            "pan=-45 (left) should be louder in the L channel, got L={l_left} R={r_left}"
        );

        // 右パン(pan=+45): Rchが大きくなるはず (台本仕様.txt: +=右)
        let out_right = proc
            .render(&input, &dummy_cast(45.0, 1.0), &default_scene())
            .unwrap();
        let (l_right, r_right) = (rms(&out_right, 0), rms(&out_right, 1));
        assert!(
            ratio(&out_right) > 0.05,
            "right ratio={:.3}",
            ratio(&out_right)
        );
        assert!(
            r_right > l_right,
            "pan=+45 (right) should be louder in the R channel, got L={l_right} R={r_right}"
        );
    }

    #[test]
    fn calc_geometry_symmetric_at_center() {
        let geo = calc_geometry(0.2, 1.0, 0.0);
        assert!((geo.dist_l - geo.dist_r).abs() < 1e-10);
    }

    #[test]
    fn apply_air_absorption_preserves_length() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.001).sin()).collect();
        let out = apply_air_absorption(&samples, 2.0, 48000, 0.05);
        assert_eq!(out.len(), samples.len());
    }

    #[test]
    fn early_reflections_disabled_matches_no_early_output() {
        let mut cfg = default_audio_config();
        cfg.early_reflections.enabled = false;
        cfg.reverb_wet = 0.0;
        let proc = AudioProcessor::new(cfg);
        let out = proc
            .render(
                &tone(4_800, 0.5),
                &dummy_cast(20.0, 2.0),
                &SceneConfig {
                    room_size: Some(0.1),
                    reverb_wet: Some(0.0),
                    ..SceneConfig::new("s")
                },
            )
            .unwrap();
        let energy: f64 = out
            .iter()
            .map(|f| (f[0] as f64).powi(2) + (f[1] as f64).powi(2))
            .sum();
        assert!(energy > 0.0, "出力が生成されること");
    }

    #[test]
    fn early_reflections_enabled_adds_energy() {
        // 信号非依存にするため広帯域ノイズを使う。純音だと早期反射の遅延と波長の
        // 位相関係（コムフィルタ）で総エネルギーが増減し得るが、無相関ノイズに
        // 遅延・減衰コピーを加えると総エネルギーは確実に増える（位相依存しない）。
        // かつて純音で FAIL した cast(20.0, 2.0) でも頑健に成り立つことを示す。
        let input = noise(4_800, 0.5);
        let scene = SceneConfig {
            room_size: Some(0.1),
            reverb_wet: Some(0.0),
            ..SceneConfig::new("s")
        };

        let energy_for = |enabled: bool| -> f64 {
            let mut cfg = default_audio_config();
            cfg.reverb_wet = 0.0;
            cfg.early_reflections.enabled = enabled;
            let proc = AudioProcessor::new(cfg);
            let out = proc.render(&input, &dummy_cast(20.0, 2.0), &scene).unwrap();
            out.iter()
                .map(|f| (f[0] as f64).powi(2) + (f[1] as f64).powi(2))
                .sum()
        };

        let e_off = energy_for(false);
        let e_on = energy_for(true);
        assert!(
            e_on > e_off * 1.01,
            "早期反射ありで総エネルギー増加(ノイズ入力): off={e_off}, on={e_on}"
        );
    }

    #[test]
    fn outdoor_scene_processes_with_near_dry_reverb() {
        let mut cfg = default_audio_config();
        cfg.early_reflections.enabled = false;
        cfg.early_reflections.ceiling.reflection_coeff = 0.0;
        cfg.early_reflections.front_wall.reflection_coeff = 0.0;
        cfg.early_reflections.back_wall.reflection_coeff = 0.0;
        cfg.early_reflections.side_walls.reflection_coeff = 0.0;
        cfg.early_reflections.floor.reflection_coeff = 0.5;
        let proc = AudioProcessor::new(cfg);
        let scene = SceneConfig {
            room_w: Some(30.0),
            room_d: Some(30.0),
            room_h: Some(15.0),
            reverb_wet: Some(1.0),
            ..SceneConfig::new("屋外")
        };
        let out = proc
            .render(&noise(4_800, 0.5), &dummy_cast(0.0, 1.0), &scene)
            .unwrap();
        assert!(
            !out.is_empty(),
            "屋外 scene でも処理が成功し出力が生成されること"
        );
    }

    #[test]
    fn scene_room_dims_affect_reverb_params() {
        let proc = AudioProcessor::new(default_audio_config());
        let small = SceneConfig {
            room_w: Some(4.0),
            room_d: Some(5.0),
            room_h: Some(3.0),
            ..SceneConfig::new("小")
        };
        let big = SceneConfig {
            room_w: Some(25.0),
            room_d: Some(45.0),
            room_h: Some(18.0),
            ..SceneConfig::new("大")
        };
        let (rt_small, _) = proc.reverb_params_for(&small, 0.1);
        let (rt_big, _) = proc.reverb_params_for(&big, 0.1);
        assert!(
            rt_big > rt_small,
            "大きい部屋ほど残響長が長い: small={rt_small}, big={rt_big}"
        );
    }

    #[test]
    fn scene_room_dims_affect_process_output_length() {
        // 大きい部屋ほど rt60 が長く、残響テール分だけ出力サンプル数が増える。
        // render() が scene 寸法を実際に残響へ反映していることを end-to-end で検証する。
        let input = noise(4_800, 0.5);
        let proc = AudioProcessor::new(default_audio_config());

        let small = SceneConfig {
            room_w: Some(4.0),
            room_d: Some(5.0),
            room_h: Some(3.0),
            reverb_wet: Some(1.0),
            ..SceneConfig::new("小")
        };
        let big = SceneConfig {
            room_w: Some(25.0),
            room_d: Some(45.0),
            room_h: Some(18.0),
            reverb_wet: Some(1.0),
            ..SceneConfig::new("大")
        };

        let n_small = proc
            .render(&input, &dummy_cast(0.0, 1.0), &small)
            .unwrap()
            .len();
        let n_big = proc
            .render(&input, &dummy_cast(0.0, 1.0), &big)
            .unwrap()
            .len();

        assert!(
            n_big > n_small,
            "大部屋は残響テールが長く出力サンプル数が多い: small={n_small}, big={n_big}"
        );
    }

    #[test]
    fn speaker_height_changes_process_output() {
        // 話者高さを変えると早期反射の床反射が変わり、出力(早期反射ON)が変化する
        let input = noise(4_800, 0.5);
        let mut cfg = default_audio_config();
        cfg.reverb_wet = 0.0; // 残響を切り早期反射のみ比較
        let proc = AudioProcessor::new(cfg);
        let scene = SceneConfig {
            room_w: Some(8.0),
            room_d: Some(8.0),
            room_h: Some(5.0),
            reverb_wet: Some(0.0),
            ..SceneConfig::new("室")
        };

        let mut low = dummy_cast(0.0, 2.0);
        low.height = Some(1.0);
        let mut high = dummy_cast(0.0, 2.0);
        high.height = Some(3.0);
        let out_low = proc.render(&input, &low, &scene).unwrap();
        let out_high = proc.render(&input, &high, &scene).unwrap();

        assert_ne!(out_low, out_high, "話者高さで出力が変わること");
    }

    #[test]
    fn speaker_height_base_plus_offset_equals_absolute() {
        // 実効高さ = cast.height + height_offset。基準1.5+行内0.3 は 絶対1.8(+0) と同一出力になること。
        let input = noise(4_800, 0.5);
        let mut cfg = default_audio_config();
        cfg.reverb_wet = 0.0;
        let proc = AudioProcessor::new(cfg);
        let scene = SceneConfig {
            room_w: Some(8.0),
            room_d: Some(8.0),
            room_h: Some(5.0),
            reverb_wet: Some(0.0),
            ..SceneConfig::new("室")
        };

        let mut combined = dummy_cast(0.0, 2.0);
        combined.height = Some(1.5);
        combined.height_offset = 0.3;
        let mut absolute = dummy_cast(0.0, 2.0);
        absolute.height = Some(1.8);
        absolute.height_offset = 0.0;

        let out_c = proc.render(&input, &combined, &scene).unwrap();
        let out_a = proc.render(&input, &absolute, &scene).unwrap();
        assert_eq!(
            out_c, out_a,
            "基準+行内オフセットの合算が絶対指定と一致すること"
        );
    }

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * ((i as f32) * 0.05).sin()).collect()
    }

    #[test]
    fn render_rejects_empty_input() {
        let p = AudioProcessor::new(default_audio_config());
        assert!(p
            .render(&[], &dummy_cast(0.0, 1.0), &default_scene())
            .is_err());
    }

    #[test]
    fn render_is_linear_without_per_line_normalization_or_limiter() {
        let p = AudioProcessor::new(default_audio_config());
        let x = tone(4_800, 0.2);
        let x4: Vec<f32> = x.iter().map(|v| v * 4.0).collect();
        let cast = dummy_cast(0.0, 0.3);
        let a = p.render(&x, &cast, &default_scene()).unwrap();
        let b = p.render(&x4, &cast, &default_scene()).unwrap();
        assert_eq!(a.len(), b.len());
        for (fa, fb) in a.iter().zip(&b) {
            assert!((fb[0] - 4.0 * fa[0]).abs() < 1e-4);
            assert!((fb[1] - 4.0 * fa[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn volume_half_lowers_output_by_6_db() {
        let p = AudioProcessor::new(default_audio_config());
        let x = tone(9_600, 0.2);
        let full = dummy_cast(0.0, 1.0);
        let mut half = full.clone();
        half.volume = 0.5;
        let a = p.render(&x, &full, &default_scene()).unwrap();
        let b = p.render(&x, &half, &default_scene()).unwrap();
        let energy = |v: &[[f32; 2]]| {
            v.iter()
                .map(|f| (f[0] * f[0] + f[1] * f[1]) as f64)
                .sum::<f64>()
        };
        let db = 10.0 * (energy(&b) / energy(&a)).log10();
        assert!((db + 6.02).abs() < 0.1, "{db}");
    }
}
