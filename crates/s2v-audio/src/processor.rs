use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ordered_float::OrderedFloat;
use s2v_core::{AudioConfig, Cast, SceneConfig};

use crate::acoustics::{compute_reverb_params, resolve_room_geometry, ReverbParams, RoomGeometry};
use crate::early::build_early_taps;
use crate::geometry::{calc_geometry, directivity_pattern};
use crate::loudness::{measure_stereo, Loudness};
use crate::reverb::{actual_wet, round4, IrCache};

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

/// 1回の render が扱う音声の上限(3600 秒 @48kHz)。遅延や残響の暴走による巨大確保を防ぐ。
pub(crate) const MAX_OUTPUT_SAMPLES: usize = 48_000 * 3_600;

/// 距離から遅延サンプル数を求める。非有限や上限超過は Err。
fn delay_samples(dist: f64, sound_speed: f64, fs: f64) -> anyhow::Result<usize> {
    let v = dist / sound_speed * fs;
    if !v.is_finite() || v > MAX_OUTPUT_SAMPLES as f64 {
        anyhow::bail!("距離が大きすぎます(伝搬遅延が 3600 秒を超えます): {dist} m");
    }
    Ok(v.max(0.0) as usize)
}

/// 残響の音量補償ゲインのキー: (rt60(4桁に丸め), pre_delay, actual_wet(百分率の整数。0.01 刻み),
/// 話者距離(0.5 m 刻みの整数。0〜50 m))。
type ReverbCompKey = (OrderedFloat<f64>, usize, u32, u32);

/// 補償の測定に使う距離の刻み(m)と上限(m)。
const COMP_DISTANCE_STEP_M: f64 = 0.5;
const COMP_DISTANCE_MAX_M: f64 = 50.0;

/// 1つのキーの補償ゲイン(単一実行: 同じキーを同時に求めても計算は1回)。失敗は文字列で保持する。
type CompSlot = Arc<OnceLock<Result<f32, String>>>;

pub struct AudioProcessor {
    config: AudioConfig,
    ir_cache: IrCache,
    /// 残響パラメータごとの固定の音量補償ゲイン(信号に依存しないので render は線形のまま)。
    reverb_comp: Mutex<HashMap<ReverbCompKey, CompSlot>>,
    /// テスト用: 補償ゲインを実際に計算した回数。
    #[cfg(test)]
    comp_computations: AtomicUsize,
    /// 基準ゲイン(設計書 5.10 C)。生成時に較正する。
    calib_gain: f64,
}

/// `AudioConfig` の数値が有限で、明らかな範囲外でないことを確認する。
fn validate_audio_config(c: &AudioConfig) -> anyhow::Result<()> {
    let er = &c.early_reflections;
    let mut finite: Vec<(&str, f64)> = vec![
        ("microphone_spacing", c.microphone_spacing),
        ("sound_speed", c.sound_speed),
        ("air_absorption_coeff", c.air_absorption_coeff),
        ("room_size", c.room_size),
        ("reverb_wet", c.reverb_wet),
        ("reference_dist", c.reference_dist),
        ("mic_directivity", c.mic_directivity),
        ("mic_angle", c.mic_angle),
        ("ear_height", er.ear_height),
        ("early_level", er.early_level),
        ("wet_distance_slope", er.wet_distance_slope),
        ("listener_offset[0]", er.listener_offset[0]),
        ("listener_offset[1]", er.listener_offset[1]),
    ];
    for i in 0..3 {
        finite.push(("room_dims_min", er.room_dims_min[i]));
        finite.push(("room_dims_max", er.room_dims_max[i]));
    }
    for m in [
        &er.floor,
        &er.ceiling,
        &er.front_wall,
        &er.back_wall,
        &er.side_walls,
    ] {
        finite.push(("reflection_coeff", m.reflection_coeff));
        finite.push(("absorption_cutoff_hz", m.absorption_cutoff_hz));
    }
    for (name, v) in finite {
        if !v.is_finite() {
            anyhow::bail!("[audio] {name} が有限の値ではありません: {v}");
        }
    }
    if c.reverb_wet < 0.0 {
        anyhow::bail!(
            "[audio] reverb_wet は 0 以上にしてください: {}",
            c.reverb_wet
        );
    }
    if !(0.0..=1.0).contains(&c.room_size) {
        anyhow::bail!("[audio] room_size は 0〜1 にしてください: {}", c.room_size);
    }
    if !(0.0..=1.0).contains(&c.mic_directivity) {
        anyhow::bail!(
            "[audio] mic_directivity は 0〜1 にしてください: {}",
            c.mic_directivity
        );
    }
    Ok(())
}

