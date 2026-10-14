//! `timeline.json` v3 の公開の型と検証(映像工程設計書 §3.5〜3.6)。
//! 書き出す側(s2v-export)も読む側(prompts・bubbles・render)も、この型だけを使う。

mod validate;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub schema_version: u32,
    pub script: String,
    pub script_sha256: String,
    pub audio_sha256: String,
    pub sample_rate: u32,
    pub duration_ms: i64,
    pub normalize: Normalize,
    pub cast: Vec<CastEntry>,
    pub scenes: Vec<SceneEntry>,
    pub takes: Vec<TakeEntry>,
    pub lines: Vec<LineEntry>,
    pub events: Vec<EventEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Normalize {
    pub target_lufs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastEntry {
    pub name: String,
    pub speaker: String,
    pub appearance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneEntry {
    pub id: String,
    pub name: Option<String>,
    pub index: usize,
    pub silent: bool,
    pub start_ms: i64,
    pub end_ms: i64,
    pub cuts: Vec<CutEntry>,
    pub notes: Vec<NoteEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CutEntry {
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub visual: Option<Visual>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visual {
    pub text: String,
    pub kind: Option<VisualKind>,
    #[serde(rename = "loop")]
    pub looped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualKind {
    Image,
    Video,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteEntry {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeEntry {
    pub id: String,
    pub tracks: Vec<Option<String>>,
    pub files: Vec<String>,
    pub trims: Option<Vec<Trim>>,
    pub missing: bool,
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    pub track: Option<String>,
    pub head_ms: i64,
    pub tail_ms: i64,
}

/// 不足の理由(①の設計書 §6 の列挙)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    NoFile,
    Duplicate,
    Unreadable,
    Silent,
    SplitFailed,
    CutsInvalid,
    OrderMismatch,
    SyncMismatch,
    BadFormat,
    Unmeasurable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineEntry {
    pub id: String,
    pub scene: String,
    pub cast: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub take: String,
    pub track: Option<String>,
    pub measured_lufs: Option<f64>,
    pub requested_gain_db: Option<f64>,
    pub norm_gain_db: Option<f64>,
    pub norm_clamped: Option<bool>,
    pub src_start_ms: Option<i64>,
    pub src_end_ms: Option<i64>,
    pub missing: bool,
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventEntry {
    Se {
        file: String,
        start_ms: i64,
        gain_db: f64,
    },
    BgmStart {
        file: String,
        start_ms: i64,
    },
    BgmStop {
        start_ms: i64,
    },
}

impl EventEntry {
    pub fn start_ms(&self) -> i64 {
        match self {
            EventEntry::Se { start_ms, .. }
            | EventEntry::BgmStart { start_ms, .. }
            | EventEntry::BgmStop { start_ms } => *start_ms,
        }
    }
}

/// 読み込みの失敗。後続のコマンドはどれでも終了コード 2 にする。
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// JSON として読めない、または型が合わない
    Json(String),
    /// `schema_version` が 3 でない(無ければ None)
    Version(Option<u64>),
    /// §3.6 の不変条件の違反
    Invalid(Vec<String>),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Json(e) => write!(f, "timeline.json を読めません: {e}"),
            LoadError::Version(v) => write!(
                f,
                "timeline.json の版が {} です(3 が必要です)。build をやり直してください",
                v.map_or("不明".to_string(), |v| v.to_string())
            ),
            LoadError::Invalid(errs) => {
                write!(
                    f,
                    "timeline.json の内容が正しくありません: {}",
                    errs.join(" / ")
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// `timeline.json` の文字列を読み、版を確かめ、検証する。
pub fn load_str(s: &str) -> Result<Timeline, LoadError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| LoadError::Json(e.to_string()))?;
    match v.get("schema_version").and_then(|x| x.as_u64()) {
        Some(n) if n == SCHEMA_VERSION as u64 => {}
        other => return Err(LoadError::Version(other)),
    }
    let t: Timeline = serde_json::from_value(v).map_err(|e| LoadError::Json(e.to_string()))?;
    t.validate().map_err(LoadError::Invalid)?;
    Ok(t)
}

impl Timeline {
    /// 設計書 §3.6 の不変条件。違反をすべて集めて返す。
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let errs = validate::check(self);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("Timeline は常に JSON にできる")
    }
}
