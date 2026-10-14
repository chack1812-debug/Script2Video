# Script2Video 計画 2a(台本の `#cut`・`#note`、timeline.json v3、共通の部品) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 台本に `#cut`・`#note`・`#visual` の `loop` を加え、`number` が番号を振り、`build` がカットとト書きの時刻を含む `timeline.json` v3 を、共通の型 `s2v-timeline` で検証してから書き出すようにする。あわせて、後続のコマンド(`prompts`・`bubbles`・`render`)が使う共通の部品(ロック、ログの追記、予約名、原子的な書き込み、鮮度の検査)と、`build` の `output.mp4` → `output.stale.mp4` の改名を作る。

**Architecture:** 新しいクレート `s2v-timeline` に v3 の公開の型と `validate()` を置き、書き出す側(`s2v-export`)はこの型を組み立てて検証してから書く。台本の解析(`s2v-core::parser`)・採番(`numbering`)・配置(`placement`)はカットとト書きをサンプル単位で記録し、新しいモジュール `s2v-core::visual_timeline` がミリ秒のカット・ト書きの区間に変換する。CLI の共通の部品は `s2v-cli` の新しいモジュール(`fsutil`・`lock`・`freshness`)に置く。

**Tech Stack:** Rust 1.96(edition 2021)、serde / serde_json、sha2、std の `File::try_lock`(Rust 1.89 以上)。新しい外部クレートは使わない。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)。§2.2〜2.4 と §3 を実装する。台本の書式は `台本仕様.md` §2.7〜2.9、§4、§5。①の設計は `docs/superpowers/specs/2026-10-12-script2video-design.md` §5.5・§6。

## Global Constraints

- 応答・コメント・エラーメッセージ・コミットメッセージは日本語(既存のコードの書き方に合わせる)。コミットの末尾に `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。
- 内部の時刻はサンプル(48kHz の整数)。ミリ秒への変換は `s2v_core::time::floor_ms`(開始)と `ceil_ms`(終了)。**カット・ト書きの境目は、連続性を保つため、開始も終了も `floor_ms` で変換する**(最後のシーンの最後の終わりだけは `duration_ms`)。
- `timeline.json` の `schema_version` は 3。後続工程は 3 以外を拒否する(設計書 §3.5)。
- カット ID は `S01-C02`(カット番号は2桁以上)、ト書き ID は `S01-N01`(2桁以上)。シーンの頭は暗黙の `C01` で、`#cut[C01]` はエラー。読むときは桁数を問わない。
- 終了コード: 0 成功、1 警告あり、2 失敗(既存の `Outcome`)。
- 予約名(大文字小文字を区別しない): `full_dialogue.wav`・`Process.log`・`output.mp4`・`output.stale.mp4`・`bubbles.json`・`bubbles.json.bak`・`.script2video.lock`・`prompts`(設計書 §2.2)。
- ロックファイルは台本フォルダの `.script2video.lock`。OS の排他ロック(`std::fs::File::try_lock`)で、取れなければ「別の script2video が実行中」で エラー 2(設計書 §2.3)。
- SHA-256 はファイルの生のバイト列に対して計算し、64 文字の小文字の16進(設計書 §2.4)。
- 検証コマンド(各タスクの最後に全部通すこと): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- 既存のテストは、仕様の変更に伴って期待値を更新してよい(v2 → v3 など)。ただし、変更した期待値の理由をコミットメッセージに書く。テストを消して通すことはしない。

## Review Focus

1. **括弧で始まる日本語の情景**(`#visual (朝) 港の様子`): 以前は本文として読めていた。印の括弧は ASCII の英数字と空白だけで書かれたものに限り、それ以外は本文として扱う(エラーにしない)。→ Task 2 のテスト `visual_paren_with_japanese_is_text`。
2. **以前の `.ids`(`max_cut`・`max_note` の行が無い)と、番号付きのカットを含む台本の再採番**: 既存の `C05` より前に番号の無い `#cut` を挿入したら `C06` になる(`C02` を再利用しない)。→ Task 3 のテスト `inserted_cut_after_existing_higher_number_continues_from_max`。
3. **`output.mp4` を動画プレーヤーで開いたまま `build`**: 改名に失敗したら エラー 2 で、前回の `full_dialogue.wav` などの成果物を消さない。→ Task 7 のテスト `stale_rename_failure_keeps_previous_outputs`(改名先をフォルダにして失敗させる)。
4. **異常終了したプロセスが残したロックファイル**: ファイルが残っていても、OS のロックは外れているので、次の実行は進める。→ Task 6 のテスト `leftover_lock_file_does_not_block`。
5. **BOM・CRLF の台本の `#cut`・`#note` の採番**: ID 以外のバイトを変えない。→ Task 3 のテスト `cut_and_note_numbering_preserves_crlf_and_bom`。

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `Cargo.toml`(ワークスペース) | `crates/s2v-timeline` を追加 | 1 |
| `crates/s2v-timeline/Cargo.toml`、`src/lib.rs`、`src/validate.rs` | v3 の型、`load_str`、`validate` | 1 |
| `crates/s2v-core/src/types.rs` | `ScriptCommand::Cut`・`Note`、`Visual` の `looped`・`line_no` | 2 |
| `crates/s2v-core/src/parser.rs` | `#cut`・`#note`・`#visual` の印の解析 | 2 |
| `crates/s2v-core/src/validate.rs` | カット・ト書きの番号の有無と重複 | 2 |
| `crates/s2v-core/src/ids.rs` | `format_cut_id`・`format_note_id`・`parse_numbered_tag` | 2 |
| `crates/s2v-export/src/script_html.rs` | カット・ト書きの表示 | 2 |
| `crates/s2v-core/src/numbering.rs` | `#cut`・`#note` の採番、`.ids` の拡張 | 3 |
| `crates/s2v-cli/src/number.rs` | 採番の件数の表示 | 3 |
| `crates/s2v-core/src/placement.rs` | カット・ト書きの記録、`#visual` の集約 | 4 |
| `crates/s2v-core/src/visual_timeline.rs`(新規) | ミリ秒のカット・ト書きの区間と警告 | 4 |
| `crates/s2v-export/src/timeline_json.rs` | v3 の書き出し(`s2v-timeline` の型) | 5 |
| `crates/s2v-cli/src/fsutil.rs`(新規) | `write_atomic`、`sha256_file` | 6 |
| `crates/s2v-cli/src/lock.rs`(新規) | `ScriptLock` | 6 |
| `crates/s2v-cli/src/proclog.rs` | `ProcLog::append` | 6 |
| `crates/s2v-cli/src/layout.rs` | 予約名の追加、`check_reserved_name` | 6 |
| `crates/s2v-cli/src/freshness.rs`(新規) | 鮮度の検査(後続コマンド用) | 6 |
| `crates/s2v-cli/src/number.rs` | ロック、予約名、原子的な書き込み | 6 |
| `crates/s2v-cli/src/build.rs` | ロック、改名、SHA-256、v3 の書き出し | 7 |
| `crates/s2v-cli/tests/build_golden.rs`、`number_cli.rs` | 結合テスト | 3, 6, 7 |
| `README.md` | 動作の説明の更新 | 7 |

---

### Task 1: `s2v-timeline` クレート(v3 の型と検証)

**Files:**
- Modify: `Cargo.toml`(members に `"crates/s2v-timeline"` を追加)
- Create: `crates/s2v-timeline/Cargo.toml`
- Create: `crates/s2v-timeline/src/lib.rs`
- Create: `crates/s2v-timeline/src/validate.rs`

**Interfaces:**
- Consumes: なし(serde・serde_json だけに依存。`s2v-core` には依存しない)
- Produces:
  - `pub const SCHEMA_VERSION: u32 = 3;`
  - `pub struct Timeline { schema_version, script, script_sha256, audio_sha256, sample_rate, duration_ms, normalize, cast, scenes, takes, lines, events }`(全フィールド pub)
  - `Normalize`、`CastEntry`、`SceneEntry { id, name, index, silent, start_ms, end_ms, cuts, notes }`、`CutEntry { id, start_ms, end_ms, visual }`、`Visual { text, kind, looped }`(JSON のキーは `loop`)、`VisualKind { Image, Video }`、`NoteEntry { id, text, start_ms, end_ms }`、`TakeEntry`、`Trim`、`Reason`、`LineEntry`、`EventEntry`
  - `impl Timeline { pub fn validate(&self) -> Result<(), Vec<String>>; pub fn to_json_pretty(&self) -> String }`
  - `pub fn load_str(s: &str) -> Result<Timeline, LoadError>`、`pub enum LoadError { Json(String), Version(Option<u64>), Invalid(Vec<String>) }`(`Display` を実装)

- [ ] **Step 1: クレートの骨組みを作る**

`crates/s2v-timeline/Cargo.toml`:

```toml
[package]
name = "s2v-timeline"
version = "0.1.0"
edition = "2021"

[dependencies]
serde.workspace = true
serde_json = "1"
```

ワークスペースの `Cargo.toml` の members を `["crates/s2v-core", "crates/s2v-audio", "crates/s2v-export", "crates/s2v-cli", "crates/s2v-timeline"]` にする。

- [ ] **Step 2: 型を書く(`src/lib.rs`)**

```rust
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
                write!(f, "timeline.json の内容が正しくありません: {}", errs.join(" / "))
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
```

- [ ] **Step 3: 検証の失敗するテストを書く(`src/validate.rs` の tests)**

まず `src/validate.rs` に空の `pub(crate) fn check(_t: &Timeline) -> Vec<String> { Vec::new() }` を置き、テストを書く。テストの土台(正しい v3):

