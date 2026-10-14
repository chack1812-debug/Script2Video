//! 直接音・早期反射で共用する幾何計算（純粋関数）。

/// ステレオマイク（中心基準、左右に ±spacing/2）から見た音源の左右距離・角度。
/// processor から移設した既存式と数値的に同一。
pub struct GeoParams {
    pub dist_l: f64,
    pub dist_r: f64,
    pub angle_l: f64,
    pub angle_r: f64,
}

/// 水平面の幾何: 音源を距離 distance・方位 pan_rad(+x=右,+y=前) に置いたときの
/// 左マイク(x=-d_h)・右マイク(x=+d_h)それぞれの距離と方位角。
pub fn calc_geometry(microphone_spacing: f64, distance: f64, pan_rad: f64) -> GeoParams {
    let d_h = microphone_spacing / 2.0;
    let sx = distance * pan_rad.sin();
    let sy = distance * pan_rad.cos();
    let dist_l = ((sx + d_h).powi(2) + sy.powi(2)).sqrt();
    let dist_r = ((sx - d_h).powi(2) + sy.powi(2)).sqrt();
    let angle_l = (sx + d_h).atan2(sy);
    let angle_r = (sx - d_h).atan2(sy);
    GeoParams {
        dist_l,
        dist_r,
        angle_l,
        angle_r,
    }
}

/// room_size(0..1) を部屋寸法 [W,D,H] に線形補間する。
pub fn room_dims(room_size: f64, min: [f64; 3], max: [f64; 3]) -> [f64; 3] {
    let r = room_size.clamp(0.0, 1.0);
    [
        min[0] + (max[0] - min[0]) * r,
        min[1] + (max[1] - min[1]) * r,
        min[2] + (max[2] - min[2]) * r,
    ]
}

/// マイク指向性パターン値。angle は音源方位、mic_angle_offset はマイク軸オフセット
/// （外開きORTF: Lマイクは +mic_angle、Rマイクは -mic_angle を渡す）。k は指向性係数。
/// 既存の直接音・早期反射のパターン式と数値的に同一。
pub fn directivity_pattern(angle: f64, k: f64, mic_angle_offset: f64) -> f64 {
    ((1.0 - k) + k * (angle + mic_angle_offset).cos()).max(0.01)
}

/// 反射面の種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Floor,
    Ceiling,
    LeftWall,
    RightWall,
    BackWall,
    FrontWall,
}

/// 音源 3D 位置 src=[x,y,z] を面 surface で鏡像化したイメージ位置を返す。
/// 箱は x∈[0,W], y∈[0,D], z∈[0,H]。
pub fn image_position(src: [f64; 3], surface: Surface, dims: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = src;
    let [w, d, h] = dims;
    match surface {
        Surface::Floor => [x, y, -z],
        Surface::Ceiling => [x, y, 2.0 * h - z],
        Surface::LeftWall => [-x, y, z],
        Surface::RightWall => [2.0 * w - x, y, z],
        Surface::BackWall => [x, -y, z],
        Surface::FrontWall => [x, 2.0 * d - y, z],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_geometry_symmetric_at_center() {
        let geo = calc_geometry(0.2, 1.0, 0.0);
        assert!((geo.dist_l - geo.dist_r).abs() < 1e-12);
    }

    #[test]
    fn room_dims_interpolates_endpoints_and_mid() {
        let min = [4.0, 5.0, 3.0];
        let max = [25.0, 45.0, 18.0];
        assert_eq!(room_dims(0.0, min, max), min);
        assert_eq!(room_dims(1.0, min, max), max);
        let mid = room_dims(0.5, min, max);
        assert!((mid[1] - 25.0).abs() < 1e-10); // (5+45)/2
    }

    #[test]
    fn image_position_floor_mirrors_below() {
        let img = image_position([2.0, 3.0, 1.2], Surface::Floor, [10.0, 12.0, 4.0]);
        assert_eq!(img, [2.0, 3.0, -1.2]);
    }

    #[test]
    fn directivity_pattern_matches_inline_formula() {
        let k = 0.5;
        let ma = 45.0_f64.to_radians();
        let angle = 0.3_f64;
        let expected = ((1.0 - k) + k * (angle + ma).cos()).max(0.01);
        assert!((directivity_pattern(angle, k, ma) - expected).abs() < 1e-15);
        // 外開き: L は +ma, R は -ma
        let r = directivity_pattern(angle, k, -ma);
        assert!((r - ((1.0 - k) + k * (angle - ma).cos()).max(0.01)).abs() < 1e-15);
    }

    #[test]
    fn image_position_front_wall_mirrors_forward() {
        // 前壁 y=D=12 → y' = 2*12 - 3 = 21
        let img = image_position([2.0, 3.0, 1.2], Surface::FrontWall, [10.0, 12.0, 4.0]);
        assert_eq!(img, [2.0, 21.0, 1.2]);
    }
}
