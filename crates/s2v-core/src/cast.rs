use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Cast {
    pub name: String,
    pub speaker_name: String,
    pub engine_type: String,
    pub pan: f64,
    pub distance: f64,
    pub volume: f64,
    pub params: HashMap<String, Value>,
    /// 話者の床面からの絶対高さ[m]の基準値。None = 聴取者と同じ高さ。
    #[serde(default)]
    pub height: Option<f64>,
    /// 行内臨時パラメータで加算される、その行限定の高さオフセット[m]。
    #[serde(default)]
    pub height_offset: f64,
    /// 役の外見・特徴の自由記述（画像/動画生成プロンプト作成用）。
    /// 台本の`@cast`セクションで定義行の次に書かれた自由記述行から設定される。
    #[serde(default)]
    pub appearance: Option<String>,
}

impl Cast {
    /// 臨時パラメータを適用した新 Cast を返す (Python版 create_effective_cast 相当)。
    /// 数値は基準値への加算、未知のパラメータは上書きとして扱う。
    pub fn with_offsets(&self, offsets: &HashMap<String, f64>) -> Self {
        let mut cast = self.clone();
        for (k, &v) in offsets {
            match k.as_str() {
                "pan" => cast.pan += v,
                "distance" => cast.distance += v,
                "volume" => cast.volume += v,
                "height" => cast.height_offset += v,
                other => {
                    if let Some(neutral) = engine_param_neutral_default(other) {
                        let base = self
                            .params
                            .get(other)
                            .and_then(|v| v.as_f64())
                            .unwrap_or(neutral);
                        cast.params.insert(other.to_string(), Value::from(base + v));
                    } else {
                        cast.params.insert(other.to_string(), Value::from(v));
                    }
                }
            }
        }
        cast
    }
}

/// VOICEVOX/AivisSpeech/XTTS のエンジン固有パラメータの中立値 (Python版 _engine_param_defaults 相当)。
/// 該当しないキーは None を返す (=上書き対象)。
fn engine_param_neutral_default(key: &str) -> Option<f64> {
    match key {
        "speedScale" | "intonationScale" | "volumeScale" | "tempoDynamicsScale" | "speed" => {
            Some(1.0)
        }
        "pitchScale" | "temperature" | "pitch" => Some(0.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_cast() -> Cast {
        Cast {
            name: "ずんだもん".to_string(),
            speaker_name: "ずんだもん".to_string(),
            engine_type: "voicevox".to_string(),
            pan: 0.0,
            distance: 1.0,
            volume: 1.0,
            params: {
                let mut m = HashMap::new();
                m.insert("style".to_string(), Value::String("ノーマル".to_string()));
                m.insert("speedScale".to_string(), serde_json::json!(1.0));
                m
            },
            height: None,
            height_offset: 0.0,
            appearance: None,
        }
    }

    #[test]
    fn base_cast_has_no_appearance_by_default() {
        let cast = base_cast();
        assert_eq!(cast.appearance, None);
    }

    #[test]
    fn appearance_field_stores_free_text() {
        let mut cast = base_cast();
        cast.appearance = Some("小柄で緑髪の元気なキャラクター。".to_string());
        assert_eq!(
            cast.appearance.as_deref(),
            Some("小柄で緑髪の元気なキャラクター。")
        );
    }

    #[test]
    fn with_offsets_adds_to_base_pan() {
        // Python版 create_effective_cast: 数値フィールドは現在値に加算する (上書きではない)
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("pan".to_string(), 30.0_f64);
        let effective = cast.with_offsets(&offsets);
        assert!((effective.pan - (cast.pan + 30.0)).abs() < 1e-10);
        assert!((effective.distance - 1.0).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_adds_to_base_distance() {
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("distance".to_string(), 2.5_f64);
        let effective = cast.with_offsets(&offsets);
        assert!((effective.distance - (cast.distance + 2.5)).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_adds_to_base_volume() {
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("volume".to_string(), 0.8_f64);
        let effective = cast.with_offsets(&offsets);
        assert!((effective.volume - (cast.volume + 0.8)).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_does_not_mutate_original() {
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("pan".to_string(), 45.0_f64);
        let _effective = cast.with_offsets(&offsets);
        assert!((cast.pan - 0.0).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_empty_leaves_cast_unchanged() {
        let cast = base_cast();
        let offsets = HashMap::new();
        let effective = cast.with_offsets(&offsets);
        assert!((effective.pan - cast.pan).abs() < 1e-10);
        assert!((effective.distance - cast.distance).abs() < 1e-10);
        assert!((effective.volume - cast.volume).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_adds_to_existing_engine_param() {
        // Python版: エンジンパラメータは params の既存値 (なければ中立値) に加算する
        let cast = base_cast(); // params["speedScale"] = 1.0
        let mut offsets = HashMap::new();
        offsets.insert("speedScale".to_string(), 0.5_f64);
        let effective = cast.with_offsets(&offsets);
        let speed = effective.params["speedScale"].as_f64().unwrap();
        assert!(
            (speed - 1.5).abs() < 1e-10,
            "expected 1.0 (base) + 0.5 (offset) = 1.5, got {speed}"
        );
    }

    #[test]
    fn with_offsets_engine_param_uses_neutral_default_when_absent() {
        // base_cast には pitchScale が無いため、中立値 0.0 を基準に加算する
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("pitchScale".to_string(), 0.3_f64);
        let effective = cast.with_offsets(&offsets);
        let pitch = effective.params["pitchScale"].as_f64().unwrap();
        assert!(
            (pitch - 0.3).abs() < 1e-10,
            "expected neutral 0.0 + 0.3 = 0.3, got {pitch}"
        );
    }

    #[test]
    fn with_offsets_unknown_key_overwrites_in_params() {
        // engine_param_keys にも numeric_fields にも該当しないキーは上書き格納する
        let cast = base_cast();
        let mut offsets = HashMap::new();
        offsets.insert("room_size".to_string(), 0.6_f64);
        let effective = cast.with_offsets(&offsets);
        let room_size = effective.params["room_size"].as_f64().unwrap();
        assert!((room_size - 0.6).abs() < 1e-10);
    }

    #[test]
    fn with_offsets_height_accumulates_into_offset() {
        // 行内 height はその行限定の加算として height_offset に積まれる(基準 height は不変)
        let cast = base_cast(); // height=None, height_offset=0.0
        let mut offsets = HashMap::new();
        offsets.insert("height".to_string(), 0.5_f64);
        let eff = cast.with_offsets(&offsets);
        assert!((eff.height_offset - 0.5).abs() < 1e-10);
        assert_eq!(eff.height, cast.height);
    }
}