```rust
#[cfg(test)]
mod tests {
    use crate::*;

    /// 2シーン(S01 は C01・C02 とト書き1つ、S02 は無言)の正しい timeline。
    pub(crate) fn sample() -> Timeline {
        Timeline {
            schema_version: 3,
            script: "第1話.txt".into(),
            script_sha256: "a".repeat(64),
            audio_sha256: "b".repeat(64),
            sample_rate: 48_000,
            duration_ms: 9_000,
            normalize: Normalize { target_lufs: -23.0 },
            cast: vec![CastEntry { name: "A".into(), speaker: "田中".into(), appearance: None }],
            scenes: vec![
                SceneEntry {
                    id: "S01".into(), name: Some("朝".into()), index: 1, silent: false,
                    start_ms: 0, end_ms: 3_000,
                    cuts: vec![
                        CutEntry { id: "S01-C01".into(), start_ms: 0, end_ms: 2_000,
                            visual: Some(Visual { text: "港".into(), kind: Some(VisualKind::Image), looped: false }) },
                        CutEntry { id: "S01-C02".into(), start_ms: 2_000, end_ms: 5_000, visual: None },
                    ],
                    notes: vec![NoteEntry { id: "S01-N01".into(), text: "翌朝".into(), start_ms: 0, end_ms: 2_000 }],
                },
                SceneEntry {
                    id: "S02".into(), name: None, index: 2, silent: true,
                    start_ms: 5_000, end_ms: 7_000,
                    cuts: vec![CutEntry { id: "S02-C01".into(), start_ms: 5_000, end_ms: 9_000, visual: None }],
                    notes: vec![],
                },
            ],
            takes: vec![
                TakeEntry { id: "S01_L001".into(), tracks: vec![None], files: vec!["S01_L001.wav".into()],
                    trims: Some(vec![Trim { track: None, head_ms: 10, tail_ms: 20 }]), missing: false, reason: None },
                TakeEntry { id: "S01_L002".into(), tracks: vec![None], files: vec![],
                    trims: None, missing: true, reason: Some(Reason::NoFile) },
            ],
            lines: vec![
                LineEntry { id: "S01-L001".into(), scene: "S01".into(), cast: "A".into(), text: "一".into(),
                    start_ms: 0, end_ms: 1_000, take: "S01_L001".into(), track: None,
                    measured_lufs: Some(-30.0), requested_gain_db: Some(7.0), norm_gain_db: Some(7.0),
                    norm_clamped: Some(false), src_start_ms: Some(10), src_end_ms: Some(1_010),
                    missing: false, reason: None },
                LineEntry { id: "S01-L002".into(), scene: "S01".into(), cast: "A".into(), text: "二".into(),
                    start_ms: 2_000, end_ms: 3_000, take: "S01_L002".into(), track: None,
                    measured_lufs: None, requested_gain_db: None, norm_gain_db: None, norm_clamped: None,
                    src_start_ms: None, src_end_ms: None, missing: true, reason: Some(Reason::NoFile) },
            ],
            events: vec![EventEntry::BgmStop { start_ms: 0 }],
        }
    }

    fn errs(t: &Timeline) -> Vec<String> {
        t.validate().err().unwrap_or_default()
    }

    #[test]
    fn sample_is_valid_and_round_trips() {
        let t = sample();
        assert_eq!(errs(&t), Vec::<String>::new());
        let back = load_str(&t.to_json_pretty()).unwrap();
        assert_eq!(back, t);
        // JSON のキーは loop
        assert!(t.to_json_pretty().contains("\"loop\": false"));
    }

    #[test]
    fn version_other_than_3_is_rejected_before_parsing() {
        assert_eq!(load_str("{\"schema_version\": 2}"), Err(LoadError::Version(Some(2))));
        assert_eq!(load_str("{}"), Err(LoadError::Version(None)));
        assert!(matches!(load_str("not json"), Err(LoadError::Json(_))));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json_pretty()).unwrap();
        v["future_field"] = serde_json::json!(1);
        v["scenes"][0]["cuts"][0]["future"] = serde_json::json!("x");
        assert!(load_str(&v.to_string()).is_ok());
    }

    /// 1つ壊して、検証が失敗することを確かめる。
    fn broken(f: impl FnOnce(&mut Timeline)) -> Vec<String> {
        let mut t = sample();
        f(&mut t);
        errs(&t)
    }

    #[test]
    fn id_formats_and_scene_parts() {
        assert!(!broken(|t| t.scenes[0].id = "X01".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].cuts[1].id = "S01-X02".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].cuts[1].id = "S02-C02".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].notes[0].id = "S01-N".into()).is_empty());
        assert!(!broken(|t| t.lines[0].id = "S02-L001".into()).is_empty());
        // 桁数は問わない
        assert!(broken(|t| t.scenes[0].cuts[1].id = "S01-C0002".into()).is_empty());
    }

    #[test]
    fn duplicates_are_rejected() {
        assert!(!broken(|t| t.scenes[1].id = "S01".into()).is_empty());
        assert!(!broken(|t| t.lines[1].id = "S01-L001".into()).is_empty());
        assert!(!broken(|t| t.takes[1].id = "S01_L001".into()).is_empty());
        assert!(!broken(|t| t.cast.push(t.cast[0].clone())).is_empty());
    }

    #[test]
    fn cuts_must_tile_the_video() {
        // 最初のカットは C01
        assert!(!broken(|t| t.scenes[0].cuts[0].id = "S01-C03".into()).is_empty());
        // カットが無いシーン
        assert!(!broken(|t| t.scenes[1].cuts.clear()).is_empty());
        // 隙間
        assert!(!broken(|t| t.scenes[0].cuts[1].start_ms = 2_100).is_empty());
        // 最初のシーンが 0 から始まらない
        assert!(!broken(|t| t.scenes[0].cuts[0].start_ms = 1).is_empty());
        // 2番目のシーンの先頭カットが start_ms と違う
        assert!(!broken(|t| { t.scenes[1].cuts[0].start_ms = 4_000; t.scenes[0].cuts[1].end_ms = 4_000; }).is_empty());
        // 最後が duration_ms で終わらない
        assert!(!broken(|t| t.scenes[1].cuts[0].end_ms = 8_000).is_empty());
        // 長さ 0 は許す
        assert!(broken(|t| { t.scenes[0].cuts[0].end_ms = 0; t.scenes[0].cuts[1].start_ms = 0;
            t.scenes[0].notes[0].end_ms = 0; }).is_empty());
    }

    #[test]
    fn notes_must_be_ordered_and_inside_the_scene_video() {
        assert!(!broken(|t| t.scenes[0].notes[0].end_ms = 6_000).is_empty());
        assert!(!broken(|t| t.scenes[0].notes.push(NoteEntry { id: "S01-N02".into(), text: "x".into(),
            start_ms: 1_000, end_ms: 1_500 })).is_empty()); // N01 の終わり 2000 > N02 の始め 1000
    }

    #[test]
    fn times_must_be_in_range() {
        assert!(!broken(|t| t.lines[0].end_ms = 9_001).is_empty());
        assert!(!broken(|t| t.lines[0].start_ms = 1_001).is_empty());
        assert!(!broken(|t| t.events.push(EventEntry::BgmStop { start_ms: 9_001 })).is_empty());
        assert!(!broken(|t| t.duration_ms = -1).is_empty());
    }

    #[test]
    fn references_must_resolve() {
        assert!(!broken(|t| t.lines[0].scene = "S09".into()).is_empty());
        assert!(!broken(|t| t.lines[0].cast = "Z".into()).is_empty());
        assert!(!broken(|t| t.lines[0].take = "S09_L001".into()).is_empty());
        assert!(!broken(|t| t.lines[0].track = Some("A".into())).is_empty()); // 単独のテイクは null
        // 所属シーンの位置が減る並び
        assert!(!broken(|t| { t.lines.swap(0, 1); t.lines[0].scene = "S02".into(); t.lines[0].id = "S02-L001".into(); }).is_empty());
    }

    #[test]
    fn line_and_take_null_rules() {
        assert!(!broken(|t| t.lines[0].reason = Some(Reason::Silent)).is_empty());
        assert!(!broken(|t| t.lines[0].norm_clamped = None).is_empty());
        assert!(!broken(|t| t.lines[0].measured_lufs = Some(f64::NAN)).is_empty());
        assert!(!broken(|t| t.lines[1].norm_clamped = Some(false)).is_empty());
        assert!(!broken(|t| t.lines[1].reason = None).is_empty());
        assert!(!broken(|t| t.takes[0].files.clear()).is_empty());
        assert!(!broken(|t| t.takes[0].files = vec!["S01_L001.A.wav".into()]).is_empty()); // 単独は id と同じ名前
        assert!(!broken(|t| t.takes[0].trims = None).is_empty());
        assert!(!broken(|t| t.takes[1].files = vec!["S01_L002.wav".into()]).is_empty()); // no_file は 0 件
        assert!(!broken(|t| t.takes[1].reason = Some(Reason::Duplicate)).is_empty()); // duplicate は 2 件以上
        assert!(!broken(|t| t.takes[0].tracks = vec![]).is_empty());
    }

    #[test]
    fn role_take_files_must_match_tracks() {
        let ok = broken(|t| {
            t.takes[0] = TakeEntry { id: "S01_L001".into(), tracks: vec![Some("A".into()), Some("B".into())],
                files: vec!["S01_L001.A.wav".into(), "S01_L001.B.flac".into()],
                trims: Some(vec![Trim { track: Some("A".into()), head_ms: 0, tail_ms: 0 },
                                 Trim { track: Some("B".into()), head_ms: 0, tail_ms: 0 }]),
                missing: false, reason: None };
            t.lines[0].track = Some("A".into());
        });
        assert!(ok.is_empty(), "{ok:?}");
        assert!(!broken(|t| {
            t.takes[0] = TakeEntry { id: "S01_L001".into(), tracks: vec![Some("A".into()), Some("B".into())],
                files: vec!["S01_L001.B.wav".into(), "S01_L001.A.wav".into()], // 逆順
                trims: Some(vec![Trim { track: Some("A".into()), head_ms: 0, tail_ms: 0 },
                                 Trim { track: Some("B".into()), head_ms: 0, tail_ms: 0 }]),
                missing: false, reason: None };
            t.lines[0].track = Some("A".into());
        }).is_empty());
    }

    #[test]
    fn scene_index_and_silent() {
        assert!(!broken(|t| t.scenes[1].index = 3).is_empty());
        assert!(!broken(|t| { t.scenes[1].silent = true; t.lines[1].scene = "S02".into();
            t.lines[1].id = "S02-L002".into(); }).is_empty());
    }
}
```

- [ ] **Step 4: テストが失敗することを確かめる**

Run: `cargo test -p s2v-timeline`
Expected: `sample_is_valid_and_round_trips`・`version_other_than_3...`・`unknown_fields_are_ignored` 以外の「壊したら失敗する」テストが FAIL(`check` が空のため)。

- [ ] **Step 5: 検証を実装する(`src/validate.rs`)**

```rust
//! 設計書 §3.6 の不変条件。

use std::collections::{HashMap, HashSet};

use crate::{Reason, Timeline};

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_scene_id(s: &str) -> bool {
    s.strip_prefix('S').is_some_and(digits)
}

/// `S01-C02` の形なら、シーンの部分(`S01`)を返す。
fn sub_id_scene<'a>(s: &'a str, letter: char) -> Option<&'a str> {
    let (scene, rest) = s.split_once('-')?;
    (is_scene_id(scene) && rest.strip_prefix(letter).is_some_and(digits)).then_some(scene)
}

fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

pub(crate) fn check(t: &Timeline) -> Vec<String> {
    let mut e: Vec<String> = Vec::new();
    let dur = t.duration_ms;
    if dur < 0 {
        e.push(format!("duration_ms が負です: {dur}"));
    }
    let in_range = |a: i64, b: i64| 0 <= a && a <= b && b <= dur;

    // シーンとカット・ト書き
    let mut scene_pos: HashMap<&str, usize> = HashMap::new();
    let mut cut_ids = HashSet::new();
    let mut note_ids = HashSet::new();
    let n = t.scenes.len();
    for (i, s) in t.scenes.iter().enumerate() {
        if !is_scene_id(&s.id) {
            e.push(format!("シーン ID の書式が違います: {}", s.id));
        }
        if scene_pos.insert(s.id.as_str(), i).is_some() {
            e.push(format!("シーン ID が重複しています: {}", s.id));
        }
        if s.index != i + 1 {
            e.push(format!("{}: index は {} のはずです({})", s.id, i + 1, s.index));
        }
        if !in_range(s.start_ms, s.end_ms) {
            e.push(format!("{}: 時刻が範囲外です", s.id));
        }
        if s.cuts.is_empty() {
            e.push(format!("{}: カットがありません", s.id));
            continue;
        }
        let first_id = format!("{}-C01", s.id);
        let first_ok = sub_id_scene(&s.cuts[0].id, 'C') == Some(s.id.as_str())
            && s.cuts[0].id[s.id.len() + 2..].parse::<u32>() == Ok(1);
        if !first_ok {
            e.push(format!("{}: 最初のカットは {first_id} のはずです({})", s.id, s.cuts[0].id));
        }
        for c in &s.cuts {
            if sub_id_scene(&c.id, 'C') != Some(s.id.as_str()) {
                e.push(format!("{}: カット ID が違います: {}", s.id, c.id));
            }
            if !cut_ids.insert(c.id.as_str()) {
                e.push(format!("カット ID が重複しています: {}", c.id));
            }
            if !in_range(c.start_ms, c.end_ms) {
                e.push(format!("{}: 時刻が範囲外です", c.id));
            }
        }
        for w in s.cuts.windows(2) {
            if w[0].end_ms != w[1].start_ms {
                e.push(format!("{} と {} の間に隙間か重なりがあります", w[0].id, w[1].id));
            }
        }
        let (vs, ve) = (s.cuts[0].start_ms, s.cuts[s.cuts.len() - 1].end_ms);
        let expect_start = if i == 0 { 0 } else { s.start_ms };
        if vs != expect_start {
            e.push(format!("{}: 映像区間の始め {vs} が {expect_start} と違います", s.id));
        }
        let expect_end = if i + 1 == n { Some(dur) } else { Some(t.scenes[i + 1].start_ms) };
        if Some(ve) != expect_end {
            e.push(format!("{}: 映像区間の終わり {ve} が {:?} と違います", s.id, expect_end));
        }
        for (k, note) in s.notes.iter().enumerate() {
            if sub_id_scene(&note.id, 'N') != Some(s.id.as_str()) {
                e.push(format!("{}: ト書き ID が違います: {}", s.id, note.id));
            }
            if !note_ids.insert(note.id.as_str()) {
                e.push(format!("ト書き ID が重複しています: {}", note.id));
            }
            if !(vs <= note.start_ms && note.start_ms <= note.end_ms && note.end_ms <= ve) {
                e.push(format!("{}: シーンの映像区間の外です", note.id));
            }
            if let Some(next) = s.notes.get(k + 1) {
                if next.start_ms < note.start_ms || note.end_ms > next.start_ms {
                    e.push(format!("{} と {} の時刻の順が違います", note.id, next.id));
                }
            }
        }
    }

    // 役・テイク
    let mut cast_names = HashSet::new();
    for c in &t.cast {
        if !cast_names.insert(c.name.as_str()) {
            e.push(format!("役名が重複しています: {}", c.name));
        }
    }
    let mut takes: HashMap<&str, &crate::TakeEntry> = HashMap::new();
    for tk in &t.takes {
        if takes.insert(tk.id.as_str(), tk).is_some() {
            e.push(format!("テイク ID が重複しています: {}", tk.id));
        }
        let distinct: HashSet<_> = tk.tracks.iter().collect();
        if tk.tracks.is_empty() || distinct.len() != tk.tracks.len() {
            e.push(format!("{}: tracks が空か重複しています", tk.id));
        }
        if !tk.missing {
            if tk.reason.is_some() {
                e.push(format!("{}: 正常なテイクに reason があります", tk.id));
            }
            let trims = tk.trims.as_deref().unwrap_or(&[]);
            if tk.trims.is_none()
                || tk.files.len() != tk.tracks.len()
                || trims.len() != tk.tracks.len()
            {
                e.push(format!("{}: files・tracks・trims の件数が合いません", tk.id));
            } else {
                for (i, track) in tk.tracks.iter().enumerate() {
                    if &trims[i].track != track {
                        e.push(format!("{}: trims[{i}].track が tracks[{i}] と違います", tk.id));
                    }
                    let want = match track {
                        None => tk.id.clone(),
                        Some(r) => format!("{}.{r}", tk.id),
                    };
                    if file_stem(&tk.files[i]) != want {
                        e.push(format!("{}: files[{i}] が {want} のファイルではありません", tk.id));
                    }
                }
            }
        } else {
            if tk.trims.is_some() {
                e.push(format!("{}: 不足のテイクに trims があります", tk.id));
            }
            match tk.reason {
                None => e.push(format!("{}: 不足のテイクに reason がありません", tk.id)),
                Some(Reason::NoFile) if !tk.files.is_empty() => {
                    e.push(format!("{}: no_file なのに files があります", tk.id))
                }
                Some(Reason::Duplicate) if tk.files.len() < 2 => {
                    e.push(format!("{}: duplicate なのに files が 2 件未満です", tk.id))
                }
                Some(Reason::NoFile) | Some(Reason::Duplicate) => {}
                Some(_) if tk.files.is_empty() => {
                    e.push(format!("{}: files がありません", tk.id))
                }
                Some(_) => {}
            }
        }
    }

    // 台詞
    let mut line_ids = HashSet::new();
    let mut last_pos = 0usize;
    for l in &t.lines {
        if !line_ids.insert(l.id.as_str()) {
            e.push(format!("台詞 ID が重複しています: {}", l.id));
        }
        match sub_id_scene(&l.id, 'L') {
            Some(sc) if sc == l.scene => {}
            _ => e.push(format!("{}: 台詞 ID の書式かシーンが違います", l.id)),
        }
        match scene_pos.get(l.scene.as_str()) {
            None => e.push(format!("{}: シーン {} がありません", l.id, l.scene)),
            Some(&p) => {
                if p < last_pos {
                    e.push(format!("{}: 台詞の並びとシーンの並びが矛盾します", l.id));
                }
                last_pos = last_pos.max(p);
                if t.scenes[p].silent {
                    e.push(format!("{}: 無言のシーン {} に台詞があります", l.id, l.scene));
                }
            }
        }
        if !cast_names.contains(l.cast.as_str()) {
            e.push(format!("{}: 役 {} がありません", l.id, l.cast));
        }
        match takes.get(l.take.as_str()) {
            None => e.push(format!("{}: テイク {} がありません", l.id, l.take)),
            Some(tk) => {
                let ok = match &l.track {
                    None => tk.tracks.iter().all(|x| x.is_none()),
                    Some(r) => tk.tracks.iter().any(|x| x.as_deref() == Some(r)),
                };
                if !ok {
                    e.push(format!("{}: track がテイク {} に属しません", l.id, l.take));
                }
            }
        }
        if !in_range(l.start_ms, l.end_ms) {
            e.push(format!("{}: 時刻が範囲外です", l.id));
        }
        let finite = |v: Option<f64>| v.is_some_and(|x| x.is_finite());
        if !l.missing {
            if l.reason.is_some()
                || l.norm_clamped.is_none()
                || !finite(l.measured_lufs)
                || !finite(l.requested_gain_db)
                || !finite(l.norm_gain_db)
                || l.src_start_ms.zip(l.src_end_ms).is_none_or(|(a, b)| a > b || a < 0)
            {
                e.push(format!("{}: 正常な台詞の値が欠けているか不正です", l.id));
            }
        } else if l.reason.is_none()
            || l.norm_clamped.is_some()
            || l.measured_lufs.is_some()
            || l.requested_gain_db.is_some()
            || l.norm_gain_db.is_some()
            || l.src_start_ms.is_some()
            || l.src_end_ms.is_some()
        {
            e.push(format!("{}: 不足の台詞の null の規則に合いません", l.id));
        }
    }

    // イベント・数値
    for ev in &t.events {
        if !(0..=dur).contains(&ev.start_ms()) {
            e.push(format!("イベントの時刻が範囲外です: {}", ev.start_ms()));
        }
        if let crate::EventEntry::Se { gain_db, .. } = ev {
            if !gain_db.is_finite() {
                e.push("SE の gain_db が有限ではありません".to_string());
            }
        }
    }
    if !t.normalize.target_lufs.is_finite() {
        e.push("normalize.target_lufs が有限ではありません".to_string());
    }
    if t.schema_version != crate::SCHEMA_VERSION {
        e.push(format!("schema_version が {} です", t.schema_version));
    }
    e
}
```

