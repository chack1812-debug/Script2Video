use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 役(`@cast` の1行)。音声合成エンジン関連の項目は持たない(台本仕様 §2.4)。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Cast {
    pub name: String,
    /// 収録者・声の担当者の表示名(任意。空文字可)。
    pub speaker_name: String,
    pub pan: f64,
    pub distance: f64,
    /// 正規化後の基準の音量(1.0)に対する倍率。
    pub volume: f64,
    /// 役ごとの残響の上書き(`room_size` / `reverb_wet`)。
    pub params: HashMap<String, Value>,
    /// 話者の床面からの絶対高さ[m]の基準値。None = 聴取者と同じ高さ。
    #[serde(default)]
    pub height: Option<f64>,
    /// 行内臨時パラメータで加算される、その行限定の高さオフセット[m]。
    #[serde(default)]
    pub height_offset: f64,
    /// 役の外見・特徴の自由記述(画像/動画生成プロンプト作成用)。
    #[serde(default)]
    pub appearance: Option<String>,
}

impl Cast {
    /// 臨時パラメータ(±差分)を適用した新 Cast を返す。
    pub fn with_offsets(&self, offsets: &HashMap<String, f64>) -> Self {
        let mut cast = self.clone();
        for (k, &v) in offsets {
            match k.as_str() {
                "pan" => cast.pan += v,
                "distance" => cast.distance += v,
                "volume" => cast.volume += v,
                "height" => cast.height_offset += v,
                _ => {}
            }
        }
        cast
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_cast() -> Cast {
        Cast {
            name: "A".to_string(),
            speaker_name: "田中".to_string(),
            pan: 0.0,
            distance: 1.0,
            volume: 1.0,
            params: HashMap::new(),
            height: None,
            height_offset: 0.0,
            appearance: None,
        }
    }

    #[test]
    fn with_offsets_adds_to_base_values() {
        let cast = base_cast();
        let offsets: HashMap<String, f64> = [
            ("pan".to_string(), 30.0),
            ("distance".to_string(), 2.5),
            ("volume".to_string(), -0.2),
            ("height".to_string(), 0.5),
        ]
        .into_iter()
        .collect();
        let e = cast.with_offsets(&offsets);
        assert!((e.pan - 30.0).abs() < 1e-10);
        assert!((e.distance - 3.5).abs() < 1e-10);
        assert!((e.volume - 0.8).abs() < 1e-10);
        assert!((e.height_offset - 0.5).abs() < 1e-10);
        assert_eq!(e.height, None);
    }

    #[test]
    fn with_offsets_does_not_mutate_original() {
        let cast = base_cast();
        let offsets: HashMap<String, f64> = [("pan".to_string(), 45.0)].into_iter().collect();
        let _ = cast.with_offsets(&offsets);
        assert_eq!(cast.pan, 0.0);
    }

    #[test]
    fn with_offsets_ignores_unknown_keys() {
        let cast = base_cast();
        let offsets: HashMap<String, f64> = [("speed".to_string(), 1.0)].into_iter().collect();
        let e = cast.with_offsets(&offsets);
        assert!(e.params.is_empty());
    }
}
