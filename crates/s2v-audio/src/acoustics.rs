//! 部屋ジオメトリの解決と、寸法×素材からの物理残響パラメータ(Sabine)算出。

use s2v_core::{EarlyConfig, SceneConfig};

use crate::geometry::room_dims;

/// 解決済みの部屋ジオメトリ。早期反射と拡散リバーブの両方に渡す。
#[derive(Clone, Copy, Debug)]
pub struct RoomGeometry {
    pub dims: [f64; 3],
    pub listener_offset: [f64; 2],
    pub listener_height: f64,
}

/// scene と config から部屋寸法・聴取オフセットを解決する。
/// 注: listener_dx/dy のどちらか一方だけ指定した場合、もう一方は config 値ではなく 0 になる。
pub fn resolve_room_geometry(
    scene: &SceneConfig,
    er: &EarlyConfig,
    fallback_room_size: f64,
) -> RoomGeometry {
    let dims = match (scene.room_w, scene.room_d, scene.room_h) {
        (Some(w), Some(d), Some(h)) => [w, d, h],
        _ => {
            let rs = scene.room_size.unwrap_or(fallback_room_size);
            room_dims(rs, er.room_dims_min, er.room_dims_max)
        }
    };
    let listener_offset = match (scene.listener_dx, scene.listener_dy) {
        (None, None) => er.listener_offset,
        (dx, dy) => [dx.unwrap_or(0.0), dy.unwrap_or(0.0)],
    };
    RoomGeometry {
        dims,
        listener_offset,
        listener_height: scene.listener_z.unwrap_or(er.ear_height),
    }
}

/// 拡散リバーブの物理パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct ReverbParams {
    pub rt60: f64,
    pub pre_delay: usize,
    pub wet_base: f64,
}