(`Option::is_none_or` は Rust 1.82 以上。ツールチェーンは 1.96。)

- [ ] **Step 6: テストが通ることを確かめる**

Run: `cargo test -p s2v-timeline`
Expected: 全テスト PASS。

- [ ] **Step 7: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: すべて成功。

```bash
git add Cargo.toml Cargo.lock crates/s2v-timeline
git commit -m "feat: timeline.json v3 の型と検証を持つ s2v-timeline クレートを追加する"
```

---

### Task 2: `#cut`・`#note`・`#visual` の印の解析と検査

**Files:**
- Modify: `crates/s2v-core/src/types.rs:76-100`(`ScriptCommand`)
- Modify: `crates/s2v-core/src/ids.rs`(ID の書式の関数を追加)
- Modify: `crates/s2v-core/src/parser.rs:408-460, 676-689`
- Modify: `crates/s2v-core/src/validate.rs`
- Modify: `crates/s2v-core/src/placement.rs:210`(パターンの追随だけ。本体の変更は Task 4)
- Modify: `crates/s2v-export/src/script_html.rs:81-93`

**Interfaces:**
- Produces:
  - `ScriptCommand::Cut { id: Option<u32>, line_no: usize }`(`id` はカット番号。`Some(2)` が `C02`)
  - `ScriptCommand::Note { id: Option<u32>, text: String, line_no: usize }`
  - `ScriptCommand::Visual { kind: Option<VisualKind>, looped: bool, text: String, line_no: usize }`
  - `s2v_core::ids::format_cut_id(scene: u32, n: u32) -> String`(`S01-C02`)、`format_note_id(scene: u32, n: u32) -> String`(`S01-N01`)、`parse_numbered_tag(s: &str, letter: char) -> Option<u32>`(`"[C02]"` → `Some(2)`。桁数は問わない)

- [ ] **Step 1: 失敗するテストを書く**

`ids.rs` の tests に:

```rust
#[test]
fn cut_and_note_ids() {
    assert_eq!(format_cut_id(1, 2), "S01-C02");
    assert_eq!(format_note_id(12, 3), "S12-N03");
    assert_eq!(format_cut_id(1, 123), "S01-C123");
    assert_eq!(parse_numbered_tag("[C02]", 'C'), Some(2));
    assert_eq!(parse_numbered_tag("[C2]", 'C'), Some(2));
    assert_eq!(parse_numbered_tag("[N01]", 'C'), None);
    assert_eq!(parse_numbered_tag("[C]", 'C'), None);
    assert_eq!(parse_numbered_tag("C02", 'C'), None);
}
```

`parser.rs` の tests に(既存の `parse_script` を使う):

```rust
fn commands(scenes: &[Scene]) -> Vec<ScriptCommand> {
    scenes[0].items.iter().filter_map(|i| match i {
        ScriptItem::Command(c) => Some(c.clone()),
        _ => None,
    }).collect()
}

#[test]
fn cut_with_and_without_number() {
    let (scenes, p) = parse_script("#cut\n#cut[C03]\n");
    let c = commands(&scenes);
    assert!(matches!(c[0], ScriptCommand::Cut { id: None, line_no: 8 }));
    assert!(matches!(c[1], ScriptCommand::Cut { id: Some(3), .. }));
    assert!(p.errors().is_empty(), "{:?}", p.errors());
}

#[test]
fn cut_errors() {
    for bad in ["#cut[C01]\n", "#cut[C00]\n", "#cut[X02]\n", "#cut 名前\n", "#cut[C02] 名前\n"] {
        let (scenes, p) = parse_script(bad);
        assert!(commands(&scenes).is_empty(), "{bad}");
        assert_eq!(p.errors().len(), 1, "{bad}: {:?}", p.errors());
    }
}

#[test]
fn note_with_and_without_number() {
    let (scenes, p) = parse_script("#note 翌朝、港。\n#note[N02] 二人は黙る。\n#note [回想] 十年前\n");
    let c = commands(&scenes);
    assert!(matches!(&c[0], ScriptCommand::Note { id: None, text, .. } if text == "翌朝、港。"));
    assert!(matches!(&c[1], ScriptCommand::Note { id: Some(2), text, .. } if text == "二人は黙る。"));
    assert!(matches!(&c[2], ScriptCommand::Note { id: None, text, .. } if text == "[回想] 十年前"));
    assert!(p.errors().is_empty(), "{:?}", p.errors());
}

#[test]
fn note_errors() {
    for bad in ["#note\n", "#note   \n", "#note[N01]\n", "#note[N00] x\n", "#note[回想]\n"] {
        let (scenes, p) = parse_script(bad);
        assert!(commands(&scenes).is_empty(), "{bad}");
        assert_eq!(p.errors().len(), 1, "{bad}: {:?}", p.errors());
    }
}

#[test]
fn visual_tags() {
    let (scenes, p) = parse_script("#visual (video loop) 波。\n#visual (loop) 炎\n#visual (image) 港\n#visual 補足\n");
    let got: Vec<_> = commands(&scenes).into_iter().map(|c| match c {
        ScriptCommand::Visual { kind, looped, text, .. } => (kind, looped, text),
        other => panic!("{other:?}"),
    }).collect();
    assert_eq!(got, vec![
        (Some(VisualKind::Video), true, "波。".to_string()),
        (None, true, "炎".to_string()),
        (Some(VisualKind::Image), false, "港".to_string()),
        (None, false, "補足".to_string()),
    ]);
    assert!(p.errors().is_empty());
}

#[test]
fn visual_tag_errors() {
    for bad in ["#visual (image video) x\n", "#visual (vidoe) x\n"] {
        let (scenes, p) = parse_script(bad);
        assert!(commands(&scenes).is_empty(), "{bad}");
        assert_eq!(p.errors().len(), 1, "{bad}");
    }
}

#[test]
fn visual_paren_with_japanese_is_text() {
    // Review Focus 1: ASCII の英数字と空白だけの括弧が印。それ以外は本文
    let (scenes, p) = parse_script("#visual (朝) 港の様子\n#visual (2人) 並ぶ\n");
    let c = commands(&scenes);
    assert!(matches!(&c[0], ScriptCommand::Visual { kind: None, looped: false, text, .. } if text == "(朝) 港の様子"));
    assert!(matches!(&c[1], ScriptCommand::Visual { kind: None, text, .. } if text == "(2人) 並ぶ"));
    assert!(p.errors().is_empty());
}
```

既存のテスト `visual_kinds` は、パターンを `ScriptCommand::Visual { kind, text, .. }` に直す。

`validate.rs` の tests に(既存のテストの書き方に合わせ、台本の文字列から `ScriptParser` で作る):

```rust
#[test]
fn cut_and_note_need_numbers_and_unique_ids() {
    let d = check("#paragraph[S01]\n[S01-L001] A:一\n#cut\n#note 翌朝\n");
    assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
    let d = check("#paragraph[S01]\n[S01-L001] A:一\n#cut[C02]\n#cut[C02]\n#note[N01] a\n#note[N01] b\n");
    assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
    // 別のシーンなら同じ番号でよい
    let d = check("#paragraph[S01]\n[S01-L001] A:一\n#cut[C02]\n#paragraph[S02]\n[S02-L001] A:二\n#cut[C02]\n");
    assert!(d.errors.is_empty(), "{:?}", d.errors);
}
```

(`check` は validate.rs の tests にある、台本の本文から `Diagnostics` を返す補助関数。無ければ、`ScriptParser::new().parse_str(&format!("@scene 居間\n@cast\nA:田中\n\n@script\n{body}"))` から `validate` を呼ぶ補助関数を tests に追加する。)

`script_html.rs` の tests に:

```rust
#[test]
fn cut_note_and_loop_are_shown() {
    let mut p = s2v_core::ScriptParser::new();
    let scenes = p.parse_str("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n#visual (video loop) 波\n#cut[C02]\n#note[N01] 翌朝<港>\n").unwrap();
    let h = render_script_html("t", &scenes);
    assert!(h.contains("（動画・ループ）波"), "{h}");
    assert!(h.contains("カット C02"), "{h}");
    assert!(h.contains("ト書き: 翌朝&lt;港&gt;"), "{h}");
}
```

- [ ] **Step 2: テストが失敗する(コンパイルできない)ことを確かめる**

Run: `cargo test -p s2v-core -p s2v-export`
Expected: `format_cut_id` などが未定義、`ScriptCommand::Cut` が未定義でコンパイルエラー。

- [ ] **Step 3: 型と ID の関数を実装する**

`types.rs` の `ScriptCommand` を次のようにする(他の列挙子は変えない):

```rust
    /// `#visual (video loop) 本文`
    Visual {
        kind: Option<VisualKind>,
        /// `loop` の印(動画の素材が短いときにループさせる)
        looped: bool,
        text: String,
        line_no: usize,
    },
    /// `#cut[C02]`。`id` はカット番号(シーンの頭の C01 は暗黙で、ここには来ない)
    Cut { id: Option<u32>, line_no: usize },
    /// `#note[N01] 本文`(ト書き)
    Note {
        id: Option<u32>,
        text: String,
        line_no: usize,
    },
```

`types.rs` の既存テスト `script_command_variants_are_constructible` の `Visual` に `looped: false, line_no: 1` を足し、`Cut`・`Note` も加えて件数を 9 にする。

`ids.rs` に:

```rust
/// カット ID `S01-C02`(カット番号は2桁以上)。
pub fn format_cut_id(scene: u32, n: u32) -> String {
    format!("{}-C{n:02}", format_scene_id(scene))
}

/// ト書き ID `S01-N01`(2桁以上)。
pub fn format_note_id(scene: u32, n: u32) -> String {
    format!("{}-N{n:02}", format_scene_id(scene))
}

/// `[C02]`・`[N01]` の番号を読む(桁数は問わない)。
pub fn parse_numbered_tag(s: &str, letter: char) -> Option<u32> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    parse_digits(inner.strip_prefix(letter)?)
}
```

- [ ] **Step 4: パーサを実装する**

`parse_script_line` の `#paragraph` の判定の直後に:

```rust
            if let Some(after) = cmd.strip_prefix("cut") {
                if after.is_empty() || after.starts_with('[') {
                    return self.parse_cut(after, arg, line_no);
                }
            }
            if let Some(after) = cmd.strip_prefix("note") {
                if after.is_empty() || after.starts_with('[') {
                    return self.parse_note(after, arg, line_no);
                }
            }
```

`match cmd` の `"visual" => Some(parse_visual(arg)),` を `"visual" => self.parse_visual(arg, line_no),` にする。`impl ScriptParser` に:

