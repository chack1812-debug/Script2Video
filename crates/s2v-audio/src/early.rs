//! 部屋を箱としたイメージソース法による一次反射タップの生成。

use s2v_core::{AudioConfig, EarlyConfig, MaterialConfig};

use crate::acoustics::RoomGeometry;
use crate::geometry::{calc_geometry, directivity_pattern, image_position, Surface};
use crate::processor::apply_air_absorption;
use crate::reverb::{butterworth_lowpass_sos, sosfilt_single_section};

/// ステレオバッファへ加算する1つの反射タップ。
/// `sig` は素材ローパス済みの信号(長さは入力 mono と同じ)。
pub struct EarlyTap {
    pub sig: Vec<f32>,
    pub rel_l: usize,
    pub rel_r: usize,
    pub gain_l: f32,
    pub gain_r: f32,
}

/// 6面の一次反射タップを生成する。`min_delay_direct` は直接音の最早到達サンプル
/// (processor が time-zero とする値)で、各タップの相対遅延の基準にする。
pub fn build_early_taps(
    mono: &[f32],
    distance: f64,
    pan_rad: f64,
    vol_factor: f64,
    audio: &AudioConfig,
    er: &EarlyConfig,
    geo: &RoomGeometry,
    source_height: f64,
    sample_rate: u32,
    min_delay_direct: usize,
) -> Vec<EarlyTap> {
    if !er.enabled {
        return Vec::new();
    }
    let dims = geo.dims;
    let [w, d, h] = dims;
    let eps = 0.05_f64;

    // 聴取者(マイクペア中心) L と音源 S を箱座標で配置。
    // lz = 聴取者の床面からの高さ(scene listener_z または config ear_height)、
    // sz = 話者の実効高さ(source_height 引数、processor で解決済み)。両者は異なり得る。
    let lx = (w / 2.0 + geo.listener_offset[0]).clamp(eps, w - eps);
    let ly = (d / 2.0 + geo.listener_offset[1]).clamp(eps, d - eps);
    let lz = geo.listener_height.clamp(eps, h - eps);
    let sx = (lx + distance * pan_rad.sin()).clamp(eps, w - eps);
    let sy = (ly + distance * pan_rad.cos()).clamp(eps, d - eps);
    let sz = source_height.clamp(eps, h - eps);
    let src = [sx, sy, sz];

    let surfaces = [
        (Surface::Floor, &er.floor),
        (Surface::Ceiling, &er.ceiling),
        (Surface::LeftWall, &er.side_walls),
        (Surface::RightWall, &er.side_walls),
        (Surface::BackWall, &er.back_wall),
        (Surface::FrontWall, &er.front_wall),
    ];

    let c = audio.sound_speed;
    let fs = sample_rate as f64;
    let k = audio.mic_directivity;
    let mic_angle_rad = audio.mic_angle.to_radians();

    let mut taps = Vec::new();
    for (surface, mat) in surfaces {
        if mat.reflection_coeff <= 0.0 {
            continue;
        }
        let img = image_position(src, surface, dims);
        // 聴取者 L 基準のベクトル
        let dx = img[0] - lx;
        let dy = img[1] - ly;
        let dz = img[2] - lz;
        let hdist = (dx * dx + dy * dy).sqrt();
        let azimuth = dx.atan2(dy);
        let mic_geo = calc_geometry(audio.microphone_spacing, hdist, azimuth);

        // 各マイクへの 3D 経路(高さ迂回 dz を加味)
        let path_l = (mic_geo.dist_l.powi(2) + dz * dz).sqrt();
        let path_r = (mic_geo.dist_r.powi(2) + dz * dz).sqrt();

        // 指向性パターン(外開きORTF: Lは+mic_angle, Rは-mic_angle)
        let pat_l = directivity_pattern(mic_geo.angle_l, k, mic_angle_rad);
        let pat_r = directivity_pattern(mic_geo.angle_r, k, -mic_angle_rad);

        let coeff = mat.reflection_coeff * er.early_level;
        let gain_l = (vol_factor * (audio.reference_dist / path_l.max(0.1)) * pat_l * coeff) as f32;
        let gain_r = (vol_factor * (audio.reference_dist / path_r.max(0.1)) * pat_r * coeff) as f32;

        // 相対遅延(直接音 time-zero 基準)。負にならないよう飽和。
        let delay_l = (path_l / c * fs) as i64 - min_delay_direct as i64;
        let delay_r = (path_r / c * fs) as i64 - min_delay_direct as i64;
        let rel_l = delay_l.max(0) as usize;
        let rel_r = delay_r.max(0) as usize;

        // 信号: 空気吸収(平均経路) → 素材ローパス
        let avg_path = (path_l + path_r) / 2.0;
        let absorbed =
            apply_air_absorption(mono, avg_path, sample_rate, audio.air_absorption_coeff);
        let sig = material_lowpass(&absorbed, mat, fs);

        taps.push(EarlyTap {
            sig,
            rel_l,
            rel_r,
            gain_l,
            gain_r,
        });
    }
    taps
}