/// 寸法×素材(反射率)から Sabine の RT60・平均自由行程プリディレイ・wet基準値を算出する。
pub fn compute_reverb_params(
    dims: [f64; 3],
    er: &EarlyConfig,
    sound_speed: f64,
    sample_rate: u32,
) -> ReverbParams {
    let [w, d, h] = dims;
    let s_floor = w * d;
    let s_ceiling = w * d;
    let s_front = w * h;
    let s_back = w * h;
    let s_side = 2.0 * d * h;
    let total_area = s_floor + s_ceiling + s_front + s_back + s_side;

    // 振幅反射係数 coeff → エネルギー吸音率 α = 1 - coeff^2
    let alpha = |coeff: f64| 1.0 - coeff * coeff;
    let total_absorption = s_floor * alpha(er.floor.reflection_coeff)
        + s_ceiling * alpha(er.ceiling.reflection_coeff)
        + s_front * alpha(er.front_wall.reflection_coeff)
        + s_back * alpha(er.back_wall.reflection_coeff)
        + s_side * alpha(er.side_walls.reflection_coeff);

    let volume = w * d * h;
    let rt60 = (0.161 * volume / total_absorption.max(1e-6)).clamp(0.05, 12.0);

    let mfp = 4.0 * volume / total_area.max(1e-6);
    // プリディレイは知覚上 50ms 超でほぼ等価。大部屋での過大バッファを避けるため上限を設ける。
    let pre_delay_s = (mfp / sound_speed).min(0.05);
    let pre_delay = ((sample_rate as f64) * pre_delay_s) as usize;

    let avg_alpha = total_absorption / total_area.max(1e-6);
    let wet_base = (1.0 - avg_alpha).clamp(0.0, 1.0);

    ReverbParams {
        rt60,
        pre_delay,
        wet_base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::MaterialConfig;

    fn er_uniform(coeff: f64) -> EarlyConfig {
        let mut er = EarlyConfig::default();
        let m = MaterialConfig {
            reflection_coeff: coeff,
            absorption_cutoff_hz: 24000.0,
        };
        er.floor = m.clone();
        er.ceiling = m.clone();
        er.front_wall = m.clone();
        er.back_wall = m.clone();
        er.side_walls = m;
        er
    }

    #[test]
    fn resolve_prefers_scene_room_dims_over_room_size() {
        let er = EarlyConfig::default();
        let scene = SceneConfig {
            room_w: Some(10.0),
            room_d: Some(20.0),
            room_h: Some(5.0),
            room_size: Some(0.0),
            ..SceneConfig::new("x")
        };
        let geo = resolve_room_geometry(&scene, &er, 0.5);
        assert_eq!(geo.dims, [10.0, 20.0, 5.0]);
    }

    #[test]
    fn resolve_falls_back_to_room_size_interpolation() {
        let er = EarlyConfig::default();
        let scene = SceneConfig {
            room_size: Some(0.0),
            ..SceneConfig::new("x")
        };
        let geo = resolve_room_geometry(&scene, &er, 0.5);
        assert_eq!(geo.dims, er.room_dims_min);
    }

    #[test]
    fn resolve_listener_uses_scene_then_config() {
        let mut er = EarlyConfig::default();
        er.listener_offset = [1.0, 2.0];
        let scene_none = SceneConfig::new("x");
        assert_eq!(
            resolve_room_geometry(&scene_none, &er, 0.5).listener_offset,
            [1.0, 2.0]
        );
        let scene_set = SceneConfig {
            listener_dx: Some(-3.0),
            listener_dy: Some(4.0),
            ..SceneConfig::new("x")
        };
        assert_eq!(
            resolve_room_geometry(&scene_set, &er, 0.5).listener_offset,
            [-3.0, 4.0]
        );
    }

    #[test]
    fn resolve_listener_height_uses_scene_then_config() {
        let mut er = EarlyConfig::default();
        er.ear_height = 1.2;
        let scene_none = SceneConfig::new("x");
        assert!((resolve_room_geometry(&scene_none, &er, 0.5).listener_height - 1.2).abs() < 1e-10);
        let scene_set = SceneConfig {
            listener_z: Some(2.0),
            ..SceneConfig::new("x")
        };
        assert!((resolve_room_geometry(&scene_set, &er, 0.5).listener_height - 2.0).abs() < 1e-10);
    }

    #[test]
    fn rt60_matches_sabine_for_known_room() {
        let er = er_uniform(0.7);
        let rp = compute_reverb_params([10.0, 20.0, 5.0], &er, 340.0, 48000);
        let s = 2.0 * (10.0 * 20.0) + 2.0 * (10.0 * 5.0) + 2.0 * (20.0 * 5.0);
        let a = s * (1.0 - 0.7_f64 * 0.7);
        let expected = 0.161 * (10.0 * 20.0 * 5.0) / a;
        assert!(
            (rp.rt60 - expected).abs() < 1e-9,
            "rt60={}, expected={}",
            rp.rt60,
            expected
        );
    }

    #[test]
    fn rt60_clamped_high_when_no_absorption() {
        let er = er_uniform(1.0);
        let rp = compute_reverb_params([10.0, 20.0, 5.0], &er, 340.0, 48000);
        assert!((rp.rt60 - 12.0).abs() < 1e-9);
    }

    #[test]
    fn wet_base_zero_when_fully_absorptive_and_high_when_reflective() {
        let absorptive = compute_reverb_params([10.0, 20.0, 5.0], &er_uniform(0.0), 340.0, 48000);
        let reflective = compute_reverb_params([10.0, 20.0, 5.0], &er_uniform(1.0), 340.0, 48000);
        assert!(
            absorptive.wet_base < 0.01,
            "全面吸音で wet_base≈0, got {}",
            absorptive.wet_base
        );
        assert!(
            reflective.wet_base > 0.99,
            "全面反射で wet_base≈1, got {}",
            reflective.wet_base
        );
    }

    #[test]
    fn rt60_clamped_low_for_very_absorptive_room() {
        // ほぼ全吸音(coeff=0.01)・極小部屋(1×1×1m) → Sabine値が0.05s未満になり下限クランプが効く
        let er = er_uniform(0.01);
        let rp = compute_reverb_params([1.0, 1.0, 1.0], &er, 340.0, 48000);
        assert!(
            (rp.rt60 - 0.05).abs() < 1e-9,
            "rt60 が下限0.05sにクランプされること, got {}",
            rp.rt60
        );
    }

    #[test]
    fn outdoor_walls_zero_gives_short_rt60_and_low_wet() {
        let mut er = er_uniform(0.0);
        er.floor = MaterialConfig {
            reflection_coeff: 0.5,
            absorption_cutoff_hz: 3500.0,
        };
        let rp = compute_reverb_params([20.0, 20.0, 10.0], &er, 340.0, 48000);
        assert!(rp.rt60 < 1.0, "屋外的: rt60 短い, got {}", rp.rt60);
        assert!(
            rp.wet_base < 0.2,
            "屋外的: wet_base 小さい, got {}",
            rp.wet_base
        );
    }
}