```rust
    fn error(&mut self, line_no: usize, message: impl Into<String>) {
        self.errors.push(ParseWarning {
            line_no,
            message: message.into(),
        });
    }

    /// `[C02]` などの番号。空なら Ok(None)。書式違い・下限未満は Err(メッセージ)。
    fn numbered(after: &str, letter: char, min: u32, what: &str) -> Result<Option<u32>, String> {
        if after.is_empty() {
            return Ok(None);
        }
        match crate::ids::parse_numbered_tag(after, letter) {
            Some(n) if n >= min => Ok(Some(n)),
            Some(_) if letter == 'C' => Err(format!(
                "{what}の番号は C02 から始まります(C01 はシーンの頭の予約です): {after}"
            )),
            Some(_) => Err(format!("{what}の番号は {letter}01 から始まります: {after}")),
            None => Err(format!(
                "{what}の番号の書式が正しくありません([{letter}02] の形式): {after}"
            )),
        }
    }

    fn parse_cut(&mut self, after: &str, arg: &str, line_no: usize) -> Option<ScriptItem> {
        let id = match Self::numbered(after, 'C', 2, "#cut ") {
            Ok(id) => id,
            Err(m) => {
                self.error(line_no, m);
                return None;
            }
        };
        if !arg.is_empty() {
            self.error(line_no, format!("#cut の後ろには何も書けません: {arg}"));
            return None;
        }
        Some(ScriptItem::Command(ScriptCommand::Cut { id, line_no }))
    }

    fn parse_note(&mut self, after: &str, arg: &str, line_no: usize) -> Option<ScriptItem> {
        let id = match Self::numbered(after, 'N', 1, "#note ") {
            Ok(id) => id,
            Err(m) => {
                self.error(line_no, m);
                return None;
            }
        };
        if arg.is_empty() {
            self.error(line_no, "#note の本文がありません");
            return None;
        }
        Some(ScriptItem::Command(ScriptCommand::Note {
            id,
            text: arg.to_string(),
            line_no,
        }))
    }

    /// `#visual (video loop) 本文`。括弧の中が ASCII の英数字と空白だけなら印として読む。
    fn parse_visual(&mut self, arg: &str, line_no: usize) -> Option<ScriptItem> {
        let mut kind = None;
        let mut looped = false;
        let mut text = arg;
        if let Some(rest) = arg.strip_prefix('(') {
            if let Some(close) = rest.find(')') {
                let inner = &rest[..close];
                let is_tags = !inner.trim().is_empty()
                    && inner
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '\t');
                if is_tags {
                    for w in inner.split_whitespace() {
                        let k = match w {
                            "image" => VisualKind::Image,
                            "video" => VisualKind::Video,
                            "loop" => {
                                looped = true;
                                continue;
                            }
                            other => {
                                self.error(
                                    line_no,
                                    format!("#visual の印「{other}」は使えません(image・video・loop)"),
                                );
                                return None;
                            }
                        };
                        if kind.is_some_and(|old| old != k) {
                            self.error(line_no, "#visual に image と video の両方は書けません");
                            return None;
                        }
                        kind = Some(k);
                    }
                    text = rest[close + 1..].trim();
                }
            }
        }
        Some(ScriptItem::Command(ScriptCommand::Visual {
            kind,
            looped,
            text: text.to_string(),
            line_no,
        }))
    }
```

ファイル末尾の自由関数 `parse_visual` は削除する。

- [ ] **Step 5: 検査(validate)と既存のパターンを直す**

`validate.rs` の `validate` に、`seen_cuts: HashMap<(u32, u32), usize>` と `seen_notes` を加え、`match item` に:

```rust
            ScriptItem::Command(ScriptCommand::Cut { id, line_no })
            | ScriptItem::Command(ScriptCommand::Note { id, line_no, .. }) => {
                let (what, seen) = if matches!(item, ScriptItem::Command(ScriptCommand::Cut { .. })) {
                    ("カット", &mut seen_cuts)
                } else {
                    ("ト書き", &mut seen_notes)
                };
                match (id, cur_scene) {
                    (None, _) => d.errors.push(pw(
                        *line_no,
                        format!("{what}の番号がありません。先に `script2video number` を実行してください"),
                    )),
                    (Some(n), Some(scene)) => {
                        if let Some(prev) = seen.insert((scene, *n), *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!("{what}の番号が重複しています({prev} 行目と同じ)"),
                            ));
                        }
                    }
                    (Some(_), None) => {} // 番号の無い #paragraph の中(そちらがエラーになる)
                }
            }
```

`placement.rs:210` の `ScriptCommand::Visual { kind, text } =>` を `ScriptCommand::Visual { kind, text, .. } =>` にする(本体は Task 4 で書き直す)。Cut・Note は placement の `match cmd` に `ScriptCommand::Cut { .. } | ScriptCommand::Note { .. } => {}` を仮に加える(Task 4 で置き換える)。

`script_html.rs` の `Visual` の分岐を:

```rust
            ScriptItem::Command(ScriptCommand::Visual { kind, looped, text, .. }) => {
                close_table(&mut h, &mut table_open);
                let prefix = match (kind, looped) {
                    (Some(VisualKind::Image), _) => "（画像）",
                    (Some(VisualKind::Video), true) => "（動画・ループ）",
                    (Some(VisualKind::Video), false) => "（動画）",
                    (None, true) => "（ループ）",
                    (None, false) => "",
                };
                h.push_str(&format!("<p class=\"visual\">{}{}</p>\n", prefix, esc(text)));
            }
            ScriptItem::Command(ScriptCommand::Cut { id, .. }) => {
                close_table(&mut h, &mut table_open);
                let label = id.map_or("C??".to_string(), |n| format!("C{n:02}"));
                h.push_str(&format!("<p class=\"cut\">カット {label}</p>\n"));
            }
            ScriptItem::Command(ScriptCommand::Note { text, .. }) => {
                close_table(&mut h, &mut table_open);
                h.push_str(&format!("<p class=\"note\">ト書き: {}</p>\n", esc(text)));
            }
```

- [ ] **Step 6: テストが通ることを確かめる**

Run: `cargo test -p s2v-core -p s2v-export`
Expected: 全 PASS。

- [ ] **Step 7: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-core crates/s2v-export
git commit -m "feat: 台本の #cut・#note と #visual の loop を解析し、番号の有無と重複を検査する"
```

---

### Task 3: `#cut`・`#note` の採番と `.ids` の拡張

**Files:**
- Modify: `crates/s2v-core/src/numbering.rs`
- Modify: `crates/s2v-cli/src/number.rs:127-130`(表示)
- Test: `crates/s2v-cli/tests/number_cli.rs`

**Interfaces:**
- Consumes: Task 2 の `parse_numbered_tag`
- Produces:
  - `IdsRecord { max_scene: u32, scene_max: BTreeMap<u32, u32>, cut_max: BTreeMap<u32, u32>, note_max: BTreeMap<u32, u32>, extra: Vec<String> }`
  - `Numbered { text, record, assigned_lines, assigned_scenes, assigned_cuts: usize, assigned_notes: usize, warnings }`

規則(台本仕様 §4、設計書 §3.3):
- `#cut` は `C02` から、`#note` は `N01` から、シーンごとに振る。既存の番号は変えない。欠番を詰めない。
- シーンごとの次の番号は「`.ids` の記録」と「そのシーンにある既存の番号の最大」の大きい方 + 1。カットは C01 が予約なので、下限を 1 とする(最初は 2)。
- 同じシーンの同じ番号の重複、`#cut[C01]` はエラーで、何も書かない。
- `.ids`: `max_cut.S01=<n>`・`max_note.S01=<n>` を読み書きする。`max_` で始まらないキーの行(`=` を含むもの)は `extra` に保存して、書き戻すときに末尾へそのまま出す。旧形式の `max_line=<n>` は読み捨てる(既存の挙動)。`=` を含まない行は捨てる(既存の挙動)。
- 1回目の走査では、シーン番号がまだ無い `#paragraph` もあるので、シーンを「`#paragraph` の出現順のブロック番号」(最初の `#paragraph` より前がブロック 0)で数え、2回目でシーン番号に対応させる。

- [ ] **Step 1: 失敗するテストを書く(`numbering.rs` の tests)**

```rust
#[test]
fn cuts_and_notes_are_numbered_per_scene() {
    let n = run("#paragraph\n#note 朝\nA:一\n#cut\n#note 昼\n#cut\n#paragraph\n#cut\n#note 夜\n", &IdsRecord::default());
    assert_eq!(n.text, script("#paragraph[S01]\n#note[N01] 朝\n[S01-L001] A:一\n#cut[C02]\n#note[N02] 昼\n#cut[C03]\n#paragraph[S02]\n#cut[C02]\n#note[N01] 夜\n"));
    assert_eq!(n.assigned_cuts, 3);
    assert_eq!(n.assigned_notes, 3);
    assert_eq!(n.record.cut_max, [(1, 3), (2, 2)].into_iter().collect());
    assert_eq!(n.record.note_max, [(1, 2), (2, 1)].into_iter().collect());
    let again = number_script(&n.text, &n.record).unwrap();
    assert_eq!(again.text, n.text);
    assert_eq!((again.assigned_cuts, again.assigned_notes), (0, 0));
}

#[test]
fn inserted_cut_after_existing_higher_number_continues_from_max() {
    // Review Focus 2: 旧 .ids(max_cut が無い)でも、既存の C05 の後に続ける
    let n = run("#paragraph[S01]\n[S01-L001] A:一\n#cut\n#cut[C05]\n", &IdsRecord::parse("max_scene=1\nmax_line.S01=1\n"));
    assert!(n.text.contains("#cut[C06]\n#cut[C05]\n"), "{}", n.text);
    assert_eq!(n.record.cut_max[&1], 6);
}

#[test]
fn record_prevents_reuse_of_deleted_cut_and_note_numbers() {
    let mut r = IdsRecord::default();
    r.cut_max.insert(1, 4);
    r.note_max.insert(1, 7);
    let n = run("#paragraph[S01]\n#cut\n#note x\n", &r);
    assert!(n.text.contains("#cut[C05]\n#note[N08] x\n"), "{}", n.text);
}

#[test]
fn cut_and_note_duplicates_and_c01_are_errors() {
    let errs = number_script(&script("#paragraph[S01]\n#cut[C02]\n#cut[C02]\n#note[N01] a\n#note[N01] b\n"), &IdsRecord::default()).unwrap_err();
    assert_eq!(errs.len(), 2, "{errs:?}");
    let errs = number_script(&script("#paragraph[S01]\n#cut[C01]\n"), &IdsRecord::default()).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    // 別のシーンなら同じ番号でよい
    assert!(number_script(&script("#paragraph[S01]\n#cut[C02]\n#paragraph[S02]\n#cut[C02]\n"), &IdsRecord::default()).is_ok());
}

#[test]
fn cut_and_note_in_s00() {
    let n = run("#note 前置き\nA:一\n#cut\n#paragraph\n", &IdsRecord::default());
    assert!(n.text.contains("#note[N01] 前置き\n[S00-L001] A:一\n#cut[C02]\n#paragraph[S01]\n"), "{}", n.text);
    assert!(n.record.to_text().contains("max_cut.S00=2\n"));
    assert!(n.record.to_text().contains("max_note.S00=1\n"));
}

#[test]
fn cut_and_note_numbering_preserves_crlf_and_bom() {
    // Review Focus 5
    let src = "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01]\r\n  #cut\r\n\t#note  翌朝\r\n";
    let n = number_script(src, &IdsRecord::default()).unwrap();
    assert_eq!(n.text, "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01]\r\n  #cut[C02]\r\n\t#note[N01]  翌朝\r\n");
}

#[test]
fn malformed_cut_and_note_tags_are_untouched() {
    let src = script("#paragraph[S01]\n#cut[X]\n#note[回想] x\n#cutting\n#notes x\n");
    let n = number_script(&src, &IdsRecord::default()).unwrap();
    assert_eq!(n.text, src);
}

#[test]
fn ids_record_keeps_unknown_keys_and_round_trips_cut_and_note() {
    let r = IdsRecord::parse("max_scene=2\nmax_line.S01=3\nmax_cut.S01=4\nmax_note.S02=5\nfuture.key=abc\nmax_line=9\nゴミ\n");
    assert_eq!(r.cut_max, [(1, 4)].into_iter().collect());
    assert_eq!(r.note_max, [(2, 5)].into_iter().collect());
    assert_eq!(r.extra, vec!["future.key=abc".to_string()]);
    let text = r.to_text();
    assert!(text.contains("max_cut.S01=4\n"), "{text}");
    assert!(text.contains("max_note.S02=5\n"), "{text}");
    assert!(text.ends_with("future.key=abc\n"), "{text}");
    assert!(!text.contains("max_line=9"), "{text}");
    assert_eq!(IdsRecord::parse(&text), r);
}
```

既存のテストの補助関数 `rec` は、新しいフィールドを `Default` で埋める(`..IdsRecord::default()`)ように直す。

`number_cli.rs` に、CLI 経由で `#cut`・`#note` に番号が付き、`.ids` に `max_cut`・`max_note` が書かれ、表示に件数が出るテストを1つ加える(既存のテストの書き方に合わせる)。

- [ ] **Step 2: テストが失敗することを確かめる**

Run: `cargo test -p s2v-core numbering`
Expected: `cut_max` などが未定義でコンパイルエラー。

- [ ] **Step 3: 実装する**

`IdsRecord`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdsRecord {
    pub max_scene: u32,
    /// シーン番号 -> そのシーンで使用済みの最大の台詞番号
    pub scene_max: BTreeMap<u32, u32>,
    /// シーン番号 -> 使用済みの最大のカット番号(C01 は予約なので、記録は 2 以上)
    pub cut_max: BTreeMap<u32, u32>,
    /// シーン番号 -> 使用済みの最大のト書き番号
    pub note_max: BTreeMap<u32, u32>,
    /// 知らないキーの行(将来の版の記録)。書き戻すときにそのまま残す
    pub extra: Vec<String>,
}

impl IdsRecord {
    pub fn parse(text: &str) -> Self {
        let mut r = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if !k.starts_with("max_") {
                if !k.is_empty() && !k.starts_with('#') {
                    r.extra.push(line.trim_end().to_string());
                }
                continue;
            }
            // 壊れた値の行は無視する(0 で上書きすると欠番が再利用されてしまう)。
            // 同じキーが複数回正しく読めた場合は、安全側として最大値を採る。
            let Ok(v) = v.trim().parse::<u32>() else {
                continue;
            };
            let per_scene = |prefix: &str| k.strip_prefix(prefix).and_then(parse_scene_id);
            let slot = if k == "max_scene" {
                r.max_scene = r.max_scene.max(v);
                continue;
            } else if let Some(s) = per_scene("max_line.") {
                r.scene_max.entry(s).or_insert(0)
            } else if let Some(s) = per_scene("max_cut.") {
                r.cut_max.entry(s).or_insert(0)
            } else if let Some(s) = per_scene("max_note.") {
                r.note_max.entry(s).or_insert(0)
            } else {
                // 旧形式の `max_line=<n>` や、シーン番号の壊れたキーは捨てる
                continue;
            };
            *slot = (*slot).max(v);
        }
        r
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# script2video number が使用した最大番号(削除した番号を再利用しないための記録)\n",
        );
        out.push_str(&format!("max_scene={}\n", self.max_scene));
        for (key, map) in [("max_line", &self.scene_max), ("max_cut", &self.cut_max), ("max_note", &self.note_max)] {
            for (scene, max) in map {
                out.push_str(&format!("{key}.{}={max}\n", format_scene_id(*scene)));
            }
        }
        for line in &self.extra {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}