/// 素材の吸音カットオフで2次Butterworthローパスをかける(f32入出力)。
fn material_lowpass(samples: &[f32], mat: &MaterialConfig, fs: f64) -> Vec<f32> {
    let nyq = fs / 2.0;
    if mat.absorption_cutoff_hz >= nyq - 1.0 {
        return samples.to_vec();
    }
    let sos = butterworth_lowpass_sos(mat.absorption_cutoff_hz, fs);
    let input: Vec<f64> = samples.iter().map(|&s| s as f64).collect();
    sosfilt_single_section(&sos, &input)
        .iter()
        .map(|&s| s as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn geo_for(room_size: f64, er: &EarlyConfig) -> RoomGeometry {
        RoomGeometry {
            dims: crate::geometry::room_dims(room_size, er.room_dims_min, er.room_dims_max),
            listener_offset: er.listener_offset,
            listener_height: er.ear_height,
        }
    }

    fn audio_cfg() -> AudioConfig {
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
            mic_directivity: 0.2,
            mic_angle: 30.0,
            engine_volume_offsets: HashMap::new(),
            early_reflections: EarlyConfig::default(),
        }
    }

    #[test]
    fn disabled_returns_no_taps() {
        let mut er = EarlyConfig::default();
        er.enabled = false;
        let mono = vec![1.0_f32; 1000];
        let taps = build_early_taps(
            &mono,
            2.0,
            0.0,
            1.0,
            &audio_cfg(),
            &er,
            &geo_for(0.1, &er),
            er.ear_height,
            48000,
            0,
        );
        assert!(taps.is_empty());
    }

    #[test]
    fn only_surfaces_with_positive_coeff_produce_taps() {
        let mut er = EarlyConfig::default();
        er.ceiling.reflection_coeff = 0.0;
        er.front_wall.reflection_coeff = 0.0;
        er.back_wall.reflection_coeff = 0.0;
        er.side_walls.reflection_coeff = 0.0;
        let mono = vec![1.0_f32; 1000];
        let taps = build_early_taps(
            &mono,
            2.0,
            0.0,
            1.0,
            &audio_cfg(),
            &er,
            &geo_for(0.1, &er),
            er.ear_height,
            48000,
            0,
        );
        assert_eq!(taps.len(), 1, "床のみ → 1タップ");
    }

    #[test]
    fn panned_source_produces_left_right_asymmetric_taps() {
        let er = EarlyConfig::default();
        let mono = vec![1.0_f32; 2000];
        // pan=+30度(右)。少なくとも1タップで gain_l != gain_r または rel_l != rel_r になること。
        let taps = build_early_taps(
            &mono,
            2.0,
            30.0_f64.to_radians(),
            1.0,
            &audio_cfg(),
            &er,
            &geo_for(0.1, &er),
            er.ear_height,
            48000,
            0,
        );
        assert!(!taps.is_empty());
        let asym = taps
            .iter()
            .any(|t| (t.gain_l - t.gain_r).abs() > 1e-6 || t.rel_l != t.rel_r);
        assert!(asym, "パンした音源は左右非対称なタップを生むこと");
    }

    #[test]
    fn floor_tap_delay_matches_analytic_value() {
        let mut er = EarlyConfig::default();
        er.ceiling.reflection_coeff = 0.0;
        er.front_wall.reflection_coeff = 0.0;
        er.back_wall.reflection_coeff = 0.0;
        er.side_walls.reflection_coeff = 0.0;
        let mono = vec![1.0_f32; 1000];
        let taps = build_early_taps(
            &mono,
            2.0,
            0.0,
            1.0,
            &audio_cfg(),
            &er,
            &geo_for(0.1, &er),
            er.ear_height,
            48000,
            0,
        );
        let eh = er.ear_height;
        let expected = ((2.0_f64.powi(2) + (2.0 * eh).powi(2)).sqrt() / 340.0 * 48000.0) as i64;
        let rel = taps[0].rel_l as i64;
        assert!(
            (rel - expected).abs() <= 5,
            "床タップ遅延 rel={rel}, expected≈{expected}"
        );
    }

    #[test]
    fn higher_source_increases_floor_reflection_delay() {
        // 床のみ残し、話者を高くすると床反射(像はz=-source_height)の経路が伸びて遅延が増える
        let mut er = EarlyConfig::default();
        er.ceiling.reflection_coeff = 0.0;
        er.front_wall.reflection_coeff = 0.0;
        er.back_wall.reflection_coeff = 0.0;
        er.side_walls.reflection_coeff = 0.0;
        let mono = vec![1.0_f32; 2000];
        let geo = geo_for(0.5, &er);
        let low = build_early_taps(&mono, 2.0, 0.0, 1.0, &audio_cfg(), &er, &geo, 1.2, 48000, 0);
        let high = build_early_taps(&mono, 2.0, 0.0, 1.0, &audio_cfg(), &er, &geo, 2.5, 48000, 0);
        assert_eq!(low.len(), 1);
        assert_eq!(high.len(), 1);
        assert!(
            high[0].rel_l > low[0].rel_l,
            "話者が高いほど床反射が遅い: low={}, high={}",
            low[0].rel_l,
            high[0].rel_l
        );
    }

    #[test]
    fn front_wall_reflection_coeff_scales_tap_gain() {
        // 前壁のみ残し、反射率を上げるとその反射タップのゲインが線形に増える(spec §6)
        let mono = vec![1.0_f32; 1000];
        let front_gain_for = |coeff: f64| -> f32 {
            let mut er = EarlyConfig::default();
            er.floor.reflection_coeff = 0.0;
            er.ceiling.reflection_coeff = 0.0;
            er.back_wall.reflection_coeff = 0.0;
            er.side_walls.reflection_coeff = 0.0;
            er.front_wall.reflection_coeff = coeff;
            let taps = build_early_taps(
                &mono,
                2.0,
                0.0,
                1.0,
                &audio_cfg(),
                &er,
                &geo_for(0.1, &er),
                er.ear_height,
                48000,
                0,
            );
            assert_eq!(taps.len(), 1, "前壁のみ → 1タップ");
            taps[0].gain_l
        };
        assert!(
            front_gain_for(0.85) > front_gain_for(0.5),
            "前壁反射率↑ → タップゲイン↑"
        );
    }

    #[test]
    fn material_lowpass_attenuates_high_frequencies() {
        let fs = 48000.0;
        let n = 4096;
        let hi: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 16000.0 * i as f32 / 48000.0).sin())
            .collect();
        let mat = MaterialConfig {
            reflection_coeff: 1.0,
            absorption_cutoff_hz: 3500.0,
        };
        let out = material_lowpass(&hi, &mat, fs);
        let peak_in = hi.iter().cloned().map(f32::abs).fold(0.0_f32, f32::max);
        let peak_out = out[1000..]
            .iter()
            .cloned()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        assert!(
            peak_out < peak_in * 0.5,
            "16kHzが半分以下に減衰すること: in={peak_in}, out={peak_out}"
        );
    }
}