impl AudioProcessor {
    /// 生成時に基準ゲインを較正する。設定が不正で較正できないときは Err。
    pub fn new(config: AudioConfig) -> anyhow::Result<Self> {
        for (name, v) in [
            ("reference_dist", config.reference_dist),
            ("sound_speed", config.sound_speed),
            ("microphone_spacing", config.microphone_spacing),
        ] {
            if !(v.is_finite() && v > 0.0) {
                anyhow::bail!("[audio] {name} は 0 より大きい有限の値にしてください: {v}");
            }
        }
        validate_audio_config(&config)?;
        let sample_rate = config.sample_rate;
        let mut p = Self {
            config,
            ir_cache: IrCache::new(sample_rate),
            reverb_comp: Mutex::new(HashMap::new()),
            #[cfg(test)]
            comp_computations: AtomicUsize::new(0),
            calib_gain: 1.0,
        };
        p.calib_gain = crate::calib::compute_calibration_gain(&p)?;
        Ok(p)
    }

    /// 残響パラメータ(rt60、pre_delay、混合比 actual_wet)に対する固定の音量補償ゲイン。
    ///
    /// 較正信号(dual-mono: L=R=信号)を、残響の前と後(`IrCache::apply` と同じパラメータ)で
    /// BS.1770 のステレオラウドネスを測り、その差を戻すゲインを返す。結果はパラメータごとに
    /// キャッシュする(同じキーの同時計算は1回に集約し、計算はキャッシュのロックの外で行う)。
    /// `actual_wet` は 0.01 刻みに量子化済みの値で、実際の混合に使う値と同じ。
    /// 較正信号は、実際の経路と同じ空気吸収を量子化した距離(0.5 m 刻み、0〜50 m)で通してから測る。
    /// 実際の経路では残響の前に早期反射が加わるが、補償は素の dual-mono を基準にして求める
    /// (許容 ±0.5 LU。テストで確認)。測定不能や不正な値は Err(丸めない)。
    pub(crate) fn reverb_comp_gain(
        &self,
        rt60: f64,
        pre_delay: usize,
        actual_wet: f64,
        distance: f64,
    ) -> anyhow::Result<f32> {
        if !distance.is_finite() {
            anyhow::bail!("距離が有限の値ではありません: {distance}");
        }
        if !actual_wet.is_finite() || !(0.0..=0.9).contains(&actual_wet) {
            anyhow::bail!("残響の混合比が範囲外です: {actual_wet}");
        }
        let key = (
            OrderedFloat(round4(rt60)),
            pre_delay,
            (actual_wet * 100.0).round() as u32,
            (distance.clamp(0.0, COMP_DISTANCE_MAX_M) / COMP_DISTANCE_STEP_M).round() as u32,
        );
        let slot = {
            let mut map = self
                .reverb_comp
                .lock()
                .map_err(|_| anyhow::anyhow!("残響の音量補償のキャッシュが使えません"))?;
            Arc::clone(map.entry(key).or_default())
        };
        slot.get_or_init(|| {
            #[cfg(test)]
            self.comp_computations.fetch_add(1, Ordering::SeqCst);
            self.compute_reverb_comp(key).map_err(|e| format!("{e:#}"))
        })
        .clone()
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn compute_reverb_comp(&self, key: ReverbCompKey) -> anyhow::Result<f32> {
        let (rt60, pre_delay) = (key.0.into_inner(), key.1);
        let wet = key.2 as f32 / 100.0;
        let distance = key.3 as f64 * COMP_DISTANCE_STEP_M;
        self.ir_cache.compute_if_needed(rt60, pre_delay)?;
        let signal = crate::calib::calibration_signal(crate::calib::CALIBRATION_SEED);
        let tail = (self.config.sample_rate as f64 * rt60) as usize + pre_delay;
        // 実際の経路と同じ空気吸収(距離に応じたローパス)を通す。左右とも量子化した話者距離を使う
        // (マイク間隔による左右の距離差は無視する。pan に依存しない基準にするため)。
        let filtered = apply_air_absorption(
            &signal,
            distance,
            self.config.sample_rate,
            self.config.air_absorption_coeff,
        );
        let mut stereo: Vec<[f32; 2]> = filtered
            .iter()
            .map(|&v| [v, v])
            .chain(std::iter::repeat_n([0.0, 0.0], tail))
            .collect();
        let Loudness::Lufs(before) = measure_stereo(&stereo) else {
            anyhow::bail!("残響の音量補償: 残響の前のラウドネスを測定できませんでした");
        };
        self.ir_cache
            .apply_with_wet(&mut stereo, rt60, pre_delay, wet)?;
        let Loudness::Lufs(after) = measure_stereo(&stereo) else {
            anyhow::bail!("残響の音量補償: 残響の後のラウドネスを測定できませんでした");
        };
        let gain = 10f64.powf((before - after) / 20.0) as f32;
        if !(gain.is_finite() && gain > 0.0) {
            anyhow::bail!("残響の音量補償ゲインが不正です: {gain}");
        }
        Ok(gain)
    }

    #[cfg(test)]
    fn reverb_comp_len(&self) -> usize {
        self.reverb_comp
            .lock()
            .expect("テスト内でキャッシュの Mutex は汚染されない")
            .len()
    }

    #[cfg(test)]
    fn comp_computations(&self) -> usize {
        self.comp_computations.load(Ordering::SeqCst)
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
    pub fn prewarm_reverb(&self, params: &[(f64, usize)]) -> anyhow::Result<()> {
        self.ir_cache.prewarm(params)
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
        if mono.len() > MAX_OUTPUT_SAMPLES {
            anyhow::bail!("音声データが長すぎます({}サンプル)", mono.len());
        }
        if mono.iter().any(|s| !s.is_finite()) {
            anyhow::bail!("音声データに NaN または無限大が含まれています");
        }
        let cast_values = [
            ("pan", Some(cast.pan)),
            ("distance", Some(cast.distance)),
            ("volume", Some(cast.volume)),
            ("height", cast.height),
            ("height_offset", Some(cast.height_offset)),
            ("基準ゲイン", Some(base_gain)),
        ];
        for (name, v) in cast_values {
            if let Some(v) = v {
                if !v.is_finite() {
                    anyhow::bail!("{name} が有限の値ではありません: {v}");
                }
            }
        }

        // --- パラメータ決定 (Cast > Scene > AudioConfig デフォルト) ---
        let (room_size, reverb_wet) =
            resolve_reverb_params(cast, scene, self.config.room_size, self.config.reverb_wet);
        if !room_size.is_finite() {
            anyhow::bail!("room_size が有限の値ではありません: {room_size}");
        }
        if !reverb_wet.is_finite() || reverb_wet < 0.0 {
            anyhow::bail!("reverb_wet は 0 以上の有限の値にしてください: {reverb_wet}");
        }
        let room_geo: RoomGeometry =
            resolve_room_geometry(scene, &self.config.early_reflections, room_size);
        if room_geo.dims.iter().any(|v| !v.is_finite())
            || !room_geo.listener_height.is_finite()
            || room_geo.listener_offset.iter().any(|v| !v.is_finite())
        {
            anyhow::bail!("部屋の寸法または聴取位置が有限の値ではありません");
        }
        let rp: ReverbParams = compute_reverb_params(
            room_geo.dims,
            &self.config.early_reflections,
            self.config.sound_speed,
            self.config.sample_rate,
        );
        if !rp.wet_base.is_finite() {
            anyhow::bail!("残響の基準混合比が有限の値ではありません");
        }
        let reverb_active = reverb_wet > 0.0 && rp.wet_base > 0.0;
        // 実際に混ぜる値(0.01 刻み)。補償もこの値で求める
        let aw = actual_wet(
            reverb_wet,
            rp.wet_base,
            cast.distance,
            self.config.early_reflections.wet_distance_slope,
        );
        if reverb_active && !(aw.is_finite() && (0.0..=0.9).contains(&aw)) {
            anyhow::bail!("残響の混合比 actual_wet が範囲外です: {aw}");
        }
        // rt60 はキー・IR と同じ4桁の丸め値を、残響の尾の長さにも使う
        let rt60 = round4(rp.rt60);
        let fs = self.config.sample_rate as f64;
        if reverb_active {
            if !(rt60.is_finite() && rt60 >= 0.0 && rt60 * fs <= MAX_OUTPUT_SAMPLES as f64)
                || rp.pre_delay > MAX_OUTPUT_SAMPLES
            {
                anyhow::bail!("残響の長さが範囲外です(rt60={})", rt60);
            }
            self.ir_cache.compute_if_needed(rt60, rp.pre_delay)?;
        }

        // --- 幾何学計算 ---
        let pan_rad = cast.pan.to_radians();
        let geo = calc_geometry(self.config.microphone_spacing, cast.distance, pan_rad);
        let delay_l = delay_samples(geo.dist_l, self.config.sound_speed, fs)?;
        let delay_r = delay_samples(geo.dist_r, self.config.sound_speed, fs)?;

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
            (self.config.sample_rate as f64 * rt60) as usize + rp.pre_delay
        } else {
            0
        };
        let out_len = mono
            .len()
            .checked_add(rel_l.max(rel_r).max(early_max_rel))
            .and_then(|n| n.checked_add(rv_samples))
            .filter(|&n| n <= MAX_OUTPUT_SAMPLES)
            .ok_or_else(|| {
                anyhow::anyhow!("出力が長すぎます(上限 {MAX_OUTPUT_SAMPLES} サンプル = 3600 秒)")
            })?;
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

        // リバーブ(混合比は上で求めた量子化済みの aw。補償で測った値と同じ)
        if reverb_active {
            self.ir_cache
                .apply_with_wet(&mut stereo, rt60, rp.pre_delay, aw as f32)?;
            if aw > 0.0 {
                // 残響による音量変化を、パラメータごとの固定ゲインで戻す(信号に依存しない)
                let g = self.reverb_comp_gain(rt60, rp.pre_delay, aw, cast.distance)?;
                for f in &mut stereo {
                    f[0] *= g;
                    f[1] *= g;
                }
            }
        }
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
        let proc = AudioProcessor::new(default_audio_config()).unwrap();
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

    fn energy_of(out: &[[f32; 2]]) -> f64 {
        out.iter()
            .map(|f| (f[0] as f64).powi(2) + (f[1] as f64).powi(2))
            .sum()
    }

    #[test]
    fn early_reflections_disabled_matches_no_early_output() {
        let input = noise(4_800, 0.5);
        let scene = SceneConfig {
            room_size: Some(0.1),
            reverb_wet: Some(0.0),
            ..SceneConfig::new("s")
        };
        let build = |enabled: bool| {
            let mut cfg = default_audio_config();
            cfg.early_reflections.enabled = enabled;
            cfg.reverb_wet = 0.0;
            AudioProcessor::new(cfg).unwrap()
        };
        let off = build(false);
        let on = build(true);

        // pan=0: 左右対称(同じ信号・同じゲイン)で、長さは入力そのまま
        let off_center = off.render(&input, &dummy_cast(0.0, 2.0), &scene).unwrap();
        assert_eq!(
            off_center.len(),
            input.len(),
            "早期反射なし・残響なしでは尾が出ない"
        );
        for f in &off_center {
            assert!((f[0] - f[1]).abs() <= 1e-6 * f[0].abs().max(1.0));
        }

        // pan=20°: 長さ = 入力 + 両耳間遅延のみ
        let cast = dummy_cast(20.0, 2.0);
        let off_out = off.render(&input, &cast, &scene).unwrap();
        let geo = calc_geometry(0.2, 2.0, 20.0_f64.to_radians());
        let dl = ((geo.dist_l / 340.0) * 48_000.0) as usize;
        let dr = ((geo.dist_r / 340.0) * 48_000.0) as usize;
        assert_eq!(off_out.len(), input.len() + dl.abs_diff(dr));

        // 早期反射ありは、長く、エネルギーも大きい
        let on_out = on.render(&input, &cast, &scene).unwrap();
        assert!(on_out.len() > off_out.len());
        assert!(
            energy_of(&on_out) > energy_of(&off_out) * 1.01,
            "off={}, on={}",
            energy_of(&off_out),
            energy_of(&on_out)
        );
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
            let proc = AudioProcessor::new(cfg).unwrap();
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
        let input = noise(4_800, 0.5);
        let mut cfg = default_audio_config();
        cfg.early_reflections.enabled = false;
        cfg.early_reflections.ceiling.reflection_coeff = 0.0;
        cfg.early_reflections.front_wall.reflection_coeff = 0.0;
        cfg.early_reflections.back_wall.reflection_coeff = 0.0;
        cfg.early_reflections.side_walls.reflection_coeff = 0.0;
        cfg.early_reflections.floor.reflection_coeff = 0.5;
        let outdoor_proc = AudioProcessor::new(cfg).unwrap();
        let outdoor = SceneConfig {
            room_w: Some(30.0),
            room_d: Some(30.0),
            room_h: Some(15.0),
            reverb_wet: Some(1.0),
            ..SceneConfig::new("屋外")
        };
        let indoor_proc = AudioProcessor::new(default_audio_config()).unwrap();
        let indoor = SceneConfig {
            room_w: Some(4.0),
            room_d: Some(5.0),
            room_h: Some(3.0),
            reverb_wet: Some(1.0),
            ..SceneConfig::new("屋内")
        };
        // 入力長を超える部分(残響の尾)と本体のエネルギー比
        let tail_ratio = |p: &AudioProcessor, scene: &SceneConfig| -> f64 {
            let out = p.render(&input, &dummy_cast(0.0, 1.0), scene).unwrap();
            assert!(out.len() > input.len());
            energy_of(&out[input.len()..]) / energy_of(&out[..input.len()])
        };
        let t_out = tail_ratio(&outdoor_proc, &outdoor);
        let t_in = tail_ratio(&indoor_proc, &indoor);
        eprintln!("tail ratios: outdoor={t_out} indoor={t_in}");
        assert!(t_out < 0.01, "屋外は残響がほぼ無いこと: {t_out}");
        assert!(
            t_out < t_in / 5.0,
            "屋外 {t_out} は屋内 {t_in} より十分小さいこと"
        );
    }

    #[test]
    fn scene_room_dims_affect_reverb_params() {
        let proc = AudioProcessor::new(default_audio_config()).unwrap();
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
        let proc = AudioProcessor::new(default_audio_config()).unwrap();

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
        let proc = AudioProcessor::new(cfg).unwrap();
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
        let proc = AudioProcessor::new(cfg).unwrap();
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
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        assert!(p
            .render(&[], &dummy_cast(0.0, 1.0), &default_scene())
            .is_err());
    }

    #[test]
    fn render_is_linear_without_per_line_normalization_or_limiter() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
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
        let p = AudioProcessor::new(default_audio_config()).unwrap();
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

    fn reverb_scene() -> SceneConfig {
        SceneConfig {
            room_w: Some(6.0),
            room_d: Some(7.0),
            room_h: Some(3.0),
            reverb_wet: Some(0.8),
            ..SceneConfig::new("残響あり")
        }
    }

    #[test]
    fn render_is_additive_with_reverb_and_early_reflections() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = tone(3_000, 0.2);
        let y = noise(3_000, 0.3);
        let xy: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a + b).collect();
        let cast = dummy_cast(25.0, 1.5);
        let scene = reverb_scene();
        let (rx, ry, rxy) = (
            p.render(&x, &cast, &scene).unwrap(),
            p.render(&y, &cast, &scene).unwrap(),
            p.render(&xy, &cast, &scene).unwrap(),
        );
        assert_eq!(rx.len(), rxy.len());
        assert!(rxy.len() > xy.len() + 1_000, "残響の尾があること");
        for i in 0..rxy.len() {
            assert!((rxy[i][0] - rx[i][0] - ry[i][0]).abs() < 1e-4);
            assert!((rxy[i][1] - rx[i][1] - ry[i][1]).abs() < 1e-4);
        }
    }