```

(上の `to_text` の配列は、rustfmt の整形に従ってよい。出力の順は `max_scene` → 全シーンの `max_line` → `max_cut` → `max_note` → `extra`。)

`Kind` に:

```rust
    /// `#cut` の直後の文字列(`[C02]` など)
    Cut { after: &'a str },
    /// `#note` の直後の文字列
    Note { after: &'a str },
```

`classify` の `#` の分岐を:

```rust
    if let Some(rest) = content.strip_prefix('#') {
        let starts_ok = |after: &str| {
            after.is_empty() || after.starts_with('[') || after.starts_with(char::is_whitespace)
        };
        if let Some(after) = rest.strip_prefix("paragraph") {
            if starts_ok(after) {
                return Kind::Paragraph { after };
            }
        }
        if let Some(after) = rest.strip_prefix("cut") {
            if starts_ok(after) {
                return Kind::Cut { after };
            }
        }
        if let Some(after) = rest.strip_prefix("note") {
            if starts_ok(after) {
                return Kind::Note { after };
            }
        }
        return Kind::Other;
    }
```

既存の番号の読み取り:

```rust
/// `[C02] …` の先頭の番号(`#cut`・`#note` の直後)。
fn existing_tag(after: &str, letter: char) -> Option<u32> {
    let close = after.find(']')?;
    crate::ids::parse_numbered_tag(&after[..=close], letter)
}
```

1回目の走査(既存の番号を集める)に、ブロック番号 `block: usize`(`#paragraph` ごとに +1、最初は 0)を数え、
`seen_cuts: HashMap<(usize, u32), usize>`・`seen_notes` に既存の番号を入れる。重複は既存と同じ形のエラー(「カット番号 C02 が重複しています(N 行目と同じ)」)。`Kind::Cut` の既存の番号が 1 なら「#cut[C01] は使えません(C01 はシーンの頭の予約です)」のエラー。
あわせて `block_cut_max: HashMap<usize, u32>`・`block_note_max` に、ブロックごとの既存の最大を記録する。

2回目の走査では、`#paragraph` でブロック番号を進め、そのブロックのシーン番号 `current_scene` が決まった時点で、カットとト書きの高水位を求める:

```rust
let cut_high = cut_high_by_scene.entry(current_scene).or_insert_with(|| {
    record.cut_max.get(&current_scene).copied().unwrap_or(1).max(1)
});
*cut_high = (*cut_high).max(block_cut_max.get(&block).copied().unwrap_or(1));
```

(ト書きは下限 0 で同様。`cut_high_by_scene`・`note_high_by_scene` は `BTreeMap<u32, u32>`。最初の `#paragraph` より前はブロック 0・シーン 0 で、走査の開始時に同じ計算をしておく。)

`Kind::Cut { after }` の処理:

```rust
            Kind::Cut { after } => {
                if existing_tag(after, 'C').is_some() {
                    out.push_str(raw); // 既存の番号(重複・C01 は1回目で検出済み)
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let high = cut_high_by_scene.get_mut(&current_scene).expect("ブロックの開始で用意済み");
                    let Some(n) = high.checked_add(1) else {
                        return Err(exhausted("カット番号"));
                    };
                    *high = n;
                    assigned_cuts += 1;
                    let at = indent_len + "#cut".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[C{n:02}]"));
                    out.push_str(&raw[at..]);
                } else {
                    out.push_str(raw); // `[X]` など。パーサがエラーにする
                }
            }
```

`Kind::Note` も同じ形(`"#note"`、`[N{n:02}]`、`note_high_by_scene`、`assigned_notes`、「ト書き番号」)。

`Numbered.record` に `cut_max`・`note_max` を入れる。値は、カットはシーンの高水位が 2 以上のシーンだけ、ト書きは 1 以上のシーンだけを入れる(記録の無い・使っていないシーンの行を増やさない)。`extra` は `record.extra.clone()` をそのまま入れる。

`number.rs` の表示を:

```rust
    println!(
        "台詞 {} 行、シーン {} 個、カット {} 個、ト書き {} 個に番号を付けました",
        numbered.assigned_lines, numbered.assigned_scenes, numbered.assigned_cuts, numbered.assigned_notes
    );
```

- [ ] **Step 4: テストが通ることを確かめる**

Run: `cargo test -p s2v-core numbering && cargo test -p s2v-cli --test number_cli`
Expected: 全 PASS(既存のテストも)。

- [ ] **Step 5: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-core/src/numbering.rs crates/s2v-cli
git commit -m "feat: number が #cut・#note にシーンごとの番号を振り、.ids に記録する"
```

---

### Task 4: 配置でのカット・ト書きの記録と、ミリ秒の区間

**Files:**
- Modify: `crates/s2v-core/src/placement.rs`
- Create: `crates/s2v-core/src/visual_timeline.rs`
- Modify: `crates/s2v-core/src/lib.rs`(`pub mod visual_timeline;`)
- Modify: `crates/s2v-export/src/timeline_json.rs`(`PlacedScene` のフィールドの変更に追随するだけ。v3 は Task 5)

**Interfaces:**
- Consumes: Task 2 の `ScriptCommand::Cut`・`Note`・`Visual { looped, line_no }`、`format_cut_id`・`format_note_id`
- Produces(`placement`):
  - `pub struct PlacedVisual { pub text: String, pub kind: Option<VisualKind>, pub looped: bool }`
  - `pub struct PlacedCut { pub number: u32, pub at: Option<i64>, pub visual: Option<PlacedVisual> }`(`number == 1` が C01 で `at == None`。`#cut` は `at == Some(現在位置のサンプル)`)
  - `pub struct PlacedNote { pub number: u32, pub at: i64, pub text: String, pub after_cut: usize }`(`after_cut` はこのト書きの直前のカットの、`cuts` の中の添字)
  - `PlacedScene` から `visual_text`・`visual_kind` を削除し、`pub cuts: Vec<PlacedCut>`(常に1つ以上、先頭は C01)と `pub notes: Vec<PlacedNote>` を追加
- Produces(`visual_timeline`):
  - `pub struct CutSpan { pub id: String, pub start_ms: i64, pub end_ms: i64, pub visual: Option<PlacedVisual> }`
  - `pub struct NoteSpan { pub id: String, pub text: String, pub start_ms: i64, pub end_ms: i64 }`
  - `pub struct SceneSpans { pub cuts: Vec<CutSpan>, pub notes: Vec<NoteSpan> }`
  - `pub fn layout_visuals(scenes: &[PlacedScene], duration_ms: i64) -> (Vec<SceneSpans>, Vec<String>)`(2つ目は警告)

規則(設計書 §3.4):
- 現在位置は `cursor`(既存の変数。全台詞の終了の最大 + その後の `#pause`)。
- `#visual` は直前のカットへ集約する: 本文は改行で連結、`kind` は最初の `Some` を採り、別の `Some` が来たら `p.errors` に「S01-C02: #visual に image と video が混在しています」、`looped` は論理和。
- 最初の `#paragraph` より前の `#cut`・`#note` は、S00 を開く(台詞と同じ)。S00 に台詞も `#pause` も無ければ、既存の「台詞も #pause もないシーンです」のエラーになる。最初の `#paragraph` より前で、何も開いていないときの `#visual` は従来どおり警告して無視する。
- ミリ秒の区間: シーン i の映像区間 `[vs_i, ve_i)`。`vs_0 = 0`、`vs_i = floor_ms(scenes[i].start)`(i ≥ 1)、`ve_i = vs_{i+1}`、最後は `duration_ms`。カット k の始めは C01 なら `vs`、他は `clamp(floor_ms(at), vs, ve)`、終わりは次のカットの始めか `ve`。ト書きの始めは `clamp(floor_ms(at), vs, ve)`、終わりは `min(次のト書きの始め, cuts[after_cut + 1] の始め(あれば), ve)`。
- 警告: 長さ 0 のシーン(「S02 は映像に現れません」)、長さ 0 のカット(C01 は除く)、長さ 0 のト書き。
- **台本の末尾の `#pause`**: 既存の `Placement::end_sample()` は台詞・シーン・イベントの最も遅い時刻で、台詞のあるシーンの後ろの `#pause` を含まない。設計書 §3.4 の「`#cut`・`#note` の後に `#pause 2000` → その間も表示される」を満たすため、`Placement` に `pub end_cursor: i64`(配置を終えた時点の `cursor`)を加え、`end_sample()` はこれも含めた最大値にする。これにより、台本の最後に書いた `#pause` の分だけ `full_dialogue.wav` の末尾に無音が付く(①の既存の台本で末尾が台詞なら、長さは変わらない)。**Ruling**: 設計書の境界条件の表に従うための変更で、①の音声の長さに影響するので Human に報告する。

- [ ] **Step 1: 失敗するテストを書く**

`placement.rs` の tests(既存の `scenes`・`present`・`ms` を使う):

```rust
#[test]
fn cuts_and_notes_record_cursor_after_overlapping_lines() {
    // 長い A(0〜5000)に B を重ね(1000〜2000)、直後の #cut・#note・#se は A の終わり(5000)
    let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B(at=L001+1000):二\n#cut[C02]\n#note[N01] 翌朝\n#se x.wav\n");
    let src = present(&[(1, 1, 5000.0), (1, 2, 1000.0)]);
    let p = place(&flatten(&s), &src, 200);
    let sc = &p.scenes[0];
    assert_eq!(sc.cuts.len(), 2);
    assert_eq!((sc.cuts[0].number, sc.cuts[0].at), (1, None));
    assert_eq!((sc.cuts[1].number, sc.cuts[1].at.map(ms)), (2, Some(5000.0)));
    assert_eq!((sc.notes[0].number, ms(sc.notes[0].at), sc.notes[0].after_cut), (1, 5000.0, 1));
    assert!(matches!(p.events[0], PlacedEvent::Se { start, .. } if ms(start) == 5000.0));
}

#[test]
fn visuals_attach_to_the_current_cut_and_aggregate() {
    let s = scenes("#paragraph[S01]\n#visual (image) 港。\n#visual 船。\n[S01-L001] A:一\n#cut[C02]\n#visual (loop) 波\n#visual (video) 岸壁\n");
    let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
    let c = &p.scenes[0].cuts;
    assert_eq!(c[0].visual, Some(PlacedVisual { text: "港。\n船。".into(), kind: Some(VisualKind::Image), looped: false }));
    assert_eq!(c[1].visual, Some(PlacedVisual { text: "波\n岸壁".into(), kind: Some(VisualKind::Video), looped: true }));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
}

#[test]
fn mixed_image_and_video_in_one_cut_is_an_error() {
    let s = scenes("#paragraph[S01]\n#visual (image) a\n#visual (video) b\n[S01-L001] A:一\n");
    let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    assert!(p.errors[0].contains("S01-C01"), "{:?}", p.errors);
}

#[test]
fn cut_or_note_before_first_paragraph_opens_s00() {
    let s = scenes("#note 前\n[S00-L001] A:一\n#paragraph[S01]\n[S01-L001] A:二\n");
    let p = place(&flatten(&s), &present(&[(0, 1, 1000.0), (1, 1, 1000.0)]), 200);
    assert_eq!(p.scenes[0].id, 0);
    assert_eq!(p.scenes[0].notes.len(), 1);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    // S00 に台詞が無ければエラー
    let s = scenes("#note 前\n#paragraph[S01]\n[S01-L001] A:二\n");
    let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
}
```

```rust
#[test]
fn trailing_pause_extends_end_sample() {
    let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n#note[N01] 余韻\n#pause 2000\n");
    let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
    assert_eq!(ms(p.end_cursor), 3000.0);
    assert_eq!(ms(p.end_sample()), 3000.0);
    // 末尾が台詞なら従来どおり
    let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n");
    let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
    assert_eq!(ms(p.end_sample()), 1000.0);
}
```

既存のテスト `bgm_events_and_visuals_are_recorded` の `visual_text`・`visual_kind` の検査は、`p.scenes[0].cuts[0].visual` の検査に置き換える。

`visual_timeline.rs` の tests(`PlacedScene` を直接組み立てる補助関数を作る):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{PlacedCut, PlacedNote, PlacedScene};

    fn s48(ms: i64) -> i64 { ms * 48 }

    fn scene(id: u32, start_ms: i64, cuts_at: &[i64], notes: &[(i64, usize)]) -> PlacedScene {
        let mut cuts = vec![PlacedCut { number: 1, at: None, visual: None }];
        for (k, &a) in cuts_at.iter().enumerate() {
            cuts.push(PlacedCut { number: k as u32 + 2, at: Some(s48(a)), visual: None });
        }
        PlacedScene {
            id, name: None, index: 0, silent: false, start: s48(start_ms), end: s48(start_ms),
            cuts,
            notes: notes.iter().enumerate().map(|(k, &(a, after))| PlacedNote {
                number: k as u32 + 1, at: s48(a), text: format!("n{k}"), after_cut: after }).collect(),
        }
    }

    fn spans(s: &SceneSpans) -> (Vec<(String, i64, i64)>, Vec<(String, i64, i64)>) {
        (s.cuts.iter().map(|c| (c.id.clone(), c.start_ms, c.end_ms)).collect(),
         s.notes.iter().map(|n| (n.id.clone(), n.start_ms, n.end_ms)).collect())
    }

    #[test]
    fn video_ranges_tile_from_zero_to_duration() {
        let sc = vec![scene(1, 500, &[3000], &[(500, 0), (3000, 1)]), scene(2, 6000, &[], &[])];
        let (v, w) = layout_visuals(&sc, 9000);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(spans(&v[0]).0, vec![("S01-C01".into(), 0, 3000), ("S01-C02".into(), 3000, 6000)]);
        // N01 は次のカット(3000)で、N02 はシーンの終わり(6000)で終わる
        assert_eq!(spans(&v[0]).1, vec![("S01-N01".into(), 500, 3000), ("S01-N02".into(), 3000, 6000)]);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 6000, 9000)]);
    }

    #[test]
    fn cut_before_first_line_makes_c01_empty_without_warning() {
        // #paragraph の直後の #cut: 現在位置は前のシーンの終わり(シーンの開始より前)→ シーンの始めに収める
        let sc = vec![scene(1, 0, &[], &[]), scene(2, 4000, &[3000], &[])];
        let (v, w) = layout_visuals(&sc, 8000);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 4000, 4000), ("S02-C02".into(), 4000, 8000)]);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn same_position_cuts_and_notes_warn() {
        // #cut #cut #note #note(どちらのト書きも C03 の後)
        let sc = vec![scene(1, 0, &[1000, 1000], &[(1000, 2), (1000, 2)])];
        let (v, w) = layout_visuals(&sc, 3000);
        assert_eq!(spans(&v[0]).0[1], ("S01-C02".into(), 1000, 1000));
        assert_eq!(spans(&v[0]).1[0], ("S01-N01".into(), 1000, 1000));
        assert_eq!(spans(&v[0]).1[1], ("S01-N02".into(), 1000, 3000));
        assert_eq!(w.len(), 2, "{w:?}"); // C02 と N01 が長さ 0
    }

    #[test]
    fn note_after_last_line_without_pause_is_empty() {
        let sc = vec![scene(1, 0, &[], &[(3000, 0)])];
        let (v, w) = layout_visuals(&sc, 3000);
        assert_eq!(spans(&v[0]).1[0], ("S01-N01".into(), 3000, 3000));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn scenes_starting_at_the_same_time_warn() {
        let sc = vec![scene(1, 0, &[], &[]), scene(2, 2000, &[], &[]), scene(3, 2000, &[], &[])];
        let (v, w) = layout_visuals(&sc, 5000);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 2000, 2000)]);
        assert!(w.iter().any(|m| m.contains("S02")), "{w:?}");
    }

    #[test]
    fn sub_millisecond_starts_are_floored_consistently() {
        // シーンの開始 1000.5ms → 1000ms(scene.start_ms と同じ floor)
        let mut s2 = scene(2, 0, &[], &[]);
        s2.start = 48_024;
        let sc = vec![scene(1, 0, &[], &[]), s2];
        let (v, _) = layout_visuals(&sc, 2000);
        assert_eq!(spans(&v[0]).0, vec![("S01-C01".into(), 0, 1000)]);
        assert_eq!(spans(&v[1]).0, vec![("S02-C01".into(), 1000, 2000)]);
    }
}
```

- [ ] **Step 2: テストが失敗することを確かめる**

Run: `cargo test -p s2v-core placement visual_timeline`
Expected: 型が無くコンパイルエラー。

- [ ] **Step 3: 配置を実装する**

`placement.rs` に型を追加し、`PlacedScene` を変える:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedVisual {
    pub text: String,
    pub kind: Option<VisualKind>,
    pub looped: bool,
}

/// カット。`number == 1` はシーンの頭の C01(`at` は None)。
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCut {
    pub number: u32,
    /// `#cut` の行の現在位置(サンプル)
    pub at: Option<i64>,
    pub visual: Option<PlacedVisual>,
}

