use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cast::Cast;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SceneConfig {
    pub name: String,
    /// 省略時は `None`。実効値への解決は処理時に AudioConfig の値へフォールバックする
    /// (Python版 audio_processor.py の `getattr(config, 'ROOM_SIZE'/'REVERB_WET', ...)` 相当)。
    pub room_size: Option<f64>,
    pub reverb_wet: Option<f64>,
    /// 部屋寸法[m]。3つすべて指定されたとき room_size より優先される。
    #[serde(default)]
    pub room_w: Option<f64>,
    #[serde(default)]
    pub room_d: Option<f64>,
    #[serde(default)]
    pub room_h: Option<f64>,
    /// 聴取者(マイクペア中心)の部屋中央からのオフセット[m]。省略時は config の listener_offset。
    #[serde(default)]
    pub listener_dx: Option<f64>,
    #[serde(default)]
    pub listener_dy: Option<f64>,
    /// 聴取者(マイクペア中心)の床面からの絶対高さ[m]。省略時は config の ear_height。
    #[serde(default)]
    pub listener_z: Option<f64>,
    /// 場面の情景・雰囲気の自由記述（画像/動画生成プロンプト作成用）。
    /// 台本の`@scene`ヘッダー行の後に書かれた自由記述行から設定される。
    #[serde(default)]
    pub description: Option<String>,
}

impl SceneConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            room_size: None,
            reverb_wet: None,
            room_w: None,
            room_d: None,
            room_h: None,
            listener_dx: None,
            listener_dy: None,
            listener_z: None,
            description: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ScriptItem {
    Speech {
        cast_name: String,
        text: String,
        display_text: String,
        offset_params: HashMap<String, f64>,
        scene_config: SceneConfig,
    },
    Command(ScriptCommand),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ScriptCommand {
    Pause(f64),
    /// 段落区切り。`Some(name)` は `#paragraph 名称` で付けられた名称。
    Paragraph(Option<String>),
    BgmStart(String),
    BgmStop,
    Se(String),
    Parallel(usize),
}

#[derive(Debug, Clone)]
pub struct PauseConfig {
    pub sentence_ms: f64,
    pub cast_ms: f64,
    pub paragraph_ms: f64,
}

impl Default for PauseConfig {
    fn default() -> Self {
        Self {
            sentence_ms: 500.0,
            cast_ms: 300.0,
            paragraph_ms: 1500.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub config: SceneConfig,
    pub casts: HashMap<String, Cast>,
    pub items: Vec<ScriptItem>,
    pub pause_config: PauseConfig,
}

impl Scene {
    pub fn new(config: SceneConfig) -> Self {
        Self {
            config,
            casts: HashMap::new(),
            items: Vec::new(),
            pause_config: PauseConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_config_defaults() {
        // 省略時は None。実効値はAudioConfigへフォールバックする (s2v-57z)
        let sc = SceneConfig::new("居間");
        assert_eq!(sc.name, "居間");
        assert_eq!(sc.room_size, None);
        assert_eq!(sc.reverb_wet, None);
        assert_eq!(sc.room_w, None);
        assert_eq!(sc.room_d, None);
        assert_eq!(sc.room_h, None);
        assert_eq!(sc.listener_dx, None);
        assert_eq!(sc.listener_dy, None);
        assert_eq!(sc.listener_z, None);
        assert_eq!(sc.description, None);
    }

    #[test]
    fn scene_config_custom_values() {
        let sc = SceneConfig {
            room_size: Some(0.8),
            reverb_wet: Some(0.3),
            ..SceneConfig::new("広場")
        };
        assert_eq!(sc.room_size, Some(0.8));
    }

    #[test]
    fn scene_config_can_hold_description() {
        let sc = SceneConfig {
            description: Some("放課後の静かな教室。".to_string()),
            ..SceneConfig::new("教室")
        };
        assert_eq!(sc.description.as_deref(), Some("放課後の静かな教室。"));
    }

    #[test]
    fn script_item_speech_round_trip() {
        let item = ScriptItem::Speech {
            cast_name: "キャラA".to_string(),
            text: "こんにちは".to_string(),
            display_text: "こんにちは".to_string(),
            offset_params: HashMap::new(),
            scene_config: SceneConfig::new("室内"),
        };
        if let ScriptItem::Speech {
            cast_name, text, ..
        } = &item
        {
            assert_eq!(cast_name, "キャラA");
            assert_eq!(text, "こんにちは");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn script_command_variants_are_constructible() {
        let cmds = vec![
            ScriptCommand::Pause(300.0),
            ScriptCommand::Paragraph(None),
            ScriptCommand::BgmStart("bgm01.wav".to_string()),
            ScriptCommand::BgmStop,
            ScriptCommand::Se("se_door.wav".to_string()),
            ScriptCommand::Parallel(3),
        ];
        assert_eq!(cmds.len(), 6);
    }

    #[test]
    fn scene_new_starts_empty() {
        let scene = Scene::new(SceneConfig::new("テスト"));
        assert!(scene.items.is_empty());
        assert!(scene.casts.is_empty());
    }

    #[test]
    fn pause_config_defaults_match_python_parser_defaults() {
        // Python版 core/parser.py:32 self.pause_config = {"sentence": 500, "cast": 300, "paragraph": 1500}
        let pc = PauseConfig::default();
        assert_eq!(pc.sentence_ms, 500.0);
        assert_eq!(pc.cast_ms, 300.0);
        assert_eq!(pc.paragraph_ms, 1500.0);
    }

    #[test]
    fn scene_accepts_casts() {
        let mut scene = Scene::new(SceneConfig::new("屋外"));
        let cast = Cast {
            name: "主人公".to_string(),
            speaker_name: "ずんだもん".to_string(),
            engine_type: "voicevox".to_string(),
            pan: -15.0,
            distance: 1.5,
            volume: 1.0,
            params: HashMap::new(),
            height: None,
            height_offset: 0.0,
            appearance: None,
        };
        scene.casts.insert("主人公".to_string(), cast);
        assert!(scene.casts.contains_key("主人公"));
    }
}