    #[test]
    fn render_scales_linearly_with_reverb_on() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = noise(3_000, 0.1);
        let x4: Vec<f32> = x.iter().map(|v| v * 4.0).collect();
        let cast = dummy_cast(-30.0, 1.2);
        let a = p.render(&x, &cast, &reverb_scene()).unwrap();
        let b = p.render(&x4, &cast, &reverb_scene()).unwrap();
        assert_eq!(a.len(), b.len());
        for (fa, fb) in a.iter().zip(&b) {
            assert!((fb[0] - 4.0 * fa[0]).abs() < 1e-4);
            assert!((fb[1] - 4.0 * fa[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn render_rejects_non_finite_values() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let scene = default_scene();
        let x = tone(1_000, 0.2);
        let mut bad = x.clone();
        bad[10] = f32::NAN;
        assert!(p.render(&bad, &dummy_cast(0.0, 1.0), &scene).is_err());
        bad[10] = f32::INFINITY;
        assert!(p.render(&bad, &dummy_cast(0.0, 1.0), &scene).is_err());
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                p.render(&x, &dummy_cast(v, 1.0), &scene).is_err(),
                "pan {v}"
            );
            assert!(
                p.render(&x, &dummy_cast(0.0, v), &scene).is_err(),
                "distance {v}"
            );
            let mut c = dummy_cast(0.0, 1.0);
            c.volume = v;
            assert!(p.render(&x, &c, &scene).is_err(), "volume {v}");
            let mut c = dummy_cast(0.0, 1.0);
            c.height = Some(v);
            assert!(p.render(&x, &c, &scene).is_err(), "height {v}");
            let mut c = dummy_cast(0.0, 1.0);
            c.height_offset = v;
            assert!(p.render(&x, &c, &scene).is_err(), "height_offset {v}");
        }
    }

    #[test]
    fn render_handles_unusual_but_finite_values() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let scene = default_scene();
        let x = tone(2_000, 0.2);
        let mut neg_vol = dummy_cast(0.0, 1.0);
        neg_vol.volume = -1.0;
        let casts = [
            dummy_cast(0.0, 0.0),
            dummy_cast(0.0, -2.0),
            dummy_cast(200.0, 1.0),
            dummy_cast(-135.0, 3.0),
            neg_vol,
        ];
        for c in &casts {
            let out = p.render(&x, c, &scene).unwrap();
            assert!(!out.is_empty());
            assert!(out.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
        }
    }

    #[test]
    fn render_rejects_absurd_distance_instead_of_allocating() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = tone(1_000, 0.2);
        assert!(p
            .render(&x, &dummy_cast(0.0, 1e12), &default_scene())
            .is_err());
        assert!(p
            .render(&x, &dummy_cast(45.0, 1e300), &default_scene())
            .is_err());
    }