/// ト書き。
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedNote {
    pub number: u32,
    /// `#note` の行の現在位置(サンプル)
    pub at: i64,
    pub text: String,
    /// 直前のカットの `cuts` での添字
    pub after_cut: usize,
}
```

`PlacedScene` の `visual_text`・`visual_kind` を `pub cuts: Vec<PlacedCut>, pub notes: Vec<PlacedNote>` に置き換える。`OpenScene` も同様に `cuts`(`new` で `vec![PlacedCut { number: 1, at: None, visual: None }]`)と `notes` を持つ。

`place` の `match cmd` を:

```rust
                ScriptCommand::Visual { kind, looped, text, .. } => match open.as_mut() {
                    Some(s) => {
                        let scene_id = s.id;
                        let cut = s.cuts.last_mut().expect("C01 は常にある");
                        match &mut cut.visual {
                            None => {
                                cut.visual = Some(PlacedVisual {
                                    text: text.clone(),
                                    kind: *kind,
                                    looped: *looped,
                                })
                            }
                            Some(v) => {
                                v.text = format!("{}\n{text}", v.text);
                                v.looped |= *looped;
                                match (v.kind, *kind) {
                                    (None, k) => v.kind = k,
                                    (Some(a), Some(b)) if a != b => p.errors.push(format!(
                                        "{}: #visual に image と video が混在しています",
                                        format_cut_id(scene_id, cut.number)
                                    )),
                                    _ => {}
                                }
                            }
                        }
                    }
                    None => p.warnings.push(
                        "#visual が最初の #paragraph より前にあります（無視します）".to_string(),
                    ),
                },
                ScriptCommand::Cut { id, .. } => {
                    let s = open.get_or_insert_with(|| {
                        first_in_scene = true;
                        OpenScene::new(0, None, cursor)
                    });
                    s.cuts.push(PlacedCut {
                        number: id.unwrap_or(0),
                        at: Some(cursor),
                        visual: None,
                    });
                }
                ScriptCommand::Note { id, text, .. } => {
                    let s = open.get_or_insert_with(|| {
                        first_in_scene = true;
                        OpenScene::new(0, None, cursor)
                    });
                    let after_cut = s.cuts.len() - 1;
                    s.notes.push(PlacedNote {
                        number: id.unwrap_or(0),
                        at: cursor,
                        text: text.clone(),
                        after_cut,
                    });
                }
```

(`get_or_insert_with` のクロージャが `first_in_scene` を借用して衝突する場合は、`if open.is_none() { open = Some(OpenScene::new(0, None, cursor)); first_in_scene = true; }` の形にする。台詞の S00 の開き方と同じ。)

`PlacedScene` を作る箇所で `cuts: s.cuts, notes: s.notes` を渡す。`Placement` に `pub end_cursor: i64` を加え、`place` の最後(シーンを閉じた後)に `p.end_cursor = cursor;` とする。`end_sample()` は `l.max(s).max(e).max(self.end_cursor)` にする。`use crate::ids::{format_cut_id, ...}` を加える。

`s2v-export/src/timeline_json.rs` は、この段階ではコンパイルを通すだけにする: `SceneDoc` の `visual` を `s.cuts[0].visual.as_ref().map(|v| VisualDoc { text: v.text.clone(), kind: v.kind })` から作り、テストの `PlacedScene` のリテラルを `cuts: vec![PlacedCut { number: 1, at: None, visual: Some(PlacedVisual { text: "港".into(), kind: Some(VisualKind::Image), looped: false }) }], notes: vec![]` に直し、`Placement` のリテラルに `end_cursor: 0` を加える(v3 への置き換えは Task 5)。

- [ ] **Step 4: ミリ秒の区間を実装する(`visual_timeline.rs`)**

```rust
//! カットとト書きのミリ秒の区間(映像工程設計書 §3.4)。
//! 境目は開始も終了も floor_ms で変換し、カットを隙間なく並べる(最後だけ duration_ms)。

use crate::ids::{format_cut_id, format_note_id, format_scene_id};
use crate::placement::{PlacedScene, PlacedVisual};
use crate::time::floor_ms;

