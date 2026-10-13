use std::path::Path;

use s2v_core::{AudioConfig, Cast, SceneConfig};

use crate::acoustics::{compute_reverb_params, resolve_room_geometry, ReverbParams, RoomGeometry};
use crate::early::build_early_taps;
use crate::geometry::{calc_geometry, directivity_pattern};
use crate::resampler::resample_mono;
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
}

impl AudioProcessor {
    pub fn new(config: AudioConfig) -> Self {
        let sample_rate = config.sample_rate;
        Self {
            config,
            ir_cache: IrCache::new(sample_rate),
        }
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

    /// WAV ファイルを読み込み、DSP 処理を施して stereo WAV として書き出す。
    /// 戻り値: 出力サンプル数（失敗時は Err）
    pub fn process(
        &self,
        input: &Path,
        output: &Path,
        cast: &Cast,
        scene: &SceneConfig,
    ) -> anyhow::Result<usize> {
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

        // --- WAV 読み込み ---
        let mut reader = hound::WavReader::open(input)?;
        let spec = reader.spec();
        let samples_raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap() as f32 / max)
                    .collect()
            }
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        };

        // チャンネル 1 へ変換 (ステレオの場合は L ch のみ使用)
        let mono: Vec<f32> = if spec.channels == 1 {
            samples_raw
        } else {
            samples_raw
                .into_iter()
                .step_by(spec.channels as usize)
                .collect()
        };

        // ピーク正規化
        let peak = mono.iter().cloned().map(f32::abs).fold(0.0_f32, f32::max);
        let mono: Vec<f32> = if peak > 0.0 {
            mono.into_iter().map(|s| s / peak).collect()
        } else {
            mono
        };

        // リサンプリング
        let mono = resample_mono(&mono, spec.sample_rate, self.config.sample_rate)?;

        // 空の音声(0サンプル)は、この先のapply_air_absorption等がout[0]等で先頭要素に
        // 直接アクセスするためワーカーパニックになり得る。ここで早期にErrへ変換する。
        if mono.is_empty() {
            anyhow::bail!("音声データが空です(0サンプル): {}", input.display());
        }

        // --- 幾何学計算 ---
        let pan_rad = cast.pan.to_radians();
        let geo = calc_geometry(self.config.microphone_spacing, cast.distance, pan_rad);

        // 空気吸収フィルター
        let data_l = apply_air_absorption(
            &mono,
            geo.dist_l,
            self.config.sample_rate,
            self.config.air_absorption_coeff,
        );
        let data_r = apply_air_absorption(
            &mono,
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

        // ILD + 基準音圧ゲイン
        let mic_angle_rad = self.config.mic_angle.to_radians();
        let k = self.config.mic_directivity;
        let nominal_pat = (1.0 - k) + k * (0.0_f64 - mic_angle_rad).cos();
        let max_gain_linear = 10.0_f64.powf(self.config.max_gain_db / 20.0);
        let base_norm = max_gain_linear / nominal_pat.max(1e-6);

        let engine_vol = self
            .config
            .engine_volume_offsets
            .get(&cast.engine_type)
            .copied()
            .unwrap_or(1.0);
        // Python版 (core/audio_processor.py) は config.REFERENCE_GAIN_DB を
        // 定義しているがゲイン計算には使用していない (未使用の設定値)。
        // 移植時に誤って乗算していたため、Python版に合わせて除外する。
        let vol_factor = base_norm * cast.volume * engine_vol;

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
            &mono,
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

        // リミッター
        let peak_out = stereo
            .iter()
            .flat_map(|s| s.iter())
            .cloned()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        if peak_out > max_gain_linear as f32 {
            let scale = max_gain_linear as f32 / peak_out;
            stereo.iter_mut().for_each(|s| {
                s[0] *= scale;
                s[1] *= scale;
            });
        }

        // --- WAV 書き出し ---
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let out_spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.config.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(output, out_spec)?;
        for frame in &stereo {
            writer.write_sample((frame[0] * 32767.0) as i16)?;
            writer.write_sample((frame[1] * 32767.0) as i16)?;
        }
        writer.finalize()?;
        Ok(stereo.len())
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
            reference_gain_db: -5.0,
            max_gain_db: -1.0,
            mic_directivity: 0.5,
            mic_angle: 45.0,
            engine_volume_offsets: {
                let mut m = HashMap::new();
                m.insert("voicevox".to_string(), 1.0);
                m
            },
            early_reflections: s2v_core::EarlyConfig::default(),
        }
    }

    fn dummy_cast(pan: f64, distance: f64) -> Cast {
        Cast {
            name: "テスト".to_string(),
            speaker_name: "ずんだもん".to_string(),
            engine_type: "voicevox".to_string(),
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

    fn write_test_wav(path: &Path, sample_rate: u32, freq: f32, duration_s: f32) {
        let n = (sample_rate as f32 * duration_s) as usize;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..n {
            let v = (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin();
            writer.write_sample((v * 32767.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// 決定的な広帯域ノイズ WAV を書き出す（テスト再現性のため固定シードLCG）。
    fn write_noise_wav(path: &Path, sample_rate: u32, duration_s: f32) {
        let n = (sample_rate as f32 * duration_s) as usize;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let mut state: u32 = 0x12345678;
        for _ in 0..n {
            // 線形合同法で [-0.5, 0.5) の擬似乱数
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = (state >> 8) as f32 / 16_777_216.0 - 0.5;
            writer.write_sample((v * 32767.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn process_returns_error_instead_of_panicking_on_empty_wav() {
        // エンジンが空の音声(0サンプル)を返した場合、apply_air_absorptionのout[0]参照で
        // ワーカーがパニックしていた(review.txt指摘)。Errとして上位に伝わるべき。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("empty.wav");
        write_test_wav(&input, 48000, 440.0, 0.0); // duration_s=0 → 0サンプル
        let output = dir.path().join("out.wav");

        let proc = AudioProcessor::new(default_audio_config());
        let result = proc.process(&input, &output, &dummy_cast(0.0, 1.0), &default_scene());

        assert!(
            result.is_err(),
            "空の音声データはErrにするべき(パニックしてはいけない)"
        );
    }

    /// hound::WavWriter は channels=0 の書き出しをサポートしない(finalize時にパニックする)ため、
    /// 壊れたWAVヘッダーを手組みしてchannels=0のファイルを直接作る。
    fn write_raw_wav_header(path: &Path, channels: u16, sample_rate: u32, bits_per_sample: u16) {
        let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8).max(1);
        let block_align = channels.wrapping_mul(bits_per_sample / 8);
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&36u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(path, buf).unwrap();
    }

    #[test]
    fn process_returns_error_instead_of_panicking_on_zero_channel_wav() {
        // channels=0 の壊れたWAVは、mono変換の step_by(0) で即パニックしていた。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("zero_channel.wav");
        write_raw_wav_header(&input, 0, 48000, 16);
        let output = dir.path().join("out.wav");

        let proc = AudioProcessor::new(default_audio_config());
        let result = proc.process(&input, &output, &dummy_cast(0.0, 1.0), &default_scene());

        assert!(
            result.is_err(),
            "channels=0のWAVはErrにするべき(パニックしてはいけない)"
        );
    }

    #[test]
    fn process_returns_error_instead_of_panicking_on_zero_bit_depth_wav() {
        // bits_per_sample=0 の壊れたWAVは、`1i64 << (bits_per_sample - 1)` の減算で
        // オーバーフローパニックし得る。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("zero_bits.wav");
        write_raw_wav_header(&input, 1, 48000, 0);
        let output = dir.path().join("out.wav");

        let proc = AudioProcessor::new(default_audio_config());
        let result = proc.process(&input, &output, &dummy_cast(0.0, 1.0), &default_scene());

        assert!(
            result.is_err(),
            "bits_per_sample=0のWAVはErrにするべき(パニックしてはいけない)"
        );
    }

    #[test]
    fn reference_gain_db_does_not_affect_output_level() {
        // Python版 (core/audio_processor.py) は config.REFERENCE_GAIN_DB を
        // 定義しているが、ゲイン計算には一切使用していない（未使用の設定値）。
        // Rust版が独自に reference_gain_db を音量係数へ乗じてしまうと、
        // Python版に対して出力音量が変化してしまう（移植バグ）。
        // よって reference_gain_db を変えても出力レベルは変わらないはず。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_test_wav(&input, 48000, 440.0, 0.1);

        let mut cfg_a = default_audio_config();
        cfg_a.reference_gain_db = -5.0;
        let mut cfg_b = default_audio_config();
        cfg_b.reference_gain_db = -20.0;

        let peak_for = |cfg: AudioConfig| -> f32 {
            let out = dir
                .path()
                .join(format!("out_{}.wav", cfg.reference_gain_db));
            let proc = AudioProcessor::new(cfg);
            proc.process(&input, &out, &dummy_cast(0.0, 1.0), &default_scene())
                .unwrap();
            let mut r = hound::WavReader::open(&out).unwrap();
            r.samples::<i16>()
                .map(|s| (s.unwrap() as f32).abs())
                .fold(0.0_f32, f32::max)
        };

        let peak_a = peak_for(cfg_a);
        let peak_b = peak_for(cfg_b);

        assert!(
            (peak_a - peak_b).abs() / peak_a.max(1.0) < 0.01,
            "reference_gain_db should not change output level (Python版に未使用): peak_a={peak_a}, peak_b={peak_b}"
        );
    }

    #[test]
    fn process_creates_stereo_wav() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        let output = dir.path().join("out.wav");

        write_test_wav(&input, 24000, 440.0, 0.1);

        let proc = AudioProcessor::new(default_audio_config());
        let n = proc
            .process(&input, &output, &dummy_cast(0.0, 1.0), &default_scene())
            .unwrap();
        assert!(n > 0);
        assert!(output.exists());

        let reader = hound::WavReader::open(&output).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48000);
    }

    #[test]
    fn process_panning_creates_stereo_asymmetry() {
        // 外開き(ORTF的)マイク配置: Lマイクは-45°(左向き)、Rマイクは+45°(右向き)。
        // pan は + で右、- で左から聞こえる仕様 (台本仕様.txt) なので、
        // 左パン(pan=-45)では音源がLマイクに近く・正面に来てLchが大きくなり、
        // 右パン(pan=+45)では音源がRマイクに近く・正面に来てRchが大きくなる。
        // (距離由来のITD/ILDと指向性パターン由来のILDが同じ方向を強め合う)
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_test_wav(&input, 48000, 440.0, 0.2);

        let proc = AudioProcessor::new(default_audio_config());

        let read_rms = |p: &Path, ch: usize| -> f64 {
            let mut r = hound::WavReader::open(p).unwrap();
            let samples: Vec<i16> = r.samples().map(|s| s.unwrap()).collect();
            let ch_samples: Vec<f64> = samples
                .iter()
                .skip(ch)
                .step_by(2)
                .map(|&s| s as f64)
                .collect();
            let sum_sq: f64 = ch_samples.iter().map(|s| s * s).sum();
            (sum_sq / ch_samples.len() as f64).sqrt()
        };

        // 中央: L≈R
        let out_center = dir.path().join("center.wav");
        proc.process(&input, &out_center, &dummy_cast(0.0, 1.0), &default_scene())
            .unwrap();
        let l_center = read_rms(&out_center, 0);
        let r_center = read_rms(&out_center, 1);
        let center_ratio = (l_center - r_center).abs() / (l_center + r_center + 1.0);
        assert!(
            center_ratio < 0.1,
            "center should be approximately symmetric, got ratio={center_ratio:.3}"
        );

        // 左パン(pan=-45): 音源は左側 → Lchが大きくなるはず (台本仕様.txt: -=左)
        let out_left = dir.path().join("left.wav");
        proc.process(&input, &out_left, &dummy_cast(-45.0, 1.0), &default_scene())
            .unwrap();
        let l_left = read_rms(&out_left, 0);
        let r_left = read_rms(&out_left, 1);
        let left_ratio = (l_left - r_left).abs() / (l_left + r_left + 1.0);
        assert!(
            left_ratio > 0.05,
            "left pan should create stereo asymmetry, got ratio={left_ratio:.3}"
        );
        assert!(
            l_left > r_left,
            "pan=-45 (left) should be louder in the L channel, got L={l_left:.1} R={r_left:.1}"
        );

        // 右パン(pan=+45): 音源は右側 → Rchが大きくなるはず (台本仕様.txt: +=右)
        let out_right = dir.path().join("right.wav");
        proc.process(&input, &out_right, &dummy_cast(45.0, 1.0), &default_scene())
            .unwrap();
        let l_right = read_rms(&out_right, 0);
        let r_right = read_rms(&out_right, 1);
        let right_ratio = (l_right - r_right).abs() / (l_right + r_right + 1.0);
        assert!(
            right_ratio > 0.05,
            "right pan should create stereo asymmetry, got ratio={right_ratio:.3}"
        );
        assert!(
            r_right > l_right,
            "pan=+45 (right) should be louder in the R channel, got L={l_right:.1} R={r_right:.1}"
        );
    }

    #[test]
    fn prewarm_then_process_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        let output = dir.path().join("out.wav");
        write_test_wav(&input, 48000, 880.0, 0.1);

        let proc = AudioProcessor::new(default_audio_config());
        let scene = default_scene();
        let params: Vec<(f64, usize)> = [0.1_f64, 0.3, 0.8]
            .iter()
            .map(|&rs| proc.reverb_params_for(&scene, rs))
            .collect();
        proc.prewarm_reverb(&params);
        let n = proc
            .process(&input, &output, &dummy_cast(30.0, 2.0), &scene)
            .unwrap();
        assert!(n > 0);
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
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_test_wav(&input, 48000, 440.0, 0.1);

        let mut cfg = default_audio_config();
        cfg.early_reflections.enabled = false;
        cfg.reverb_wet = 0.0;
        let proc = AudioProcessor::new(cfg);
        let out = dir.path().join("out.wav");
        proc.process(
            &input,
            &out,
            &dummy_cast(20.0, 2.0),
            &SceneConfig {
                room_size: Some(0.1),
                reverb_wet: Some(0.0),
                ..SceneConfig::new("s")
            },
        )
        .unwrap();

        let mut r = hound::WavReader::open(&out).unwrap();
        let energy: f64 = r
            .samples::<i16>()
            .map(|s| {
                let v = s.unwrap() as f64;
                v * v
            })
            .sum();
        assert!(energy > 0.0, "出力が生成されること");
    }

    #[test]
    fn early_reflections_enabled_adds_energy() {
        // 信号非依存にするため広帯域ノイズを使う。純音だと早期反射の遅延と波長の
        // 位相関係（コムフィルタ）で総エネルギーが増減し得るが、無相関ノイズに
        // 遅延・減衰コピーを加えると総エネルギーは確実に増える（位相依存しない）。
        // かつて純音で FAIL した cast(20.0, 2.0) でも頑健に成り立つことを示す。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_noise_wav(&input, 48000, 0.1);
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
            let out = dir.path().join(format!("out_{enabled}.wav"));
            proc.process(&input, &out, &dummy_cast(20.0, 2.0), &scene)
                .unwrap();
            let mut r = hound::WavReader::open(&out).unwrap();
            r.samples::<i16>()
                .map(|s| {
                    let v = s.unwrap() as f64;
                    v * v
                })
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
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_noise_wav(&input, 48000, 0.1);
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
        let n = proc
            .process(
                &input,
                &dir.path().join("outdoor.wav"),
                &dummy_cast(0.0, 1.0),
                &scene,
            )
            .unwrap();
        assert!(n > 0, "屋外 scene でも処理が成功し出力が生成されること");
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
        // process() が scene 寸法を実際に残響へ反映していることを end-to-end で検証する。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_noise_wav(&input, 48000, 0.1);
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
            .process(
                &input,
                &dir.path().join("small.wav"),
                &dummy_cast(0.0, 1.0),
                &small,
            )
            .unwrap();
        let n_big = proc
            .process(
                &input,
                &dir.path().join("big.wav"),
                &dummy_cast(0.0, 1.0),
                &big,
            )
            .unwrap();

        assert!(
            n_big > n_small,
            "大部屋は残響テールが長く出力サンプル数が多い: small={n_small}, big={n_big}"
        );
    }

    #[test]
    fn speaker_height_changes_process_output() {
        // 話者高さを変えると早期反射の床反射が変わり、出力(早期反射ON)が変化する
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_noise_wav(&input, 48000, 0.1);
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
        let out_low = dir.path().join("low.wav");
        let out_high = dir.path().join("high.wav");
        proc.process(&input, &out_low, &low, &scene).unwrap();
        proc.process(&input, &out_high, &high, &scene).unwrap();

        let read = |p: &std::path::Path| -> Vec<i16> {
            let mut r = hound::WavReader::open(p).unwrap();
            r.samples::<i16>().map(|s| s.unwrap()).collect()
        };
        assert_ne!(
            read(&out_low),
            read(&out_high),
            "話者高さで出力が変わること"
        );
    }

    #[test]
    fn speaker_height_base_plus_offset_equals_absolute() {
        // 実効高さ = cast.height + height_offset。基準1.5+行内0.3 は 絶対1.8(+0) と同一出力になること。
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        write_noise_wav(&input, 48000, 0.1);
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

        let out_c = dir.path().join("combined.wav");
        let out_a = dir.path().join("absolute.wav");
        proc.process(&input, &out_c, &combined, &scene).unwrap();
        proc.process(&input, &out_a, &absolute, &scene).unwrap();

        let read = |p: &std::path::Path| -> Vec<i16> {
            let mut r = hound::WavReader::open(p).unwrap();
            r.samples::<i16>().map(|s| s.unwrap()).collect()
        };
        assert_eq!(
            read(&out_c),
            read(&out_a),
            "基準+行内オフセットの合算が絶対指定と一致すること"
        );
    }
}