    #[test]
    fn new_fails_when_calibration_is_impossible() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e12] {
            let mut cfg = default_audio_config();
            cfg.reference_dist = bad;
            let err = AudioProcessor::new(cfg)
                .err()
                .unwrap_or_else(|| panic!("{bad}"));
            assert!(
                format!("{err:#}").contains("reference_dist"),
                "{bad}: {err:#}"
            );
        }
    }

    /// 音声風(4Hz で振幅が揺れる帯域制限ノイズ)。較正信号とは別の種を使う。
    fn speech_like(seed: u64) -> Vec<f32> {
        let mut x = crate::calib::calibration_signal(seed);
        for (k, s) in x.iter_mut().enumerate() {
            let t = k as f32 / 48_000.0;
            *s *= (0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin()).powi(2) * 1.6;
        }
        x
    }

    /// 残響あり/なし(同じ早期反射設定)の出力ラウドネス差(LU)。
    fn reverb_loudness_diff(
        p: &AudioProcessor,
        input: &[f32],
        cast: &Cast,
        scene: &SceneConfig,
    ) -> f64 {
        use crate::loudness::{measure_stereo, Loudness};
        let dry_scene = SceneConfig {
            reverb_wet: Some(0.0),
            ..scene.clone()
        };
        let wet = p.render(input, cast, scene).unwrap();
        let dry = p.render(input, cast, &dry_scene).unwrap();
        match (measure_stereo(&wet), measure_stereo(&dry)) {
            (Loudness::Lufs(a), Loudness::Lufs(b)) => a - b,
            _ => panic!("測定できませんでした"),
        }
    }

    /// 既知の限界(Human 承認済みの残差): 早期反射が支配的な条件では、補償の基準(素の dual-mono)が
    /// 早期反射の左右相関を捉えられず、最大約 0.5 LU の誤差が残る。
    /// - 早期反射を極端に強めた(early_level 2.5 = 既定の2.5倍)小部屋
    /// - 距離 10 m(既定の小さい部屋では話者が壁に押し込まれ、直接音より早期反射が支配的になる。
    ///   早期反射を切ると距離 10〜30 m でも 0.1 LU 未満)
    ///
    /// 通常のテストからは外し、無視されたテストで 0.6 LU 未満であることだけ確認する。
    const KNOWN_LIMIT: [&str; 2] = ["small strong-early", "dist10"];

    /// 残響あり/なしのラウドネス差の表を作り、許容(`limit` LU)を超えた条件を返す。
    /// `known_limit` が true なら既知の限界の条件だけ、false ならそれ以外だけを調べる。
    fn lu_table_failures(known_limit: bool, limit: f64) -> Vec<String> {
        let base = s2v_core::Config::builtin().audio;
        let mut strong = base.clone();
        strong.early_reflections.early_level = 2.5;
        let p_base = AudioProcessor::new(base).unwrap();
        let p_strong = AudioProcessor::new(strong).unwrap();
        let wet = |w: f64| SceneConfig {
            reverb_wet: Some(w),
            ..SceneConfig::new("w")
        };
        let room = |w: f64, d: f64, h: f64, rw: f64| SceneConfig {
            room_w: Some(w),
            room_d: Some(d),
            room_h: Some(h),
            reverb_wet: Some(rw),
            ..SceneConfig::new("r")
        };
        let with_height = |pan: f64, dist: f64, h: f64| {
            let mut c = dummy_cast(pan, dist);
            c.height = Some(h);
            c
        };
        // (名前, 早期反射を強くしたプロセッサか, 場面, 台詞)
        let cases: Vec<(&str, bool, SceneConfig, Cast)> = vec![
            (
                "default",
                false,
                SceneConfig::new("d"),
                dummy_cast(0.0, 1.0),
            ),
            ("wet0.3", false, wet(0.3), dummy_cast(0.0, 1.0)),
            ("wet1.0", false, wet(1.0), dummy_cast(0.0, 1.0)),
            (
                "big",
                false,
                room(25.0, 45.0, 18.0, 1.0),
                dummy_cast(0.0, 1.0),
            ),
            ("dist0.5", false, wet(1.0), dummy_cast(0.0, 0.5)),
            ("dist3", false, wet(1.0), dummy_cast(0.0, 3.0)),
            ("dist10", false, wet(1.0), dummy_cast(0.0, 10.0)),
            ("dist20", false, wet(1.0), dummy_cast(0.0, 20.0)),
            ("dist30", false, wet(1.0), dummy_cast(0.0, 30.0)),
            ("pan+60", false, wet(1.0), dummy_cast(60.0, 1.0)),
            ("pan-60", false, wet(1.0), dummy_cast(-60.0, 1.0)),
            ("pan+60 wet0.3", false, wet(0.3), dummy_cast(60.0, 3.0)),
            (
                "small strong-early",
                true,
                room(3.0, 4.0, 2.5, 1.0),
                dummy_cast(0.0, 1.0),
            ),
            (
                "small strong-early pan-60",
                true,
                room(3.0, 4.0, 2.5, 0.3),
                dummy_cast(-60.0, 0.5),
            ),
            (
                "height0.3",
                false,
                room(8.0, 8.0, 5.0, 1.0),
                with_height(0.0, 2.0, 0.3),
            ),
            (
                "height2.0",
                false,
                room(8.0, 8.0, 5.0, 1.0),
                with_height(0.0, 2.0, 2.0),
            ),
        ];
        let mut failures = Vec::new();
        eprintln!("LU TABLE (reverb - no reverb)");
        for seed in [7u64, 8] {
            let input = speech_like(seed);
            for (name, strong_early, scene, cast) in &cases {
                if KNOWN_LIMIT.iter().any(|k| name == k) != known_limit {
                    continue;
                }
                let p = if *strong_early { &p_strong } else { &p_base };
                let diff = reverb_loudness_diff(p, &input, cast, scene);
                eprintln!("LOUDNESS seed {seed} {name}: {diff:.3}");
                if diff.abs() >= limit {
                    failures.push(format!("seed {seed} {name}: {diff}"));
                }
            }
        }
        failures
    }

    #[test]
    fn reverb_scenes_keep_the_reference_loudness() {
        let failures = lu_table_failures(false, 0.5);
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn air_absorption_aware_compensation_holds_at_long_distances_without_early_reflections() {
        // 距離ごとの空気吸収を補償の測定に含めているため、早期反射が無ければ遠距離でも ±0.5 LU に入る
        let mut cfg = s2v_core::Config::builtin().audio;
        cfg.early_reflections.enabled = false;
        let p = AudioProcessor::new(cfg).unwrap();
        let scene = SceneConfig {
            reverb_wet: Some(1.0),
            ..SceneConfig::new("w")
        };
        for seed in [7u64, 8] {
            let input = speech_like(seed);
            for d in [10.0, 20.0, 30.0] {
                let diff = reverb_loudness_diff(&p, &input, &dummy_cast(0.0, d), &scene);
                eprintln!("LOUDNESS no-early seed {seed} dist{d}: {diff:.3}");
                assert!(diff.abs() < 0.5, "seed {seed} dist {d}: {diff}");
            }
        }
    }

    #[test]
    #[ignore = "既知の限界(Human 承認済み): 早期反射が支配的な条件(極端に強い早期反射の小部屋、距離10m)では最大約 0.5 LU の誤差が残る"]
    fn reverb_scenes_known_limit_stays_below_0_6_lu() {
        let failures = lu_table_failures(true, 0.6);
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn reverb_comp_gain_is_above_one_for_strong_wet_and_not_applied_when_reverb_off() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let strong = p.reverb_comp_gain(1.0, 480, 0.9, 1.0).unwrap();
        let weak = p.reverb_comp_gain(1.0, 480, 0.1, 1.0).unwrap();
        assert!(strong > 1.0, "{strong}");
        assert!(strong > weak, "strong={strong} weak={weak}");
        // 残響なしでは補償を計算も参照もしない(キャッシュに入らない)
        let before = p.reverb_comp_len();
        let x = tone(2_000, 0.2);
        let off = SceneConfig {
            reverb_wet: Some(0.0),
            ..default_scene()
        };
        p.render(&x, &dummy_cast(0.0, 1.0), &off).unwrap();
        assert_eq!(p.reverb_comp_len(), before);
    }

    #[test]
    fn reverb_comp_gain_is_cached_and_depends_on_parameters() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        assert_eq!(p.reverb_comp_len(), 0);
        let a = p.reverb_comp_gain(0.8, 240, 0.5, 1.0).unwrap();
        assert_eq!(p.reverb_comp_len(), 1);
        let a2 = p.reverb_comp_gain(0.8, 240, 0.5, 1.0).unwrap();
        // 4桁に丸めた同じキーは同じゲイン(再計算しない)
        let a3 = p.reverb_comp_gain(0.800_001, 240, 0.500_001, 1.1).unwrap();
        assert_eq!(p.reverb_comp_len(), 1);
        assert_eq!(a, a2);
        assert_eq!(a, a3);
        let b = p.reverb_comp_gain(0.8, 240, 0.9, 1.0).unwrap();
        let c = p.reverb_comp_gain(2.0, 240, 0.5, 1.0).unwrap();
        let d = p.reverb_comp_gain(0.8, 480, 0.5, 1.0).unwrap();
        // 距離は 0.5 m 刻み: 10.0 と 10.2 は同じキー、1.0 とは別キー
        let e = p.reverb_comp_gain(0.8, 240, 0.5, 10.0).unwrap();
        let e2 = p.reverb_comp_gain(0.8, 240, 0.5, 10.2).unwrap();
        assert_eq!(e, e2);
        assert_ne!(a, e, "距離が違えば空気吸収が違い、補償も変わる");
        assert_eq!(p.reverb_comp_len(), 5);
        assert!(a != b && a != c);
        assert!(d.is_finite() && d > 0.0);
    }

    #[test]
    fn new_rejects_non_finite_or_invalid_audio_config() {
        type Mutate = fn(&mut AudioConfig);
        let cases: Vec<(&str, Mutate)> = vec![
            ("reverb_wet NaN", |c| c.reverb_wet = f64::NAN),
            ("reverb_wet 負", |c| c.reverb_wet = -0.1),
            ("wet_distance_slope NaN", |c| {
                c.early_reflections.wet_distance_slope = f64::NAN
            }),
            ("wet_distance_slope inf", |c| {
                c.early_reflections.wet_distance_slope = f64::INFINITY
            }),
            ("room_size NaN", |c| c.room_size = f64::NAN),
            ("room_size 範囲外", |c| c.room_size = 1.5),
            ("mic_directivity NaN", |c| c.mic_directivity = f64::NAN),
            ("mic_directivity 範囲外", |c| c.mic_directivity = 2.0),
            ("air_absorption_coeff NaN", |c| {
                c.air_absorption_coeff = f64::NAN
            }),
            ("mic_angle NaN", |c| c.mic_angle = f64::NAN),
            ("ear_height NaN", |c| {
                c.early_reflections.ear_height = f64::NAN
            }),
            ("early_level NaN", |c| {
                c.early_reflections.early_level = f64::NAN
            }),
            ("floor 反射率 NaN", |c| {
                c.early_reflections.floor.reflection_coeff = f64::NAN
            }),
            ("side 遮断周波数 inf", |c| {
                c.early_reflections.side_walls.absorption_cutoff_hz = f64::INFINITY
            }),
            ("listener_offset NaN", |c| {
                c.early_reflections.listener_offset = [0.0, f64::NAN]
            }),
            ("room_dims_min NaN", |c| {
                c.early_reflections.room_dims_min = [4.0, f64::NAN, 3.0]
            }),
            ("room_dims_max inf", |c| {
                c.early_reflections.room_dims_max = [f64::INFINITY, 5.0, 3.0]
            }),
        ];
        for (name, m) in cases {
            let mut cfg = default_audio_config();
            m(&mut cfg);
            assert!(AudioProcessor::new(cfg).is_err(), "{name}");
        }
    }

    #[test]
    fn render_rejects_non_finite_or_negative_reverb_parameters() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = tone(2_000, 0.2);
        let cast = dummy_cast(0.0, 1.0);
        for v in [f64::NAN, f64::INFINITY, -0.5] {
            let scene = SceneConfig {
                reverb_wet: Some(v),
                ..default_scene()
            };
            let out = p.render(&x, &cast, &scene);
            assert!(out.is_err(), "scene reverb_wet {v}");
        }
        let scene = SceneConfig {
            room_size: Some(f64::NAN),
            ..default_scene()
        };
        assert!(p.render(&x, &cast, &scene).is_err());
        let mut c = dummy_cast(0.0, 1.0);
        c.params
            .insert("reverb_wet".to_string(), serde_json::json!(-1.0));
        assert!(p.render(&x, &c, &default_scene()).is_err());
    }

    #[test]
    fn render_returns_err_instead_of_panicking_when_the_ir_cache_is_poisoned() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        p.ir_cache.poison_for_test();
        let r = p.render(&tone(2_000, 0.2), &dummy_cast(0.0, 1.0), &reverb_scene());
        assert!(r.is_err());
        assert!(p.prewarm_reverb(&[(0.5, 100)]).is_err());
    }

    #[test]
    fn render_returns_err_instead_of_panicking_when_the_comp_cache_is_poisoned() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                let _g = p.reverb_comp.lock().unwrap();
                panic!("poison the compensation cache");
            });
            assert!(h.join().is_err());
        });
        let r = p.render(&tone(2_000, 0.2), &dummy_cast(0.0, 1.0), &reverb_scene());
        assert!(r.is_err());
    }

    #[test]
    fn concurrent_renders_compute_the_compensation_once() {
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = noise(2_000, 0.2);
        let outs: Vec<Vec<[f32; 2]>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6)
                .map(|_| {
                    s.spawn(|| {
                        p.render(&x, &dummy_cast(10.0, 1.3), &reverb_scene())
                            .unwrap()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(p.comp_computations(), 1);
        assert_eq!(p.reverb_comp_len(), 1);
        for o in &outs[1..] {
            assert_eq!(o, &outs[0]);
        }
    }

    #[test]
    fn actual_wet_is_quantized_and_shared_by_mix_and_compensation() {
        use crate::reverb::actual_wet;
        // 同じ 0.01 の刻みなら同じ値
        assert_eq!(
            actual_wet(0.5021, 1.0, 0.0, 0.0),
            actual_wet(0.5049, 1.0, 0.0, 0.0)
        );
        assert_ne!(
            actual_wet(0.5021, 1.0, 0.0, 0.0),
            actual_wet(0.5151, 1.0, 0.0, 0.0)
        );
        assert_eq!(actual_wet(5.0, 1.0, 0.0, 0.0), 0.9);
        // 同じ刻みの2つの wet は、補償ゲインも出力も同一
        let p = AudioProcessor::new(default_audio_config()).unwrap();
        let x = noise(2_000, 0.2);
        let mk = |w: f64| SceneConfig {
            reverb_wet: Some(w),
            ..reverb_scene()
        };
        let a = p.render(&x, &dummy_cast(0.0, 1.0), &mk(0.6000)).unwrap();
        let b = p.render(&x, &dummy_cast(0.0, 1.0), &mk(0.6003)).unwrap();
        assert_eq!(a, b);
        assert_eq!(p.reverb_comp_len(), 1);
    }

    #[test]
    fn many_distances_share_a_bounded_number_of_compensation_entries() {
        use crate::acoustics::{compute_reverb_params, resolve_room_geometry};
        use crate::reverb::actual_wet;
        let cfg = default_audio_config();
        let p = AudioProcessor::new(cfg.clone()).unwrap();
        let scene = reverb_scene(); // reverb_wet 0.8(最悪側)
        let x = noise(2_000, 0.2);
        let geo = resolve_room_geometry(&scene, &cfg.early_reflections, cfg.room_size);
        let rp = compute_reverb_params(geo.dims, &cfg.early_reflections, cfg.sound_speed, 48_000);
        let mut distinct = std::collections::BTreeSet::new();
        let t = std::time::Instant::now();
        for i in 0..40 {
            let d = 0.5 + (20.0 - 0.5) * i as f64 / 39.0;
            let w = actual_wet(
                0.8,
                rp.wet_base,
                d,
                cfg.early_reflections.wet_distance_slope,
            );
            distinct.insert(((w * 100.0).round() as u32, (d / 0.5).round() as u32));
            p.render(&x, &dummy_cast(0.0, d), &scene).unwrap();
        }
        eprintln!(
            "40 distances: {:?}, entries={}, distinct={}",
            t.elapsed(),
            p.reverb_comp_len(),
            distinct.len()
        );
        assert_eq!(p.reverb_comp_len(), distinct.len());
        assert!(distinct.len() <= 40);
    }
}