#[derive(Debug, Clone, PartialEq)]
pub struct CutSpan {
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub visual: Option<PlacedVisual>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteSpan {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneSpans {
    pub cuts: Vec<CutSpan>,
    pub notes: Vec<NoteSpan>,
}

/// シーンごとのカット・ト書きの区間と、警告(長さ 0 のシーン・カット・ト書き)。
pub fn layout_visuals(scenes: &[PlacedScene], duration_ms: i64) -> (Vec<SceneSpans>, Vec<String>) {
    let mut out = Vec::with_capacity(scenes.len());
    let mut warnings = Vec::new();
    let starts: Vec<i64> = scenes
        .iter()
        .enumerate()
        .map(|(i, s)| if i == 0 { 0 } else { floor_ms(s.start) })
        .collect();
    for (i, s) in scenes.iter().enumerate() {
        let vs = starts[i];
        let ve = starts.get(i + 1).copied().unwrap_or(duration_ms).max(vs);
        if vs == ve {
            warnings.push(format!(
                "{} は、次のシーンと同じ時刻に始まるので映像に現れません",
                format_scene_id(s.id)
            ));
        }
        let clamp = |samples: i64| floor_ms(samples).clamp(vs, ve);
        let cut_starts: Vec<i64> = s
            .cuts
            .iter()
            .map(|c| c.at.map_or(vs, clamp))
            .collect();
        let mut cuts = Vec::with_capacity(s.cuts.len());
        for (k, c) in s.cuts.iter().enumerate() {
            let start = cut_starts[k];
            let end = cut_starts.get(k + 1).copied().unwrap_or(ve);
            let id = format_cut_id(s.id, c.number);
            if start == end && c.number != 1 && vs != ve {
                warnings.push(format!("{id} は長さが 0 なので映像に現れません"));
            }
            cuts.push(CutSpan { id, start_ms: start, end_ms: end, visual: c.visual.clone() });
        }
        let note_starts: Vec<i64> = s.notes.iter().map(|n| clamp(n.at)).collect();
        let mut notes = Vec::with_capacity(s.notes.len());
        for (k, n) in s.notes.iter().enumerate() {
            let start = note_starts[k];
            let mut end = ve;
            if let Some(&next) = note_starts.get(k + 1) {
                end = end.min(next);
            }
            if let Some(&next_cut) = cut_starts.get(n.after_cut + 1) {
                end = end.min(next_cut);
            }
            let end = end.max(start);
            let id = format_note_id(s.id, n.number);
            if start == end && vs != ve {
                warnings.push(format!("{id} は長さが 0 なので表示されません"));
            }
            notes.push(NoteSpan { id, text: n.text.clone(), start_ms: start, end_ms: end });
        }
        out.push(SceneSpans { cuts, notes });
    }
    (out, warnings)
}
```

(長さ 0 のシーンの中のカット・ト書きは、シーンの警告だけにして個別には警告しない。)

`lib.rs` に `pub mod visual_timeline;` を加える。

- [ ] **Step 5: テストが通ることを確かめる**

Run: `cargo test -p s2v-core && cargo test -p s2v-export`
Expected: 全 PASS。

- [ ] **Step 6: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-core crates/s2v-export
git commit -m "feat: 配置でカット・ト書きを現在位置に記録し、ミリ秒の区間を求める"
```

---

### Task 5: `timeline.json` v3 の書き出し

**Files:**
- Modify: `crates/s2v-export/Cargo.toml`(`s2v-timeline = { path = "../s2v-timeline" }`)
- Modify: `crates/s2v-export/src/timeline_json.rs`

**Interfaces:**
- Consumes: Task 1 の `s2v_timeline::*`、Task 4 の `SceneSpans`
- Produces:
  - `TimelineInput` に `pub script_sha256: String`、`pub audio_sha256: String`、`pub visuals: &'a [SceneSpans]` を追加
  - `pub fn build_timeline(input: &TimelineInput<'_>) -> anyhow::Result<s2v_timeline::Timeline>`(組み立てて `validate()` を通す。違反は「timeline.json の検証に失敗しました(内部の誤り): …」で Err)
  - `timeline_value(input) -> anyhow::Result<serde_json::Value>`(`build_timeline` の結果を Value にする。既存のテスト用)
  - `write_timeline_json(path, input) -> anyhow::Result<()>`(`build_timeline(...)?.to_json_pretty()` を書く)

- [ ] **Step 1: 失敗するテストを書く**

既存のテスト `timeline_v2_shape_and_rounding` を `timeline_v3_shape_and_rounding` に改名し:
- `TimelineInput` に `script_sha256: "a".repeat(64)`、`audio_sha256: "b".repeat(64)`、`visuals: &visuals` を与える。`visuals` は `layout_visuals(&placement.scenes, ceil_ms(216_001))` で作る。
- 期待値: `v["schema_version"] == 3`、`v["script_sha256"]`・`v["audio_sha256"]`、`v["scenes"][0]["cuts"][0]["visual"]["text"] == "港"`・`["kind"] == "image"`・`["loop"] == false`、`v["scenes"][0]["visual"]` が無いこと(`v["scenes"][0].get("visual").is_none()`)。
- テストの `Placement` が §3.6 の不変条件を満たすようにする: `casts` に役 B を加える(台詞の役がすべて `cast` にある)、`takes` を台詞ごとに置く(既存のとおり)、シーンの映像区間がつながるようにする。
- 結果を `s2v_timeline::load_str(&serde_json::to_string(&v).unwrap())` で読み直して成功することも確かめる。

新しいテスト:

```rust
#[test]
fn invalid_input_is_rejected_before_writing() {
    // 台詞の役が cast に無い → 内部の誤りとして Err、ファイルは作らない
    let placement = sample_placement();
    let audio = sample_audio();
    let (visuals, _) = layout_visuals(&placement.scenes, 4_501);
    let mut input = sample_input(&placement, &audio, &visuals);
    input.casts.clear();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("timeline.json");
    let e = write_timeline_json(&path, &input).unwrap_err().to_string();
    assert!(e.contains("内部の誤り"), "{e}");
    assert!(!path.exists());
}
```

(`sample_audio`・`sample_input` は、既存のテストの組み立てを関数に切り出したもの。)

- [ ] **Step 2: テストが失敗することを確かめる**

Run: `cargo test -p s2v-export timeline_json`
Expected: `visuals` などが未定義でコンパイルエラー。

- [ ] **Step 3: 実装する**

既存の `TimelineDoc` などの private な Serialize 用の型を削除し、`s2v_timeline` の型を組み立てる。対応:

```rust
fn reason(r: MissingReason) -> s2v_timeline::Reason {
    use s2v_timeline::Reason as R;
    match r {
        MissingReason::NoFile => R::NoFile,
        MissingReason::Duplicate => R::Duplicate,
        MissingReason::Unreadable => R::Unreadable,
        MissingReason::Silent => R::Silent,
        MissingReason::BadFormat => R::BadFormat,
        MissingReason::Unmeasurable => R::Unmeasurable,
    }
}

fn kind(k: s2v_core::VisualKind) -> s2v_timeline::VisualKind {
    match k {
        s2v_core::VisualKind::Image => s2v_timeline::VisualKind::Image,
        s2v_core::VisualKind::Video => s2v_timeline::VisualKind::Video,
    }
}
```

シーンは `p.scenes` と `input.visuals` を添字で組み合わせる(件数が違えば `anyhow::bail!("内部の誤り: シーンとカットの区間の数が違います")`):

```rust
SceneEntry {
    id: format_scene_id(s.id),
    name: s.name.clone(),
    index: s.index,
    silent: s.silent,
    start_ms: floor_ms(s.start),
    end_ms: ceil_ms(s.end),
    cuts: v.cuts.iter().map(|c| CutEntry {
        id: c.id.clone(),
        start_ms: c.start_ms,
        end_ms: c.end_ms,
        visual: c.visual.as_ref().map(|x| Visual {
            text: x.text.clone(),
            kind: x.kind.map(kind),
            looped: x.looped,
        }),
    }).collect(),
    notes: v.notes.iter().map(|n| NoteEntry {
        id: n.id.clone(),
        text: n.text.clone(),
        start_ms: n.start_ms,
        end_ms: n.end_ms,
    }).collect(),
}
```

台詞・テイク・イベント・役は、既存の `LineDoc`・`TakeDoc`・`EventDoc`・`CastDoc` と同じ値を `LineEntry`・`TakeEntry`・`EventEntry`・`CastEntry` に入れる(丸め `r2`・`finite_r2` も既存どおり)。`schema_version: s2v_timeline::SCHEMA_VERSION`、`script_sha256`・`audio_sha256` は入力のまま。

最後に:

```rust
    let t = Timeline { /* … */ };
    if let Err(errs) = t.validate() {
        anyhow::bail!("timeline.json の検証に失敗しました(内部の誤り): {}", errs.join(" / "));
    }
    Ok(t)
```

`write_timeline_json` は `std::fs::write(path, build_timeline(input)?.to_json_pretty())`。検証が通らなければ書かない。

- [ ] **Step 4: テストが通ることを確かめる**

Run: `cargo test -p s2v-export`
Expected: 全 PASS。`s2v-cli` はまだ新しいフィールドを渡していないのでコンパイルエラーになる。`build.rs` の `TimelineInput` の組み立てに、仮に `script_sha256: String::new()` ではなく(検証で落ちる)、次の最小の変更を入れる: 台本のバイト列から `sha256_hex` を、`full_dialogue.wav` を書いた後に読み直して `sha256_hex` を求め、`layout_visuals(&placement.scenes, ceil_ms(total))` の結果を渡す(警告は `log.warn`)。本格的な統合(ロック・改名)は Task 7。

ただし `script_sha256` の検証は「64 文字の16進」までは §3.6 の項目に無いので、空文字でも通る。手抜きを避けるため、この段階で正しい値を渡すこと。

- [ ] **Step 5: `build_golden.rs` の期待値を v3 に更新する**

`golden_timeline_and_outputs` の `assert_eq!(v["schema_version"], 2)` を 3 に、`v["scenes"][0]["visual"]["text"]` を `v["scenes"][0]["cuts"][0]["visual"]["text"]` にする。さらに次を加える:

```rust
    // カットの区間: S01 [0, 3720)、S02 [3720, 6720)、S03 [6720, duration)
    let cut = |s: usize| (v["scenes"][s]["cuts"][0]["start_ms"].as_i64().unwrap(), v["scenes"][s]["cuts"][0]["end_ms"].as_i64().unwrap());
    assert_eq!(cut(0), (0, 3_720));
    assert_eq!(cut(1), (3_720, 6_720));
    assert_eq!(cut(2).0, 6_720);
    assert_eq!(cut(2).1, v["duration_ms"].as_i64().unwrap());
    // 共通の型で読めて、検証を通る
    s2v_timeline::load_str(&fs::read_to_string(out.join("timeline").join("timeline.json")).unwrap()).unwrap();
```

`s2v-cli` の `[dev-dependencies]` に `s2v-timeline = { path = "../s2v-timeline" }` を加える(Task 6 で本体の依存にもする)。

- [ ] **Step 6: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-export crates/s2v-cli
git commit -m "feat: timeline.json を v3(カット・ト書き・SHA-256)で、共通の型で検証してから書き出す"
```

---

### Task 6: CLI の共通の部品(原子的な書き込み、SHA-256、ロック、ログの追記、予約名、鮮度の検査)と `number` への適用

**Files:**
- Create: `crates/s2v-cli/src/fsutil.rs`
- Create: `crates/s2v-cli/src/lock.rs`
- Create: `crates/s2v-cli/src/freshness.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod fsutil; pub mod freshness; pub mod lock;`)
- Modify: `crates/s2v-cli/src/proclog.rs`
- Modify: `crates/s2v-cli/src/layout.rs:9-14, 127-134`
- Modify: `crates/s2v-cli/src/number.rs`
- Modify: `crates/s2v-cli/Cargo.toml`(`s2v-timeline` を `[dependencies]` へ)
- Test: `crates/s2v-cli/tests/number_cli.rs`

**Interfaces:**
- Produces:
  - `fsutil::write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()>`(同じフォルダの `.<名前>.tmp-<pid>` に書いて `rename`。失敗したら一時ファイルを消す)
  - `fsutil::sha256_file(path: &Path) -> std::io::Result<String>`(1 MiB ずつ読む。64 文字の小文字の16進)
  - `lock::ScriptLock::acquire(script_dir: &Path) -> anyhow::Result<ScriptLock>`(保持中はロック。`Drop` で外れる)、`lock::LOCK_FILE: &str = ".script2video.lock"`
  - `ProcLog::append(path: &Path, command: &str) -> anyhow::Result<ProcLog>`(追記で開き、`== <command> YYYY-MM-DD HH:MM:SS ==` の行を書く。時刻はローカル時刻が取れないので UTC で `std::time::SystemTime` から計算し、`(UTC)` を付ける)
  - `layout::RESERVED_NAMES`(8 個)、`layout::check_reserved_name(script: &Path) -> anyhow::Result<()>`
  - `freshness::check_fresh(script: &Path, need_audio: bool) -> Result<Fresh, String>`、`pub struct Fresh { pub timeline: s2v_timeline::Timeline, pub warnings: Vec<String> }`

鮮度の検査の規則(設計書 §2.4): `timeline/timeline.json` が無い → Err(「timeline/timeline.json がありません。先に build を実行してください」)。`load_str` の失敗 → Err(`LoadError` の表示)。台本の SHA-256 が違う → warnings に「台本が build の後に変更されています。build をやり直してください」。`need_audio` のとき: `full_dialogue.wav` の SHA-256 が違う(または無い)→ Err、`duration_ms == 0` → Err、`duration_ms > 86_400_000` → Err。

- [ ] **Step 1: 失敗するテストを書く**

`fsutil.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        std::fs::write(&p, "old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn sha256_file_matches_known_value() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        std::fs::write(&p, "abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // 1 MiB を超える大きさでも、一度に読んだ値と同じ
        let big = vec![7u8; 3 * 1024 * 1024 + 5];
        std::fs::write(&p, &big).unwrap();
        assert_eq!(sha256_file(&p).unwrap(), crate::work::sha256_hex(&big));
    }
}
```

`lock.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_first_is_held() {
        let d = tempfile::tempdir().unwrap();
        let first = ScriptLock::acquire(d.path()).unwrap();
        let e = ScriptLock::acquire(d.path()).err().unwrap().to_string();
        assert!(e.contains("別の script2video が実行中"), "{e}");
        drop(first);
        assert!(ScriptLock::acquire(d.path()).is_ok());
    }

    #[test]
    fn leftover_lock_file_does_not_block() {
        // Review Focus 4: 異常終了で残ったロックファイル
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(LOCK_FILE), "").unwrap();
        assert!(ScriptLock::acquire(d.path()).is_ok());
    }
}
```

`proclog.rs` の tests:

```rust
    #[test]
    fn append_keeps_previous_content_and_writes_a_header() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Process.log");
        std::fs::write(&p, "[INFO] build\n").unwrap();
        let mut log = ProcLog::append(&p, "render").unwrap();
        log.info("ok");
        drop(log);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("[INFO] build\n== render "), "{text}");
        assert!(text.ends_with("[INFO] ok\n"), "{text}");
    }
```

`layout.rs` の既存テスト `reserved_names_ignore_case` に、`"Output.Stale.MP4"`・`"BUBBLES.JSON"`・`"bubbles.json.bak"`・`".script2video.lock"`・`"Prompts"` を加える。

`freshness.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 台本・音声・timeline.json(v3)を置いたフォルダを作る。
    fn setup(dur: i64) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("s.txt");
        std::fs::write(&script, "台本").unwrap();
        std::fs::write(d.path().join("full_dialogue.wav"), "音声").unwrap();
        let t = minimal_timeline(
            &crate::work::sha256_hex("台本".as_bytes()),
            &crate::work::sha256_hex("音声".as_bytes()),
            dur,
        );
        std::fs::create_dir_all(d.path().join("timeline")).unwrap();
        std::fs::write(d.path().join("timeline").join("timeline.json"), t.to_json_pretty()).unwrap();
        (d, script)
    }

    #[test]
    fn fresh_build_passes_without_warnings() {
        let (_d, script) = setup(1000);
        let f = check_fresh(&script, true).unwrap();
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    }

    #[test]
    fn edited_script_is_a_warning() {
        let (_d, script) = setup(1000);
        std::fs::write(&script, "台本を直した").unwrap();
        let f = check_fresh(&script, true).unwrap();
        assert_eq!(f.warnings.len(), 1);
    }

    #[test]
    fn audio_mismatch_missing_timeline_and_zero_duration_are_errors() {
        let (d, script) = setup(1000);
        std::fs::write(d.path().join("full_dialogue.wav"), "別の音声").unwrap();
        assert!(check_fresh(&script, true).is_err());
        assert!(check_fresh(&script, false).is_ok()); // prompts は音声を見ない
        let (_d2, script2) = setup(0);
        assert!(check_fresh(&script2, true).is_err());
        assert!(check_fresh(&script2, false).is_ok());
        let d3 = tempfile::tempdir().unwrap();
        let e = check_fresh(&d3.path().join("s.txt"), false).err().unwrap();
        assert!(e.contains("build"), "{e}");
    }

    #[test]
    fn v2_timeline_is_rejected() {
        let (d, script) = setup(1000);
        std::fs::write(d.path().join("timeline").join("timeline.json"), "{\"schema_version\":2}").unwrap();
        assert!(check_fresh(&script, false).is_err());
    }
}
```

(`minimal_timeline(script_sha, audio_sha, duration_ms)` は tests 内の補助関数。台詞の無い無言のシーン1つ(`S01`、`silent: true`、`start_ms 0`、`end_ms = duration`、カット `S01-C01` が `[0, duration)`)の `Timeline` を返す。)

`number_cli.rs` に:
- 予約名の台本(`bubbles.json` という名前のファイルを台本として渡す)で エラー 2 になり、何も書かないこと。
- `.script2video.lock` をテストの中で `ScriptLock::acquire` で保持している間は `number` が エラー 2 になり、台本も `.ids` も変わらないこと。

- [ ] **Step 2: テストが失敗することを確かめる**

Run: `cargo test -p s2v-cli`
Expected: モジュールが無くコンパイルエラー。

- [ ] **Step 3: 実装する**

`fsutil.rs`:

```rust
//! ファイルの書き込みの共通の部品。

use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// 同じフォルダの一時ファイルに書いてから置き換える(途中で失敗しても元のファイルは壊れない)。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// ファイルの生のバイト列の SHA-256(64 文字の小文字の16進)。大きなファイルも少しずつ読む。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
```

(`work::sha256_hex` と同じ書式 `format!("{:x}", …)`。)

`lock.rs`:

```rust
//! 台本フォルダの排他ロック(映像工程設計書 §2.3)。プロセスが終われば OS が外す。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub const LOCK_FILE: &str = ".script2video.lock";

pub struct ScriptLock {
    _file: File,
}

impl ScriptLock {
    pub fn acquire(script_dir: &Path) -> anyhow::Result<ScriptLock> {
        let path = script_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("ロックファイルを開けません: {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(ScriptLock { _file: file }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "別の script2video が実行中です(同じ台本フォルダで同時に実行できません): {}",
                script_dir.display()
            ),
            Err(TryLockError::Error(e)) => {
                anyhow::bail!("ロックを取れません: {}: {e}", path.display())
            }
        }
    }
}
```

`proclog.rs` に(既存の `create` は「作り直し」のまま):

```rust
    /// 追記で開き、区切りの行 `== <command> <時刻>(UTC) ==` を書く(prompts・bubbles・render 用)。
    pub fn append(path: &Path, command: &str) -> anyhow::Result<Self> {
        let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let mut log = Self::from_writer(Box::new(f));
        let r = writeln!(log.out, "== {command} {} ==", utc_now()).and_then(|()| log.out.flush());
        if let Err(e) = r {
            log.write_error = Some(e);
        }
        Ok(log)
    }
```

`utc_now()` は `SystemTime::now().duration_since(UNIX_EPOCH)` の秒から、日数と時分秒を計算して `YYYY-MM-DD HH:MM:SS(UTC)` を返す自前の関数(グレゴリオ暦の日付の変換は Howard Hinnant の days_from_civil の逆関数 `civil_from_days` を使う)。テストで `civil_from_days(0) == (1970, 1, 1)`、`civil_from_days(19_000) == (2022, 1, 8)` を確かめる。

`layout.rs`:

```rust
/// 台本のファイル名にできない、出力側の名前(大文字小文字は区別しない。映像工程設計書 §2.2)。
pub const RESERVED_NAMES: [&str; 8] = [
    "full_dialogue.wav",
    "Process.log",
    "output.mp4",
    "output.stale.mp4",
    "bubbles.json",
    "bubbles.json.bak",
    ".script2video.lock",
    "prompts",
];

