use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cast::Cast;
use crate::ids::{LineId, LineRef};

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

// 後続タスクが ScriptItem::Speech { .. } を直接パターンマッチするため Box 化しない(要素は少数で短命)。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ScriptItem {
    Speech {
        /// 行頭の `[S01-L003]`。`number` を実行していなければ None。
        id: Option<LineId>,
        /// 台本の行番号(1始まり。エラーメッセージ用)。
        line_no: usize,
        cast_name: String,
        /// 台詞テキスト(字幕・吹き出しに使う。読み指定は廃止)。
        text: String,
        /// 臨時パラメータ(pan / distance / height / volume の±差分)。
        offset_params: HashMap<String, f64>,
        /// `overlap=ミリ秒`
        overlap_ms: Option<f64>,
        /// `at=台詞ID+ミリ秒`
        at: Option<LineRef>,
        /// 合成役の台詞のみ Some(台詞を読んだ時点の役の指定)。
        synth: Option<crate::cast::SynthSpec>,
        /// 行内 `voice=rec`。
        voice_rec: bool,
        /// 読み置換後のテキスト(合成役のみ)。
        synth_text: Option<String>,
        scene_config: SceneConfig,
    },
    Command(ScriptCommand),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ScriptCommand {
    Pause(f64),
    /// `#paragraph[S02] 名称`
    Paragraph {
        id: Option<u32>,
        name: Option<String>,
        line_no: usize,
    },
    /// `#visual (video loop) 本文`
    Visual {
        kind: Option<VisualKind>,
        /// `loop` の印(動画の素材が短いときにループさせる)
        looped: bool,
        text: String,
        line_no: usize,
    },
    /// `#cut[C02]`。`id` はカット番号(シーンの頭の C01 は暗黙で、ここには来ない)
    Cut {
        id: Option<u32>,
        line_no: usize,
    },
    /// `#note[N01] 本文`(ト書き)
    Note {
        id: Option<u32>,
        text: String,
        line_no: usize,
    },
    BgmStart(String),
    BgmStop,
    /// `#se ファイル名 [at=|end=] [gain=]`
    Se {
        path: String,
        anchor: Option<SeAnchor>,
        gain_db: f64,
        line_no: usize,
    },
    Parallel(usize),
}

/// `#visual` の素材の種類の希望。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualKind {
    Image,
    Video,
}

/// `#se` の位置指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SeAnchor {
    /// `at=`: 台詞の開始が基準
    Start(LineRef),
    /// `end=`: 台詞の終了が基準
    End(LineRef),
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
            id: Some(LineId { scene: 1, line: 3 }),
            line_no: 10,
            cast_name: "キャラA".to_string(),
            text: "こんにちは".to_string(),
            offset_params: HashMap::new(),
            overlap_ms: None,
            at: None,
            synth: None,
            voice_rec: false,
            synth_text: None,
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
        let cmds = [
            ScriptCommand::Pause(300.0),
            ScriptCommand::Paragraph {
                id: Some(1),
                name: None,
                line_no: 1,
            },
            ScriptCommand::Visual {
                kind: Some(VisualKind::Image),
                looped: false,
                text: "港".to_string(),
                line_no: 1,
            },
            ScriptCommand::Cut {
                id: Some(2),
                line_no: 1,
            },
            ScriptCommand::Note {
                id: None,
                text: "翌朝".to_string(),
                line_no: 1,
            },
            ScriptCommand::BgmStart("bgm01.wav".to_string()),
            ScriptCommand::BgmStop,
            ScriptCommand::Se {
                path: "se_door.wav".to_string(),
                anchor: None,
                gain_db: 0.0,
                line_no: 2,
            },
            ScriptCommand::Parallel(3),
        ];
        assert_eq!(cmds.len(), 9);
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
            pan: -15.0,
            distance: 1.5,
            volume: 1.0,
            params: HashMap::new(),
            height: None,
            height_offset: 0.0,
            appearance: None,
            synth: None,
        };
        scene.casts.insert("主人公".to_string(), cast);
        assert!(scene.casts.contains_key("主人公"));
    }
}