/// 台本のファイル名が予約名なら Err。どのコマンドも、何かを書く前に呼ぶ。
pub fn check_reserved_name(script: &Path) -> anyhow::Result<()> {
    let name = script
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if RESERVED_NAMES.iter().any(|r| name.eq_ignore_ascii_case(r)) {
        bail!("台本のファイル名 {name} は出力ファイルと同じ名前です。台本の名前を変えてください");
    }
    Ok(())
}
```

`check_layout` の予約名の判定は `check_reserved_name(script)?;` に置き換える。

`freshness.rs`:

```rust
//! 後続コマンド(prompts・bubbles・render)の共通の鮮度の検査(映像工程設計書 §2.4)。

use std::path::{Path, PathBuf};

use s2v_timeline::Timeline;

use crate::fsutil::sha256_file;

/// 24 時間(ミリ秒)。これを超える台本は bubbles・render で扱わない(設計書 §4.2)。
pub const MAX_DURATION_MS: i64 = 86_400_000;

pub struct Fresh {
    pub timeline: Timeline,
    pub warnings: Vec<String>,
}

fn folder(script: &Path) -> PathBuf {
    match script.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Err は終了コード 2 の理由。`need_audio` は bubbles・render(音声と長さも確かめる)。
pub fn check_fresh(script: &Path, need_audio: bool) -> Result<Fresh, String> {
    let dir = folder(script);
    let tl_path = dir.join("timeline").join("timeline.json");
    let text = match std::fs::read_to_string(&tl_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "{} がありません。先に build を実行してください",
                tl_path.display()
            ))
        }
        Err(e) => return Err(format!("{} を読めません: {e}", tl_path.display())),
    };
    let timeline = s2v_timeline::load_str(&text).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    match sha256_file(script) {
        Ok(h) if h == timeline.script_sha256 => {}
        Ok(_) => warnings.push(
            "台本が build の後に変更されています。build をやり直してください".to_string(),
        ),
        Err(e) => return Err(format!("台本を読めません: {}: {e}", script.display())),
    }
    if need_audio {
        let wav = dir.join("full_dialogue.wav");
        match sha256_file(&wav) {
            Ok(h) if h == timeline.audio_sha256 => {}
            Ok(_) => {
                return Err(
                    "full_dialogue.wav が timeline.json と別の build のものです。build をやり直してください"
                        .to_string(),
                )
            }
            Err(e) => return Err(format!("{} を読めません: {e}", wav.display())),
        }
        if timeline.duration_ms == 0 {
            return Err("台本の長さが 0 です(台詞も #pause もありません)".to_string());
        }
        if timeline.duration_ms > MAX_DURATION_MS {
            return Err("台本の長さが 24 時間を超えています".to_string());
        }
    }
    Ok(Fresh { timeline, warnings })
}
```

`number.rs`:
- 最初に `layout::check_reserved_name(script)` を呼び、Err なら `[ERROR]` を出して Failed(何も書かない)。
- 台本を読む前に `lock::ScriptLock::acquire(&台本フォルダ)` を取り、関数の終わりまで保持する(台本フォルダは `script.parent()`、空なら `.`)。取れなければ `[ERROR]` を出して Failed。
- `.ids` と台本の書き込みを `fsutil::write_atomic` に置き換える(`.bak` と HTML は従来どおり `fs::write` でよい)。

- [ ] **Step 4: テストが通ることを確かめる**

Run: `cargo test -p s2v-cli`
Expected: 全 PASS。

- [ ] **Step 5: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-cli
git commit -m "feat: ロック・ログの追記・予約名・原子的な書き込み・鮮度の検査の共通部品を加え、number に適用する"
```

---

### Task 7: `build` の統合(ロック、`output.stale.mp4` への改名、SHA-256)と README

**Files:**
- Modify: `crates/s2v-cli/src/build.rs`
- Test: `crates/s2v-cli/tests/build_golden.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: Task 6 の `ScriptLock`・`sha256_file`、Task 4 の `layout_visuals`、Task 5 の `TimelineInput` の新しいフィールド
- Produces: `fn rename_stale_output(out_dir: &Path) -> std::io::Result<bool>`(build.rs の private。`output.mp4` があれば `output.stale.mp4` に改名して `Ok(true)`、無ければ `Ok(false)`)

規則(設計書 §2.2、台本仕様 §5):
- `check_layout` の直後にロックを取り、`build` の終わりまで保持する。取れなければ `[ERROR]` を出して Failed(何も書かない)。
- `Process.log` を作った後、`run_after_commit` を呼ぶ前に `rename_stale_output` を呼ぶ。失敗したら `log.error("古い output.mp4 を output.stale.mp4 に改名できません: …")` と `log.error("出力していません")` を書いて Failed。**前回の最終成果物は消さない**(`remove_final_artifacts` を呼ばない)。成功して改名したら `log.info("古い output.mp4 を output.stale.mp4 に改名しました")`。
- `output.mp4` がファイルでない(フォルダなど)ときは改名せずエラーにする(`symlink_metadata` で確かめる)。
- 台本は `fs::read` でバイト列として読み、SHA-256 を `sha256_hex` で求めてから UTF-8 として解釈する(Task 5 の仮の実装を、この形に整える)。`audio_sha256` は `write_i24_stereo` の後に `sha256_file(full_dialogue.wav)`。

- [ ] **Step 1: 失敗するテストを書く(`build_golden.rs`)**

```rust
#[test]
fn old_output_mp4_is_renamed_to_stale_and_previous_stale_is_replaced() {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S01_L003", "S03_L001"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    fs::write(f.dir.join("output.mp4"), b"new-old").unwrap();
    fs::write(f.dir.join("output.stale.mp4"), b"older").unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    assert!(!f.dir.join("output.mp4").exists());
    assert_eq!(fs::read(f.dir.join("output.stale.mp4")).unwrap(), b"new-old");
    let log = fs::read_to_string(f.dir.join("Process.log")).unwrap();
    assert!(log.contains("output.stale.mp4"), "{log}");
}

#[test]
fn stale_rename_failure_keeps_previous_outputs() {
    // Review Focus 3: 改名できない(改名先が空でないフォルダ)ときは エラー 2 で、前回の成果物を消さない
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S01_L003", "S03_L001"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    fs::write(f.dir.join("output.mp4"), b"mp4").unwrap();
    fs::create_dir_all(f.dir.join("output.stale.mp4").join("x")).unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert_all_final_artifacts(&f.dir);
    assert_eq!(fs::read(f.dir.join("output.mp4")).unwrap(), b"mp4");
}

#[test]
fn build_fails_while_another_command_holds_the_lock() {
    let f = fixture(SCRIPT);
    let _held = s2v_cli::lock::ScriptLock::acquire(&f.dir).unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert_no_final_artifacts(&f.dir);
}

#[test]
fn sha256_fields_match_the_files() {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S01_L003", "S03_L001"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    run_build(&args(&f, None, false));
    let v = read_json(&f.dir.join("timeline").join("timeline.json"));
    assert_eq!(v["script_sha256"].as_str().unwrap(), s2v_cli::fsutil::sha256_file(&f.script).unwrap());
    assert_eq!(v["audio_sha256"].as_str().unwrap(), s2v_cli::fsutil::sha256_file(&f.dir.join("full_dialogue.wav")).unwrap());
    // 鮮度の検査が通る
    assert!(s2v_cli::freshness::check_fresh(&f.script, true).unwrap().warnings.is_empty());
}

#[test]
fn cuts_and_notes_reach_timeline_v3() {
    let script = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中\n\n@script\n#paragraph[S01] 朝\n#visual (image) 港\n#note[N01] 翌朝、港。\n[S01-L001] A:おはよう。\n#cut[C02]\n#visual (video loop) 波\n#note[N02] 二人は黙る。\n#pause 2000\n";
    let f = fixture(script);
    write_voice(&f.voices.join("S01_L001.wav"));
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    let v = read_json(&f.dir.join("timeline").join("timeline.json"));
    let s = &v["scenes"][0];
    assert_eq!(s["cuts"][0]["id"], "S01-C01");
    assert_eq!(s["cuts"][1]["id"], "S01-C02");
    assert_eq!(s["cuts"][1]["visual"]["kind"], "video");
    assert_eq!(s["cuts"][1]["visual"]["loop"], true);
    let l_end = v["lines"][0]["end_ms"].as_i64().unwrap();
    // C02 と N02 は台詞の終わり(現在位置。floor)から。N01 は C02 で終わる
    assert!((l_end - 1..=l_end).contains(&s["cuts"][1]["start_ms"].as_i64().unwrap()));
    assert_eq!(s["notes"][0]["end_ms"], s["cuts"][1]["start_ms"]);
    assert_eq!(s["notes"][1]["start_ms"], s["cuts"][1]["start_ms"]);
    // N02 は #pause 2000 の間も表示される(最後まで)
    assert_eq!(s["notes"][1]["end_ms"], v["duration_ms"]);
    s2v_timeline::load_str(&fs::read_to_string(f.dir.join("timeline").join("timeline.json")).unwrap()).unwrap();
}

#[test]
fn unnumbered_cut_fails_build() {
    let f = fixture(&SCRIPT.replace("#visual (image) 朝の居間。\n", "#cut\n"));
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
}
```

- [ ] **Step 2: テストが失敗することを確かめる**

Run: `cargo test -p s2v-cli --test build_golden`
Expected: 改名・ロック・SHA の各テストが FAIL。

- [ ] **Step 3: 実装する**

`build.rs` の `build`:

```rust
    if let Err(e) = check_layout(&args.script, args.voice_dir.as_deref(), &cwd) {
        eprintln!("[ERROR] {e:#}");
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }
    let _lock = match crate::lock::ScriptLock::acquire(&out_dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            eprintln!("[ERROR] 出力していません");
            return Ok(Outcome::Failed);
        }
    };
```

台本の読み込み:

```rust
    let script_bytes = std::fs::read(&args.script)
        .with_context(|| format!("台本を読めません: {}", args.script.display()))?;
    let script_sha256 = sha256_hex(&script_bytes);
    let text = String::from_utf8(script_bytes).map_err(|_| {
        anyhow::anyhow!(
            "台本を読めません(UTF-8 で保存してください): {}",
            args.script.display()
        )
    })?;
```

`Job` に `script_sha256: &'a str` を加えて `produce` まで渡す。

`ProcLog::create` の後、`run_after_commit` の前:

```rust
    match rename_stale_output(&out_dir) {
        Ok(true) => log.info("古い output.mp4 を output.stale.mp4 に改名しました"),
        Ok(false) => {}
        Err(e) => {
            log.error(format!(
                "古い output.mp4 を output.stale.mp4 に改名できません: {e}"
            ));
            log.error("出力していません");
            return Ok(Outcome::Failed);
        }
    }
```

```rust
/// コミットポイントの直前に、古い完成動画を output.stale.mp4 に改名する(映像工程設計書 §2.2)。
fn rename_stale_output(out_dir: &Path) -> std::io::Result<bool> {
    let mp4 = out_dir.join("output.mp4");
    match std::fs::symlink_metadata(&mp4) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
        Ok(m) if !m.is_file() => Err(std::io::Error::other(
            "output.mp4 がファイルではありません",
        )),
        Ok(_) => {
            std::fs::rename(&mp4, out_dir.join("output.stale.mp4"))?;
            Ok(true)
        }
    }
}
```

`produce` の 9.(timeline)で:

```rust
    let audio_sha256 = crate::fsutil::sha256_file(&out_dir.join("full_dialogue.wav"))
        .context("full_dialogue.wav を読み直せません")?;
    let (visuals, visual_warnings) =
        s2v_core::visual_timeline::layout_visuals(&placement.scenes, ceil_ms(total));
    for w in &visual_warnings {
        log.warn(w);
    }
```

を加え、`TimelineInput` に `script_sha256: job.script_sha256.to_string()`、`audio_sha256`、`visuals: &visuals` を渡す。

- [ ] **Step 4: README を更新する**

`README.md` を次のように直す:
- 「`output.mp4` に触れない」の3か所(17・54・66 行目付近)を、「`build` は `output.mp4` を作らない。前回の `output.mp4` があれば、出力を始める時点で `output.stale.mp4` に改名する(前の `output.stale.mp4` は上書き)」に直す。
- 予約名の一覧(78 行目付近)に `output.stale.mp4`・`bubbles.json`・`bubbles.json.bak`・`.script2video.lock`・`prompts` を加える。
- 「同じ台本フォルダで `number`・`build` を同時に実行すると、後から始めた方が終了コード 2 で止まる」を加える。
- 台本の書き方の節に、`#cut`・`#note`・`#visual (video loop)` の短い例と、`number` がそれらにも番号を付けることを加える(詳しくは台本仕様 §2.8・§2.9 へのリンク)。
- `timeline.json` が v3 になったこと(`scenes[].cuts[]`・`scenes[].notes[]`、`scenes[].visual` の廃止)を出力の説明に加える。

- [ ] **Step 5: テストが通ることを確かめる**

Run: `cargo test -p s2v-cli`
Expected: 全 PASS。

- [ ] **Step 6: 全体の検証とコミット**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/s2v-cli README.md
git commit -m "feat: build にロックと output.stale.mp4 への改名を加え、v3 に台本と音声の SHA-256 を記録する"
```

- [ ] **Step 7: 手動の確認(Human)**

sample の台本(git 管理外の `sample/`)の写しに `#cut`・`#note` を加え、`script2video number` → `script2video build` を実行して、`timeline/timeline.json` の `cuts`・`notes` と `.ids` の `max_cut`・`max_note` を確認してもらう。インストールは `cargo install --path crates/s2v-cli --locked`。
