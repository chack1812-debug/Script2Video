# Script2Video ①a(単独ファイル)実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 1台詞1ファイルで収録した肉声から、`script2video number`(採番・台本HTML)と `script2video build`(48kHz 変換・無音削除・ラウドネス正規化・配置・音響処理・ミックス・`timeline.json` v2・SRT・FCPXML)を動かす。

**Architecture:** Script2Voice-Rust版(commit `d28ec03`)の `s2v-core`・`s2v-audio`・`s2v-export` をコピーして改造し、新規に `s2v-cli`(実行ファイル `script2video`)を作る。純粋なロジック(ID・採番・照合・配置)は `s2v-core`、信号処理(取り込み・検出・ラウドネス・DSP・リミッタ)は `s2v-audio`、ファイル出力(ミックス・JSON・SRT・FCPXML・台本HTML)は `s2v-export`、オーケストレーションとキャッシュは `s2v-cli` に置く。内部の時刻はすべて 48kHz のサンプル単位の整数で持ち、ミリ秒への変換は出力時だけ行う。

**Tech Stack:** Rust 1.96(edition 2021)、hound 3(WAV)、claxon 0.4(FLAC 読み込み)、rubato 0.15(既存リサンプラ)、realfft 3(既存残響)、serde/serde_json、toml 0.8、clap 4、sha2 0.10、rand 0.8、tempfile 3(テスト)。

**Spec:** `docs/superpowers/specs/2026-10-12-script2video-design.md`(設計書)と `台本仕様.md`。この計画は設計書 §5.0 の **①a** だけを実装する。①b(混合トラックのテイク)・①c(役別トラック)は対象外。

## Global Constraints

- サンプルレートは **48000Hz 固定**(設定では変えない)。元音源は**モノラル前提**。チャンネル数が 1 以外、非有限値、壊れたヘッダは `bad_format`。
- 入力形式: WAV(16/24/32bit 整数、32bit 浮動小数)、`[voice] allow_flac = true` のとき FLAC。
- 内部の位置は 48kHz のサンプル単位の整数。出力の整数ミリ秒は、開始が**切り捨て**、終了が**切り上げ**(`duration_ms` も切り上げ)。ミリ秒の入力はサンプル = ms × 48。
- 10ms RMS 窓は 480 サンプル、ホップ 480、起点はサンプル 0、末尾の端数は実サンプル数の短い窓。「音が現れる位置」は超えた最初の窓の開始、「音が消える位置」は超えた最後の窓の終了。
- 既定値: `trim_threshold_db` -50、`trim_head_margin_ms` 30、`trim_tail_margin_ms` 80、`placeholder_ms_per_char` 200、`target_lufs` -23.0(有効範囲 -40〜-10)、`max_boost_db`/`max_cut_db` 24(0〜48)、`master_ceiling_db` -1.0(-6〜0)、`dump_prefade` false。
- ラウドネスは ITU-R BS.1770-4。測定範囲 400ms 以上はブロック 400ms・オーバーラップ 75%・絶対ゲート -70 LUFS・相対ゲート -10 LU、400ms 未満はゲートなし。台詞ごとに 1 つの固定ゲイン。台詞ごとのピーク制限・リミッタは行わない。
- フェードは正規化工程で 1 回だけ、両端 5ms(240 サンプル)の半コサイン。測定はフェード前。10ms 未満の台詞のフェード長は台詞長の半分。
- `tracks/*.wav` は 32bit 浮動小数ステレオ(マスター前)、`full_dialogue.wav` は 24bit 整数ステレオ(マスターのソフトリミッタ後)。
- 終了コード: 0=全件OK、1=警告・不足あり(出力は完成)、2=台本の構文エラー・設定値の範囲外・`work/` に書けない等(出力なし)。
- **元の収録ファイルは変更しない。**
- `timeline.json` は `schema_version: 2`(設計書 §6 の形)。
- ①a は範囲を示すファイル名(`-` を含む)を照合の対象から外し、警告だけを出す。単独ファイルは §5.9(切り分け)を通さない。
- コード中のコメント・ユーザー向けメッセージは日本語(流用元の規約に合わせる)。
- 検証コマンド: `cargo fmt --check` / `cargo test --workspace` / `cargo build --workspace`。

## Review Focus

1. **日本語や空白を含むパス**(このリポジトリ自体が `宇摩コードワークス` の下にある。`bgm_dir` も空白を許す): 台本・収録フォルダ・出力が日本語と空白を含むパスでも、`number` と `build` が正常に動くこと。→ Task 7(`number_cli` テスト)と Task 17(ゴールデンテスト)で、`台本 テスト` という名前のフォルダを使う。
2. **CRLF 改行と BOM 付きの台本**(Windows のメモ帳で保存した台本): `number` が ID 以外のバイトを変えないこと、`build` が BOM を無視して解析できること。→ Task 6(`crlf_and_bom_are_preserved`)、Task 7(`bom_is_ignored`)。
3. **台詞の本文が `[` で始まる、または `:` を含む**(`A:[笑]それは:どうかな`): 台詞IDと誤認しないこと、役名の区切りは最初の半角 `:` だけであること、`number` を2回実行しても変わらないこと。→ Task 4(`dialogue_text_starting_with_bracket_is_not_an_id`)、Task 6(`bracket_text_is_numbered_once`)。
4. **収録フォルダが空(まだ一本も録っていない)**: すべて仮の無音で `timeline.json`・SRT・音声が出力され、終了コード 1 になること。→ Task 17(`empty_voice_dir_produces_placeholders`)。
5. **小さな声の中に大きなクリック(高クレストの収録)**: 正規化で大きく増幅されても、`tracks/dialogue.wav` は浮動小数でクリップせず、`full_dialogue.wav` はマスターの上限を超えないこと。→ Task 17(`loud_click_is_limited_only_at_master`)。

## File Structure

```
Cargo.toml                         ワークスペース(members = crates/*)
config.toml                        既定設定(Config::builtin が include_str! で埋め込む)
PORTING.md                         流用元のコミットとコピーしたファイルの記録
crates/s2v-core/src/
  lib.rs                           再エクスポート
  config.rs        (改造)          Config / AudioConfig / VoiceConfig / NormalizeConfig / MixConfig と検証
  cast.rs          (改造)          Cast(engine_type を削除)
  types.rs         (改造)          ScriptItem / ScriptCommand / VisualKind / SeAnchor
  parser.rs        (改造)          台本パーサ(新しい @cast、台詞ID、overlap/at、#visual、#se オプション)
  ids.rs           (新規)          LineId / LineRef / シーン番号の書式
  validate.rs      (新規)          構文エラー(終了コード 2)の検査
  numbering.rs     (新規)          number の採番と .ids
  voices.rs        (新規)          収録ファイルの照合(①a は単独ファイルだけ)
  time.rs          (新規)          サンプル⇔ミリ秒の丸め規則
  placement.rs     (新規)          台詞・シーン・イベントの配置(cursor / last_line_end)
  (timeline.rs は削除)
crates/s2v-audio/src/
  lib.rs
  acoustics.rs / early.rs / geometry.rs / reverb.rs / resampler.rs   (流用。early.rs のテスト用設定のみ修正)
  processor.rs     (改造)          render(): 正規化済みモノラル → ステレオ。ピーク正規化・台詞ごとのリミッタを廃止
  filters.rs       (新規)          双二次フィルタ
  input.rs         (新規)          WAV/FLAC 読み込み、モノラル検査、48kHz 変換
  detect.rs        (新規)          10ms RMS 窓、音の範囲、使用範囲
  loudness.rs      (新規)          BS.1770 測定、ゲイン決定、フェード
  calib.rs         (新規)          基準ゲインの較正信号と較正
  limiter.rs       (新規)          マスターのソフトリミッタ
crates/s2v-export/src/
  lib.rs
  wavio.rs         (新規)          BGM/SE の読み込み、32bit float / 24bit の書き出し
  bgm.rs           (新規)          BGM 区間とクロスフェード(流用元 compute_bgm_segments の移植)
  mix.rs           (新規)          トラック別ミックスとマスター
  timeline_json.rs (新規)          timeline.json v2
  srt.rs           (新規)          字幕
  fcpxml.rs        (新規)          FCPXML(トラック別 wav を 3 レーンに置く)
  script_html.rs   (新規)          台本HTML
  (exporter.rs は削除)
crates/s2v-cli/
  src/main.rs                      引数の解析だけ
  src/lib.rs                       Outcome と各コマンドの公開
  src/number.rs                    number コマンド
  src/work.rs                      work/ の中間ファイルとキャッシュ
  src/build.rs                     build コマンド
  src/proclog.rs                   Process.log
  tests/number_cli.rs / tests/build_golden.rs
  examples/verify_loudness.rs      ①a 完了条件の確認ツール
```

## 実装開始前(Human の指示が必要)

設計書 §11 により、Beads の導入(`bd init`)と Obsidian への概要作成は Human の指示に従う。実装に着手する前に、Human に「Beads を導入してタスクを起票するか」を確認する。導入する場合は、この計画の Task 1〜18 をそれぞれ1つの Issue として起票し、各タスクの完了時に close する。

---

### Task 1: ワークスペースの作成と流用元の移植

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `PORTING.md`, `crates/s2v-cli/Cargo.toml`, `crates/s2v-cli/src/main.rs`, `crates/s2v-cli/src/lib.rs`
- Copy: `crates/s2v-core/`, `crates/s2v-audio/`, `crates/s2v-export/`, `Cargo.lock`(流用元 commit `d28ec03`)

**Interfaces:**
- Consumes: なし
- Produces: `s2v_cli::Outcome { Ok = 0, Warnings = 1, Failed = 2 }`、`Outcome::code(self) -> u8`、`Outcome::worst(self, other: Outcome) -> Outcome`。実行ファイル名 `script2video`。

- [ ] **Step 1: 流用元が想定のコミットであることを確認する**

Run: `git -C "D:/UDS/宇摩コードワークス/Script2Voice-Rust版" log --oneline -1`
Expected: `d28ec03 chore(beads): s2v-18b.15 のクローズ記録を追加する`(異なる場合は作業を止め、Human に報告する)

- [ ] **Step 2: 3つのクレートと Cargo.lock をコピーする**

```bash
SRC="D:/UDS/宇摩コードワークス/Script2Voice-Rust版"
mkdir -p crates
cp -r "$SRC/crates/s2v-core" "$SRC/crates/s2v-audio" "$SRC/crates/s2v-export" crates/
cp "$SRC/Cargo.lock" .
rm -rf crates/*/target
```

- [ ] **Step 3: ワークスペースの `Cargo.toml` と `.gitignore` を作る**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/s2v-core", "crates/s2v-audio", "crates/s2v-export", "crates/s2v-cli"]
resolver = "2"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
tracing = "0.1"
```

`.gitignore`:
```
/target
```

- [ ] **Step 4: `s2v-cli` の雛形を作る**

`crates/s2v-cli/Cargo.toml`:
```toml
[package]
name = "s2v-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "script2video"
path = "src/main.rs"

[lib]
path = "src/lib.rs"

[dependencies]
s2v-core = { path = "../s2v-core" }
s2v-audio = { path = "../s2v-audio" }
s2v-export = { path = "../s2v-export" }
anyhow.workspace = true
serde.workspace = true
serde_json = "1"
clap = { version = "4", features = ["derive"] }
sha2 = "0.10"
hound = "3"

[dev-dependencies]
tempfile = "3"
```

`crates/s2v-cli/src/lib.rs`:
```rust
//! Script2Video のコマンド実装。`main.rs` は引数の解析だけを行い、ここを呼ぶ。

/// 終了コード(設計書 5.6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 全件OK
    Ok = 0,
    /// 警告・不足あり(出力は完成)
    Warnings = 1,
    /// 台本の構文エラー等(出力なし)
    Failed = 2,
}

impl Outcome {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 2つの結果のうち重い方を返す。
    pub fn worst(self, other: Outcome) -> Outcome {
        if other.code() > self.code() {
            other
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_picks_the_more_severe_outcome() {
        assert_eq!(Outcome::Ok.worst(Outcome::Warnings), Outcome::Warnings);
        assert_eq!(Outcome::Failed.worst(Outcome::Warnings), Outcome::Failed);
        assert_eq!(Outcome::Warnings.worst(Outcome::Ok), Outcome::Warnings);
    }
}
```

`crates/s2v-cli/src/main.rs`:
```rust
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use s2v_cli::Outcome;

#[derive(Parser)]
#[command(
    name = "script2video",
    version,
    about = "台本と収録音声から、動画用の音声とタイムラインを作る"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 台本に台詞IDとシーン番号を書き込み、台本HTMLを出力する
    Number { script: PathBuf },
    /// 収録音声から音声・字幕・タイムラインを作る
    Build {
        script: PathBuf,
        /// 収録ファイルのフォルダ
        #[arg(long)]
        voice_dir: PathBuf,
        /// 設定ファイル(省略時は同梱の既定設定)
        #[arg(long)]
        config: Option<PathBuf>,
        /// 中間ファイルのキャッシュを使わない
        #[arg(long)]
        no_cache: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Number { .. } | Command::Build { .. } => {
            eprintln!("このコマンドはまだ実装されていません");
            Outcome::Failed
        }
    };
    ExitCode::from(outcome.code())
}
```

- [ ] **Step 5: `PORTING.md` を作る**

```markdown
# 流用元の記録

- 流用元: `D:\UDS\宇摩コードワークス\Script2Voice-Rust版`
- コミット: `d28ec03`(chore(beads): s2v-18b.15 のクローズ記録を追加する)
- コピーしたもの: `crates/s2v-core`、`crates/s2v-audio`、`crates/s2v-export`、`Cargo.lock`
- コピーしなかったもの: `s2v-engines`(音声合成エンジン。Script2Video は持たない)、`s2v-gui`、`s2v-video`(④最終合成で改めて検討)、ルートの `src/`
- 以後は独立して進化させる。流用元へ変更を戻さない。
```

- [ ] **Step 6: ビルドとテストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS(流用元のテスト + `worst_picks_the_more_severe_outcome`)

Run: `cargo run -q -p s2v-cli -- --version`
Expected: `script2video 0.1.0`

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore PORTING.md crates
git commit -m "chore: Script2Voice-Rust版(d28ec03)から s2v-core/audio/export を移植し、s2v-cli の雛形を作る"
```

---

### Task 2: 設定ファイルの再構成

**Files:**
- Modify: `crates/s2v-core/src/config.rs`(全面書き換え)
- Modify: `crates/s2v-core/src/lib.rs`
- Create: `config.toml`

**Interfaces:**
- Consumes: なし
- Produces:
  - `Config { audio: AudioConfig, bgm: BgmConfig, export: ExportConfig, voice: VoiceConfig, normalize: NormalizeConfig, mix: MixConfig }`
  - `Config::from_toml(&str) -> anyhow::Result<Config>`、`Config::from_file(&Path) -> anyhow::Result<Config>`、`Config::builtin() -> Config`、`Config::validate(&self) -> Vec<String>`
  - `VoiceConfig { trim_threshold_db: f64, trim_head_margin_ms: u32, trim_tail_margin_ms: u32, placeholder_ms_per_char: u32, allow_flac: bool }`
  - `NormalizeConfig { target_lufs: f64, max_boost_db: f64, max_cut_db: f64, dump_prefade: bool }`
  - `MixConfig { master_ceiling_db: f64 }`
  - `AudioConfig` は流用元のまま(このタスクでは `reference_gain_db`・`max_gain_db`・`engine_volume_offsets` に serde の既定値を付けるだけ。削除は Task 12)

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/config.rs` の `#[cfg(test)] mod tests` を、次の内容に**置き換える**(流用元のテストは音声合成エンジンの設定を前提にしているため削除する):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[audio]
sample_rate = 48000
microphone_spacing = 0.2
sound_speed = 340.0
air_absorption_coeff = 0.05
room_size = 0.1
reverb_wet = 1.0
reference_dist = 1.0
mic_directivity = 0.2
mic_angle = 30.0

[bgm]
crossfade_s = 3.0
se_fade_out_s = 0.05
"#;

    #[test]
    fn builtin_config_loads_and_is_valid() {
        let c = Config::builtin();
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        assert_eq!(c.audio.sample_rate, 48_000);
    }

    #[test]
    fn omitted_sections_use_defaults() {
        let c = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(c.voice.trim_threshold_db, -50.0);
        assert_eq!(c.voice.trim_head_margin_ms, 30);
        assert_eq!(c.voice.trim_tail_margin_ms, 80);
        assert_eq!(c.voice.placeholder_ms_per_char, 200);
        assert!(!c.voice.allow_flac);
        assert_eq!(c.normalize.target_lufs, -23.0);
        assert_eq!(c.normalize.max_boost_db, 24.0);
        assert_eq!(c.normalize.max_cut_db, 24.0);
        assert!(!c.normalize.dump_prefade);
        assert_eq!(c.mix.master_ceiling_db, -1.0);
        assert!(c.validate().is_empty());
    }

    #[test]
    fn validate_reports_each_out_of_range_value() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.normalize.target_lufs = -5.0;
        c.normalize.max_boost_db = 60.0;
        c.normalize.max_cut_db = -1.0;
        c.mix.master_ceiling_db = 0.5;
        let errors = c.validate();
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors[0].contains("target_lufs"));
    }

    #[test]
    fn validate_rejects_non_48k_sample_rate() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.audio.sample_rate = 44_100;
        let errors = c.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("48000"));
    }

    #[test]
    fn boundary_values_are_accepted() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.normalize.target_lufs = -40.0;
        c.normalize.max_boost_db = 48.0;
        c.normalize.max_cut_db = 0.0;
        c.mix.master_ceiling_db = -6.0;
        assert!(c.validate().is_empty());
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p s2v-core config`
Expected: コンパイルエラー(`VoiceConfig` などが無い)

- [ ] **Step 3: `config.rs` の本体を書き換える**

`crates/s2v-core/src/config.rs` のテスト以外を次の内容にする(`EarlyConfig`・`MaterialConfig` とその既定値関数 `er_*` は流用元から**そのまま残す**。`EngineUrl`・`ConcurrencyConfig`・`HttpConfig` とその既定値関数は削除する):

```rust
use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Script2Video の設定(`config.toml`)。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub audio: AudioConfig,
    pub bgm: BgmConfig,
    /// エクスポート形式の設定(省略可)。
    #[serde(default)]
    pub export: ExportConfig,
    /// 収録音声の取り込みと無音削除(省略可。設計書 5.3、5.6)。
    #[serde(default)]
    pub voice: VoiceConfig,
    /// ラウドネス正規化(省略可。設計書 5.10)。
    #[serde(default)]
    pub normalize: NormalizeConfig,
    /// ミックス(省略可。設計書 5.4)。
    #[serde(default)]
    pub mix: MixConfig,
}

/// エクスポート(FCPXML等)に関する設定。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ExportConfig {
    /// FCPXMLを29.97fps(NTSCドロップフレーム相当)で出力するか(既定 false = 30fps)。
    #[serde(default)]
    pub fcpxml_2997fps: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    /// 48000 固定(検証で 48000 以外を拒否する)。
    pub sample_rate: u32,
    pub microphone_spacing: f64,
    pub sound_speed: f64,
    pub air_absorption_coeff: f64,
    pub room_size: f64,
    pub reverb_wet: f64,
    pub reference_dist: f64,
    /// Task 12 で削除する(基準ゲインの再較正に置き換える)。
    #[serde(default = "legacy_reference_gain_db")]
    pub reference_gain_db: f64,
    /// Task 12 で削除する(台詞ごとのリミッタを廃止する)。
    #[serde(default = "legacy_max_gain_db")]
    pub max_gain_db: f64,
    pub mic_directivity: f64,
    pub mic_angle: f64,
    /// Task 12 で削除する(音声合成エンジンごとの補正。Script2Video では使わない)。
    #[serde(default)]
    pub engine_volume_offsets: HashMap<String, f64>,
    #[serde(default)]
    pub early_reflections: EarlyConfig,
}

fn legacy_reference_gain_db() -> f64 {
    -5.0
}
fn legacy_max_gain_db() -> f64 {
    -1.0
}

// (MaterialConfig / EarlyConfig / er_* / impl Default for EarlyConfig は流用元のまま)

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BgmConfig {
    pub crossfade_s: f64,
    pub se_fade_out_s: f64,
}

/// 収録音声の取り込みと無音削除の設定。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceConfig {
    /// 無音削除のしきい値[dBFS]。10ms 窓の RMS がこれを超えると「音がある」。
    #[serde(default = "d_trim_threshold_db")]
    pub trim_threshold_db: f64,
    /// 最初の音の手前に残す長さ[ms]。
    #[serde(default = "d_trim_head_margin_ms")]
    pub trim_head_margin_ms: u32,
    /// 最後の音の後に残す長さ[ms]。
    #[serde(default = "d_trim_tail_margin_ms")]
    pub trim_tail_margin_ms: u32,
    /// 不足した台詞の仮の長さ[ms/文字]。
    #[serde(default = "d_placeholder_ms_per_char")]
    pub placeholder_ms_per_char: u32,
    /// `.flac` の収録ファイルを許可するか。
    #[serde(default)]
    pub allow_flac: bool,
}

fn d_trim_threshold_db() -> f64 {
    -50.0
}
fn d_trim_head_margin_ms() -> u32 {
    30
}
fn d_trim_tail_margin_ms() -> u32 {
    80
}
fn d_placeholder_ms_per_char() -> u32 {
    200
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            trim_threshold_db: d_trim_threshold_db(),
            trim_head_margin_ms: d_trim_head_margin_ms(),
            trim_tail_margin_ms: d_trim_tail_margin_ms(),
            placeholder_ms_per_char: d_placeholder_ms_per_char(),
            allow_flac: false,
        }
    }
}

/// ラウドネス正規化の設定(設計書 5.10 E)。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NormalizeConfig {
    /// 台詞の目標ラウドネス[LUFS](-40〜-10)。
    #[serde(default = "d_target_lufs")]
    pub target_lufs: f64,
    /// ゲインの上限[dB](0〜48)。
    #[serde(default = "d_gain_limit_db")]
    pub max_boost_db: f64,
    /// ゲインの下限の絶対値[dB](0〜48)。
    #[serde(default = "d_gain_limit_db")]
    pub max_cut_db: f64,
    /// フェード前の音声を `work/lines_prefade/` にも書く(①a 完了条件の検証用)。
    #[serde(default)]
    pub dump_prefade: bool,
}

fn d_target_lufs() -> f64 {
    -23.0
}
fn d_gain_limit_db() -> f64 {
    24.0
}

impl Default for NormalizeConfig {
    fn default() -> Self {
        Self {
            target_lufs: d_target_lufs(),
            max_boost_db: d_gain_limit_db(),
            max_cut_db: d_gain_limit_db(),
            dump_prefade: false,
        }
    }
}

/// ミックスの設定。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MixConfig {
    /// マスターのソフトリミッタの上限[dBFS](-6〜0)。
    #[serde(default = "d_master_ceiling_db")]
    pub master_ceiling_db: f64,
}

fn d_master_ceiling_db() -> f64 {
    -1.0
}

impl Default for MixConfig {
    fn default() -> Self {
        Self {
            master_ceiling_db: d_master_ceiling_db(),
        }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Self::from_toml(&s)
    }

    /// リポジトリ同梱の既定設定(`config.toml`)。
    pub fn builtin() -> Self {
        Self::from_toml(include_str!("../../../config.toml")).expect("同梱の config.toml が不正です")
    }

    /// 値の範囲を検査する(設計書 5.10 E)。範囲外の項目ごとに1つのメッセージを返す。
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        let n = &self.normalize;
        if !(-40.0..=-10.0).contains(&n.target_lufs) {
            e.push(format!(
                "[normalize] target_lufs は -40〜-10 の範囲で指定してください: {}",
                n.target_lufs
            ));
        }
        if !(0.0..=48.0).contains(&n.max_boost_db) {
            e.push(format!(
                "[normalize] max_boost_db は 0〜48 の範囲で指定してください: {}",
                n.max_boost_db
            ));
        }
        if !(0.0..=48.0).contains(&n.max_cut_db) {
            e.push(format!(
                "[normalize] max_cut_db は 0〜48 の範囲で指定してください: {}",
                n.max_cut_db
            ));
        }
        if !(-6.0..=0.0).contains(&self.mix.master_ceiling_db) {
            e.push(format!(
                "[mix] master_ceiling_db は -6〜0 の範囲で指定してください: {}",
                self.mix.master_ceiling_db
            ));
        }
        if self.audio.sample_rate != 48_000 {
            e.push(format!(
                "[audio] sample_rate は 48000 固定です: {}",
                self.audio.sample_rate
            ));
        }
        e
    }
}
```

- [ ] **Step 4: `config.toml` を作る**

リポジトリ直下に `config.toml`:
```toml
# Script2Video 設定ファイル

[audio]
sample_rate = 48000        # 48kHz 固定(変更不可)
microphone_spacing = 0.2
sound_speed = 340.0
air_absorption_coeff = 0.05
room_size = 0.1
reverb_wet = 1.0           # 物理基準値 wet_base へのスケーラ(1.0 = 物理量そのまま)
reference_dist = 1.0
mic_directivity = 0.2
mic_angle = 30.0

[audio.early_reflections]
enabled = true
ear_height = 1.2
listener_offset = [0.0, 0.0]
room_dims_min = [4.0, 5.0, 3.0]
room_dims_max = [25.0, 45.0, 18.0]
early_level = 1.0
wet_distance_slope = 0.1
floor = { reflection_coeff = 0.5, absorption_cutoff_hz = 3500.0 }
ceiling = { reflection_coeff = 0.6, absorption_cutoff_hz = 6000.0 }
front_wall = { reflection_coeff = 0.85, absorption_cutoff_hz = 10000.0 }
back_wall = { reflection_coeff = 0.40, absorption_cutoff_hz = 4000.0 }
side_walls = { reflection_coeff = 0.70, absorption_cutoff_hz = 8000.0 }

[bgm]
crossfade_s = 3.0
se_fade_out_s = 0.05

[export]
fcpxml_2997fps = false

[voice]
trim_threshold_db = -50.0
trim_head_margin_ms = 30
trim_tail_margin_ms = 80
placeholder_ms_per_char = 200
allow_flac = false

[normalize]
target_lufs = -23.0
max_boost_db = 24.0
max_cut_db = 24.0
dump_prefade = false

[mix]
master_ceiling_db = -1.0
```

- [ ] **Step 5: `lib.rs` の再エクスポートを直す**

`crates/s2v-core/src/lib.rs` の `pub use config::{...}` を次に置き換える:
```rust
pub use config::{
    AudioConfig, BgmConfig, Config, EarlyConfig, ExportConfig, MaterialConfig, MixConfig,
    NormalizeConfig, VoiceConfig,
};
```

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS

- [ ] **Step 7: Commit**

```bash
git add config.toml crates/s2v-core/src/config.rs crates/s2v-core/src/lib.rs
git commit -m "feat(core): Script2Video の設定(voice/normalize/mix)と範囲検査を追加し、エンジン設定を削除する"
```

---

### Task 3: キャストの簡素化と新しい @cast 書式

**Files:**
- Modify: `crates/s2v-core/src/cast.rs`(全面書き換え)
- Modify: `crates/s2v-core/src/parser.rs`(`parse_cast_line`・`looks_like_cast_definition`・テスト)
- Modify: `crates/s2v-core/src/types.rs`(テストの `Cast` 生成)
- Modify: `crates/s2v-audio/src/processor.rs`(`engine_vol` の除去、テストの `Cast` 生成)

**Interfaces:**
- Consumes: なし
- Produces: `Cast { name: String, speaker_name: String, pan: f64, distance: f64, volume: f64, params: HashMap<String, serde_json::Value>, height: Option<f64>, height_offset: f64, appearance: Option<String> }`、`Cast::with_offsets(&self, &HashMap<String, f64>) -> Cast`。`@cast` の書式は `役名:話者名,パラメータ...`(台本仕様 §2.4)。

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/parser.rs` の `mod tests` の末尾に追加する:

```rust
    #[test]
    fn cast_line_new_format_with_speaker_and_params() {
        let mut p = ScriptParser::new();
        let scenes = p
            .parse_str("@scene 居間\n@cast\nA:田中,pan=-30,distance=1.5,height=1.6,volume=0.8\n\n@script\nA:こんにちは\n")
            .unwrap();
        let a = &scenes[0].casts["A"];
        assert_eq!(a.speaker_name, "田中");
        assert_eq!(a.pan, -30.0);
        assert_eq!(a.distance, 1.5);
        assert_eq!(a.height, Some(1.6));
        assert_eq!(a.volume, 0.8);
        assert!(p.warnings().is_empty(), "{:?}", p.warnings());
    }

    #[test]
    fn cast_line_speaker_is_optional() {
        let mut p = ScriptParser::new();
        let scenes = p
            .parse_str("@scene 居間\n@cast\nA:pan=10\n\nB:\n\n@script\nA:やあ\n")
            .unwrap();
        assert_eq!(scenes[0].casts["A"].speaker_name, "");
        assert_eq!(scenes[0].casts["A"].pan, 10.0);
        assert_eq!(scenes[0].casts["B"].speaker_name, "");
    }

    #[test]
    fn legacy_script2voice_cast_line_is_rejected_with_warning() {
        let mut p = ScriptParser::new();
        let scenes = p
            .parse_str("@scene 居間\n@cast\nA:ずんだもん:ノーマル,voicevox,pan=-30\n\n@script\n")
            .unwrap();
        assert!(!scenes[0].casts.contains_key("A"));
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].message.contains("Script2Voice 形式"));
    }

    #[test]
    fn unknown_cast_param_warns() {
        let mut p = ScriptParser::new();
        p.parse_str("@scene 居間\n@cast\nA:田中,speedScale=1.2\n\n@script\n")
            .unwrap();
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].message.contains("speedScale"));
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p s2v-core cast_line`
Expected: FAIL(旧パーサは `役名:話者名:style,...` を前提にしている)

- [ ] **Step 3: `cast.rs` を書き換える**

`crates/s2v-core/src/cast.rs` の全体:
```rust
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
```

- [ ] **Step 4: パーサの `@cast` 解析を書き換える**

`crates/s2v-core/src/parser.rs` の `looks_like_cast_definition` と `parse_cast_line` を次に置き換える:

```rust
    /// 収集中のキャストがある状態で読んだ行が、キャスト定義行(`役名:話者名,パラメータ=値`)に
    /// 見えるかを判定する。空行区切りの付け忘れで定義行が自由記述に飲み込まれた可能性を警告するために使う。
    fn looks_like_cast_definition(line: &str) -> bool {
        match line.split_once(':') {
            Some((role, rest)) => {
                !role.trim().is_empty()
                    && !role.contains(char::is_whitespace)
                    && rest.contains('=')
                    && !rest.contains(':')
            }
            None => false,
        }
    }

    fn parse_cast_line(&mut self, line: &str, line_no: usize) {
        let Some((name, remain)) = line.split_once(':') else {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "キャスト定義行の書式（役名:話者名,パラメータ...）になっていません（この行は無視されます）: {line}"
                ),
            });
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!("キャスト定義行に役名がありません（この行は無視されます）: {line}"),
            });
            return;
        }
        if remain.contains(':') {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "Script2Voice 形式(役名:話者名:style,エンジン名,...)のキャスト定義です。Script2Video では「役名:話者名,パラメータ...」で書いてください（この行は無視されます）: {line}"
                ),
            });
            return;
        }
        let mut parts = remain.splitn(2, ',');
        let first = parts.next().unwrap_or("").trim();
        let (speaker_name, params_str) = if first.contains('=') {
            (String::new(), remain.trim())
        } else {
            (first.to_string(), parts.next().unwrap_or(""))
        };

        let (mut params, rejected) = extract_cast_params(params_str);
        self.warn_non_finite(line_no, &rejected);
        let mut take_number = |key: &str| -> Option<f64> {
            let v = params.remove(key)?;
            match v.as_f64() {
                Some(f) => Some(f),
                None => {
                    self.warnings.push(ParseWarning {
                        line_no,
                        message: format!(
                            "パラメータ「{key}」の値が数値ではありません（既定値を使います）: {v}"
                        ),
                    });
                    None
                }
            }
        };
        let pan = take_number("pan").unwrap_or(0.0);
        let distance = take_number("distance").unwrap_or(1.0);
        let volume = take_number("volume").unwrap_or(1.0);
        let height = take_number("height");

        // room_size / reverb_wet は役ごとの残響の上書きとして残す(流用元と同じ優先順 Cast > Scene > 設定)
        let mut kept = HashMap::new();
        let mut unknown: Vec<String> = Vec::new();
        for (k, v) in params {
            match k.as_str() {
                "room_size" | "reverb_wet" if v.as_f64().is_some() => {
                    kept.insert(k, v);
                }
                _ => unknown.push(k),
            }
        }
        unknown.sort();
        for k in unknown {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "キャストのパラメータ「{k}」は使えません（無視します）。使えるのは pan、distance、height、volume です"
                ),
            });
        }

        let cast_key = name.clone();
        self.casts.insert(
            cast_key.clone(),
            Cast {
                name,
                speaker_name,
                pan,
                distance,
                volume,
                params: kept,
                height,
                height_offset: 0.0,
                appearance: None,
            },
        );
        self.pending_cast_name = Some(cast_key);
    }
```

- [ ] **Step 5: 既存テストの台本を新書式に直す**

`crates/s2v-core/src/parser.rs` のテストにある旧書式のキャスト定義行を、新書式に置き換える。規則: `役名:話者名:style,エンジン名,パラメータ...` → `役名:話者名,パラメータ...`(style とエンジン名を消す。パラメータが無ければ `役名:話者名`)。例:

- `ずんだもん:ずんだもん:ノーマル,voicevox,pan=-30,distance=1.0` → `ずんだもん:ずんだもん,pan=-30,distance=1.0`
- `四国めたん:四国めたん:ノーマル,voicevox,pan=30` → `四国めたん:四国めたん,pan=30`
- `A:A:スタイル,voicevox` → `A:A`
- `A:話者:ノーマル,voicevox,pan=0,height=1.7` → `A:話者,pan=0,height=1.7`(文字列リテラル中の `\n` 区切りの台本も同様)

次のテストは、エンジン固有の値(`engine_type`、`style`、`language`、`speedScale` など)を検証するためのものなので削除する: `cast_string_param_is_kept_as_string`、`cast_numeric_param_stays_numeric`。他のテストで `engine_type` や `params["style"]` を参照するアサーションがあれば、その行だけ削除する。`malformed_cast_definition_line_warns_with_line_number` は、期待するメッセージを「役名:話者名,パラメータ...」に直す。

`crates/s2v-core/src/types.rs` のテスト `scene_accepts_casts` の `Cast { ... }` から `engine_type` の行を削除する。

- [ ] **Step 6: `processor.rs` からエンジン補正を除く**

`crates/s2v-audio/src/processor.rs` の次の部分:
```rust
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
```
を次に置き換える:
```rust
        let vol_factor = base_norm * cast.volume;
```
同じファイルのテストの `dummy_cast` などで `Cast { ... }` を作っている箇所から `engine_type` の行を削除する。

- [ ] **Step 7: テストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS

- [ ] **Step 8: Commit**

```bash
git add crates/s2v-core crates/s2v-audio/src/processor.rs
git commit -m "feat(core): @cast を「役名:話者名,パラメータ」に簡素化し、音声合成エンジン関連の項目を削除する"
```

---

### Task 4: 台詞ID・新しいコマンドの解析

**Files:**
- Create: `crates/s2v-core/src/ids.rs`
- Modify: `crates/s2v-core/src/types.rs`、`crates/s2v-core/src/parser.rs`、`crates/s2v-core/src/lib.rs`

**Interfaces:**
- Consumes: Task 3 の `Cast`
- Produces:
  - `ids::LineId { scene: u32, line: u32 }`(`Copy, Eq, Hash, Ord, Serialize`)、`LineId::display() -> String`(`S01-L003`)、`LineId::file_stem() -> String`(`S01_L003`)、`LineId::parse(&str) -> Option<LineId>`、`LineId::parse_file_stem(&str) -> Option<LineId>`
  - `ids::LineRef { line: u32, offset_ms: i64 }`、`ids::parse_line_ref(&str) -> Option<LineRef>`
  - `ids::format_scene_id(u32) -> String`(`S02`)、`ids::parse_scene_id(&str) -> Option<u32>`、`ids::split_line_id_prefix(&str) -> (Option<LineId>, &str)`
  - `ScriptItem::Speech { id: Option<LineId>, line_no: usize, cast_name: String, text: String, offset_params: HashMap<String, f64>, overlap_ms: Option<f64>, at: Option<LineRef>, scene_config: SceneConfig }`
  - `ScriptCommand::{ Pause(f64), Paragraph { id: Option<u32>, name: Option<String>, line_no: usize }, Visual { kind: Option<VisualKind>, text: String }, BgmStart(String), BgmStop, Se { path: String, anchor: Option<SeAnchor>, gain_db: f64, line_no: usize }, Parallel(usize) }`
  - `VisualKind::{ Image, Video }`、`SeAnchor::{ Start(LineRef), End(LineRef) }`
  - `ScriptParser::errors(&self) -> &[ParseWarning]`(終了コード 2 になる問題)

- [ ] **Step 1: `ids.rs` のテストを書く**

`crates/s2v-core/src/ids.rs` を作り、まずテストだけを書く(本体は Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_id_display_and_file_stem() {
        let id = LineId { scene: 1, line: 3 };
        assert_eq!(id.display(), "S01-L003");
        assert_eq!(id.file_stem(), "S01_L003");
        let big = LineId { scene: 123, line: 4567 };
        assert_eq!(big.display(), "S123-L4567");
    }

    #[test]
    fn line_id_parse_accepts_any_digit_count() {
        assert_eq!(LineId::parse("S01-L003"), Some(LineId { scene: 1, line: 3 }));
        assert_eq!(LineId::parse("S1-L12345"), Some(LineId { scene: 1, line: 12345 }));
        assert_eq!(LineId::parse("S01-L"), None);
        assert_eq!(LineId::parse("S01_L003"), None);
        assert_eq!(LineId::parse("S01-L003x"), None);
        assert_eq!(LineId::parse_file_stem("S01_L003"), Some(LineId { scene: 1, line: 3 }));
        assert_eq!(LineId::parse_file_stem("S01_L003-L007"), None);
    }

    #[test]
    fn scene_id_format_and_parse() {
        assert_eq!(format_scene_id(2), "S02");
        assert_eq!(parse_scene_id("S02"), Some(2));
        assert_eq!(parse_scene_id("S"), None);
        assert_eq!(parse_scene_id("X02"), None);
    }

    #[test]
    fn split_line_id_prefix_only_takes_valid_ids() {
        let (id, rest) = split_line_id_prefix("[S01-L003] A:おはよう");
        assert_eq!(id, Some(LineId { scene: 1, line: 3 }));
        assert_eq!(rest, "A:おはよう");
        let (id, rest) = split_line_id_prefix("[笑] A:おはよう");
        assert_eq!(id, None);
        assert_eq!(rest, "[笑] A:おはよう");
    }

    #[test]
    fn parse_line_ref_variants() {
        assert_eq!(parse_line_ref("L003+500"), Some(LineRef { line: 3, offset_ms: 500 }));
        assert_eq!(parse_line_ref("L003"), Some(LineRef { line: 3, offset_ms: 0 }));
        assert_eq!(parse_line_ref("L003-200"), Some(LineRef { line: 3, offset_ms: -200 }));
        assert_eq!(parse_line_ref("S01-L003+10"), Some(LineRef { line: 3, offset_ms: 10 }));
        assert_eq!(parse_line_ref("L+5"), None);
        assert_eq!(parse_line_ref("003+5"), None);
        assert_eq!(parse_line_ref("L003+"), None);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-core/src/lib.rs` に `pub mod ids;` を追加してから実行する。

Run: `cargo test -p s2v-core ids`
Expected: コンパイルエラー(`LineId` が無い)

- [ ] **Step 3: `ids.rs` の本体を書く**

`crates/s2v-core/src/ids.rs` のテストの上に:
```rust
//! 台詞ID(`S01-L003`)とシーン番号(`S02`)の書式(台本仕様 §1、§4)。

use serde::{Deserialize, Serialize};

/// 台詞ID。台詞番号 `line` は台本全体で一意。`scene` は採番時のシーン番号(表示用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineId {
    pub scene: u32,
    pub line: u32,
}

impl LineId {
    /// `S01-L003` の形式。シーンは2桁以上、台詞は3桁以上。
    pub fn display(&self) -> String {
        format!("S{:02}-L{:03}", self.scene, self.line)
    }

    /// 収録ファイル名の語幹 `S01_L003`。
    pub fn file_stem(&self) -> String {
        format!("S{:02}_L{:03}", self.scene, self.line)
    }

    /// `S01-L003` を読む(桁数は問わない)。
    pub fn parse(s: &str) -> Option<LineId> {
        Self::parse_with_sep(s, "-L")
    }

    /// 収録ファイル名の語幹 `S01_L003` を読む。範囲(`S01_L003-L007`)は None。
    pub fn parse_file_stem(s: &str) -> Option<LineId> {
        Self::parse_with_sep(s, "_L")
    }

    fn parse_with_sep(s: &str, sep: &str) -> Option<LineId> {
        let rest = s.strip_prefix('S')?;
        let (scene, line) = rest.split_once(sep)?;
        Some(LineId {
            scene: parse_digits(scene)?,
            line: parse_digits(line)?,
        })
    }
}

/// 台詞の参照 `L003+500`(`at=`、`#se at=/end=`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRef {
    pub line: u32,
    pub offset_ms: i64,
}

/// `L003`、`L003+500`、`L003-200`、`S01-L003+500` を読む。
pub fn parse_line_ref(s: &str) -> Option<LineRef> {
    let s = s.trim();
    let s = match s.split_once("-L") {
        Some((scene, rest)) if scene.starts_with('S') && parse_digits(&scene[1..]).is_some() => {
            rest
        }
        _ => s.strip_prefix('L')?,
    };
    let split = s.find(['+', '-']).unwrap_or(s.len());
    let line = parse_digits(&s[..split])?;
    let offset_ms = if split == s.len() {
        0
    } else {
        let sign: i64 = if s.as_bytes()[split] == b'-' { -1 } else { 1 };
        sign * parse_digits(&s[split + 1..])? as i64
    };
    Some(LineRef { line, offset_ms })
}

/// シーン番号 `S02`(2桁以上)。
pub fn format_scene_id(n: u32) -> String {
    format!("S{n:02}")
}

/// `S02` を読む。
pub fn parse_scene_id(s: &str) -> Option<u32> {
    parse_digits(s.strip_prefix('S')?)
}

/// 行頭の `[S01-L003]` を取り出す。台詞IDの書式でなければ (None, 元の文字列)。
pub fn split_line_id_prefix(content: &str) -> (Option<LineId>, &str) {
    if let Some(rest) = content.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            if let Some(id) = LineId::parse(&rest[..close]) {
                return (Some(id), rest[close + 1..].trim_start());
            }
        }
    }
    (None, content)
}

fn parse_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
```

Run: `cargo test -p s2v-core ids`
Expected: PASS

- [ ] **Step 4: パーサの新しい動作のテストを書く**

`crates/s2v-core/src/parser.rs` の `mod tests` の末尾に追加する:
```rust
    const CAST_HEADER: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n";

    fn parse_script(body: &str) -> (Vec<Scene>, ScriptParser) {
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(&format!("{CAST_HEADER}{body}")).unwrap();
        (scenes, p)
    }

    #[test]
    fn speech_line_with_id_prefix() {
        let (scenes, p) = parse_script("[S01-L003] A:おはよう。\n");
        match &scenes[0].items[0] {
            ScriptItem::Speech { id, cast_name, text, line_no, .. } => {
                assert_eq!(*id, Some(crate::ids::LineId { scene: 1, line: 3 }));
                assert_eq!(cast_name, "A");
                assert_eq!(text, "おはよう。");
                assert_eq!(*line_no, 8);
            }
            other => panic!("{other:?}"),
        }
        assert!(p.errors().is_empty());
    }

    #[test]
    fn dialogue_text_starting_with_bracket_is_not_an_id() {
        let (scenes, _) = parse_script("A:[笑]それは:どうかな\n");
        match &scenes[0].items[0] {
            ScriptItem::Speech { id, text, .. } => {
                assert_eq!(*id, None);
                assert_eq!(text, "[笑]それは:どうかな");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn overlap_and_at_and_offsets_are_extracted() {
        let (scenes, p) = parse_script(
            "[S01-L001] A:一\n[S01-L002] B(overlap=400,pan=10):二\n[S01-L003] A(at=L001+500,volume=-0.2):三\n",
        );
        match &scenes[0].items[1] {
            ScriptItem::Speech { overlap_ms, at, offset_params, .. } => {
                assert_eq!(*overlap_ms, Some(400.0));
                assert_eq!(*at, None);
                assert_eq!(offset_params.get("pan"), Some(&10.0));
            }
            other => panic!("{other:?}"),
        }
        match &scenes[0].items[2] {
            ScriptItem::Speech { at, offset_params, .. } => {
                assert_eq!(*at, Some(crate::ids::LineRef { line: 1, offset_ms: 500 }));
                assert_eq!(offset_params.get("volume"), Some(&-0.2));
            }
            other => panic!("{other:?}"),
        }
        assert!(p.warnings().is_empty(), "{:?}", p.warnings());
    }

    #[test]
    fn paragraph_with_scene_id_and_name() {
        let (scenes, _) = parse_script("#paragraph[S02] 港の朝\n#paragraph\n#paragraph[S03]\n");
        let ids: Vec<_> = scenes[0]
            .items
            .iter()
            .map(|i| match i {
                ScriptItem::Command(ScriptCommand::Paragraph { id, name, .. }) => (*id, name.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            ids,
            vec![(Some(2), Some("港の朝".to_string())), (None, None), (Some(3), None)]
        );
    }

    #[test]
    fn visual_kinds() {
        let (scenes, _) = parse_script("#visual (image) 朝の港。\n#visual (video) パンする。\n#visual 補足\n");
        let got: Vec<_> = scenes[0]
            .items
            .iter()
            .map(|i| match i {
                ScriptItem::Command(ScriptCommand::Visual { kind, text }) => (*kind, text.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(VisualKind::Image), "朝の港。".to_string()),
                (Some(VisualKind::Video), "パンする。".to_string()),
                (None, "補足".to_string()),
            ]
        );
    }

    #[test]
    fn se_options_and_filename_with_spaces() {
        let (scenes, _) = parse_script("#se door open.wav at=L003+200 gain=-3\n#se bell.wav end=L004-100\n");
        match &scenes[0].items[0] {
            ScriptItem::Command(ScriptCommand::Se { path, anchor, gain_db, .. }) => {
                assert!(path.ends_with("door open.wav"), "{path}");
                assert_eq!(
                    *anchor,
                    Some(SeAnchor::Start(crate::ids::LineRef { line: 3, offset_ms: 200 }))
                );
                assert_eq!(*gain_db, -3.0);
            }
            other => panic!("{other:?}"),
        }
        match &scenes[0].items[1] {
            ScriptItem::Command(ScriptCommand::Se { anchor, gain_db, .. }) => {
                assert_eq!(
                    *anchor,
                    Some(SeAnchor::End(crate::ids::LineRef { line: 4, offset_ms: -100 }))
                );
                assert_eq!(*gain_db, 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn undefined_cast_is_an_error() {
        let (scenes, p) = parse_script("C:だれ？\n");
        assert!(scenes[0].items.is_empty());
        assert_eq!(p.errors().len(), 1);
        assert!(p.errors()[0].message.contains("C"));
    }

    #[test]
    fn ruby_like_quote_is_kept_as_plain_text() {
        let (scenes, _) = parse_script("A:'今日|きょう'は晴れ\n");
        match &scenes[0].items[0] {
            ScriptItem::Speech { text, .. } => assert_eq!(text, "'今日|きょう'は晴れ"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_command_and_bad_pause_warn() {
        let (scenes, p) = parse_script("#fade 100\n#pause abc\n");
        assert!(scenes[0].items.is_empty());
        assert_eq!(p.warnings().len(), 2);
    }
```

- [ ] **Step 5: テストが失敗することを確認する**

Run: `cargo test -p s2v-core parser`
Expected: コンパイルエラー(`errors()`、`VisualKind` などが無い)

- [ ] **Step 6: `types.rs` を変更する**

`crates/s2v-core/src/types.rs` の `ScriptItem` と `ScriptCommand` を次に置き換え、`VisualKind` と `SeAnchor` を追加する(`SceneConfig`・`PauseConfig`・`Scene` はそのまま):
```rust
use crate::ids::{LineId, LineRef};

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
    /// `#visual (image) 本文`
    Visual {
        kind: Option<VisualKind>,
        text: String,
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
```
同じファイルのテスト `script_item_speech_round_trip` と `script_command_variants_are_constructible` を、新しいフィールドで書き直す:
```rust
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
            scene_config: SceneConfig::new("室内"),
        };
        if let ScriptItem::Speech { cast_name, text, .. } = &item {
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
            ScriptCommand::Paragraph { id: Some(1), name: None, line_no: 1 },
            ScriptCommand::Visual { kind: Some(VisualKind::Image), text: "港".to_string() },
            ScriptCommand::BgmStart("bgm01.wav".to_string()),
            ScriptCommand::BgmStop,
            ScriptCommand::Se { path: "se_door.wav".to_string(), anchor: None, gain_db: 0.0, line_no: 2 },
            ScriptCommand::Parallel(3),
        ];
        assert_eq!(cmds.len(), 7);
    }
```
`lib.rs` の `pub use types::{...}` に `SeAnchor, VisualKind` を追加し、`pub use ids::{LineId, LineRef};` を追加する。

- [ ] **Step 7: パーサを変更する**

`crates/s2v-core/src/parser.rs` を次のように変更する。

(a) `use` に追加する:
```rust
use crate::ids::{parse_line_ref, parse_scene_id, split_line_id_prefix, LineRef};
use crate::types::{SeAnchor, VisualKind};
```

(b) `ScriptParser` に `errors: Vec<ParseWarning>` を追加し、`new()` で `errors: Vec::new()` を初期化、`parse_str` の冒頭で `self.errors.clear();` を呼ぶ。アクセサを追加する:
```rust
    /// 出力を止める問題(終了コード 2)。未定義の役名など。
    pub fn errors(&self) -> &[ParseWarning] {
        &self.errors
    }
```

(c) `parse_script_line` を次に置き換える:
```rust
    fn parse_script_line(&mut self, line: &str, line_no: usize) -> Option<ScriptItem> {
        let trimmed = line.trim();
        // 数字のみ → 同時発声(流用元と同じ)
        if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
            let n: usize = trimmed.parse().ok()?;
            return Some(ScriptItem::Command(ScriptCommand::Parallel(n)));
        }

        if let Some(rest) = trimmed.strip_prefix('#') {
            // `# ` または `#\t` → コメント
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
                return None;
            }
            let (cmd, arg) = match rest.split_once(char::is_whitespace) {
                Some((c, a)) => (c, a.trim()),
                None => (rest, ""),
            };
            if let Some(after) = cmd.strip_prefix("paragraph") {
                if after.is_empty() || after.starts_with('[') {
                    return self.parse_paragraph(after, arg, line_no);
                }
            }
            return match cmd {
                "pause" => match arg.parse::<f64>() {
                    Ok(ms) if ms.is_finite() && ms >= 0.0 => {
                        Some(ScriptItem::Command(ScriptCommand::Pause(ms)))
                    }
                    _ => {
                        self.warnings.push(ParseWarning {
                            line_no,
                            message: format!("#pause の値が正しくありません（この行は無視されます）: {arg}"),
                        });
                        None
                    }
                },
                "visual" => Some(parse_visual(arg)),
                "bgm_start" => Some(ScriptItem::Command(ScriptCommand::BgmStart(
                    self.resolve_asset_path("bgm_dir", "BGM", arg, line_no),
                ))),
                "bgm_stop" => Some(ScriptItem::Command(ScriptCommand::BgmStop)),
                "se" => self.parse_se(arg, line_no),
                other => {
                    self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("不明なコマンド「#{other}」です（この行は無視されます）"),
                    });
                    None
                }
            };
        }

        // 台詞行: `[S01-L003] 役名(params):テキスト`(区切りは半角コロンのみ)
        let (id, content) = split_line_id_prefix(trimmed);
        let Some((name_part, raw_text)) = content.split_once(':') else {
            if content.contains('：') {
                self.warnings.push(ParseWarning {
                    line_no,
                    message: "行に全角「：」が使われています。役名の区切りは半角「:」です（この行は無視されます）".to_string(),
                });
            }
            return None;
        };
        let name_part = name_part.trim();
        let (role, params_str) = if let Some(idx) = name_part.find('(') {
            (name_part[..idx].trim(), name_part[idx + 1..].trim_end_matches(')'))
        } else {
            (name_part, "")
        };

        if !self.casts.contains_key(role) {
            let hint = if role.contains('：') {
                "（役名の区切りが全角「：」になっていないか確認してください）"
            } else {
                ""
            };
            self.errors.push(ParseWarning {
                line_no,
                message: format!("キャスト「{role}」が未定義です{hint}"),
            });
            return None;
        }

        let (offset_params, overlap_ms, at) = self.parse_speech_params(params_str, line_no);
        Some(ScriptItem::Speech {
            id,
            line_no,
            cast_name: role.to_string(),
            text: raw_text.trim().to_string(),
            offset_params,
            overlap_ms,
            at,
            scene_config: crate::types::SceneConfig::new(String::new()),
        })
    }

    fn parse_paragraph(&mut self, after: &str, arg: &str, line_no: usize) -> Option<ScriptItem> {
        let id = if after.is_empty() {
            None
        } else {
            match after
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .and_then(parse_scene_id)
            {
                Some(n) => Some(n),
                None => {
                    self.warnings.push(ParseWarning {
                        line_no,
                        message: format!(
                            "#paragraph のシーン番号の書式が正しくありません（[S02] の形式。この行は無視されます）: {after}"
                        ),
                    });
                    return None;
                }
            }
        };
        let name = match (!arg.is_empty()).then(|| arg.to_string()) {
            Some(n) if n.contains(']') => {
                self.warnings.push(ParseWarning {
                    line_no,
                    message: format!("#paragraph の名称に「]」は使えません（名称なしとして扱います）: {n}"),
                });
                None
            }
            Some(n) => {
                if n.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
                    self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("#paragraph の名称にファイル名として使えない文字が含まれます（素材の自動検索が失敗します）: {n}"),
                    });
                }
                Some(n)
            }
            None => None,
        };
        Some(ScriptItem::Command(ScriptCommand::Paragraph { id, name, line_no }))
    }

    fn parse_se(&mut self, arg: &str, line_no: usize) -> Option<ScriptItem> {
        let mut tokens: Vec<&str> = arg.split_whitespace().collect();
        let mut anchor: Option<SeAnchor> = None;
        let mut gain_db = 0.0;
        while let Some(&last) = tokens.last() {
            let Some((k, v)) = last.split_once('=') else { break };
            match k {
                "at" | "end" => match parse_line_ref(v) {
                    Some(r) => {
                        if anchor.is_some() {
                            self.warnings.push(ParseWarning {
                                line_no,
                                message: "#se の at= と end= は同時に指定できません（先に書いた指定を使います）".to_string(),
                            });
                        }
                        anchor = Some(if k == "at" { SeAnchor::Start(r) } else { SeAnchor::End(r) });
                    }
                    None => self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("#se の {k}= の書式が正しくありません（無視します）: {v}"),
                    }),
                },
                "gain" => match v.parse::<f64>() {
                    Ok(g) if g.is_finite() => gain_db = g,
                    _ => self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("#se の gain= の値が正しくありません（無視します）: {v}"),
                    }),
                },
                _ => break,
            }
            tokens.pop();
        }
        let file = tokens.join(" ");
        if file.is_empty() {
            self.warnings.push(ParseWarning {
                line_no,
                message: "#se にファイル名がありません（この行は無視されます）".to_string(),
            });
            return None;
        }
        let path = self.resolve_asset_path("se_dir", "SE", &file, line_no);
        Some(ScriptItem::Command(ScriptCommand::Se { path, anchor, gain_db, line_no }))
    }

    /// 台詞行の臨時パラメータ。戻り値: (±差分, overlap, at)
    fn parse_speech_params(
        &mut self,
        s: &str,
        line_no: usize,
    ) -> (HashMap<String, f64>, Option<f64>, Option<LineRef>) {
        let mut offsets = HashMap::new();
        let mut overlap = None;
        let mut at = None;
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((k, v)) = part.split_once('=') else {
                self.warnings.push(ParseWarning {
                    line_no,
                    message: format!("臨時パラメータの書式が正しくありません（無視します）: {part}"),
                });
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "at" => match parse_line_ref(v) {
                    Some(r) if r.offset_ms >= 0 => at = Some(r),
                    _ => self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("at= の書式が正しくありません（at=L003+500 の形式。無視します）: {v}"),
                    }),
                },
                "overlap" => match v.parse::<f64>() {
                    Ok(ms) if ms.is_finite() && ms >= 0.0 => overlap = Some(ms),
                    _ => self.warnings.push(ParseWarning {
                        line_no,
                        message: format!("overlap= の値が正しくありません（無視します）: {v}"),
                    }),
                },
                "pan" | "distance" | "height" | "volume" => match v.parse::<f64>() {
                    Ok(x) if x.is_finite() => {
                        offsets.insert(k.to_string(), x);
                    }
                    _ => self.warn_non_finite(line_no, &[k.to_string()]),
                },
                other => self.warnings.push(ParseWarning {
                    line_no,
                    message: format!("臨時パラメータ「{other}」は使えません（無視します）"),
                }),
            }
        }
        (offsets, overlap, at)
    }
```

(d) 関数 `ruby_re` と `expand_ruby` を削除し、ファイル末尾(`extract_kv_params` の前)に `parse_visual` を追加する:
```rust
/// `#visual (image) 本文` を読む。
fn parse_visual(arg: &str) -> ScriptItem {
    let (kind, text) = if let Some(t) = arg.strip_prefix("(image)") {
        (Some(VisualKind::Image), t.trim())
    } else if let Some(t) = arg.strip_prefix("(video)") {
        (Some(VisualKind::Video), t.trim())
    } else {
        (None, arg)
    };
    ScriptItem::Command(ScriptCommand::Visual {
        kind,
        text: text.to_string(),
    })
}
```

(e) 既存テストを直す: 読み指定のテスト(`ruby_notation_separates_text_and_display`、`ruby_word_may_contain_colon`、`legacy_colon_ruby_is_not_expanded`、`fullwidth_pipe_is_not_a_ruby_separator`)を削除する。`ScriptCommand::Paragraph(None)` / `Paragraph(Some(..))` で照合しているテストは `ScriptCommand::Paragraph { name, .. }` で照合するように直す。`ScriptCommand::Se(path)` は `ScriptCommand::Se { path, .. }` に直す。`display_text` を参照しているアサーションは `text` に直す。`unknown_cast_produces_warning_with_line_number` は `p.errors()` を検査するように直す(メッセージに役名と行番号が入ること)。

- [ ] **Step 8: テストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS(`s2v-export` の旧 `exporter.rs` がコンパイルに失敗する場合は、Task 14 で削除するまでの間 `exporter.rs` の該当箇所を最小限直す。`ScriptCommand` を直接使っていないので通常は影響しない)

- [ ] **Step 9: Commit**

```bash
git add crates/s2v-core
git commit -m "feat(core): 台詞ID・overlap/at・#paragraph[Sxx]・#visual・#se オプションを解析し、読み指定を廃止する"
```

---

### Task 5: 構文エラーの検査

**Files:**
- Create: `crates/s2v-core/src/validate.rs`
- Modify: `crates/s2v-core/src/lib.rs`(`pub mod validate;` と `pub use validate::{validate, Diagnostics};`)

**Interfaces:**
- Consumes: Task 4 の `ScriptItem`、`ScriptCommand`、`ParseWarning`
- Produces: `validate::Diagnostics { errors: Vec<ParseWarning>, warnings: Vec<ParseWarning> }`、`validate::validate(scenes: &[Scene]) -> Diagnostics`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/validate.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ScriptParser;

    fn check(body: &str) -> Diagnostics {
        let text = format!("@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n{body}");
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(&text).unwrap();
        validate(&scenes)
    }

    fn messages(v: &[ParseWarning]) -> Vec<String> {
        v.iter().map(|w| w.message.clone()).collect()
    }

    #[test]
    fn valid_script_has_no_errors() {
        let d = check("#paragraph[S01] 朝\n[S01-L001] A:一\n[S01-L002] B(at=L001+100):二\n#se a.wav at=L002\n#paragraph[S02]\n#pause 1000\n");
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn duplicate_line_and_scene_numbers_are_errors() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] B:二\n#paragraph[S01]\n[S01-L002] A:三\n");
        assert_eq!(d.errors.len(), 2, "{:?}", messages(&d.errors));
    }

    #[test]
    fn at_must_reference_an_earlier_line() {
        let d = check("#paragraph[S01]\n[S01-L001] A(at=L002):一\n[S01-L002] B(at=L002):二\n[S01-L003] A(at=L009):三\n");
        assert_eq!(d.errors.len(), 3, "{:?}", messages(&d.errors));
    }

    #[test]
    fn overlap_and_at_together_is_an_error() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B(overlap=100,at=L001):二\n");
        assert_eq!(d.errors.len(), 1);
        assert!(d.errors[0].message.contains("overlap"));
    }

    #[test]
    fn missing_ids_are_errors() {
        let d = check("#paragraph\nA:一\n");
        assert_eq!(d.errors.len(), 2, "{:?}", messages(&d.errors));
        assert!(d.errors.iter().all(|e| e.message.contains("number")));
    }

    #[test]
    fn scene_without_lines_needs_pause() {
        let ok = check("#paragraph[S01]\n#pause 500\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert!(ok.errors.is_empty(), "{:?}", messages(&ok.errors));
        let empty = check("#paragraph[S01]\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert_eq!(empty.errors.len(), 1);
        let zero = check("#paragraph[S01]\n#pause 0\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert_eq!(zero.errors.len(), 1);
    }

    #[test]
    fn se_reference_to_unknown_line_is_an_error() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n#se a.wav end=L005\n");
        assert_eq!(d.errors.len(), 1);
    }

    #[test]
    fn lines_before_first_paragraph_warn_once() {
        let d = check("[S00-L001] A:一\n[S00-L002] B:二\n#paragraph[S01]\n[S01-L003] A:三\n");
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].message.contains("S00"));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod validate;` を追加してから実行する。

Run: `cargo test -p s2v-core validate`
Expected: コンパイルエラー(`validate` が無い)

- [ ] **Step 3: 実装する**

`crates/s2v-core/src/validate.rs` のテストの上に:
```rust
//! 出力を止める問題(終了コード 2)の検査(設計書 5.7)。パースの後、配置の前に行う。

use std::collections::HashMap;

use crate::parser::ParseWarning;
use crate::types::{Scene, ScriptCommand, ScriptItem, SeAnchor};

#[derive(Debug, Default)]
pub struct Diagnostics {
    /// 出力を止める問題(終了コード 2)
    pub errors: Vec<ParseWarning>,
    /// 継続できる問題(終了コード 1)
    pub warnings: Vec<ParseWarning>,
}

/// 検査中のシーン(`#paragraph` から次の `#paragraph` まで)。
struct Block {
    line_no: usize,
    lines: usize,
    pause_ms: f64,
}

pub fn validate(scenes: &[Scene]) -> Diagnostics {
    let mut d = Diagnostics::default();
    let mut seen_lines: HashMap<u32, usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    let mut se_refs: Vec<(u32, usize)> = Vec::new();
    let mut block: Option<Block> = None;
    let mut warned_s00 = false;

    for item in scenes.iter().flat_map(|s| s.items.iter()) {
        match item {
            ScriptItem::Speech { id, line_no, overlap_ms, at, .. } => {
                if overlap_ms.is_some() && at.is_some() {
                    d.errors.push(pw(*line_no, "overlap と at は同時に指定できません"));
                }
                if let Some(r) = at {
                    if !seen_lines.contains_key(&r.line) {
                        d.errors.push(pw(
                            *line_no,
                            format!("at= は前にある台詞だけを参照できます（L{:03} は前にありません）", r.line),
                        ));
                    }
                }
                match id {
                    None => d.errors.push(pw(
                        *line_no,
                        "台詞IDがありません。先に `script2video number` を実行してください",
                    )),
                    Some(id) => {
                        if let Some(prev) = seen_lines.insert(id.line, *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!("台詞番号 L{:03} が重複しています（{prev} 行目と同じ）", id.line),
                            ));
                        }
                    }
                }
                match block.as_mut() {
                    Some(b) => b.lines += 1,
                    None => {
                        if !warned_s00 {
                            d.warnings.push(pw(
                                *line_no,
                                "最初の #paragraph より前の台詞は、暗黙のシーン S00 に属します",
                            ));
                            warned_s00 = true;
                        }
                    }
                }
            }
            ScriptItem::Command(ScriptCommand::Paragraph { id, line_no, .. }) => {
                close_block(&mut block, &mut d);
                match id {
                    None => d.errors.push(pw(
                        *line_no,
                        "シーン番号がありません。先に `script2video number` を実行してください",
                    )),
                    Some(n) => {
                        if let Some(prev) = seen_scenes.insert(*n, *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!("シーン番号 S{n:02} が重複しています（{prev} 行目と同じ）"),
                            ));
                        }
                    }
                }
                block = Some(Block { line_no: *line_no, lines: 0, pause_ms: 0.0 });
            }
            ScriptItem::Command(ScriptCommand::Pause(ms)) => {
                if let Some(b) = block.as_mut() {
                    b.pause_ms += ms;
                }
            }
            ScriptItem::Command(ScriptCommand::Se { anchor: Some(a), line_no, .. }) => {
                let r = match a {
                    SeAnchor::Start(r) | SeAnchor::End(r) => r,
                };
                se_refs.push((r.line, *line_no));
            }
            _ => {}
        }
    }
    close_block(&mut block, &mut d);

    for (line, line_no) in se_refs {
        if !seen_lines.contains_key(&line) {
            d.errors.push(pw(line_no, format!("#se の基準の台詞 L{line:03} が台本にありません")));
        }
    }
    d
}

fn close_block(block: &mut Option<Block>, d: &mut Diagnostics) {
    if let Some(b) = block.take() {
        if b.lines == 0 && b.pause_ms <= 0.0 {
            d.errors.push(pw(
                b.line_no,
                "台詞も #pause もないシーンです（無言のシーンには #pause が必要です）",
            ));
        }
    }
}

fn pw(line_no: usize, message: impl Into<String>) -> ParseWarning {
    ParseWarning { line_no, message: message.into() }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-core validate`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-core/src/validate.rs crates/s2v-core/src/lib.rs
git commit -m "feat(core): ID重複・未来参照・overlap と at の併用・台詞も #pause もないシーンを検査する"
```

---
### Task 6: 採番(`number` の中核)

**Files:**
- Create: `crates/s2v-core/src/numbering.rs`
- Modify: `crates/s2v-core/src/lib.rs`(`pub mod numbering;`)

**Interfaces:**
- Consumes: Task 4 の `LineId`、`format_scene_id`、`parse_scene_id`、`split_line_id_prefix`、`ParseWarning`
- Produces:
  - `numbering::IdsRecord { max_line: u32, max_scene: u32 }`(`Default`)、`IdsRecord::parse(&str) -> IdsRecord`、`IdsRecord::to_text(&self) -> String`
  - `numbering::Numbered { text: String, record: IdsRecord, assigned_lines: usize, assigned_scenes: usize, warnings: Vec<ParseWarning> }`
  - `numbering::number_script(text: &str, record: IdsRecord) -> Result<Numbered, Vec<ParseWarning>>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/numbering.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n";

    fn run(body: &str, record: IdsRecord) -> Numbered {
        number_script(&format!("{HEAD}{body}"), record).unwrap()
    }

    #[test]
    fn assigns_line_and_scene_numbers() {
        let n = run("#paragraph 朝\nA:おはよう\nB:おはよう\n#paragraph\nA:またね\n", IdsRecord::default());
        assert_eq!(
            n.text,
            format!("{HEAD}#paragraph[S01] 朝\n[S01-L001] A:おはよう\n[S01-L002] B:おはよう\n#paragraph[S02]\n[S02-L003] A:またね\n")
        );
        assert_eq!(n.assigned_lines, 3);
        assert_eq!(n.assigned_scenes, 2);
        assert_eq!(n.record, IdsRecord { max_line: 3, max_scene: 2 });
    }

    #[test]
    fn keeps_existing_ids_and_continues_after_the_maximum() {
        let n = run("#paragraph[S03] 朝\n[S03-L007] A:一\nB:二\n#paragraph\nA:三\n", IdsRecord::default());
        assert!(n.text.contains("[S03-L007] A:一\n[S03-L008] B:二\n#paragraph[S04]\n[S04-L009] A:三\n"), "{}", n.text);
    }

    #[test]
    fn record_prevents_reuse_of_deleted_numbers() {
        let n = run("#paragraph[S01]\n[S01-L001] A:一\nB:二\n", IdsRecord { max_line: 10, max_scene: 4 });
        assert!(n.text.contains("[S01-L011] B:二"), "{}", n.text);
        assert_eq!(n.record, IdsRecord { max_line: 11, max_scene: 4 });
    }

    #[test]
    fn duplicates_are_errors_and_nothing_is_written() {
        let errs = number_script(
            &format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] B:二\n#paragraph[S01]\n"),
            IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn crlf_and_bom_are_preserved() {
        let src = "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph 朝\r\n  A:おはよう\r\n";
        let n = number_script(src, IdsRecord::default()).unwrap();
        assert_eq!(
            n.text,
            "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01] 朝\r\n  [S01-L001] A:おはよう\r\n"
        );
    }

    #[test]
    fn lines_outside_script_and_commands_are_untouched() {
        let n = run("# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n", IdsRecord::default());
        assert_eq!(n.text, format!("{HEAD}# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n"));
        assert_eq!(n.assigned_lines, 0);
    }

    #[test]
    fn lines_before_first_paragraph_get_s00_and_a_warning() {
        let n = run("A:前置き\n#paragraph\nB:本編\n", IdsRecord::default());
        assert!(n.text.contains("[S00-L001] A:前置き\n#paragraph[S01]\n[S01-L002] B:本編\n"), "{}", n.text);
        assert_eq!(n.warnings.len(), 1);
    }

    #[test]
    fn bracket_text_is_numbered_once() {
        let first = run("#paragraph\nA:[笑]それは:どうかな\n", IdsRecord::default());
        assert!(first.text.contains("[S01-L001] A:[笑]それは:どうかな\n"), "{}", first.text);
        let second = number_script(&first.text, first.record).unwrap();
        assert_eq!(second.text, first.text);
        assert_eq!(second.assigned_lines, 0);
    }

    #[test]
    fn ids_record_round_trip() {
        let r = IdsRecord { max_line: 12, max_scene: 3 };
        assert_eq!(IdsRecord::parse(&r.to_text()), r);
        assert_eq!(IdsRecord::parse("ゴミ\n"), IdsRecord::default());
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod numbering;` を追加してから実行する。

Run: `cargo test -p s2v-core numbering`
Expected: コンパイルエラー(`number_script` が無い)

- [ ] **Step 3: 実装する**

`crates/s2v-core/src/numbering.rs` のテストの上に:
```rust
//! `script2video number` の採番(台本仕様 §4)。台本を行単位で書き換え、ID 以外のバイトは変えない。

use std::collections::HashMap;

use crate::ids::{format_scene_id, parse_scene_id, split_line_id_prefix, LineId};
use crate::parser::ParseWarning;

/// `台本名.ids`: 使用済みの最大番号(削除した番号を再利用しないための記録)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdsRecord {
    pub max_line: u32,
    pub max_scene: u32,
}

impl IdsRecord {
    /// `.ids` の内容を読む。読めない行は無視する。
    pub fn parse(text: &str) -> Self {
        let mut r = Self::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().parse::<u32>().unwrap_or(0);
                match k.trim() {
                    "max_line" => r.max_line = v,
                    "max_scene" => r.max_scene = v,
                    _ => {}
                }
            }
        }
        r
    }

    pub fn to_text(&self) -> String {
        format!(
            "# script2video number が使用した最大番号(削除した番号を再利用しないための記録)\nmax_line={}\nmax_scene={}\n",
            self.max_line, self.max_scene
        )
    }
}

#[derive(Debug)]
pub struct Numbered {
    pub text: String,
    pub record: IdsRecord,
    pub assigned_lines: usize,
    pub assigned_scenes: usize,
    pub warnings: Vec<ParseWarning>,
}

enum Kind<'a> {
    Other,
    /// `#paragraph` の直後の文字列(`[S02] 名称` など)
    Paragraph { after: &'a str },
    Speech { id: Option<LineId> },
}

/// 台本に台詞IDとシーン番号を書き込む。ID の重複があれば何も変えずに Err を返す。
pub fn number_script(text: &str, record: IdsRecord) -> Result<Numbered, Vec<ParseWarning>> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => ("\u{feff}", b),
        None => ("", text),
    };
    let lines: Vec<&str> = body.split_inclusive('\n').collect();

    // 1回目: 既存の番号を集め、重複を検出する
    let mut errors = Vec::new();
    let mut seen_lines: HashMap<u32, usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    let mut section = String::new();
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let (content, _) = split_content(raw);
        update_section(&mut section, content);
        if section != "@script" {
            continue;
        }
        match classify(content) {
            Kind::Paragraph { after } => {
                if let Some(n) = existing_scene(after) {
                    if let Some(p) = seen_scenes.insert(n, line_no) {
                        errors.push(ParseWarning {
                            line_no,
                            message: format!("シーン番号 {} が重複しています（{p} 行目と同じ）", format_scene_id(n)),
                        });
                    }
                }
            }
            Kind::Speech { id: Some(id) } => {
                if let Some(p) = seen_lines.insert(id.line, line_no) {
                    errors.push(ParseWarning {
                        line_no,
                        message: format!("台詞番号 L{:03} が重複しています（{p} 行目と同じ）", id.line),
                    });
                }
            }
            _ => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut next_line = seen_lines.keys().copied().max().unwrap_or(0).max(record.max_line) + 1;
    let mut next_scene = seen_scenes.keys().copied().max().unwrap_or(0).max(record.max_scene) + 1;

    // 2回目: 番号のない行に書き込む
    let mut out = String::with_capacity(text.len() + 256);
    out.push_str(bom);
    let mut section = String::new();
    let mut current_scene = 0u32;
    let (mut assigned_lines, mut assigned_scenes) = (0usize, 0usize);
    let mut warnings = Vec::new();
    let mut warned_s00 = false;
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let (content, indent_len) = split_content(raw);
        update_section(&mut section, content);
        if section != "@script" {
            out.push_str(raw);
            continue;
        }
        match classify(content) {
            Kind::Paragraph { after } => {
                if let Some(n) = existing_scene(after) {
                    current_scene = n;
                    out.push_str(raw);
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let n = next_scene;
                    next_scene += 1;
                    assigned_scenes += 1;
                    current_scene = n;
                    let at = indent_len + "#paragraph".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[{}]", format_scene_id(n)));
                    out.push_str(&raw[at..]);
                } else {
                    // `[` で始まるが書式が不正: パーサが警告するので、ここでは触らない
                    out.push_str(raw);
                }
            }
            Kind::Speech { id: Some(_) } => out.push_str(raw),
            Kind::Speech { id: None } => {
                if current_scene == 0 && !warned_s00 {
                    warnings.push(ParseWarning {
                        line_no,
                        message: "最初の #paragraph より前の台詞は、暗黙のシーン S00 に属します".to_string(),
                    });
                    warned_s00 = true;
                }
                let id = LineId { scene: current_scene, line: next_line };
                next_line += 1;
                assigned_lines += 1;
                out.push_str(&raw[..indent_len]);
                out.push_str(&format!("[{}] ", id.display()));
                out.push_str(&raw[indent_len..]);
            }
            Kind::Other => out.push_str(raw),
        }
    }

    Ok(Numbered {
        text: out,
        record: IdsRecord { max_line: next_line - 1, max_scene: next_scene - 1 },
        assigned_lines,
        assigned_scenes,
        warnings,
    })
}

/// 改行を除き、前後の空白を除いた内容と、行頭の字下げの長さ(バイト)を返す。
fn split_content(raw: &str) -> (&str, usize) {
    let no_eol = raw.trim_end_matches(['\n', '\r']);
    let content = no_eol.trim_start();
    let indent_len = no_eol.len() - content.len();
    (content.trim_end(), indent_len)
}

fn update_section(section: &mut String, content: &str) {
    if content.starts_with('@') {
        *section = content.split_whitespace().next().unwrap_or("").to_string();
    }
}

fn classify(content: &str) -> Kind<'_> {
    if content.is_empty() || content.starts_with('@') {
        return Kind::Other;
    }
    if let Some(rest) = content.strip_prefix('#') {
        if let Some(after) = rest.strip_prefix("paragraph") {
            if after.is_empty() || after.starts_with('[') || after.starts_with(char::is_whitespace) {
                return Kind::Paragraph { after };
            }
        }
        return Kind::Other;
    }
    if content.chars().all(|c| c.is_ascii_digit()) {
        return Kind::Other;
    }
    let (id, rest) = split_line_id_prefix(content);
    if id.is_some() {
        return Kind::Speech { id };
    }
    match rest.split_once(':') {
        Some((role, _)) if !role.trim().is_empty() => Kind::Speech { id: None },
        _ => Kind::Other,
    }
}

/// `[S02] 名称` からシーン番号を読む。
fn existing_scene(after: &str) -> Option<u32> {
    let inner = after.strip_prefix('[')?;
    let close = inner.find(']')?;
    parse_scene_id(&inner[..close])
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-core numbering`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-core/src/numbering.rs crates/s2v-core/src/lib.rs
git commit -m "feat(core): 台詞IDとシーン番号の採番(既存IDは不変、.ids で欠番を再利用しない)を追加する"
```

---

### Task 7: 台本HTMLと `number` コマンド

**Files:**
- Create: `crates/s2v-export/src/script_html.rs`、`crates/s2v-cli/src/number.rs`、`crates/s2v-cli/tests/number_cli.rs`
- Modify: `crates/s2v-export/src/lib.rs`、`crates/s2v-cli/src/lib.rs`、`crates/s2v-cli/src/main.rs`、`crates/s2v-core/src/parser.rs`(BOM の無視)

**Interfaces:**
- Consumes: Task 6 の `number_script`、`IdsRecord`。Task 4 の `ScriptParser`、`ScriptItem`、`VisualKind`
- Produces: `s2v_export::render_script_html(title: &str, scenes: &[Scene]) -> String`、`s2v_cli::run_number(script: &Path) -> Outcome`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-export/src/script_html.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::ScriptParser;

    #[test]
    fn html_contains_cast_scenes_and_lines() {
        let text = "@scene 居間\n@cast\nA:田中,pan=-30\n30代の男性。<紺のスーツ>\n\nB:佐藤\n\n@script\n[S00-L001] A:前置き\n#paragraph[S01] 港の朝\n#visual (image) 朝焼けの港。\n[S01-L002] B:おはよう & <こんにちは>\n";
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(text).unwrap();
        let html = render_script_html("第1話", &scenes);
        assert!(html.contains("<title>第1話</title>"));
        assert!(html.contains("@media print"));
        assert!(html.contains("田中"));
        assert!(html.contains("&lt;紺のスーツ&gt;"));
        assert!(html.contains("S00"));
        assert!(html.contains("S01 港の朝"));
        assert!(html.contains("（画像）朝焼けの港。"));
        assert!(html.contains("S01-L002"));
        assert!(html.contains("おはよう &amp; &lt;こんにちは&gt;"));
    }
}
```

`crates/s2v-core/src/parser.rs` の `mod tests` に追加する:
```rust
    #[test]
    fn bom_is_ignored() {
        let mut p = ScriptParser::new();
        let scenes = p
            .parse_str("\u{feff}@scene 居間\n@cast\nA:田中\n\n@script\n[S01-L001] A:やあ\n")
            .unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].items.len(), 1);
    }
```

`crates/s2v-cli/tests/number_cli.rs`:
```rust
use std::fs;

use s2v_cli::{run_number, Outcome};

const SCRIPT: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n#paragraph 朝\nA:おはよう\nB:おはよう\n";

#[test]
fn number_rewrites_script_and_writes_bak_ids_and_html() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("台本 テスト");
    fs::create_dir(&work).unwrap();
    let script = work.join("第1話.txt");
    fs::write(&script, SCRIPT).unwrap();

    assert_eq!(run_number(&script), Outcome::Ok);

    let text = fs::read_to_string(&script).unwrap();
    assert!(text.contains("#paragraph[S01] 朝\n[S01-L001] A:おはよう\n[S01-L002] B:おはよう\n"), "{text}");
    assert_eq!(fs::read_to_string(work.join("第1話.bak")).unwrap(), SCRIPT);
    assert!(fs::read_to_string(work.join("第1話.ids")).unwrap().contains("max_line=2"));
    assert!(fs::read_to_string(work.join("第1話.html")).unwrap().contains("S01-L002"));

    // 2回目は何も変わらない
    assert_eq!(run_number(&script), Outcome::Ok);
    assert_eq!(fs::read_to_string(&script).unwrap(), text);
}

#[test]
fn number_refuses_duplicates_and_leaves_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dup.txt");
    let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] A:二\n";
    fs::write(&script, src).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert_eq!(fs::read_to_string(&script).unwrap(), src);
    assert!(!dir.path().join("dup.bak").exists());
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-export/src/lib.rs` に `pub mod script_html;` と `pub use script_html::render_script_html;` を追加し、`crates/s2v-cli/src/lib.rs` に `pub mod number;` と `pub use number::run_number;` を追加してから実行する。

Run: `cargo test --workspace`
Expected: コンパイルエラー(`render_script_html`・`run_number` が無い)

- [ ] **Step 3: パーサで BOM を無視する**

`crates/s2v-core/src/parser.rs` の `parse_str` の冒頭(状態の消去の直後)に追加する:
```rust
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
```

- [ ] **Step 4: 台本HTMLを実装する**

`crates/s2v-export/src/script_html.rs` のテストの上に:
```rust
//! 台本HTML(印刷・閲覧用。設計書 5.1)。

use std::collections::BTreeMap;

use s2v_core::{Cast, Scene, ScriptCommand, ScriptItem, VisualKind};

/// HTML 本文に埋め込む文字列をエスケープする。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = r#"
body { font-family: "Yu Gothic", "Hiragino Sans", sans-serif; margin: 2em; line-height: 1.6; }
table { border-collapse: collapse; width: 100%; margin-bottom: 1.5em; }
th, td { border: 1px solid #999; padding: 0.3em 0.6em; vertical-align: top; text-align: left; }
th { background: #eee; }
td.id { white-space: nowrap; font-family: monospace; }
p.visual { color: #444; font-style: italic; }
@media print {
  body { margin: 0; }
  h2 { page-break-after: avoid; }
  tr { page-break-inside: avoid; }
}
"#;

/// 台本HTMLを作る: タイトル、登場人物、シーンごとの見出し(番号・名称・情景)、台詞の表。
pub fn render_script_html(title: &str, scenes: &[Scene]) -> String {
    let mut casts: BTreeMap<&str, &Cast> = BTreeMap::new();
    for s in scenes {
        for (name, c) in &s.casts {
            casts.insert(name.as_str(), c);
        }
    }

    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n");
    h.push_str(&format!("<title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n", esc(title)));
    h.push_str(&format!("<h1>{}</h1>\n", esc(title)));

    h.push_str("<h2>登場人物</h2>\n<table>\n<tr><th>役名</th><th>話者名</th><th>外見描写</th></tr>\n");
    for (name, c) in &casts {
        h.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            esc(name),
            esc(&c.speaker_name),
            esc(c.appearance.as_deref().unwrap_or("")).replace('\n', "<br>")
        ));
    }
    h.push_str("</table>\n");

    let mut table_open = false;
    let mut heading_done = false;
    let close_table = |h: &mut String, open: &mut bool| {
        if *open {
            h.push_str("</table>\n");
            *open = false;
        }
    };
    for item in scenes.iter().flat_map(|s| s.items.iter()) {
        match item {
            ScriptItem::Command(ScriptCommand::Paragraph { id, name, .. }) => {
                close_table(&mut h, &mut table_open);
                let label = id.map(s2v_core::ids::format_scene_id).unwrap_or_else(|| "S??".to_string());
                let heading = match name {
                    Some(n) => format!("{label} {n}"),
                    None => label,
                };
                h.push_str(&format!("<h2>{}</h2>\n", esc(&heading)));
                heading_done = true;
            }
            ScriptItem::Command(ScriptCommand::Visual { kind, text }) => {
                close_table(&mut h, &mut table_open);
                let prefix = match kind {
                    Some(VisualKind::Image) => "（画像）",
                    Some(VisualKind::Video) => "（動画）",
                    None => "",
                };
                h.push_str(&format!("<p class=\"visual\">{}{}</p>\n", prefix, esc(text)));
            }
            ScriptItem::Speech { id, cast_name, text, .. } => {
                if !heading_done {
                    h.push_str("<h2>S00</h2>\n");
                    heading_done = true;
                }
                if !table_open {
                    h.push_str("<table>\n<tr><th>台詞ID</th><th>役名</th><th>台詞</th></tr>\n");
                    table_open = true;
                }
                let id = id.map(|i| i.display()).unwrap_or_default();
                h.push_str(&format!(
                    "<tr><td class=\"id\">{}</td><td>{}</td><td>{}</td></tr>\n",
                    esc(&id),
                    esc(cast_name),
                    esc(text)
                ));
            }
            _ => {}
        }
    }
    close_table(&mut h, &mut table_open);
    h.push_str("</body>\n</html>\n");
    h
}
```
`s2v_core::ids` を外部から使うため、`crates/s2v-core/src/lib.rs` の `pub mod ids;` が `pub` であることを確認する(Task 4 で追加済み)。

- [ ] **Step 5: `number` コマンドを実装する**

`crates/s2v-cli/src/number.rs`:
```rust
//! `script2video number`(台本仕様 §4、設計書 5.1)。

use std::fs;
use std::path::Path;

use s2v_core::numbering::{number_script, IdsRecord};
use s2v_core::ScriptParser;
use s2v_export::render_script_html;

use crate::Outcome;

pub fn run_number(script: &Path) -> Outcome {
    let bytes = match fs::read(script) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[ERROR] 台本を読めません: {}: {e}", script.display());
            return Outcome::Failed;
        }
    };
    let Ok(text) = String::from_utf8(bytes.clone()) else {
        eprintln!("[ERROR] 台本が UTF-8 ではありません: {}", script.display());
        return Outcome::Failed;
    };

    let ids_path = script.with_extension("ids");
    let record = fs::read_to_string(&ids_path)
        .map(|s| IdsRecord::parse(&s))
        .unwrap_or_default();

    let numbered = match number_script(&text, record) {
        Ok(n) => n,
        Err(errors) => {
            for e in errors {
                eprintln!("[ERROR] {} 行目: {}", e.line_no, e.message);
            }
            eprintln!("[ERROR] 台本は書き換えていません");
            return Outcome::Failed;
        }
    };

    let mut outcome = Outcome::Ok;
    for w in &numbered.warnings {
        eprintln!("[WARN] {} 行目: {}", w.line_no, w.message);
        outcome = outcome.worst(Outcome::Warnings);
    }

    if numbered.text != text {
        if let Err(e) = fs::write(script.with_extension("bak"), &bytes) {
            eprintln!("[ERROR] .bak を書けません: {e}");
            return Outcome::Failed;
        }
        if let Err(e) = fs::write(script, numbered.text.as_bytes()) {
            eprintln!("[ERROR] 台本を書けません: {e}");
            return Outcome::Failed;
        }
    }
    if let Err(e) = fs::write(&ids_path, numbered.record.to_text()) {
        eprintln!("[ERROR] .ids を書けません: {e}");
        return Outcome::Failed;
    }

    let mut parser = ScriptParser::new();
    match parser.parse_str(&numbered.text) {
        Ok(scenes) => {
            // number は検査をしないので、パーサの問題は警告として表示する
            for w in parser.warnings().iter().chain(parser.errors()) {
                eprintln!("[WARN] {} 行目: {}", w.line_no, w.message);
                outcome = outcome.worst(Outcome::Warnings);
            }
            let title = script
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Err(e) = fs::write(script.with_extension("html"), render_script_html(&title, &scenes)) {
                eprintln!("[ERROR] 台本HTMLを書けません: {e}");
                return Outcome::Failed;
            }
        }
        Err(e) => {
            eprintln!("[ERROR] 台本を解析できません: {e}");
            return Outcome::Failed;
        }
    }

    println!(
        "台詞 {} 行、シーン {} 個に番号を付けました",
        numbered.assigned_lines, numbered.assigned_scenes
    );
    outcome
}
```

`crates/s2v-cli/src/main.rs` の `match` を次にする:
```rust
    let outcome = match cli.command {
        Command::Number { script } => s2v_cli::run_number(&script),
        Command::Build { .. } => {
            eprintln!("このコマンドはまだ実装されていません");
            Outcome::Failed
        }
    };
```

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS

- [ ] **Step 7: Commit**

```bash
git add crates/s2v-core/src/parser.rs crates/s2v-export crates/s2v-cli
git commit -m "feat: number コマンド(採番・.bak・.ids・台本HTML)を追加する"
```

---

### Task 8: 収録ファイルの照合

**Files:**
- Create: `crates/s2v-core/src/voices.rs`
- Modify: `crates/s2v-core/src/lib.rs`(`pub mod voices;`)

**Interfaces:**
- Consumes: Task 4 の `LineId::parse_file_stem`
- Produces: `voices::VoiceMatch { files: BTreeMap<LineId, Vec<PathBuf>>, warnings: Vec<String> }`、`voices::match_voice_files(dir: &Path, script_ids: &[LineId], allow_flac: bool) -> anyhow::Result<VoiceMatch>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/voices.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    fn ids() -> Vec<LineId> {
        (1..=4).map(|n| LineId { scene: 1, line: n }).collect()
    }

    #[test]
    fn matches_single_files_and_reports_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("収録 フォルダ");
        std::fs::create_dir(&dir).unwrap();
        touch(&dir, "S01_L001.wav");
        touch(&dir, "S01_L002.wav");
        touch(&dir, "S01_L002.flac");
        touch(&dir, "S01_L003-L004.wav");
        touch(&dir, "S09_L999.wav");
        touch(&dir, "memo.txt");
        touch(&dir, "take1.wav");

        let m = match_voice_files(&dir, &ids(), true).unwrap();
        assert_eq!(m.files[&LineId { scene: 1, line: 1 }].len(), 1);
        assert_eq!(m.files[&LineId { scene: 1, line: 2 }].len(), 2, "wav と flac は重複");
        assert!(!m.files.contains_key(&LineId { scene: 1, line: 3 }));
        assert_eq!(m.warnings.len(), 3, "{:?}", m.warnings);
        assert!(m.warnings.iter().any(|w| w.contains("S01_L003-L004.wav") && w.contains("未対応")));
        assert!(m.warnings.iter().any(|w| w.contains("S09_L999.wav")));
        assert!(m.warnings.iter().any(|w| w.contains("take1.wav")));
    }

    #[test]
    fn flac_is_ignored_with_warning_unless_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L001.FLAC");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(m.files.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert!(m.warnings[0].contains("allow_flac"));
    }

    #[test]
    fn scene_part_must_match_the_script_id() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S02_L001.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(m.files.is_empty());
        assert_eq!(m.warnings.len(), 1);
    }
}
```

`crates/s2v-core/Cargo.toml` の `[dev-dependencies]` に `tempfile = "3"` があることを確認する(流用元にある)。

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod voices;` を追加してから実行する。

Run: `cargo test -p s2v-core voices`
Expected: コンパイルエラー

- [ ] **Step 3: 実装する**

`crates/s2v-core/src/voices.rs` のテストの上に:
```rust
//! 収録ファイルの照合(台本仕様 §3、設計書 5.0)。①a は単独ファイル(`S01_L003.wav`)だけを扱う。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ids::LineId;

#[derive(Debug, Default)]
pub struct VoiceMatch {
    /// 台詞ごとの収録ファイル(名前順)。2つ以上なら重複。
    pub files: BTreeMap<LineId, Vec<PathBuf>>,
    /// 照合から外したファイルの警告(終了コード 1)。
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum NameKind {
    Single(LineId),
    /// 範囲を示すファイル名(`S01_L003-L007` など)。①a では未対応
    Range,
    NotVoice,
}

fn classify(stem: &str) -> NameKind {
    if let Some(id) = LineId::parse_file_stem(stem) {
        return NameKind::Single(id);
    }
    if stem.starts_with('S') && stem.contains("_L") && stem.contains('-') {
        return NameKind::Range;
    }
    NameKind::NotVoice
}

/// `dir` 直下の収録ファイルを台詞IDに対応付ける。サブフォルダは見ない。
pub fn match_voice_files(dir: &Path, script_ids: &[LineId], allow_flac: bool) -> anyhow::Result<VoiceMatch> {
    let known: HashSet<LineId> = script_ids.iter().copied().collect();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    let mut m = VoiceMatch::default();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        match ext.to_ascii_lowercase().as_str() {
            "wav" => {}
            "flac" if allow_flac => {}
            "flac" => {
                m.warnings.push(format!(
                    "FLAC は設定で許可されていません（[voice] allow_flac = true で許可。無視します）: {name}"
                ));
                continue;
            }
            _ => continue,
        }
        match classify(stem) {
            NameKind::Single(id) if known.contains(&id) => {
                m.files.entry(id).or_default().push(path);
            }
            NameKind::Single(_) => {
                m.warnings.push(format!("台本にない台詞IDのファイルです（無視します）: {name}"));
            }
            NameKind::Range => m.warnings.push(format!(
                "範囲を示すファイル名は、この版(①a)では未対応です（照合の対象から外します）: {name}"
            )),
            NameKind::NotVoice => {
                m.warnings.push(format!("台詞IDの形式でないファイル名です（無視します）: {name}"));
            }
        }
    }
    Ok(m)
}
```
`crates/s2v-core/Cargo.toml` の `[dependencies]` に `anyhow.workspace = true` があることを確認する(流用元にある)。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-core voices`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-core/src/voices.rs crates/s2v-core/src/lib.rs
git commit -m "feat(core): 収録ファイルを台詞IDに照合する(範囲ファイルは①aでは照合から外す)"
```

---

### Task 9: 収録音声の取り込み(モノラル検査・48kHz 変換)

**Files:**
- Create: `crates/s2v-audio/src/input.rs`
- Modify: `crates/s2v-audio/Cargo.toml`(`claxon = "0.4"`)、`crates/s2v-audio/src/lib.rs`

**Interfaces:**
- Consumes: 流用元の `resample_mono(&[f32], u32, u32) -> anyhow::Result<Vec<f32>>`
- Produces: `input::WORK_SAMPLE_RATE: u32 = 48_000`、`input::InputError::{ BadFormat(String), Unreadable(String) }`(`Display`)、`input::read_mono_48k(path: &Path) -> Result<Vec<f32>, InputError>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-audio/src/input.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sine(freq: f64, amp: f64, rate: u32, secs: f64) -> Vec<f64> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin())
            .collect()
    }

    fn write_int(dir: &Path, name: &str, bits: u16, rate: u32, channels: u16, data: &[f64]) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: bits, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        let max = ((1i64 << (bits - 1)) - 1) as f64;
        for &v in data {
            for _ in 0..channels {
                w.write_sample((v * max).round() as i32).unwrap();
            }
        }
        w.finalize().unwrap();
        path
    }

    fn write_float(dir: &Path, name: &str, data: &[f32]) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for &v in data {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    fn rms_db(x: &[f32]) -> f64 {
        let ms = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64;
        10.0 * ms.log10()
    }

    #[test]
    fn reads_16_24_32_bit_integer_and_float() {
        let tmp = tempfile::tempdir().unwrap();
        let data = sine(1000.0, 0.5, 48_000, 0.1);
        for bits in [16u16, 24, 32] {
            let p = write_int(tmp.path(), &format!("i{bits}.wav"), bits, 48_000, 1, &data);
            let got = read_mono_48k(&p).unwrap();
            assert_eq!(got.len(), data.len());
            assert!((got[100] as f64 - data[100]).abs() < 1e-3, "{bits}bit");
        }
        let f: Vec<f32> = data.iter().map(|&v| v as f32).collect();
        let p = write_float(tmp.path(), "f32.wav", &f);
        assert_eq!(read_mono_48k(&p).unwrap(), f);
    }

    #[test]
    fn resamples_to_48k_without_changing_level() {
        let tmp = tempfile::tempdir().unwrap();
        for rate in [44_100u32, 96_000] {
            let data = sine(1000.0, 0.5, rate, 1.0);
            let p = write_int(tmp.path(), &format!("r{rate}.wav"), 24, rate, 1, &data);
            let got = read_mono_48k(&p).unwrap();
            assert!((got.len() as i64 - 48_000).abs() <= 48, "{rate}: {}", got.len());
            let mid = &got[4_800..43_200];
            let expected = 20.0 * (0.5f64 / 2f64.sqrt()).log10();
            assert!((rms_db(mid) - expected).abs() < 0.1, "{rate}: {}", rms_db(mid));
        }
    }

    #[test]
    fn stereo_is_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_int(tmp.path(), "st.wav", 16, 48_000, 2, &sine(440.0, 0.1, 48_000, 0.05));
        match read_mono_48k(&p) {
            Err(InputError::BadFormat(m)) => assert!(m.contains("モノラル"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn non_finite_and_8bit_and_broken_header_are_bad_format() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_float(tmp.path(), "nan.wav", &[0.0, f32::NAN, 0.0]);
        assert!(matches!(read_mono_48k(&p), Err(InputError::BadFormat(_))));
        let p8 = write_int(tmp.path(), "b8.wav", 8, 48_000, 1, &[0.0, 0.5]);
        assert!(matches!(read_mono_48k(&p8), Err(InputError::BadFormat(_))));
        let broken = tmp.path().join("broken.wav");
        std::fs::write(&broken, b"RIFF....not a wave").unwrap();
        assert!(matches!(read_mono_48k(&broken), Err(InputError::BadFormat(_))));
        let empty = write_float(tmp.path(), "empty.wav", &[]);
        assert!(matches!(read_mono_48k(&empty), Err(InputError::BadFormat(_))));
    }

    #[test]
    fn missing_file_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(read_mono_48k(&tmp.path().join("none.wav")), Err(InputError::Unreadable(_))));
    }

    /// FLAC のエンコーダは依存に入れないため、ffmpeg がある環境でだけ確認する。
    #[test]
    fn reads_mono_flac_when_ffmpeg_is_available() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg が無いため FLAC のテストを省略します");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wav = write_int(tmp.path(), "src.wav", 16, 44_100, 1, &sine(1000.0, 0.5, 44_100, 0.5));
        let flac = tmp.path().join("src.flac");
        let st = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&wav)
            .arg(&flac)
            .status()
            .unwrap();
        assert!(st.success());
        let got = read_mono_48k(&flac).unwrap();
        assert!((got.len() as i64 - 24_000).abs() <= 48);
    }
}
```

`crates/s2v-audio/Cargo.toml` の `[dev-dependencies]` に `tempfile = "3"` があることを確認する(流用元にある)。

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-audio/src/lib.rs` に `pub mod input;` を追加し、`Cargo.toml` の `[dependencies]` に `claxon = "0.4"` を追加してから実行する。

Run: `cargo test -p s2v-audio input`
Expected: コンパイルエラー

- [ ] **Step 3: 実装する**

`crates/s2v-audio/src/input.rs` のテストの上に:
```rust
//! 収録音声の取り込み(設計書 5.10 A)。モノラルであることを確認し、48kHz に変換する。レベルは変えない。

use std::fmt;
use std::path::Path;

use crate::resampler::resample_mono;

/// 作業サンプルレート(固定)。
pub const WORK_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    /// 非モノラル・非有限値・壊れたヘッダ・未対応形式(reason: bad_format)
    BadFormat(String),
    /// ファイルを読めない(reason: unreadable)
    Unreadable(String),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::BadFormat(m) => write!(f, "形式が不正です: {m}"),
            InputError::Unreadable(m) => write!(f, "読み込めません: {m}"),
        }
    }
}

/// 収録ファイルを読み、モノラル・48kHz・f32 で返す。
pub fn read_mono_48k(path: &Path) -> Result<Vec<f32>, InputError> {
    if !path.is_file() {
        return Err(InputError::Unreadable(format!("ファイルがありません: {}", path.display())));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (samples, rate) = if ext == "flac" { read_flac(path)? } else { read_wav(path)? };
    if samples.is_empty() {
        return Err(InputError::BadFormat("サンプルがありません".to_string()));
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(InputError::BadFormat("非有限の値(NaN/Inf)を含みます".to_string()));
    }
    if rate == WORK_SAMPLE_RATE {
        return Ok(samples);
    }
    resample_mono(&samples, rate, WORK_SAMPLE_RATE)
        .map_err(|e| InputError::BadFormat(format!("リサンプルできません: {e}")))
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), InputError> {
    let mut reader = hound::WavReader::open(path).map_err(map_hound)?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(InputError::BadFormat(format!(
            "モノラルではありません（チャンネル数 {}）",
            spec.channels
        )));
    }
    if spec.sample_rate == 0 {
        return Err(InputError::BadFormat("サンプルレートが 0 です".to_string()));
    }
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16 | 24 | 32) => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .map_err(map_hound)?
        }
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(map_hound)?,
        (f, b) => {
            return Err(InputError::BadFormat(format!(
                "対応していない形式です（{f:?} {b}bit。16/24/32bit 整数、32bit 浮動小数に対応）"
            )))
        }
    };
    Ok((samples, spec.sample_rate))
}

fn map_hound(e: hound::Error) -> InputError {
    match e {
        hound::Error::IoError(io) if io.kind() != std::io::ErrorKind::UnexpectedEof => {
            InputError::Unreadable(io.to_string())
        }
        other => InputError::BadFormat(other.to_string()),
    }
}

fn read_flac(path: &Path) -> Result<(Vec<f32>, u32), InputError> {
    let mut reader = claxon::FlacReader::open(path).map_err(map_claxon)?;
    let info = reader.streaminfo();
    if info.channels != 1 {
        return Err(InputError::BadFormat(format!(
            "モノラルではありません（チャンネル数 {}）",
            info.channels
        )));
    }
    let max = (1i64 << (info.bits_per_sample - 1)) as f32;
    let samples = reader
        .samples()
        .map(|s| s.map(|v| v as f32 / max))
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_claxon)?;
    Ok((samples, info.sample_rate))
}

fn map_claxon(e: claxon::Error) -> InputError {
    match e {
        claxon::Error::IoError(io) if io.kind() != std::io::ErrorKind::UnexpectedEof => {
            InputError::Unreadable(io.to_string())
        }
        other => InputError::BadFormat(other.to_string()),
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-audio input`
Expected: PASS(ffmpeg が無い環境では FLAC のテストは「省略します」と表示して PASS)

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-audio
git commit -m "feat(audio): 収録音声の取り込み(モノラル検査・WAV/FLAC・48kHz 変換)を追加する"
```

---

### Task 10: 時刻の丸め規則と無音削除

**Files:**
- Create: `crates/s2v-core/src/time.rs`、`crates/s2v-audio/src/detect.rs`
- Modify: `crates/s2v-core/src/lib.rs`(`pub mod time;`)、`crates/s2v-audio/src/lib.rs`(`pub mod detect;`)

**Interfaces:**
- Consumes: なし
- Produces:
  - `time::SAMPLES_PER_MS: i64 = 48`、`time::ms_to_samples(ms: f64) -> i64`、`time::floor_ms(samples: i64) -> i64`、`time::ceil_ms(samples: i64) -> i64`
  - `detect::RMS_WINDOW: usize = 480`、`detect::Extent { start: usize, end: usize }`、`detect::sound_extent(&[f32], threshold_db: f64) -> Option<Extent>`
  - `detect::UsedRange { used: Extent, measure: Extent }`、`detect::single_used_range(&[f32], threshold_db: f64, head_margin_ms: u32, tail_margin_ms: u32) -> Option<UsedRange>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/time.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_and_samples_round_trip_rules() {
        assert_eq!(ms_to_samples(1.0), 48);
        assert_eq!(ms_to_samples(30.0), 1_440);
        assert_eq!(floor_ms(0), 0);
        assert_eq!(ceil_ms(0), 0);
        assert_eq!(floor_ms(1), 0);
        assert_eq!(ceil_ms(1), 1);
        assert_eq!(floor_ms(48), 1);
        assert_eq!(ceil_ms(48), 1);
        assert_eq!(floor_ms(481), 10);
        assert_eq!(ceil_ms(481), 11);
    }
}
```

`crates/s2v-audio/src/detect.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, from: usize, to: usize, amp: f32) -> Vec<f32> {
        (0..len)
            .map(|i| if (from..to).contains(&i) { amp * ((i as f32) * 0.3).sin() } else { 0.0 })
            .collect()
    }

    #[test]
    fn silence_has_no_extent() {
        assert_eq!(sound_extent(&vec![0.0; 10_000], -50.0), None);
        assert_eq!(single_used_range(&vec![0.0; 10_000], -50.0, 30, 80), None);
    }

    #[test]
    fn extent_snaps_to_windows_and_margins_are_added() {
        // 200ms の無音 + 1000ms の音 + 300ms の無音(48kHz)
        let x = tone(72_000, 9_600, 57_600, 0.1);
        assert_eq!(sound_extent(&x, -50.0), Some(Extent { start: 9_600, end: 57_600 }));
        let r = single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(r.measure, Extent { start: 9_600, end: 57_600 });
        assert_eq!(r.used, Extent { start: 8_160, end: 61_440 });
    }

    #[test]
    fn margins_are_clamped_to_file_edges() {
        let x = tone(4_800, 0, 4_800, 0.1);
        let r = single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(r.used, Extent { start: 0, end: 4_800 });
    }

    #[test]
    fn partial_last_window_is_evaluated_with_its_own_length() {
        // 1000 サンプル: 最後の窓は 40 サンプル(960..1000)だけで、そこにだけ音がある
        let x = tone(1_000, 960, 1_000, 0.5);
        assert_eq!(sound_extent(&x, -50.0), Some(Extent { start: 960, end: 1_000 }));
    }

    #[test]
    fn quiet_minus_45_dbfs_voice_is_still_used() {
        // 設計書 §10 項目2e: 単独ファイルは take_gate_db(-40)ではなく trim_threshold_db(-50)で判定する
        let amp = 10f32.powf(-45.0 / 20.0) * 2f32.sqrt();
        let x = tone(48_000, 4_800, 43_200, amp);
        assert!(single_used_range(&x, -50.0, 30, 80).is_some());
    }

    #[test]
    fn single_sample_just_above_and_below_threshold() {
        // 480 サンプル窓に1サンプルだけ値 a: RMS = a / sqrt(480)
        let a_at_threshold = 10f64.powf(-50.0 / 20.0) * (480f64).sqrt();
        let mut above = vec![0.0f32; 960];
        above[500] = (a_at_threshold * 1.01) as f32;
        assert_eq!(sound_extent(&above, -50.0), Some(Extent { start: 480, end: 960 }));
        let mut below = vec![0.0f32; 960];
        below[500] = (a_at_threshold * 0.99) as f32;
        assert_eq!(sound_extent(&below, -50.0), None);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` にそれぞれ `pub mod time;`、`pub mod detect;` を追加してから実行する。

Run: `cargo test -p s2v-core time && cargo test -p s2v-audio detect`
Expected: コンパイルエラー

- [ ] **Step 3: 実装する**

`crates/s2v-core/src/time.rs` のテストの上に:
```rust
//! サンプルと時刻の規則(設計書 5.11)。内部は 48kHz のサンプル単位の整数で持つ。

pub const SAMPLE_RATE: u32 = 48_000;
pub const SAMPLES_PER_MS: i64 = 48;

/// ミリ秒の入力をサンプルにする(ms × 48)。
pub fn ms_to_samples(ms: f64) -> i64 {
    (ms * SAMPLES_PER_MS as f64).round() as i64
}

/// 開始の時刻: 切り捨て。
pub fn floor_ms(samples: i64) -> i64 {
    samples.div_euclid(SAMPLES_PER_MS)
}

/// 終了の時刻・長さ: 切り上げ。
pub fn ceil_ms(samples: i64) -> i64 {
    -(-samples).div_euclid(SAMPLES_PER_MS)
}
```

`crates/s2v-audio/src/detect.rs` のテストの上に:
```rust
//! 10ms RMS 窓による音の範囲の検出と、単独ファイルの使用範囲(設計書 5.3、5.11)。

/// 10ms 窓(48kHz で 480 サンプル)。ホップも同じで重ならない。
pub const RMS_WINDOW: usize = 480;

/// サンプル位置の半開区間 [start, end)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub start: usize,
    pub end: usize,
}

impl Extent {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// しきい値を超えた最初の窓の開始から、最後の窓の終了まで。音が無ければ None。
pub fn sound_extent(samples: &[f32], threshold_db: f64) -> Option<Extent> {
    let mut first = None;
    let mut last_end = 0;
    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + RMS_WINDOW).min(samples.len());
        if window_db(&samples[pos..end]) > threshold_db {
            if first.is_none() {
                first = Some(pos);
            }
            last_end = end;
        }
        pos = end;
    }
    first.map(|start| Extent { start, end: last_end })
}

fn window_db(w: &[f32]) -> f64 {
    let ms = w.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / w.len() as f64;
    if ms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * ms.log10()
    }
}

/// 単独ファイルの範囲。`used` は音声として使う範囲(マージン込み)、`measure` はラウドネスの測定範囲(マージンを含めない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsedRange {
    pub used: Extent,
    pub measure: Extent,
}

/// 単独ファイルの使用範囲(設計書 5.0)。音が無ければ None(→ `silent`)。
pub fn single_used_range(
    samples: &[f32],
    threshold_db: f64,
    head_margin_ms: u32,
    tail_margin_ms: u32,
) -> Option<UsedRange> {
    let m = sound_extent(samples, threshold_db)?;
    let head = head_margin_ms as usize * 48;
    let tail = tail_margin_ms as usize * 48;
    Some(UsedRange {
        used: Extent {
            start: m.start.saturating_sub(head),
            end: (m.end + tail).min(samples.len()),
        },
        measure: m,
    })
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-core time && cargo test -p s2v-audio detect`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-core/src/time.rs crates/s2v-core/src/lib.rs crates/s2v-audio/src/detect.rs crates/s2v-audio/src/lib.rs
git commit -m "feat: サンプルとミリ秒の丸め規則と、10ms 窓による無音削除を追加する"
```

---
### Task 11: ラウドネス測定・ゲイン・フェード

**Files:**
- Create: `crates/s2v-audio/src/filters.rs`、`crates/s2v-audio/src/loudness.rs`
- Modify: `crates/s2v-audio/src/lib.rs`(`pub(crate) mod filters;`、`pub mod loudness;`)

**Interfaces:**
- Consumes: Task 2 の `NormalizeConfig`
- Produces:
  - `filters::Biquad`(crate 内部): `Biquad::new(b: [f64; 3], a: [f64; 2])`、`Biquad::lowpass(fc: f64, q: f64, fs: f64)`、`Biquad::highpass(fc: f64, q: f64, fs: f64)`、`Biquad::process(&mut self, x: f64) -> f64`
  - `loudness::Loudness::{ Lufs(f64), Unmeasurable }`、`loudness::measure_mono(&[f32]) -> Loudness`、`loudness::measure_stereo(&[[f32; 2]]) -> Loudness`
  - `loudness::GainDecision { requested_db: f64, applied_db: f64, clamped: bool }`、`loudness::decide_gain(measured_lufs: f64, cfg: &NormalizeConfig) -> GainDecision`
  - `loudness::apply_gain_db(&mut [f32], db: f64)`、`loudness::FADE_SAMPLES: usize = 240`、`loudness::apply_edge_fades(&mut [f32])`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-audio/src/loudness.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    fn lufs(l: Loudness) -> f64 {
        match l {
            Loudness::Lufs(v) => v,
            Loudness::Unmeasurable => panic!("測定できませんでした"),
        }
    }

    #[test]
    fn full_scale_997hz_sine_reads_minus_3_01() {
        // BS.1770 の基準: 997Hz・振幅1.0 の正弦波(1チャンネル)は -3.01 LUFS
        assert!((lufs(measure_mono(&sine(997.0, 1.0, 96_000))) + 3.01).abs() < 0.1);
        assert!((lufs(measure_mono(&sine(997.0, 0.1, 96_000))) + 23.01).abs() < 0.1);
    }

    #[test]
    fn short_line_is_measured_without_gating() {
        // 300ms(400ms 未満)
        assert!((lufs(measure_mono(&sine(997.0, 0.1, 14_400))) + 23.01).abs() < 0.3);
    }

    #[test]
    fn method_switches_at_400ms() {
        // -80 dBFS の音: ゲートなしなら測れる、ゲート付きなら絶対ゲート(-70)で測定不能
        let amp = 10f64.powf(-80.0 / 20.0) * 2f64.sqrt();
        assert!(matches!(measure_mono(&sine(997.0, amp, 19_199)), Loudness::Lufs(_)));
        assert_eq!(measure_mono(&sine(997.0, amp, 19_200)), Loudness::Unmeasurable);
        assert_eq!(measure_mono(&sine(997.0, amp, 19_201)), Loudness::Unmeasurable);
    }

    #[test]
    fn silence_and_empty_are_unmeasurable() {
        assert_eq!(measure_mono(&[]), Loudness::Unmeasurable);
        assert_eq!(measure_mono(&vec![0.0; 1_000]), Loudness::Unmeasurable);
        assert_eq!(measure_mono(&vec![0.0; 96_000]), Loudness::Unmeasurable);
    }

    #[test]
    fn relative_gate_drops_quiet_parts() {
        let mut x = sine(997.0, 0.1, 96_000);
        x.extend(sine(997.0, 0.001, 96_000));
        assert!((lufs(measure_mono(&x)) + 23.0).abs() < 0.5);
    }

    #[test]
    fn stereo_sums_channels() {
        let m = sine(997.0, 0.1, 96_000);
        let st: Vec<[f32; 2]> = m.iter().map(|&v| [v, v]).collect();
        // 2チャンネル分のエネルギー: モノラルより +3.01 dB
        assert!((lufs(measure_stereo(&st)) - lufs(measure_mono(&m)) - 3.01).abs() < 0.05);
    }

    #[test]
    fn different_recording_levels_end_at_the_same_target() {
        let cfg = s2v_core::NormalizeConfig::default();
        for amp in [0.01, 0.1] {
            let mut x = sine(997.0, amp, 96_000);
            let g = decide_gain(lufs(measure_mono(&x)), &cfg);
            apply_gain_db(&mut x, g.applied_db);
            assert!((lufs(measure_mono(&x)) + 23.0).abs() < 0.05, "{amp}");
        }
    }

    #[test]
    fn late_short_voice_in_a_450ms_file_is_measurable() {
        // 単独ファイルで、最初の 400ms が無音・末尾 50ms だけ発声する 450ms の波形
        let mut x = vec![0.0f32; 19_200];
        x.extend(sine(997.0, 0.1, 2_400));
        let r = crate::detect::single_used_range(&x, -50.0, 30, 80).unwrap();
        assert_eq!(r.measure.start, 19_200);
        assert!(matches!(measure_mono(&x[r.measure.start..r.measure.end]), Loudness::Lufs(_)));
    }

    #[test]
    fn gain_is_clamped_and_recorded() {
        let cfg = s2v_core::NormalizeConfig::default();
        let g = decide_gain(-31.0, &cfg);
        assert_eq!(g, GainDecision { requested_db: 8.0, applied_db: 8.0, clamped: false });
        let g = decide_gain(-55.0, &cfg);
        assert_eq!(g, GainDecision { requested_db: 32.0, applied_db: 24.0, clamped: true });
        let g = decide_gain(5.0, &cfg);
        assert_eq!(g, GainDecision { requested_db: -28.0, applied_db: -24.0, clamped: true });
    }

    #[test]
    fn gain_is_a_single_fixed_factor() {
        let mut x = vec![0.1f32, -0.5, 0.25];
        apply_gain_db(&mut x, 20.0);
        assert!((x[0] - 1.0).abs() < 1e-6 && (x[1] + 5.0).abs() < 1e-5 && (x[2] - 2.5).abs() < 1e-5);
    }

    #[test]
    fn edge_fades_are_half_cosine_and_short_lines_use_half_length() {
        let mut long = vec![1.0f32; 2_000];
        apply_edge_fades(&mut long);
        assert_eq!(long[0], 0.0);
        assert_eq!(long[1_999], 0.0);
        assert!((long[120] - 0.5).abs() < 1e-3);
        assert_eq!(long[240], 1.0);
        assert_eq!(long[1_000], 1.0);

        let mut short = vec![1.0f32; 10];
        apply_edge_fades(&mut short);
        assert_eq!(short[0], 0.0);
        assert_eq!(short[9], 0.0);
        assert!(short[4] > 0.0 && short[4] < 1.0);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub(crate) mod filters;` と `pub mod loudness;` を追加してから実行する。

Run: `cargo test -p s2v-audio loudness`
Expected: コンパイルエラー

- [ ] **Step 3: 双二次フィルタを書く**

`crates/s2v-audio/src/filters.rs`:
```rust
//! 双二次フィルタ(転置直接形 II)。

#[derive(Debug, Clone)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// `a` は [a1, a2](a0 = 1 に正規化済み)。
    pub(crate) fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b0: b[0], b1: b[1], b2: b[2], a1: a[0], a2: a[1], z1: 0.0, z2: 0.0 }
    }

    /// RBJ のローパス。
    pub(crate) fn lowpass(fc: f64, q: f64, fs: f64) -> Self {
        let (cos, alpha) = Self::rbj(fc, q, fs);
        let a0 = 1.0 + alpha;
        Self::new(
            [(1.0 - cos) / 2.0 / a0, (1.0 - cos) / a0, (1.0 - cos) / 2.0 / a0],
            [-2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    /// RBJ のハイパス。
    pub(crate) fn highpass(fc: f64, q: f64, fs: f64) -> Self {
        let (cos, alpha) = Self::rbj(fc, q, fs);
        let a0 = 1.0 + alpha;
        Self::new(
            [(1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0],
            [-2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    fn rbj(fc: f64, q: f64, fs: f64) -> (f64, f64) {
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        (w0.cos(), w0.sin() / (2.0 * q))
    }

    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}
```

- [ ] **Step 4: ラウドネスを実装する**

`crates/s2v-audio/src/loudness.rs` のテストの上に:
```rust
//! ITU-R BS.1770-4 のラウドネス測定と、台詞の正規化(設計書 5.10 B)。48kHz 専用。

use s2v_core::NormalizeConfig;

use crate::filters::Biquad;

/// 400ms ブロック(48kHz)。
const BLOCK: usize = 19_200;
/// 100ms(オーバーラップ 75%)。
const STEP: usize = 4_800;
/// 絶対ゲート[LUFS]。
const ABS_GATE: f64 = -70.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Loudness {
    Lufs(f64),
    /// 測定できない(reason: unmeasurable)
    Unmeasurable,
}

/// K 特性フィルタ(48kHz の係数。状態はゼロから始める)。
fn k_weight(samples: &[f32]) -> Vec<f64> {
    let mut shelf = Biquad::new(
        [1.53512485958697, -2.69169618940638, 1.19839281085285],
        [-1.69065929318241, 0.73248077421585],
    );
    let mut rlb = Biquad::new([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]);
    samples
        .iter()
        .map(|&x| rlb.process(shelf.process(x as f64)))
        .collect()
}

fn lufs_of(mean_square: f64) -> f64 {
    -0.691 + 10.0 * mean_square.log10()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// モノラルの台詞を測る。400ms 未満はブロックなし(ゲートなし)、400ms 以上はゲート付き。
pub fn measure_mono(samples: &[f32]) -> Loudness {
    let sq: Vec<f64> = k_weight(samples).iter().map(|v| v * v).collect();
    gated_or_short(&sq)
}

/// ステレオ(左右チャンネルの加算)を測る。基準ゲインの較正に使う。
pub fn measure_stereo(frames: &[[f32; 2]]) -> Loudness {
    let l: Vec<f32> = frames.iter().map(|f| f[0]).collect();
    let r: Vec<f32> = frames.iter().map(|f| f[1]).collect();
    let (yl, yr) = (k_weight(&l), k_weight(&r));
    let sq: Vec<f64> = yl.iter().zip(&yr).map(|(a, b)| a * a + b * b).collect();
    gated_or_short(&sq)
}

fn gated_or_short(sq: &[f64]) -> Loudness {
    if sq.is_empty() {
        return Loudness::Unmeasurable;
    }
    if sq.len() < BLOCK {
        return finite(lufs_of(mean(sq)));
    }
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + BLOCK <= sq.len() {
        blocks.push(mean(&sq[start..start + BLOCK]));
        start += STEP;
    }
    let above_abs: Vec<f64> = blocks.into_iter().filter(|&z| lufs_of(z) > ABS_GATE).collect();
    if above_abs.is_empty() {
        return Loudness::Unmeasurable;
    }
    let rel_gate = lufs_of(mean(&above_abs)) - 10.0;
    let kept: Vec<f64> = above_abs.into_iter().filter(|&z| lufs_of(z) > rel_gate).collect();
    if kept.is_empty() {
        return Loudness::Unmeasurable;
    }
    finite(lufs_of(mean(&kept)))
}

fn finite(v: f64) -> Loudness {
    if v.is_finite() {
        Loudness::Lufs(v)
    } else {
        Loudness::Unmeasurable
    }
}

/// 正規化のゲイン。`applied_db` は制限後に実際に掛ける値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainDecision {
    pub requested_db: f64,
    pub applied_db: f64,
    pub clamped: bool,
}

pub fn decide_gain(measured_lufs: f64, cfg: &NormalizeConfig) -> GainDecision {
    let requested = cfg.target_lufs - measured_lufs;
    let applied = requested.clamp(-cfg.max_cut_db, cfg.max_boost_db);
    GainDecision {
        requested_db: requested,
        applied_db: applied,
        clamped: applied != requested,
    }
}

/// 1つの固定ゲインを掛ける(圧縮や自動ゲイン制御はしない)。
pub fn apply_gain_db(samples: &mut [f32], db: f64) {
    let g = 10f64.powf(db / 20.0) as f32;
    for s in samples {
        *s *= g;
    }
}

/// 5ms(48kHz で 240 サンプル)。
pub const FADE_SAMPLES: usize = 240;

/// 両端に半コサインのフェードを掛ける。台詞が 10ms 未満なら長さの半分。
pub fn apply_edge_fades(samples: &mut [f32]) {
    let len = samples.len();
    let n = FADE_SAMPLES.min(len / 2);
    for i in 0..n {
        let g = (0.5 - 0.5 * (std::f64::consts::PI * i as f64 / n as f64).cos()) as f32;
        samples[i] *= g;
        samples[len - 1 - i] *= g;
    }
}
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p s2v-audio loudness`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/s2v-audio/src/filters.rs crates/s2v-audio/src/loudness.rs crates/s2v-audio/src/lib.rs
git commit -m "feat(audio): BS.1770 のラウドネス測定、ゲインの決定と制限、端のフェードを追加する"
```

---

### Task 12: 音響処理の改修と基準ゲインの較正

**Files:**
- Create: `crates/s2v-audio/src/calib.rs`
- Modify: `crates/s2v-audio/src/processor.rs`、`crates/s2v-audio/src/early.rs`(テスト用設定)、`crates/s2v-audio/src/lib.rs`、`crates/s2v-core/src/config.rs`

**Interfaces:**
- Consumes: Task 11 の `measure_mono`、`measure_stereo`、`apply_gain_db`、`filters::Biquad`
- Produces:
  - `AudioProcessor::new(AudioConfig) -> AudioProcessor`(生成時に較正する)
  - `AudioProcessor::render(&self, mono: &[f32], cast: &Cast, scene: &SceneConfig) -> anyhow::Result<Vec<[f32; 2]>>`
  - `AudioProcessor::calibration_gain(&self) -> f64`、`AudioProcessor::reference_dist(&self) -> f64`
  - `calib::calibration_signal(seed: u64) -> Vec<f32>`、`calib::reference_cast(reference_dist: f64) -> Cast`、`calib::reference_scene() -> SceneConfig`
  - `AudioConfig` から `reference_gain_db`・`max_gain_db`・`engine_volume_offsets` を削除
  - `AudioProcessor::process`(WAV 入出力)は削除

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-audio/src/calib.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::{measure_mono, measure_stereo, Loudness};
    use crate::processor::AudioProcessor;

    fn processor() -> AudioProcessor {
        AudioProcessor::new(s2v_core::Config::builtin().audio)
    }

    fn lufs(l: Loudness) -> f64 {
        match l {
            Loudness::Lufs(v) => v,
            Loudness::Unmeasurable => panic!("測定できませんでした"),
        }
    }

    #[test]
    fn calibration_signal_is_minus_23_lufs_and_deterministic() {
        let a = calibration_signal(1);
        assert_eq!(a.len(), 240_000);
        assert!((lufs(measure_mono(&a)) + 23.0).abs() < 0.01);
        assert_eq!(a, calibration_signal(1));
        assert_ne!(a, calibration_signal(2));
    }

    #[test]
    fn reference_line_output_matches_input_loudness() {
        // 較正に使ったものと別の信号でも ±0.5 LU に入ること(設計書 5.10 C)
        let p = processor();
        let input = calibration_signal(99);
        let out = p
            .render(&input, &reference_cast(p.reference_dist()), &reference_scene())
            .unwrap();
        let diff = lufs(measure_stereo(&out)) - lufs(measure_mono(&input));
        assert!(diff.abs() < 0.5, "{diff}");
    }
}
```

`crates/s2v-audio/src/processor.rs` の `mod tests` に追加する:
```rust
    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * ((i as f32) * 0.05).sin()).collect()
    }

    #[test]
    fn render_rejects_empty_input() {
        let p = AudioProcessor::new(default_audio_config());
        assert!(p.render(&[], &dummy_cast(0.0, 1.0), &default_scene()).is_err());
    }

    #[test]
    fn render_is_linear_without_per_line_normalization_or_limiter() {
        let p = AudioProcessor::new(default_audio_config());
        let x = tone(4_800, 0.2);
        let x4: Vec<f32> = x.iter().map(|v| v * 4.0).collect();
        let cast = dummy_cast(0.0, 0.3);
        let a = p.render(&x, &cast, &default_scene()).unwrap();
        let b = p.render(&x4, &cast, &default_scene()).unwrap();
        assert_eq!(a.len(), b.len());
        for (fa, fb) in a.iter().zip(&b) {
            assert!((fb[0] - 4.0 * fa[0]).abs() < 1e-4);
            assert!((fb[1] - 4.0 * fa[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn volume_half_lowers_output_by_6_db() {
        let p = AudioProcessor::new(default_audio_config());
        let x = tone(9_600, 0.2);
        let full = dummy_cast(0.0, 1.0);
        let mut half = full.clone();
        half.volume = 0.5;
        let a = p.render(&x, &full, &default_scene()).unwrap();
        let b = p.render(&x, &half, &default_scene()).unwrap();
        let energy = |v: &[[f32; 2]]| v.iter().map(|f| (f[0] * f[0] + f[1] * f[1]) as f64).sum::<f64>();
        let db = 10.0 * (energy(&b) / energy(&a)).log10();
        assert!((db + 6.02).abs() < 0.1, "{db}");
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod calib;` を追加してから実行する。

Run: `cargo test -p s2v-audio`
Expected: コンパイルエラー(`render` が無い)

- [ ] **Step 3: `AudioConfig` から使わない項目を削除する**

`crates/s2v-core/src/config.rs` の `AudioConfig` から `reference_gain_db`・`max_gain_db`・`engine_volume_offsets` の3項目と、関数 `legacy_reference_gain_db`・`legacy_max_gain_db` を削除する。`use std::collections::HashMap;` が不要になれば削除する。

`crates/s2v-audio/src/early.rs` のテストで `AudioConfig { ... }` を作っている箇所から `reference_gain_db`・`max_gain_db`・`engine_volume_offsets` の3行を削除する。`processor.rs` のテストの `default_audio_config()` からも同じ3行を削除する。

- [ ] **Step 4: `processor.rs` を改修する**

`crates/s2v-audio/src/processor.rs` の `AudioProcessor` 構造体・`new`・`process` を、次に置き換える(`resolve_reverb_params`、`config_sample_rate`、`config_room_size`、`reverb_params_for`、`prewarm_reverb`、`apply_air_absorption` はそのまま残す):
```rust
pub struct AudioProcessor {
    config: AudioConfig,
    ir_cache: IrCache,
    /// 基準ゲイン(設計書 5.10 C)。生成時に較正する。
    calib_gain: f64,
}

impl AudioProcessor {
    pub fn new(config: AudioConfig) -> Self {
        let sample_rate = config.sample_rate;
        let mut p = Self {
            config,
            ir_cache: IrCache::new(sample_rate),
            calib_gain: 1.0,
        };
        p.calib_gain = crate::calib::compute_calibration_gain(&p);
        p
    }

    /// 較正で求めた基準ゲイン。
    pub fn calibration_gain(&self) -> f64 {
        self.calib_gain
    }

    pub fn reference_dist(&self) -> f64 {
        self.config.reference_dist
    }

    /// 正規化済みのモノラル(48kHz)に、定位・距離・早期反射・残響を掛けてステレオにする。
    /// ピーク正規化と台詞ごとのリミッタは行わない(設計書 5.10 C)。
    pub fn render(&self, mono: &[f32], cast: &Cast, scene: &SceneConfig) -> anyhow::Result<Vec<[f32; 2]>> {
        self.render_with_gain(mono, cast, scene, self.calib_gain)
    }

    pub(crate) fn render_with_gain(
        &self,
        mono: &[f32],
        cast: &Cast,
        scene: &SceneConfig,
        base_gain: f64,
    ) -> anyhow::Result<Vec<[f32; 2]>> {
        // 空の音声は、この先の apply_air_absorption 等が先頭要素に直接アクセスするため Err にする
        if mono.is_empty() {
            anyhow::bail!("音声データが空です(0サンプル)");
        }

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

        // --- 幾何学計算 ---
        let pan_rad = cast.pan.to_radians();
        let geo = calc_geometry(self.config.microphone_spacing, cast.distance, pan_rad);

        // 空気吸収フィルター
        let data_l = apply_air_absorption(mono, geo.dist_l, self.config.sample_rate, self.config.air_absorption_coeff);
        let data_r = apply_air_absorption(mono, geo.dist_r, self.config.sample_rate, self.config.air_absorption_coeff);

        // ITD
        let delay_l = ((geo.dist_l / self.config.sound_speed) * self.config.sample_rate as f64) as usize;
        let delay_r = ((geo.dist_r / self.config.sound_speed) * self.config.sample_rate as f64) as usize;
        let min_delay = delay_l.min(delay_r);
        let rel_l = delay_l - min_delay;
        let rel_r = delay_r - min_delay;

        // ILD + 基準ゲイン
        let mic_angle_rad = self.config.mic_angle.to_radians();
        let k = self.config.mic_directivity;
        let nominal_pat = (1.0 - k) + k * (0.0_f64 - mic_angle_rad).cos();
        let vol_factor = base_gain / nominal_pat.max(1e-6) * cast.volume;

        let dist_gain_l = self.config.reference_dist / geo.dist_l.max(0.1);
        let dist_gain_r = self.config.reference_dist / geo.dist_r.max(0.1);
        // Lマイクは外側 (-mic_angle / 左向き)、Rマイクは外側 (+mic_angle / 右向き) を向く(流用元と同じ)
        let pat_l = directivity_pattern(geo.angle_l, k, mic_angle_rad);
        let pat_r = directivity_pattern(geo.angle_r, k, -mic_angle_rad);
        let gain_l = (vol_factor * dist_gain_l * pat_l) as f32;
        let gain_r = (vol_factor * dist_gain_r * pat_r) as f32;

        // --- 早期反射タップ(イメージソース法)---
        let source_height = cast.height.unwrap_or(room_geo.listener_height) + cast.height_offset;
        let early_taps = build_early_taps(
            mono,
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
        let early_max_rel = early_taps.iter().map(|t| t.rel_l.max(t.rel_r)).max().unwrap_or(0);

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
        Ok(stereo)
    }
}
```
ファイル冒頭の `use std::path::Path;` と `use crate::resampler::resample_mono;` が不要になれば削除する。

- [ ] **Step 5: 較正を実装する**

`crates/s2v-audio/src/calib.rs` のテストの上に:
```rust
//! 基準ゲインの較正(設計書 5.10 C)。基準の台詞を残響なしで処理したとき、
//! マスター前のステレオのラウドネスが入力のラウドネスと一致するゲインを求める。

use std::collections::HashMap;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use s2v_core::{Cast, SceneConfig};

use crate::filters::Biquad;
use crate::loudness::{apply_gain_db, measure_mono, measure_stereo, Loudness};
use crate::processor::AudioProcessor;

/// 較正に使う信号の種(固定)。
pub const CALIBRATION_SEED: u64 = 0x5C21_7B1D;

/// 較正信号: 100Hz〜8kHz に帯域制限したピンクノイズ 5 秒、-23 LUFS。`seed` が同じなら同じ信号。
pub fn calibration_signal(seed: u64) -> Vec<f32> {
    let n = 5 * 48_000;
    let mut rng = SmallRng::seed_from_u64(seed);
    // Paul Kellet のピンクノイズフィルタ
    let mut b = [0.0f64; 7];
    let mut hp = Biquad::highpass(100.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut lp = Biquad::lowpass(8_000.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0);
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            let w: f64 = rng.gen_range(-1.0..1.0);
            b[0] = 0.99886 * b[0] + w * 0.0555179;
            b[1] = 0.99332 * b[1] + w * 0.0750759;
            b[2] = 0.96900 * b[2] + w * 0.1538520;
            b[3] = 0.86650 * b[3] + w * 0.3104856;
            b[4] = 0.55000 * b[4] + w * 0.5329522;
            b[5] = -0.7616 * b[5] - w * 0.0168980;
            let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
            b[6] = w * 0.115926;
            lp.process(hp.process(pink)) as f32
        })
        .collect();
    if let Loudness::Lufs(l) = measure_mono(&out) {
        apply_gain_db(&mut out, -23.0 - l);
    }
    out
}

/// 基準の台詞: distance が reference_dist、pan 0、height 既定、volume 1.0。
pub fn reference_cast(reference_dist: f64) -> Cast {
    Cast {
        name: "基準".to_string(),
        speaker_name: String::new(),
        pan: 0.0,
        distance: reference_dist,
        volume: 1.0,
        params: HashMap::new(),
        height: None,
        height_offset: 0.0,
        appearance: None,
    }
}

/// 残響なしの場面(早期反射は設定どおり)。
pub fn reference_scene() -> SceneConfig {
    SceneConfig {
        reverb_wet: Some(0.0),
        ..SceneConfig::new("較正")
    }
}

pub(crate) fn compute_calibration_gain(p: &AudioProcessor) -> f64 {
    let input = calibration_signal(CALIBRATION_SEED);
    let Loudness::Lufs(lin) = measure_mono(&input) else {
        return 1.0;
    };
    let out = p
        .render_with_gain(&input, &reference_cast(p.reference_dist()), &reference_scene(), 1.0)
        .expect("較正信号の処理に失敗しました");
    let Loudness::Lufs(lout) = measure_stereo(&out) else {
        return 1.0;
    };
    10f64.powf((lin - lout) / 20.0)
}
```

- [ ] **Step 6: `process` を使っていた既存テストを直す**

`processor.rs` の `mod tests` を次の方針で直す:
- 削除する(WAV 入出力・ピーク正規化・台詞ごとのリミッタ・音声合成エンジン補正を検証していたため): `process_returns_error_instead_of_panicking_on_empty_wav`、`process_returns_error_instead_of_panicking_on_zero_channel_wav`、`process_returns_error_instead_of_panicking_on_zero_bit_depth_wav`、`reference_gain_db_does_not_affect_output_level`、`process_creates_stereo_wav`、`prewarm_then_process_succeeds`
- 書き換える(物理の挙動を検証しているので残す): `process_panning_creates_stereo_asymmetry`、`early_reflections_disabled_matches_no_early_output`、`early_reflections_enabled_adds_energy`、`outdoor_scene_processes_with_near_dry_reverb`、`scene_room_dims_affect_process_output_length`、`speaker_height_changes_process_output`、`speaker_height_base_plus_offset_equals_absolute`。書き換え方: テスト内で WAV を書いて `p.process(&in_path, &out_path, &cast, &scene)` を呼び、出力 WAV を読み直している部分を、`let out = p.render(&tone(4_800, 0.5), &cast, &scene).unwrap();` にして、`out`(`Vec<[f32; 2]>`)を直接検査する。出力サンプル数を戻り値で比べていた箇所は `out.len()` を比べる。

- [ ] **Step 7: テストが通ることを確認する**

Run: `cargo test --workspace`
Expected: すべて PASS

- [ ] **Step 8: Commit**

```bash
git add crates/s2v-audio crates/s2v-core/src/config.rs
git commit -m "feat(audio): 音響処理を正規化済み入力の render に改め、ピーク正規化と台詞ごとのリミッタを廃止して基準ゲインを較正する"
```

---

### Task 13: 配置

**Files:**
- Create: `crates/s2v-core/src/placement.rs`
- Delete: `crates/s2v-core/src/timeline.rs`
- Modify: `crates/s2v-core/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 の `ScriptItem`・`ScriptCommand`・`SeAnchor`・`VisualKind`・`LineId`、Task 10 の `ms_to_samples`
- Produces:
  - `placement::MissingReason::{ NoFile, Duplicate, Unreadable, Silent, BadFormat, Unmeasurable }`、`MissingReason::as_str(self) -> &'static str`
  - `placement::LineSource::{ Present { len: i64 }, Missing(MissingReason) }`
  - `placement::FlatItem<'a> { item: &'a ScriptItem, pause: &'a PauseConfig, scene_index: usize }`、`placement::flatten(&[Scene]) -> Vec<FlatItem<'_>>`
  - `placement::PlacedLine { id: LineId, scene: u32, cast: String, text: String, start: i64, end: i64, missing: Option<MissingReason>, flat_index: usize }`
  - `placement::PlacedScene { id: u32, name: Option<String>, index: usize, silent: bool, start: i64, end: i64, visual_text: Option<String>, visual_kind: Option<VisualKind> }`
  - `placement::PlacedEvent::{ Se { path: String, start: i64, gain_db: f64 }, BgmStart { path: String, start: i64 }, BgmStop { start: i64 } }`、`PlacedEvent::start(&self) -> i64`
  - `placement::Placement { lines, scenes, events, warnings: Vec<String>, errors: Vec<String> }`、`Placement::end_sample(&self) -> i64`
  - `placement::place(items: &[FlatItem<'_>], sources: &HashMap<LineId, LineSource>, placeholder_ms_per_char: u32) -> Placement`
  - 位置・長さはすべて 48kHz のサンプル数

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-core/src/placement.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ScriptParser;

    fn scenes(body: &str) -> Vec<Scene> {
        let text = format!(
            "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中\n\nB:佐藤\n\n@script\n{body}"
        );
        ScriptParser::new().parse_str(&text).unwrap()
    }

    fn present(list: &[(u32, u32, f64)]) -> HashMap<LineId, LineSource> {
        list.iter()
            .map(|&(scene, line, ms)| (LineId { scene, line }, LineSource::Present { len: ms_to_samples(ms) }))
            .collect()
    }

    fn ms(samples: i64) -> f64 {
        samples as f64 / 48.0
    }

    fn line_ms(p: &Placement, line: u32) -> (f64, f64) {
        let l = p.lines.iter().find(|l| l.id.line == line).unwrap();
        (ms(l.start), ms(l.end))
    }

    #[test]
    fn first_line_at_zero_and_gaps_follow_pause_rules() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] A:二\n[S01-L003] B:三\n#paragraph[S02]\n[S02-L004] A:四\n");
        let src = present(&[(1, 1, 1000.0), (1, 2, 1000.0), (1, 3, 1000.0), (2, 4, 1000.0)]);
        let p = place(&flatten(&s), &src, 200);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(line_ms(&p, 1), (0.0, 1000.0));
        assert_eq!(line_ms(&p, 2), (1500.0, 2500.0));
        assert_eq!(line_ms(&p, 3), (2800.0, 3800.0));
        assert_eq!(line_ms(&p, 4), (4800.0, 5800.0));
        assert_eq!(p.scenes.len(), 2);
        assert_eq!((ms(p.scenes[0].start), ms(p.scenes[0].end)), (0.0, 3800.0));
        assert_eq!(p.scenes[1].index, 2);
    }

    #[test]
    fn overlap_uses_last_line_end_and_normal_line_uses_cursor() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B(at=L001+1000):二\n[S01-L003] A(overlap=100):三\n[S01-L004] B:四\n");
        let src = present(&[(1, 1, 5000.0), (1, 2, 1000.0), (1, 3, 1000.0), (1, 4, 1000.0)]);
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 2), (1000.0, 2000.0));
        assert_eq!(line_ms(&p, 3), (1900.0, 2900.0));
        assert_eq!(line_ms(&p, 4), (5300.0, 6300.0));
    }

    #[test]
    fn silent_scene_takes_pause_time_without_paragraph_pause() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n#paragraph[S02] 間\n#pause 2000\n#paragraph[S03]\n[S03-L002] A:二\n");
        let src = present(&[(1, 1, 1000.0), (3, 2, 1000.0)]);
        let p = place(&flatten(&s), &src, 200);
        let s2 = &p.scenes[1];
        assert!(s2.silent);
        assert_eq!((ms(s2.start), ms(s2.end)), (1000.0, 3000.0));
        assert_eq!(s2.name.as_deref(), Some("間"));
        assert_eq!(line_ms(&p, 2), (4000.0, 5000.0));
        assert!(!p.scenes[2].silent);
    }

    #[test]
    fn missing_lines_use_placeholder_length_and_keep_reason() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:あいうえお\n[S01-L002] B:かき\n");
        let mut src = HashMap::new();
        src.insert(LineId { scene: 1, line: 2 }, LineSource::Missing(MissingReason::Silent));
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 1), (0.0, 1000.0));
        assert_eq!(p.lines[0].missing, Some(MissingReason::NoFile));
        assert_eq!(line_ms(&p, 2), (1300.0, 1700.0));
        assert_eq!(p.lines[1].missing, Some(MissingReason::Silent));
    }

    #[test]
    fn se_positions_follow_anchors_and_are_clamped() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n#se now.wav\n#se door.wav at=L001+200\n#se bell.wav end=L001-200 gain=-3\n#se early.wav at=L001-500\n");
        let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
        let at = |name: &str| {
            p.events
                .iter()
                .find_map(|e| match e {
                    PlacedEvent::Se { path, start, gain_db } if path.ends_with(name) => Some((ms(*start), *gain_db)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(at("now.wav"), (1000.0, 0.0));
        assert_eq!(at("door.wav"), (200.0, 0.0));
        assert_eq!(at("bell.wav"), (800.0, -3.0));
        assert_eq!(at("early.wav"), (0.0, 0.0));
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn parallel_lines_start_together() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n2\n[S01-L002] A:二\n[S01-L003] B:三\n[S01-L004] A:四\n");
        let src = present(&[(1, 1, 1000.0), (1, 2, 1000.0), (1, 3, 2000.0), (1, 4, 1000.0)]);
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 2), (1500.0, 2500.0));
        assert_eq!(line_ms(&p, 3), (1500.0, 3500.0));
        assert_eq!(line_ms(&p, 4), (3800.0, 4800.0));
    }

    #[test]
    fn scene_start_regression_is_an_error_but_same_time_is_allowed() {
        let bad = scenes("#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L002] B(overlap=3500):二\n");
        let src = present(&[(1, 1, 1000.0), (2, 2, 1000.0)]);
        let p = place(&flatten(&bad), &src, 200);
        assert_eq!(line_ms(&p, 1), (3000.0, 4000.0));
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);

        let same = scenes("#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L002] B(at=L001):二\n");
        let p = place(&flatten(&same), &src, 200);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
    }

    #[test]
    fn bgm_events_and_visuals_are_recorded() {
        let s = scenes("#bgm_start a.wav\n#paragraph[S01] 朝\n#visual (image) 港。\n#visual 船。\n[S01-L001] A:一\n#bgm_stop\n");
        let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
        assert!(matches!(p.events[0], PlacedEvent::BgmStart { start: 0, .. }));
        assert!(matches!(p.events[1], PlacedEvent::BgmStop { start } if start == ms_to_samples(1000.0)));
        assert_eq!(p.scenes[0].visual_text.as_deref(), Some("港。\n船。"));
        assert_eq!(p.scenes[0].visual_kind, Some(VisualKind::Image));
        assert_eq!(p.end_sample(), ms_to_samples(1000.0));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` で `pub mod timeline;` と `pub use timeline::{...};` を削除し、`pub mod placement;` を追加する。`crates/s2v-core/src/timeline.rs` を削除する(`git rm crates/s2v-core/src/timeline.rs`)。`s2v-export` の旧 `exporter.rs` が `TimelineEvent` を使っているので、ここで `crates/s2v-export/src/exporter.rs` も削除し、`crates/s2v-export/src/lib.rs` から `pub mod exporter;` と `pub use exporter::{...};` を削除する(Task 14・15 で置き換える。BGM の区間計算は Task 14 で移植する)。

Run: `cargo test -p s2v-core placement`
Expected: コンパイルエラー(`place` が無い)

- [ ] **Step 3: 実装する**

`crates/s2v-core/src/placement.rs` のテストの上に:
```rust
//! 台詞・シーン・イベントの配置(設計書 5.5)。位置と長さは 48kHz のサンプル数。

use std::collections::HashMap;

use serde::Serialize;

use crate::ids::{format_scene_id, LineId};
use crate::time::ms_to_samples;
use crate::types::{PauseConfig, Scene, ScriptCommand, ScriptItem, SeAnchor, VisualKind};

/// 台詞が不足した理由(`timeline.json` の `reason`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    NoFile,
    Duplicate,
    Unreadable,
    Silent,
    BadFormat,
    Unmeasurable,
}

impl MissingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            MissingReason::NoFile => "no_file",
            MissingReason::Duplicate => "duplicate",
            MissingReason::Unreadable => "unreadable",
            MissingReason::Silent => "silent",
            MissingReason::BadFormat => "bad_format",
            MissingReason::Unmeasurable => "unmeasurable",
        }
    }
}

/// 台詞の音声の有無と長さ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSource {
    /// 使用範囲の長さ(サンプル)
    Present { len: i64 },
    Missing(MissingReason),
}

/// 台本の並びに平らにした項目。`scene_index` は `@scene`(音響の場面)の番号。
pub struct FlatItem<'a> {
    pub item: &'a ScriptItem,
    pub pause: &'a PauseConfig,
    pub scene_index: usize,
}

pub fn flatten(scenes: &[Scene]) -> Vec<FlatItem<'_>> {
    scenes
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            s.items.iter().map(move |item| FlatItem {
                item,
                pause: &s.pause_config,
                scene_index: i,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLine {
    pub id: LineId,
    /// 属するシーン(`#paragraph`)の番号。最初の `#paragraph` より前は 0。
    pub scene: u32,
    pub cast: String,
    pub text: String,
    pub start: i64,
    pub end: i64,
    pub missing: Option<MissingReason>,
    /// `flatten` の結果での位置
    pub flat_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedScene {
    pub id: u32,
    pub name: Option<String>,
    /// 1始まりの並び順
    pub index: usize,
    /// 無言のシーン(台詞なし、#pause のみ)
    pub silent: bool,
    pub start: i64,
    pub end: i64,
    pub visual_text: Option<String>,
    pub visual_kind: Option<VisualKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacedEvent {
    Se { path: String, start: i64, gain_db: f64 },
    BgmStart { path: String, start: i64 },
    BgmStop { start: i64 },
}

impl PlacedEvent {
    pub fn start(&self) -> i64 {
        match self {
            PlacedEvent::Se { start, .. } | PlacedEvent::BgmStart { start, .. } | PlacedEvent::BgmStop { start } => *start,
        }
    }
}

#[derive(Debug, Default)]
pub struct Placement {
    pub lines: Vec<PlacedLine>,
    pub scenes: Vec<PlacedScene>,
    pub events: Vec<PlacedEvent>,
    /// 継続できる問題(終了コード 1)
    pub warnings: Vec<String>,
    /// 出力を止める問題(終了コード 2)
    pub errors: Vec<String>,
}

impl Placement {
    /// 台詞・シーン・イベントの最も遅い時刻(サンプル)。
    pub fn end_sample(&self) -> i64 {
        let l = self.lines.iter().map(|l| l.end).max().unwrap_or(0);
        let s = self.scenes.iter().map(|s| s.end).max().unwrap_or(0);
        let e = self.events.iter().map(|e| e.start()).max().unwrap_or(0);
        l.max(s).max(e)
    }
}

struct OpenScene {
    id: u32,
    name: Option<String>,
    open_cursor: i64,
    pause_total: i64,
    span: Option<(i64, i64)>,
    visual_text: Option<String>,
    visual_kind: Option<VisualKind>,
}

impl OpenScene {
    fn new(id: u32, name: Option<String>, open_cursor: i64) -> Self {
        Self { id, name, open_cursor, pause_total: 0, span: None, visual_text: None, visual_kind: None }
    }

    fn add_line(&mut self, start: i64, end: i64) {
        self.span = Some(match self.span {
            Some((a, b)) => (a.min(start), b.max(end)),
            None => (start, end),
        });
    }
}

fn placeholder_len(text: &str, ms_per_char: u32) -> i64 {
    text.chars().count() as i64 * ms_to_samples(ms_per_char as f64)
}

/// 台本の並びに台詞を配置する。`sources` に無い台詞は `no_file` として仮の無音を置く。
pub fn place(
    items: &[FlatItem<'_>],
    sources: &HashMap<LineId, LineSource>,
    placeholder_ms_per_char: u32,
) -> Placement {
    let mut p = Placement::default();
    // cursor: ここまでに置いた全台詞の終了時刻の最大値(後退しない)
    let mut cursor: i64 = 0;
    // last_end: 台本の並びで直前の台詞の終了時刻(overlap の基準)
    let mut last_end: i64 = 0;
    let mut prev_cast: Option<&str> = None;
    let mut any_line = false;
    let mut first_in_scene = true;
    let mut open: Option<OpenScene> = None;
    let mut closed: Vec<OpenScene> = Vec::new();
    let mut spans: HashMap<u32, (i64, i64)> = HashMap::new();
    let mut deferred_se: Vec<(String, SeAnchor, f64)> = Vec::new();
    let mut parallel_left = 0usize;
    let mut parallel_anchor: Option<i64> = None;

    for (flat_index, fi) in items.iter().enumerate() {
        match fi.item {
            ScriptItem::Command(cmd) => match cmd {
                ScriptCommand::Paragraph { id, name, .. } => {
                    if let Some(s) = open.take() {
                        closed.push(s);
                    }
                    open = Some(OpenScene::new(id.unwrap_or(0), name.clone(), cursor));
                    first_in_scene = true;
                }
                ScriptCommand::Visual { kind, text } => match open.as_mut() {
                    Some(s) => {
                        s.visual_text = Some(match s.visual_text.take() {
                            Some(t) => format!("{t}\n{text}"),
                            None => text.clone(),
                        });
                        if s.visual_kind.is_none() {
                            s.visual_kind = *kind;
                        }
                    }
                    None => p.warnings.push("#visual が最初の #paragraph より前にあります（無視します）".to_string()),
                },
                ScriptCommand::Pause(ms) => {
                    let n = ms_to_samples(*ms);
                    cursor += n;
                    if let Some(s) = open.as_mut() {
                        s.pause_total += n;
                    }
                }
                ScriptCommand::Parallel(n) => {
                    parallel_left = *n;
                    parallel_anchor = None;
                }
                ScriptCommand::BgmStart(path) => p.events.push(PlacedEvent::BgmStart { path: path.clone(), start: cursor }),
                ScriptCommand::BgmStop => p.events.push(PlacedEvent::BgmStop { start: cursor }),
                ScriptCommand::Se { path, anchor, gain_db, .. } => match anchor {
                    None => p.events.push(PlacedEvent::Se { path: path.clone(), start: cursor, gain_db: *gain_db }),
                    Some(a) => deferred_se.push((path.clone(), *a, *gain_db)),
                },
            },
            ScriptItem::Speech { id, cast_name, text, overlap_ms, at, .. } => {
                // ID の無い台詞は検証(validate)で弾かれる
                let Some(id) = *id else { continue };
                if open.is_none() {
                    open = Some(OpenScene::new(0, None, cursor));
                    first_in_scene = true;
                }
                let (len, missing) = match sources.get(&id) {
                    Some(LineSource::Present { len }) => (*len, None),
                    Some(LineSource::Missing(r)) => (placeholder_len(text, placeholder_ms_per_char), Some(*r)),
                    None => (placeholder_len(text, placeholder_ms_per_char), Some(MissingReason::NoFile)),
                };
                let gap = if !any_line {
                    0
                } else if first_in_scene {
                    ms_to_samples(fi.pause.paragraph_ms)
                } else if prev_cast == Some(cast_name.as_str()) {
                    ms_to_samples(fi.pause.sentence_ms)
                } else {
                    ms_to_samples(fi.pause.cast_ms)
                };
                let mut start = if let Some(n) = overlap_ms {
                    let s = last_end - ms_to_samples(*n);
                    if s < 0 {
                        p.warnings.push(format!("{}: overlap が大きすぎて開始が 0 より前になります（0 にします）", id.display()));
                        0
                    } else {
                        s
                    }
                } else if let Some(r) = at {
                    match spans.get(&r.line) {
                        Some(&(s, _)) => s + ms_to_samples(r.offset_ms as f64),
                        None => cursor + gap,
                    }
                } else {
                    cursor + gap
                };
                if parallel_left > 0 {
                    match parallel_anchor {
                        Some(a) => start = a,
                        None => parallel_anchor = Some(start),
                    }
                    parallel_left -= 1;
                }
                let end = start + len;
                spans.insert(id.line, (start, end));
                cursor = cursor.max(end);
                last_end = end;
                prev_cast = Some(cast_name.as_str());
                any_line = true;
                first_in_scene = false;
                let s = open.as_mut().expect("直前で開いている");
                s.add_line(start, end);
                let scene = s.id;
                p.lines.push(PlacedLine {
                    id,
                    scene,
                    cast: cast_name.clone(),
                    text: text.clone(),
                    start,
                    end,
                    missing,
                    flat_index,
                });
            }
        }
    }
    if let Some(s) = open.take() {
        closed.push(s);
    }

    // 台詞を基準にした SE(書かれた位置とは無関係に、指定した台詞を基準に置く)
    for (path, anchor, gain_db) in deferred_se {
        let (r, base) = match anchor {
            SeAnchor::Start(r) => (r, spans.get(&r.line).map(|v| v.0)),
            SeAnchor::End(r) => (r, spans.get(&r.line).map(|v| v.1)),
        };
        let Some(base) = base else {
            p.errors.push(format!("#se の基準の台詞 L{:03} が見つかりません: {path}", r.line));
            continue;
        };
        let mut start = base + ms_to_samples(r.offset_ms as f64);
        if start < 0 {
            p.warnings.push(format!("#se の位置が 0 より前になります（0 にします）: {path}"));
            start = 0;
        }
        p.events.push(PlacedEvent::Se { path, start, gain_db });
    }

    // シーンの時間: 所属する台詞の最小開始〜最大終了。無言のシーンは #pause の合計
    for (i, s) in closed.into_iter().enumerate() {
        let (start, end, silent) = match s.span {
            Some((a, b)) => (a, b, false),
            None if s.pause_total > 0 => (s.open_cursor, s.open_cursor + s.pause_total, true),
            None => {
                p.errors.push(format!("{}: 台詞も #pause もないシーンです", format_scene_id(s.id)));
                (s.open_cursor, s.open_cursor, true)
            }
        };
        p.scenes.push(PlacedScene {
            id: s.id,
            name: s.name,
            index: i + 1,
            silent,
            start,
            end,
            visual_text: s.visual_text,
            visual_kind: s.visual_kind,
        });
    }
    // シーンの開始時刻は台本の並びで単調非減少(同時刻は許す)
    for w in p.scenes.windows(2) {
        if w[1].start < w[0].start {
            p.errors.push(format!(
                "シーン {} の開始({}ms)が、前のシーン {} の開始({}ms)より早くなっています",
                format_scene_id(w[1].id),
                w[1].start / 48,
                format_scene_id(w[0].id),
                w[0].start / 48
            ));
        }
    }
    p
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-core placement`
Expected: PASS

Run: `cargo build --workspace`
Expected: 成功(`s2v-export` は `script_html` だけになっている)

- [ ] **Step 5: Commit**

```bash
git add -A crates/s2v-core crates/s2v-export
git commit -m "feat(core): cursor と last_line_end による配置、無言のシーン、台詞基準の SE、シーン順序の検査を追加する"
```

---

### Task 14: マスターリミッタとミックス

**Files:**
- Create: `crates/s2v-audio/src/limiter.rs`、`crates/s2v-export/src/wavio.rs`、`crates/s2v-export/src/bgm.rs`、`crates/s2v-export/src/mix.rs`
- Modify: `crates/s2v-audio/src/lib.rs`(`pub mod limiter;`)、`crates/s2v-export/src/lib.rs`

**Interfaces:**
- Consumes: Task 13 の `PlacedEvent`、Task 2 の `BgmConfig`、流用元の `resample_mono`
- Produces:
  - `limiter::soft_limit(buf: &mut [[f32; 2]], ceiling_db: f64)`(ステレオ連動、先読み 5ms、リリース 50ms)
  - `wavio::read_stereo_48k(path: &Path) -> anyhow::Result<Vec<[f32; 2]>>`、`wavio::write_f32_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()>`、`wavio::write_i24_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()>`
  - `bgm::BgmSegment { path: PathBuf, mix_start_s: f64, mix_duration_s: f64, fade_in_s: f64, fade_out_s: f64 }`、`bgm::compute_bgm_segments(events: &[PlacedEvent], total_s: f64, crossfade_s: f64) -> Vec<BgmSegment>`
  - `mix::MixRequest<'a> { dialogue: Vec<(i64, Vec<[f32; 2]>)>, events: &'a [PlacedEvent], timeline_end: i64, bgm: &'a BgmConfig, master_ceiling_db: f64 }`
  - `mix::Mixed { dialogue: Vec<[f32; 2]>, bgm: Vec<[f32; 2]>, se: Vec<[f32; 2]>, master: Vec<[f32; 2]>, warnings: Vec<String> }`、`mix::mix(req: MixRequest<'_>) -> Mixed`

- [ ] **Step 1: リミッタの失敗するテストを書く**

`crates/s2v-audio/src/limiter.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(n: usize, amp_l: f32, amp_r: f32) -> Vec<[f32; 2]> {
        (0..n)
            .map(|i| {
                let s = ((i as f32) * 0.01).sin();
                [amp_l * s, amp_r * s]
            })
            .collect()
    }

    #[test]
    fn output_never_exceeds_ceiling() {
        let mut x = stereo_sine(48_000, 2.0, 1.5);
        soft_limit(&mut x, -1.0);
        let c = 10f32.powf(-1.0 / 20.0);
        let peak = x.iter().flat_map(|f| f.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= c + 1e-6, "{peak}");
        assert!(peak > c * 0.9, "{peak}");
    }

    #[test]
    fn signal_below_ceiling_is_untouched() {
        let mut x = stereo_sine(4_800, 0.5, 0.3);
        let orig = x.clone();
        soft_limit(&mut x, -1.0);
        assert_eq!(x, orig);
    }

    #[test]
    fn channels_are_linked() {
        let mut x = stereo_sine(48_000, 2.0, 0.5);
        soft_limit(&mut x, -1.0);
        for f in x.iter().filter(|f| f[0].abs() > 0.1) {
            assert!((f[1] / f[0] - 0.25).abs() < 1e-3);
        }
    }

    #[test]
    fn empty_buffer_is_fine() {
        let mut x: Vec<[f32; 2]> = Vec::new();
        soft_limit(&mut x, -1.0);
        assert!(x.is_empty());
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod limiter;` を追加してから実行する。

Run: `cargo test -p s2v-audio limiter`
Expected: コンパイルエラー

- [ ] **Step 3: リミッタを実装する**

`crates/s2v-audio/src/limiter.rs` のテストの上に:
```rust
//! マスターのソフトリミッタ(設計書 5.4)。ステレオ連動・先読み 5ms・リリース 50ms。
//! 各サンプルに必要なゲインの「先読み区間の最小値」を同じ長さで移動平均するので、
//! ゲインはなめらかに下がり、かつ必要なゲイン以下になることが保証される。

use std::collections::VecDeque;

/// 先読み(48kHz で 5ms)。
pub const LOOKAHEAD: usize = 240;
/// リリースの時定数[秒]。
pub const RELEASE_S: f64 = 0.05;

pub fn soft_limit(buf: &mut [[f32; 2]], ceiling_db: f64) {
    let n = buf.len();
    if n == 0 {
        return;
    }
    let ceiling = 10f64.powf(ceiling_db / 20.0);
    let req: Vec<f64> = buf
        .iter()
        .map(|f| {
            let p = f[0].abs().max(f[1].abs()) as f64;
            if p > ceiling {
                ceiling / p
            } else {
                1.0
            }
        })
        .collect();
    if req.iter().all(|&r| r >= 1.0) {
        return;
    }

    // m[k] = min(req[k .. k+L))
    let l = LOOKAHEAD;
    let mut m = vec![1.0f64; n];
    let mut dq: VecDeque<usize> = VecDeque::new();
    for k in (0..n).rev() {
        while let Some(&b) = dq.back() {
            if req[b] >= req[k] {
                dq.pop_back();
            } else {
                break;
            }
        }
        dq.push_back(k);
        while let Some(&f) = dq.front() {
            if f >= k + l {
                dq.pop_front();
            } else {
                break;
            }
        }
        m[k] = req[*dq.front().expect("直前に追加している")];
    }

    // g[i] = mean(m[i-L+1 ..= i]): 平均される m[k] はすべて i を区間に含むので g[i] <= req[i]
    let a = 1.0 - (-1.0 / (RELEASE_S * 48_000.0)).exp();
    let c = ceiling as f32;
    let mut sum = 0.0f64;
    let mut env = 1.0f64;
    for i in 0..n {
        sum += m[i];
        if i >= l {
            sum -= m[i - l];
        }
        let g = sum / (i + 1).min(l) as f64;
        // 下がるときは即座に、戻るときはリリースの時定数で
        env = if g < env { g } else { env + (g - env) * a };
        let gi = env as f32;
        buf[i][0] = (buf[i][0] * gi).clamp(-c, c);
        buf[i][1] = (buf[i][1] * gi).clamp(-c, c);
    }
}
```

Run: `cargo test -p s2v-audio limiter`
Expected: PASS

- [ ] **Step 4: ミックスの失敗するテストを書く**

`crates/s2v-export/src/mix.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::wavio::{write_f32_stereo, write_i24_stereo};

    fn bgm_cfg() -> BgmConfig {
        BgmConfig { crossfade_s: 3.0, se_fade_out_s: 0.05 }
    }

    fn write_mono(path: &Path, value: f32, n: usize) {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..n {
            w.write_sample(value).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn dialogue_is_placed_and_tracks_are_separate() {
        let cfg = bgm_cfg();
        let out = mix(MixRequest {
            dialogue: vec![(480, vec![[0.5, 0.25]; 100])],
            events: &[],
            timeline_end: 1_000,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.dialogue.len(), 1_000);
        assert_eq!(out.dialogue[480], [0.5, 0.25]);
        assert_eq!(out.dialogue[479], [0.0, 0.0]);
        assert_eq!(out.master[500], [0.5, 0.25]);
        assert!(out.bgm.iter().all(|f| *f == [0.0, 0.0]));
        assert!(out.se.iter().all(|f| *f == [0.0, 0.0]));
    }

    #[test]
    fn se_uses_gain_and_missing_files_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let se = tmp.path().join("効果 音.wav");
        write_mono(&se, 0.5, 4_800);
        let events = vec![
            PlacedEvent::Se { path: se.to_string_lossy().into_owned(), start: 0, gain_db: -6.0206 },
            PlacedEvent::Se { path: tmp.path().join("none.wav").to_string_lossy().into_owned(), start: 0, gain_db: 0.0 },
        ];
        let cfg = bgm_cfg();
        let out = mix(MixRequest { dialogue: vec![], events: &events, timeline_end: 0, bgm: &cfg, master_ceiling_db: -1.0 });
        assert_eq!(out.se.len(), 4_800);
        assert!((out.se[100][0] - 0.25).abs() < 1e-4);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn bgm_loops_until_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let bgm = tmp.path().join("bgm.wav");
        write_mono(&bgm, 0.5, 4_800);
        let events = vec![
            PlacedEvent::BgmStart { path: bgm.to_string_lossy().into_owned(), start: 0 },
            PlacedEvent::BgmStop { start: 48_000 },
        ];
        let cfg = bgm_cfg();
        let out = mix(MixRequest { dialogue: vec![], events: &events, timeline_end: 96_000, bgm: &cfg, master_ceiling_db: -1.0 });
        assert!((out.bgm[24_000][0] - 0.15).abs() < 1e-4);
        assert_eq!(out.bgm[60_000], [0.0, 0.0]);
    }

    #[test]
    fn master_is_limited_but_dialogue_track_is_not() {
        let cfg = bgm_cfg();
        let out = mix(MixRequest {
            dialogue: vec![(0, vec![[3.0, -3.0]; 4_800])],
            events: &[],
            timeline_end: 0,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.dialogue[2_400], [3.0, -3.0]);
        let c = 10f32.powf(-1.0 / 20.0);
        assert!(out.master.iter().all(|f| f[0].abs() <= c + 1e-6 && f[1].abs() <= c + 1e-6));
    }

    #[test]
    fn writers_produce_expected_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = vec![[0.5f32, -0.5]; 10];
        let f = tmp.path().join("f.wav");
        let i = tmp.path().join("i.wav");
        write_f32_stereo(&f, &[[2.0, -2.0]]).unwrap();
        write_i24_stereo(&i, &buf).unwrap();
        let rf = hound::WavReader::open(&f).unwrap();
        assert_eq!((rf.spec().bits_per_sample, rf.spec().sample_format, rf.spec().channels), (32, hound::SampleFormat::Float, 2));
        let mut ri = hound::WavReader::open(&i).unwrap();
        assert_eq!((ri.spec().bits_per_sample, ri.spec().sample_rate), (24, 48_000));
        let first: i32 = ri.samples::<i32>().next().unwrap().unwrap();
        assert_eq!(first, 4_194_304);
    }
}
```

`crates/s2v-export/Cargo.toml` の `[dev-dependencies]` に `tempfile = "3"` があることを確認する(流用元にある)。

- [ ] **Step 5: テストが失敗することを確認する**

`crates/s2v-export/src/lib.rs` を次にする:
```rust
pub mod bgm;
pub mod mix;
pub mod script_html;
pub mod wavio;

pub use mix::{mix, MixRequest, Mixed};
pub use script_html::render_script_html;
```

Run: `cargo test -p s2v-export mix`
Expected: コンパイルエラー

- [ ] **Step 6: WAV 入出力を実装する**

`crates/s2v-export/src/wavio.rs`:
```rust
//! BGM/SE の読み込みと、トラック・ミックスの書き出し。

use std::path::Path;

/// BGM/SE を読む(モノラル・ステレオ可)。48kHz でなければ左右別々にリサンプルする。
pub fn read_stereo_48k(path: &Path) -> anyhow::Result<Vec<[f32; 2]>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()?
        }
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };
    let ch = spec.channels.max(1) as usize;
    let stereo: Vec<[f32; 2]> = if ch == 1 {
        raw.iter().map(|&s| [s, s]).collect()
    } else {
        raw.chunks(ch).map(|c| [c[0], c[1]]).collect()
    };
    if spec.sample_rate == 48_000 {
        return Ok(stereo);
    }
    let left: Vec<f32> = stereo.iter().map(|s| s[0]).collect();
    let right: Vec<f32> = stereo.iter().map(|s| s[1]).collect();
    let left = s2v_audio::resample_mono(&left, spec.sample_rate, 48_000)?;
    let right = s2v_audio::resample_mono(&right, spec.sample_rate, 48_000)?;
    Ok(left.into_iter().zip(right).map(|(l, r)| [l, r]).collect())
}

/// 32bit 浮動小数ステレオ(クリップしない)。`tracks/*.wav` 用。
pub fn write_f32_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec)?;
    for f in buf {
        w.write_sample(f[0])?;
        w.write_sample(f[1])?;
    }
    w.finalize()?;
    Ok(())
}

/// 24bit 整数ステレオ。`full_dialogue.wav` 用(マスターのリミッタ後なので ±1 を超えない)。
pub fn write_i24_stereo(path: &Path, buf: &[[f32; 2]]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    const MAX: f32 = 8_388_607.0;
    for f in buf {
        for v in f {
            w.write_sample((v.clamp(-1.0, 1.0) * MAX).round() as i32)?;
        }
    }
    w.finalize()?;
    Ok(())
}
```
`0.5 * 8388607 = 4194303.5` は `round()` で `4194304` になる(テストの期待値)。

- [ ] **Step 7: BGM の区間計算を移植する**

`crates/s2v-export/src/bgm.rs`(流用元 `exporter.rs` の `compute_bgm_segments` を `PlacedEvent` 用に移植):
```rust
//! BGM の区間とクロスフェード(流用元 compute_bgm_segments の移植)。

use std::path::{Path, PathBuf};

use s2v_core::placement::PlacedEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct BgmSegment {
    pub path: PathBuf,
    pub mix_start_s: f64,
    pub mix_duration_s: f64,
    pub fade_in_s: f64,
    pub fade_out_s: f64,
}

/// `#bgm_start` から次の `#bgm_start` / `#bgm_stop` / 終端までを1区間とする。
/// 区間の境目は `crossfade_s` でクロスフェードする(区間長の 1/3 を上限)。
pub fn compute_bgm_segments(events: &[PlacedEvent], total_s: f64, crossfade_s: f64) -> Vec<BgmSegment> {
    let half = crossfade_s / 2.0;
    struct Raw {
        path: PathBuf,
        start: f64,
        end: f64,
    }
    let mut raw: Vec<Raw> = Vec::new();
    let mut pending: Option<(f64, PathBuf)> = None;
    let mut ordered: Vec<&PlacedEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.start());
    for e in ordered {
        match e {
            PlacedEvent::BgmStart { path, start } => {
                let t = *start as f64 / 48_000.0;
                if let Some((s, p)) = pending.take() {
                    raw.push(Raw { path: p, start: s, end: t });
                }
                pending = Some((t, PathBuf::from(path)));
            }
            PlacedEvent::BgmStop { start } => {
                if let Some((s, p)) = pending.take() {
                    raw.push(Raw { path: p, start: s, end: *start as f64 / 48_000.0 });
                }
            }
            PlacedEvent::Se { .. } => {}
        }
    }
    if let Some((s, p)) = pending {
        let mut end = total_s;
        if end <= s {
            let file_dur = wav_duration_s(&p);
            end = s + if file_dur > 0.0 { file_dur } else { 30.0 };
        }
        raw.push(Raw { path: p, start: s, end });
    }

    let n = raw.len();
    raw.into_iter()
        .enumerate()
        .map(|(k, seg)| {
            let dur = (seg.end - seg.start).max(0.0);
            let clamp = dur / 3.0;
            let fi_half = if k > 0 { half.min(clamp) } else { 0.0 };
            let fo_half = if k + 1 < n { half.min(clamp) } else { 0.0 };
            let mix_start = (seg.start - fi_half).max(0.0);
            let mix_end = seg.end + fo_half;
            BgmSegment {
                path: seg.path,
                mix_start_s: mix_start,
                mix_duration_s: (mix_end - mix_start).max(0.0),
                fade_in_s: fi_half * 2.0,
                fade_out_s: fo_half * 2.0,
            }
        })
        .collect()
}

fn wav_duration_s(path: &Path) -> f64 {
    let Ok(reader) = hound::WavReader::open(path) else {
        return 0.0;
    };
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return 0.0;
    }
    reader.duration() as f64 / spec.sample_rate as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_segments_crossfade_and_last_runs_to_end() {
        let events = vec![
            PlacedEvent::BgmStart { path: "a.wav".into(), start: 0 },
            PlacedEvent::BgmStart { path: "b.wav".into(), start: 10 * 48_000 },
        ];
        let segs = compute_bgm_segments(&events, 20.0, 3.0);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].mix_start_s, segs[0].fade_in_s, segs[0].fade_out_s), (0.0, 0.0, 3.0));
        assert!((segs[0].mix_duration_s - 11.5).abs() < 1e-9);
        assert_eq!((segs[1].mix_start_s, segs[1].fade_in_s, segs[1].fade_out_s), (8.5, 3.0, 0.0));
        assert!((segs[1].mix_duration_s - 11.5).abs() < 1e-9);
    }
}
```

- [ ] **Step 8: ミックスを実装する**

`crates/s2v-export/src/mix.rs` のテストの上に:
```rust
//! トラック別ミックスとマスター(設計書 5.4)。dialogue / bgm / se は浮動小数のまま、
//! マスター(full_dialogue)にだけソフトリミッタを掛ける。

use std::path::Path;

use s2v_audio::limiter::soft_limit;
use s2v_core::placement::PlacedEvent;
use s2v_core::BgmConfig;

use crate::bgm::compute_bgm_segments;
use crate::wavio::read_stereo_48k;

/// BGM の音量(流用元と同じ -10dB 相当)。
const BGM_GAIN: f32 = 0.3;

pub struct MixRequest<'a> {
    /// (開始サンプル, 音響処理後のステレオ)
    pub dialogue: Vec<(i64, Vec<[f32; 2]>)>,
    pub events: &'a [PlacedEvent],
    /// 配置の終わり(無言のシーン・仮の無音を含む)。音声はこれより短くしない
    pub timeline_end: i64,
    pub bgm: &'a BgmConfig,
    pub master_ceiling_db: f64,
}

pub struct Mixed {
    pub dialogue: Vec<[f32; 2]>,
    pub bgm: Vec<[f32; 2]>,
    pub se: Vec<[f32; 2]>,
    pub master: Vec<[f32; 2]>,
    /// 見つからない・読めない BGM/SE(終了コード 1)
    pub warnings: Vec<String>,
}

fn add_at(buf: &mut [[f32; 2]], start: usize, src: &[[f32; 2]], gain: f32) {
    for (i, s) in src.iter().enumerate() {
        if let Some(d) = buf.get_mut(start + i) {
            d[0] += s[0] * gain;
            d[1] += s[1] * gain;
        }
    }
}

pub fn mix(req: MixRequest<'_>) -> Mixed {
    let mut warnings = Vec::new();

    // SE を読む
    let mut ses: Vec<(usize, Vec<[f32; 2]>, f32)> = Vec::new();
    for e in req.events {
        if let PlacedEvent::Se { path, start, gain_db } = e {
            match read_stereo_48k(Path::new(path)) {
                Ok(mut se) => {
                    let fo = ((req.bgm.se_fade_out_s * 48_000.0) as usize).min(se.len());
                    let len = se.len();
                    for (i, s) in se[len - fo..].iter_mut().enumerate() {
                        let g = 1.0 - i as f32 / fo as f32;
                        s[0] *= g;
                        s[1] *= g;
                    }
                    ses.push((*start as usize, se, 10f32.powf(*gain_db as f32 / 20.0)));
                }
                Err(err) => warnings.push(format!("SE を読めません（無視します）: {path}: {err}")),
            }
        }
    }

    let dialogue_end = req.dialogue.iter().map(|(s, v)| *s as usize + v.len()).max().unwrap_or(0);
    let base_end = dialogue_end.max(req.timeline_end.max(0) as usize);
    let segs = compute_bgm_segments(req.events, base_end as f64 / 48_000.0, req.bgm.crossfade_s);
    let bgm_end = segs
        .iter()
        .map(|s| ((s.mix_start_s + s.mix_duration_s) * 48_000.0) as usize)
        .max()
        .unwrap_or(0);
    let se_end = ses.iter().map(|(s, v, _)| s + v.len()).max().unwrap_or(0);
    let total = base_end.max(bgm_end).max(se_end);

    let mut dialogue = vec![[0.0f32; 2]; total];
    for (start, v) in &req.dialogue {
        add_at(&mut dialogue, *start as usize, v, 1.0);
    }

    let mut se = vec![[0.0f32; 2]; total];
    for (start, v, g) in &ses {
        add_at(&mut se, *start, v, *g);
    }

    let mut bgm = vec![[0.0f32; 2]; total];
    for seg in &segs {
        let src = match read_stereo_48k(&seg.path) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => continue,
            Err(err) => {
                warnings.push(format!("BGM を読めません（無視します）: {}: {err}", seg.path.display()));
                continue;
            }
        };
        let need = (seg.mix_duration_s * 48_000.0) as usize;
        let mut looped: Vec<[f32; 2]> = src.iter().copied().cycle().take(need).collect();
        let fi = ((seg.fade_in_s * 48_000.0) as usize).min(looped.len());
        for (i, s) in looped[..fi].iter_mut().enumerate() {
            let g = i as f32 / fi.max(1) as f32;
            s[0] *= g;
            s[1] *= g;
        }
        let fo = ((seg.fade_out_s * 48_000.0) as usize).min(looped.len());
        let fo_start = looped.len() - fo;
        for (i, s) in looped[fo_start..].iter_mut().enumerate() {
            let g = 1.0 - i as f32 / fo.max(1) as f32;
            s[0] *= g;
            s[1] *= g;
        }
        add_at(&mut bgm, (seg.mix_start_s * 48_000.0) as usize, &looped, BGM_GAIN);
    }

    let mut master: Vec<[f32; 2]> = (0..total)
        .map(|i| {
            [
                dialogue[i][0] + bgm[i][0] + se[i][0],
                dialogue[i][1] + bgm[i][1] + se[i][1],
            ]
        })
        .collect();
    soft_limit(&mut master, req.master_ceiling_db);

    Mixed { dialogue, bgm, se, master, warnings }
}
```

- [ ] **Step 9: テストが通ることを確認する**

Run: `cargo test -p s2v-audio limiter && cargo test -p s2v-export`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add crates/s2v-audio crates/s2v-export
git commit -m "feat: マスターのソフトリミッタと、dialogue/bgm/se のトラック別ミックスを追加する"
```

---
### Task 15: 出力(timeline.json v2・SRT・FCPXML)

**Files:**
- Create: `crates/s2v-export/src/timeline_json.rs`、`crates/s2v-export/src/srt.rs`、`crates/s2v-export/src/fcpxml.rs`
- Modify: `crates/s2v-export/src/lib.rs`

**Interfaces:**
- Consumes: Task 13 の `Placement`・`PlacedLine`・`PlacedScene`・`PlacedEvent`・`MissingReason`、Task 10 の `floor_ms`・`ceil_ms`
- Produces:
  - `timeline_json::CastInfo { name: String, speaker: String, appearance: Option<String> }`
  - `timeline_json::TakeInfo { id: String, files: Vec<String>, trims: Option<(i64, i64)>, missing: Option<MissingReason> }`(`trims` は (head_ms, tail_ms))
  - `timeline_json::LineAudioInfo { src_start: i64, src_end: i64, measured_lufs: f64, requested_gain_db: f64, norm_gain_db: f64, norm_clamped: bool }`(位置はサンプル)
  - `timeline_json::TimelineInput<'a> { script: String, duration_samples: i64, target_lufs: f64, casts: Vec<CastInfo>, placement: &'a Placement, takes: Vec<TakeInfo>, line_audio: &'a HashMap<LineId, LineAudioInfo> }`
  - `timeline_json::timeline_value(&TimelineInput) -> serde_json::Value`、`timeline_json::write_timeline_json(path: &Path, input: &TimelineInput) -> anyhow::Result<()>`
  - `srt::render_srt(lines: &[PlacedLine]) -> String`
  - `fcpxml::render_fcpxml(project: &str, total_samples: i64, tracks: &[(&str, PathBuf)], fps2997: bool) -> String`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-export/src/timeline_json.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::placement::{PlacedLine, PlacedScene};
    use s2v_core::VisualKind;

    fn sample_placement() -> Placement {
        Placement {
            lines: vec![
                PlacedLine { id: LineId { scene: 1, line: 1 }, scene: 1, cast: "A".into(), text: "一".into(), start: 1, end: 48_001, missing: None, flat_index: 0 },
                PlacedLine { id: LineId { scene: 1, line: 2 }, scene: 1, cast: "B".into(), text: "二".into(), start: 96_000, end: 120_000, missing: Some(MissingReason::NoFile), flat_index: 1 },
            ],
            scenes: vec![
                PlacedScene { id: 1, name: Some("朝".into()), index: 1, silent: false, start: 1, end: 120_000, visual_text: Some("港".into()), visual_kind: Some(VisualKind::Image) },
                PlacedScene { id: 2, name: None, index: 2, silent: true, start: 120_000, end: 216_000, visual_text: None, visual_kind: None },
            ],
            events: vec![
                PlacedEvent::Se { path: "C:/se/door.wav".into(), start: 4_800, gain_db: -3.0 },
                PlacedEvent::BgmStop { start: 0 },
            ],
            warnings: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn timeline_v2_shape_and_rounding() {
        let placement = sample_placement();
        let mut audio = HashMap::new();
        audio.insert(
            LineId { scene: 1, line: 1 },
            LineAudioInfo { src_start: 8_160, src_end: 61_441, measured_lufs: -31.234, requested_gain_db: 8.234, norm_gain_db: 8.234, norm_clamped: false },
        );
        let input = TimelineInput {
            script: "第1話.txt".into(),
            duration_samples: 216_001,
            target_lufs: -23.0,
            casts: vec![CastInfo { name: "A".into(), speaker: "田中".into(), appearance: None }],
            placement: &placement,
            takes: vec![
                TakeInfo { id: "S01_L001".into(), files: vec!["S01_L001.wav".into()], trims: Some((170, 220)), missing: None },
                TakeInfo { id: "S01_L002".into(), files: vec![], trims: None, missing: Some(MissingReason::NoFile) },
            ],
            line_audio: &audio,
        };
        let v = timeline_value(&input);
        assert_eq!(v["schema_version"], 2);
        assert_eq!(v["sample_rate"], 48_000);
        assert_eq!(v["duration_ms"], 4_501);
        assert_eq!(v["normalize"]["target_lufs"], -23.0);
        assert_eq!(v["cast"][0]["speaker"], "田中");
        assert_eq!(v["scenes"][0]["id"], "S01");
        assert_eq!(v["scenes"][0]["visual"]["kind"], "image");
        assert_eq!(v["scenes"][1]["silent"], true);
        assert!(v["scenes"][1]["visual"].is_null());
        assert_eq!(v["takes"][0]["tracks"][0], serde_json::Value::Null);
        assert_eq!(v["takes"][0]["trims"][0]["head_ms"], 170);
        assert!(v["takes"][1]["trims"].is_null());
        assert_eq!(v["takes"][1]["reason"], "no_file");
        let l0 = &v["lines"][0];
        assert_eq!((l0["start_ms"].as_i64(), l0["end_ms"].as_i64()), (Some(0), Some(1_001)));
        assert_eq!((l0["src_start_ms"].as_i64(), l0["src_end_ms"].as_i64()), (Some(170), Some(1_281)));
        assert_eq!(l0["id"], "S01-L001");
        assert_eq!(l0["scene"], "S01");
        assert_eq!(l0["take"], "S01_L001");
        assert!(l0["track"].is_null());
        assert_eq!(l0["measured_lufs"], -31.23);
        assert_eq!(l0["norm_clamped"], false);
        let l1 = &v["lines"][1];
        assert_eq!(l1["missing"], true);
        assert_eq!(l1["reason"], "no_file");
        assert!(l1["measured_lufs"].is_null() && l1["norm_gain_db"].is_null() && l1["src_start_ms"].is_null());
        assert_eq!(v["events"][0]["type"], "bgm_stop");
        assert_eq!(v["events"][1]["type"], "se");
        assert_eq!(v["events"][1]["file"], "door.wav");
        assert_eq!(v["events"][1]["start_ms"], 100);
    }
}
```

`crates/s2v-export/src/srt.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::ids::LineId;

    #[test]
    fn lines_are_sorted_and_times_use_floor_and_ceil() {
        let line = |n: u32, start: i64, end: i64, text: &str| PlacedLine {
            id: LineId { scene: 1, line: n },
            scene: 1,
            cast: "A".into(),
            text: text.into(),
            start,
            end,
            missing: None,
            flat_index: n as usize,
        };
        let srt = render_srt(&[line(2, 72_000, 120_001, "二"), line(1, 0, 48_000, "一")]);
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:01,000\n一\n\n2\n00:00:01,500 --> 00:00:02,501\n二\n\n"
        );
    }
}
```

`crates/s2v-export/src/fcpxml.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_lanes_with_escaped_uris_and_total_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a & b");
        std::fs::create_dir(&dir).unwrap();
        let tracks: Vec<(&str, PathBuf)> = ["dialogue", "bgm", "se"]
            .iter()
            .map(|n| {
                let p = dir.join(format!("{n}.wav"));
                std::fs::write(&p, b"x").unwrap();
                (*n, p)
            })
            .collect();
        let xml = render_fcpxml("第1話", 48_000, &tracks, false);
        assert!(xml.contains(r#"duration="30000/30000s""#));
        assert!(xml.contains(r#"lane="3""#));
        assert!(xml.contains("a &amp; b"));
        assert!(xml.contains("FFVideoFormat1080p30"));
        assert!(render_fcpxml("x", 1, &tracks, true).contains("1001/30000s"));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-export/src/lib.rs` に `pub mod timeline_json;`、`pub mod srt;`、`pub mod fcpxml;` を追加してから実行する。

Run: `cargo test -p s2v-export`
Expected: コンパイルエラー

- [ ] **Step 3: timeline.json を実装する**

`crates/s2v-export/src/timeline_json.rs` のテストの上に:
```rust
//! `timeline.json` v2(設計書 §6)。後続工程との契約。

use std::collections::HashMap;
use std::path::Path;

use s2v_core::ids::{format_scene_id, LineId};
use s2v_core::placement::{MissingReason, PlacedEvent, Placement};
use s2v_core::time::{ceil_ms, floor_ms};
use s2v_core::VisualKind;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct CastInfo {
    pub name: String,
    pub speaker: String,
    pub appearance: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TakeInfo {
    /// 単独の台詞は台詞IDの `-` を `_` にした名前(`S01_L003`)
    pub id: String,
    pub files: Vec<String>,
    /// (head_ms, tail_ms)。不足なら None
    pub trims: Option<(i64, i64)>,
    pub missing: Option<MissingReason>,
}

/// 正常な台詞の、収録ファイル上の使用範囲(サンプル)と正規化の記録。
#[derive(Debug, Clone, Copy)]
pub struct LineAudioInfo {
    pub src_start: i64,
    pub src_end: i64,
    pub measured_lufs: f64,
    pub requested_gain_db: f64,
    pub norm_gain_db: f64,
    pub norm_clamped: bool,
}

pub struct TimelineInput<'a> {
    pub script: String,
    pub duration_samples: i64,
    pub target_lufs: f64,
    pub casts: Vec<CastInfo>,
    pub placement: &'a Placement,
    pub takes: Vec<TakeInfo>,
    pub line_audio: &'a HashMap<LineId, LineAudioInfo>,
}

#[derive(Serialize)]
struct TimelineDoc {
    schema_version: u32,
    script: String,
    sample_rate: u32,
    duration_ms: i64,
    normalize: NormalizeDoc,
    cast: Vec<CastDoc>,
    scenes: Vec<SceneDoc>,
    takes: Vec<TakeDoc>,
    lines: Vec<LineDoc>,
    events: Vec<EventDoc>,
}

#[derive(Serialize)]
struct NormalizeDoc {
    target_lufs: f64,
}

#[derive(Serialize)]
struct CastDoc {
    name: String,
    speaker: String,
    appearance: Option<String>,
}

#[derive(Serialize)]
struct VisualDoc {
    text: String,
    kind: Option<VisualKind>,
}

#[derive(Serialize)]
struct SceneDoc {
    id: String,
    name: Option<String>,
    index: usize,
    silent: bool,
    start_ms: i64,
    end_ms: i64,
    visual: Option<VisualDoc>,
}

#[derive(Serialize)]
struct TrimDoc {
    track: Option<String>,
    head_ms: i64,
    tail_ms: i64,
}

#[derive(Serialize)]
struct TakeDoc {
    id: String,
    tracks: Vec<Option<String>>,
    files: Vec<String>,
    trims: Option<Vec<TrimDoc>>,
    missing: bool,
    reason: Option<MissingReason>,
}

#[derive(Serialize)]
struct LineDoc {
    id: String,
    scene: String,
    cast: String,
    text: String,
    start_ms: i64,
    end_ms: i64,
    take: String,
    track: Option<String>,
    measured_lufs: Option<f64>,
    requested_gain_db: Option<f64>,
    norm_gain_db: Option<f64>,
    norm_clamped: Option<bool>,
    src_start_ms: Option<i64>,
    src_end_ms: Option<i64>,
    missing: bool,
    reason: Option<MissingReason>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventDoc {
    Se { file: String, start_ms: i64, gain_db: f64 },
    BgmStart { file: String, start_ms: i64 },
    BgmStop { start_ms: i64 },
}

/// 小数第2位に丸める(LUFS・dB の表示用)。
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

pub fn timeline_value(input: &TimelineInput<'_>) -> serde_json::Value {
    let p = input.placement;
    let lines = p
        .lines
        .iter()
        .map(|l| {
            let a = if l.missing.is_none() { input.line_audio.get(&l.id) } else { None };
            LineDoc {
                id: l.id.display(),
                scene: format_scene_id(l.scene),
                cast: l.cast.clone(),
                text: l.text.clone(),
                start_ms: floor_ms(l.start),
                end_ms: ceil_ms(l.end),
                take: l.id.file_stem(),
                track: None,
                measured_lufs: a.map(|a| r2(a.measured_lufs)),
                requested_gain_db: a.map(|a| r2(a.requested_gain_db)),
                norm_gain_db: a.map(|a| r2(a.norm_gain_db)),
                norm_clamped: a.map(|a| a.norm_clamped),
                src_start_ms: a.map(|a| floor_ms(a.src_start)),
                src_end_ms: a.map(|a| ceil_ms(a.src_end)),
                missing: l.missing.is_some(),
                reason: l.missing,
            }
        })
        .collect();

    let mut events: Vec<&PlacedEvent> = p.events.iter().collect();
    events.sort_by_key(|e| e.start());
    let events = events
        .into_iter()
        .map(|e| match e {
            PlacedEvent::Se { path, start, gain_db } => EventDoc::Se { file: file_name(path), start_ms: floor_ms(*start), gain_db: *gain_db },
            PlacedEvent::BgmStart { path, start } => EventDoc::BgmStart { file: file_name(path), start_ms: floor_ms(*start) },
            PlacedEvent::BgmStop { start } => EventDoc::BgmStop { start_ms: floor_ms(*start) },
        })
        .collect();

    let doc = TimelineDoc {
        schema_version: 2,
        script: input.script.clone(),
        sample_rate: 48_000,
        duration_ms: ceil_ms(input.duration_samples),
        normalize: NormalizeDoc { target_lufs: input.target_lufs },
        cast: input
            .casts
            .iter()
            .map(|c| CastDoc { name: c.name.clone(), speaker: c.speaker.clone(), appearance: c.appearance.clone() })
            .collect(),
        scenes: p
            .scenes
            .iter()
            .map(|s| SceneDoc {
                id: format_scene_id(s.id),
                name: s.name.clone(),
                index: s.index,
                silent: s.silent,
                start_ms: floor_ms(s.start),
                end_ms: ceil_ms(s.end),
                visual: s.visual_text.as_ref().map(|t| VisualDoc { text: t.clone(), kind: s.visual_kind }),
            })
            .collect(),
        takes: input
            .takes
            .iter()
            .map(|t| TakeDoc {
                id: t.id.clone(),
                tracks: vec![None],
                files: t.files.clone(),
                trims: t.trims.map(|(h, tl)| vec![TrimDoc { track: None, head_ms: h, tail_ms: tl }]),
                missing: t.missing.is_some(),
                reason: t.missing,
            })
            .collect(),
        lines,
        events,
    };
    serde_json::to_value(doc).expect("timeline.json の直列化に失敗しました")
}

pub fn write_timeline_json(path: &Path, input: &TimelineInput<'_>) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(&timeline_value(input))?)?;
    Ok(())
}
```

- [ ] **Step 4: SRT と FCPXML を実装する**

`crates/s2v-export/src/srt.rs` のテストの上に:
```rust
//! 字幕(SRT)。台詞を開始時刻の順に並べる。不足の台詞も出力する。

use s2v_core::placement::PlacedLine;
use s2v_core::time::{ceil_ms, floor_ms};

fn srt_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms % 3_600_000) / 60_000,
        (ms % 60_000) / 1_000,
        ms % 1_000
    )
}

pub fn render_srt(lines: &[PlacedLine]) -> String {
    let mut sorted: Vec<&PlacedLine> = lines.iter().collect();
    sorted.sort_by_key(|l| (l.start, l.flat_index));
    let mut out = String::new();
    for (i, l) in sorted.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            srt_time(floor_ms(l.start)),
            srt_time(ceil_ms(l.end)),
            l.text
        ));
    }
    out
}
```

`crates/s2v-export/src/fcpxml.rs` のテストの上に:
```rust
//! FCPXML(Filmora 向け)。トラック別 wav(dialogue / bgm / se)を3つのレーンに置く。
//! タイムベースは 30000分の1秒(流用元と同じ)。

use std::path::{Path, PathBuf};

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 絶対パスを `file://` URI にする(Windows の verbatim パスの接頭辞を外す。流用元と同じ)。
fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    };
    let s = s.replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//") {
        format!("file://{unc}")
    } else if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

pub fn render_fcpxml(project: &str, total_samples: i64, tracks: &[(&str, PathBuf)], fps2997: bool) -> String {
    // 48kHz のサンプル → 30000分の1秒(切り上げ)
    let ticks = (total_samples.max(0) * 5 + 7) / 8;
    let (format_name, frame) = if fps2997 {
        ("FFVideoFormat1080p2997", "1001/30000s")
    } else {
        ("FFVideoFormat1080p30", "1000/30000s")
    };
    let mut assets = String::new();
    let mut clips = String::new();
    for (i, (name, path)) in tracks.iter().enumerate() {
        let n = i + 1;
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        assets.push_str(&format!(
            "        <asset id=\"a{n}\" name=\"{}\" src=\"{}\" start=\"0s\" duration=\"{ticks}/30000s\" hasAudio=\"1\" audioSources=\"1\" audioChannels=\"2\" audioRate=\"48000\"/>\n",
            xml_escape(&file),
            xml_escape(&file_uri(&abs)),
        ));
        clips.push_str(&format!(
            "                            <asset-clip ref=\"a{n}\" lane=\"{n}\" offset=\"0s\" name=\"{}\" duration=\"{ticks}/30000s\"/>\n",
            xml_escape(name),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.8">
    <resources>
        <format id="r1" name="{format_name}" frameDuration="{frame}"/>
{assets}    </resources>
    <library>
        <event name="Script2Video">
            <project name="{project}">
                <sequence format="r1" duration="{ticks}/30000s">
                    <spine>
                        <gap name="Gap" offset="0s" duration="{ticks}/30000s">
{clips}                        </gap>
                    </spine>
                </sequence>
            </project>
        </event>
    </library>
</fcpxml>
"#,
        project = xml_escape(project),
    )
}
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p s2v-export`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/s2v-export
git commit -m "feat(export): timeline.json v2・字幕・3レーンの FCPXML を出力する"
```

---

### Task 16: 中間ファイルとキャッシュ

**Files:**
- Create: `crates/s2v-cli/src/work.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod work;`)

**Interfaces:**
- Consumes: なし
- Produces:
  - `work::NormMeta { measured_lufs: f64, requested_gain_db: f64, norm_gain_db: f64, norm_clamped: bool, measure_start: u64, measure_end: u64, src_start: u64, src_end: u64, warnings: Vec<String> }`(位置は収録ファイル上のサンプル)
  - `work::CacheEntry { key: String, bytes: u64, sha256: String, norm: Option<NormMeta> }`
  - `work::WorkDir::open(root: &Path, no_cache: bool) -> anyhow::Result<(WorkDir, Option<String>)>`(2つ目は `cache.json` を破棄したときのログ用メッセージ)
  - `WorkDir::lookup(&mut self, rel: &str, key: &str) -> Option<CacheEntry>`、`WorkDir::store(&mut self, rel: &str, key: &str, samples: &[f32], norm: Option<NormMeta>) -> anyhow::Result<()>`、`WorkDir::read_mono(&self, rel: &str) -> anyhow::Result<Vec<f32>>`、`WorkDir::release(&mut self, rel: &str)`、`WorkDir::finish(self) -> anyhow::Result<()>`
  - `work::load_cache(root: &Path) -> anyhow::Result<BTreeMap<String, CacheEntry>>`
  - `work::sha256_hex(&[u8]) -> String`、`work::resampled_key(src_sha: &str) -> String`、`work::LineKeyParams { src_start: u64, src_end: u64, trim_threshold_db: f64, target_lufs: f64, max_boost_db: f64, max_cut_db: f64, prefade: bool }`、`work::line_key(resampled_key: &str, p: &LineKeyParams) -> String`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-cli/src/work.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> NormMeta {
        NormMeta {
            measured_lufs: -30.0,
            requested_gain_db: 7.0,
            norm_gain_db: 7.0,
            norm_clamped: false,
            measure_start: 10,
            measure_end: 20,
            src_start: 5,
            src_end: 25,
            warnings: vec!["注意".into()],
        }
    }

    #[test]
    fn store_then_lookup_hits_and_keeps_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, note) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(note.is_none());
        w.store("lines/S01-L001.wav", "k1", &[0.1, 0.2], Some(meta())).unwrap();
        let (mut w2, _) = WorkDir::open(tmp.path(), false).unwrap();
        let e = w2.lookup("lines/S01-L001.wav", "k1").unwrap();
        assert_eq!(e.norm, Some(meta()));
        assert_eq!(w2.read_mono("lines/S01-L001.wav").unwrap(), vec![0.1, 0.2]);
    }

    #[test]
    fn key_mismatch_changed_file_and_no_cache_all_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        w.store("resampled/S01_L001.wav", "k1", &[0.5], None).unwrap();
        assert!(w.lookup("resampled/S01_L001.wav", "k2").is_none());
        std::fs::write(tmp.path().join("resampled/S01_L001.wav"), b"broken").unwrap();
        assert!(w.lookup("resampled/S01_L001.wav", "k1").is_none());
        let (mut nc, _) = WorkDir::open(tmp.path(), true).unwrap();
        nc.store("resampled/S01_L002.wav", "k", &[0.5], None).unwrap();
        assert!(nc.lookup("resampled/S01_L002.wav", "k").is_none());
    }

    #[test]
    fn corrupt_cache_json_is_discarded_with_a_note() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cache.json"), "{ 壊れた").unwrap();
        let (mut w, note) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(note.unwrap().contains("cache.json"));
        assert!(w.lookup("lines/x.wav", "k").is_none());
    }

    #[test]
    fn finish_deletes_unused_and_released_files() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
            w.store("lines/S01-L001.wav", "a", &[0.1], None).unwrap();
            w.store("lines/S01-L002.wav", "b", &[0.1], None).unwrap();
            w.store("resampled/S01_L002.wav", "c", &[0.1], None).unwrap();
            w.finish().unwrap();
        }
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(w.lookup("lines/S01-L001.wav", "a").is_some());
        w.store("resampled/S01_L002.wav", "c", &[0.1], None).unwrap();
        w.release("resampled/S01_L002.wav");
        w.finish().unwrap();
        assert!(tmp.path().join("lines/S01-L001.wav").exists());
        assert!(!tmp.path().join("lines/S01-L002.wav").exists());
        assert!(!tmp.path().join("resampled/S01_L002.wav").exists());
        let entries = load_cache(tmp.path()).unwrap();
        assert_eq!(entries.keys().collect::<Vec<_>>(), vec!["lines/S01-L001.wav"]);
    }

    #[test]
    fn keys_change_with_every_parameter() {
        let base = LineKeyParams { src_start: 1, src_end: 2, trim_threshold_db: -50.0, target_lufs: -23.0, max_boost_db: 24.0, max_cut_db: 24.0, prefade: false };
        let k = line_key("r", &base);
        assert_ne!(k, line_key("r2", &base));
        assert_ne!(k, line_key("r", &LineKeyParams { src_end: 3, ..base }));
        assert_ne!(k, line_key("r", &LineKeyParams { target_lufs: -20.0, ..base }));
        assert_ne!(k, line_key("r", &LineKeyParams { prefade: true, ..base }));
        assert_ne!(resampled_key("a"), resampled_key("b"));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-cli/src/lib.rs` に `pub mod work;` を追加してから実行する。

Run: `cargo test -p s2v-cli work`
Expected: コンパイルエラー

- [ ] **Step 3: 実装する**

`crates/s2v-cli/src/work.rs` のテストの上に:
```rust
//! 出力フォルダ `work/` の中間ファイルとキャッシュ(設計書 5.10 D)。
//! - `resampled/<台詞ID語幹>.wav`: 48kHz モノラル・レベルを変えない変換結果
//! - `lines/<台詞ID>.wav`: 正規化・フェード後の台詞
//! - `lines_prefade/<台詞ID>.wav`: フェード前(`dump_prefade = true` のとき)
//! - `cache.json`: キー・出力のバイト数と SHA-256・正規化の記録

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 変換処理のバージョン(処理を変えたら上げる)。
pub const CONVERT_VERSION: u32 = 1;
/// 正規化処理のバージョン(測定方式・400ms・フェードを含む。変えたら上げる)。
pub const NORMALIZE_VERSION: u32 = 1;

const SUBDIRS: [&str; 3] = ["resampled", "lines", "lines_prefade"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormMeta {
    pub measured_lufs: f64,
    pub requested_gain_db: f64,
    pub norm_gain_db: f64,
    pub norm_clamped: bool,
    /// 測定範囲(収録ファイル上のサンプル位置)
    pub measure_start: u64,
    pub measure_end: u64,
    /// 使用範囲(収録ファイル上のサンプル位置)
    pub src_start: u64,
    pub src_end: u64,
    /// キャッシュの命中時にも同じ警告を出すための記録
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub norm: Option<NormMeta>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: BTreeMap<String, CacheEntry>,
}

pub struct WorkDir {
    root: PathBuf,
    cache: CacheFile,
    no_cache: bool,
    used: HashSet<String>,
}

impl WorkDir {
    /// `root`(出力フォルダの `work/`)を開く。書き込めなければ Err(→ 終了コード 2)。
    pub fn open(root: &Path, no_cache: bool) -> anyhow::Result<(WorkDir, Option<String>)> {
        for sub in SUBDIRS {
            std::fs::create_dir_all(root.join(sub))?;
        }
        let (entries, note) = match std::fs::read_to_string(root.join("cache.json")) {
            Ok(s) => match serde_json::from_str::<CacheFile>(&s) {
                Ok(c) if c.version == 1 => (c.entries, None),
                _ => (
                    BTreeMap::new(),
                    Some("cache.json が壊れているため破棄し、中間ファイルを作り直します".to_string()),
                ),
            },
            Err(_) => (BTreeMap::new(), None),
        };
        Ok((
            WorkDir {
                root: root.to_path_buf(),
                cache: CacheFile { version: 1, entries },
                no_cache,
                used: HashSet::new(),
            },
            note,
        ))
    }

    /// キーが一致し、出力ファイルのバイト数と SHA-256 が一致すれば記録を返す(命中したファイルは使用中にする)。
    pub fn lookup(&mut self, rel: &str, key: &str) -> Option<CacheEntry> {
        if self.no_cache {
            return None;
        }
        let e = self.cache.entries.get(rel)?;
        if e.key != key {
            return None;
        }
        let bytes = std::fs::read(self.root.join(rel)).ok()?;
        if bytes.len() as u64 != e.bytes || sha256_hex(&bytes) != e.sha256 {
            return None;
        }
        self.used.insert(rel.to_string());
        Some(e.clone())
    }

    /// モノラル 32bit 浮動小数で書き、記録を更新する(一時ファイルから rename で確定)。
    pub fn store(&mut self, rel: &str, key: &str, samples: &[f32], norm: Option<NormMeta>) -> anyhow::Result<()> {
        let path = self.root.join(rel);
        let tmp = path.with_extension("wav.tmp");
        write_mono_f32(&tmp, samples)?;
        std::fs::rename(&tmp, &path)?;
        let bytes = std::fs::read(&path)?;
        self.cache.entries.insert(
            rel.to_string(),
            CacheEntry { key: key.to_string(), bytes: bytes.len() as u64, sha256: sha256_hex(&bytes), norm },
        );
        self.used.insert(rel.to_string());
        self.save()
    }

    pub fn read_mono(&self, rel: &str) -> anyhow::Result<Vec<f32>> {
        let mut r = hound::WavReader::open(self.root.join(rel))?;
        Ok(r.samples::<f32>().collect::<Result<_, _>>()?)
    }

    /// 不足になった台詞のファイルを、使用中から外す(`finish` で削除される)。
    pub fn release(&mut self, rel: &str) {
        self.used.remove(rel);
    }

    /// 今回使わなかった中間ファイルと記録を削除する。
    pub fn finish(mut self) -> anyhow::Result<()> {
        for sub in SUBDIRS {
            for entry in std::fs::read_dir(self.root.join(sub))? {
                let entry = entry?;
                let rel = format!("{sub}/{}", entry.file_name().to_string_lossy());
                if !self.used.contains(&rel) {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        let used = &self.used;
        self.cache.entries.retain(|k, _| used.contains(k));
        self.save()
    }

    fn save(&self) -> anyhow::Result<()> {
        let tmp = self.root.join("cache.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.cache)?)?;
        std::fs::rename(&tmp, self.root.join("cache.json"))?;
        Ok(())
    }
}

/// `cache.json` の記録を読む(完了条件の確認ツール用)。
pub fn load_cache(root: &Path) -> anyhow::Result<BTreeMap<String, CacheEntry>> {
    let s = std::fs::read_to_string(root.join("cache.json"))?;
    Ok(serde_json::from_str::<CacheFile>(&s)?.entries)
}

fn write_mono_f32(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// `resampled/` のキー: 収録ファイルの内容 + 変換処理のバージョン + 48kHz。
pub fn resampled_key(src_sha: &str) -> String {
    sha256_hex(format!("resampled|v{CONVERT_VERSION}|48000|{src_sha}").as_bytes())
}

#[derive(Debug, Clone, Copy)]
pub struct LineKeyParams {
    pub src_start: u64,
    pub src_end: u64,
    pub trim_threshold_db: f64,
    pub target_lufs: f64,
    pub max_boost_db: f64,
    pub max_cut_db: f64,
    pub prefade: bool,
}

/// `lines/` のキー: `resampled/` のキー + 使用範囲 + 測定範囲を決める設定 + 正規化の設定 + 処理のバージョン。
pub fn line_key(resampled_key: &str, p: &LineKeyParams) -> String {
    sha256_hex(
        format!(
            "lines|v{NORMALIZE_VERSION}|{resampled_key}|{}|{}|{}|{}|{}|{}|{}",
            p.src_start, p.src_end, p.trim_threshold_db, p.target_lufs, p.max_boost_db, p.max_cut_db, p.prefade
        )
        .as_bytes(),
    )
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p s2v-cli work`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/s2v-cli/src/work.rs crates/s2v-cli/src/lib.rs
git commit -m "feat(cli): work/ の中間ファイルとキャッシュ(SHA-256 照合・記録の再現・古いファイルの削除)を追加する"
```

---

### Task 17: `build` コマンドの統合

**Files:**
- Create: `crates/s2v-cli/src/proclog.rs`、`crates/s2v-cli/src/build.rs`、`crates/s2v-cli/tests/build_golden.rs`
- Modify: `crates/s2v-cli/src/lib.rs`、`crates/s2v-cli/src/main.rs`

**Interfaces:**
- Consumes: Task 2〜16 のすべて
- Produces: `s2v_cli::BuildArgs { script: PathBuf, voice_dir: PathBuf, config: Option<PathBuf>, no_cache: bool }`、`s2v_cli::run_build(&BuildArgs) -> Outcome`、`proclog::ProcLog`

- [ ] **Step 1: 失敗するテストを書く(ゴールデンと Review Focus)**

`crates/s2v-cli/tests/build_golden.rs`:
```rust
use std::fs;
use std::path::{Path, PathBuf};

use s2v_cli::{run_build, BuildArgs, Outcome};

const SCRIPT: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中,pan=-30\n\nB:佐藤,pan=30\n\n@script\n#paragraph[S01] 朝\n#visual (image) 朝の居間。\n[S01-L001] A:おはよう。\n[S01-L002] B:おはよう。\n[S01-L003] A(overlap=200):今日は早いね。\n#paragraph[S02] 夜\n#pause 2000\n#paragraph[S03]\n[S03-L004] B:おやすみ。\n";

/// 200ms 無音 + 1000ms の 440Hz(振幅 0.1)+ 300ms 無音、16bit モノラル 48kHz。
fn write_voice(path: &Path) {
    write_samples(path, &voice_samples(0.1, None));
}

fn voice_samples(amp: f64, click_at: Option<usize>) -> Vec<f64> {
    (0..72_000)
        .map(|i| {
            if let Some(c) = click_at {
                if (c..c + 10).contains(&i) {
                    return 0.99;
                }
            }
            if (9_600..57_600).contains(&i) {
                amp * (2.0 * std::f64::consts::PI * 440.0 * (i - 9_600) as f64 / 48_000.0).sin()
            } else {
                0.0
            }
        })
        .collect()
}

fn write_samples(path: &Path, data: &[f64]) {
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &v in data {
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    voices: PathBuf,
}

fn fixture(script: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("収録 音声");
    fs::create_dir_all(&voices).unwrap();
    let script_path = dir.join("第1話.txt");
    fs::write(&script_path, script).unwrap();
    Fixture { _tmp: tmp, dir, script: script_path, voices }
}

fn args(f: &Fixture, config: Option<PathBuf>, no_cache: bool) -> BuildArgs {
    BuildArgs { script: f.script.clone(), voice_dir: f.voices.clone(), config, no_cache }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn wav_frames(path: &Path) -> u32 {
    hound::WavReader::open(path).unwrap().duration()
}

#[test]
fn golden_timeline_and_outputs() {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S03_L004"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings, "L003 が不足");

    let out = f.dir.join("第1話");
    let v = read_json(&out.join("timeline.json"));
    assert_eq!(v["schema_version"], 2);
    assert_eq!(v["script"], "第1話.txt");
    let ms = |i: usize, k: &str| v["lines"][i][k].as_i64().unwrap();
    assert_eq!((ms(0, "start_ms"), ms(0, "end_ms")), (0, 1_110));
    assert_eq!((ms(1, "start_ms"), ms(1, "end_ms")), (1_410, 2_520));
    assert_eq!((ms(2, "start_ms"), ms(2, "end_ms")), (2_320, 3_720));
    assert_eq!((ms(3, "start_ms"), ms(3, "end_ms")), (6_720, 7_830));
    assert_eq!((ms(0, "src_start_ms"), ms(0, "src_end_ms")), (170, 1_280));
    assert_eq!(v["lines"][2]["missing"], true);
    assert_eq!(v["lines"][2]["reason"], "no_file");
    assert!(v["lines"][2]["measured_lufs"].is_null());
    let measured = v["lines"][0]["measured_lufs"].as_f64().unwrap();
    assert!((measured + 23.0).abs() < 0.5, "{measured}");
    assert_eq!(v["lines"][0]["norm_gain_db"], v["lines"][0]["requested_gain_db"]);
    assert_eq!(v["scenes"][1]["silent"], true);
    assert_eq!((v["scenes"][1]["start_ms"].as_i64(), v["scenes"][1]["end_ms"].as_i64()), (Some(3_720), Some(5_720)));
    assert_eq!(v["scenes"][2]["id"], "S03");
    assert_eq!(v["scenes"][0]["visual"]["text"], "朝の居間。");
    assert_eq!(v["takes"][0]["trims"][0]["head_ms"], 170);
    assert_eq!(v["takes"][0]["trims"][0]["tail_ms"], 220);
    assert_eq!(v["takes"][2]["files"].as_array().unwrap().len(), 0);
    assert!(v["takes"][2]["trims"].is_null());

    // 時刻の一致: duration_ms = full_dialogue の長さ(切り上げ)
    let frames = wav_frames(&out.join("full_dialogue.wav")) as i64;
    assert_eq!(v["duration_ms"].as_i64().unwrap(), (frames + 47) / 48);
    assert!(frames >= 7_830 * 48);
    for t in ["dialogue", "bgm", "se"] {
        assert_eq!(wav_frames(&out.join("tracks").join(format!("{t}.wav"))) as i64, frames);
    }
    let srt = fs::read_to_string(out.join("subtitles.srt")).unwrap();
    assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:01,110\nおはよう。\n"), "{srt}");
    assert!(srt.contains("00:00:02,320 --> 00:00:03,720\n今日は早いね。"));
    let ticks = (frames * 5 + 7) / 8;
    assert!(fs::read_to_string(out.join("timeline.fcpxml")).unwrap().contains(&format!("duration=\"{ticks}/30000s\"")));
    assert!(fs::read_to_string(out.join("Process.log")).unwrap().contains("S01-L003"));
    assert_eq!(fs::read_dir(out.join("work").join("lines")).unwrap().count(), 3);

    // キャッシュあり・なしで同じ結果
    let first = fs::read_to_string(out.join("timeline.json")).unwrap();
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    assert_eq!(fs::read_to_string(out.join("timeline.json")).unwrap(), first);
    assert_eq!(run_build(&args(&f, None, true)), Outcome::Warnings);
    assert_eq!(fs::read_to_string(out.join("timeline.json")).unwrap(), first);

    // 元の収録ファイルは変わらない
    let again = f.dir.join("比較.wav");
    write_voice(&again);
    assert_eq!(fs::read(f.voices.join("S01_L001.wav")).unwrap(), fs::read(&again).unwrap());
}

#[test]
fn empty_voice_dir_produces_placeholders() {
    let f = fixture(SCRIPT);
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Warnings);
    let out = f.dir.join("第1話");
    let v = read_json(&out.join("timeline.json"));
    assert!(v["lines"].as_array().unwrap().iter().all(|l| l["missing"] == true));
    assert!(out.join("full_dialogue.wav").exists());
    assert!(fs::read_to_string(out.join("subtitles.srt")).unwrap().contains("おやすみ。"));
}

#[test]
fn loud_click_is_limited_only_at_master() {
    let f = fixture("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:あ\n");
    // 小さな声(振幅 0.01)の中に 0dBFS 近いクリック: 正規化で大きく増幅される
    write_samples(&f.voices.join("S01_L001.wav"), &voice_samples(0.01, Some(30_000)));
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Ok);
    let out = f.dir.join("第1話");
    let mut d = hound::WavReader::open(out.join("tracks").join("dialogue.wav")).unwrap();
    let peak_track = d.samples::<f32>().map(|s| s.unwrap().abs()).fold(0.0f32, f32::max);
    assert!(peak_track > 1.0, "{peak_track}");
    let mut m = hound::WavReader::open(out.join("full_dialogue.wav")).unwrap();
    let peak_master = m.samples::<i32>().map(|s| s.unwrap().abs()).max().unwrap() as f64 / 8_388_607.0;
    assert!(peak_master <= 10f64.powf(-1.0 / 20.0) + 1e-6, "{peak_master}");
}

#[test]
fn out_of_range_config_fails_without_output() {
    let f = fixture(SCRIPT);
    let cfg = f.dir.join("bad.toml");
    let builtin = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml")).unwrap();
    fs::write(&cfg, builtin.replace("target_lufs = -23.0", "target_lufs = -5.0")).unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Failed);
    assert!(!f.dir.join("第1話").exists());
}

#[test]
fn syntax_error_fails_without_output() {
    let f = fixture("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] C:だれ？\n");
    assert_eq!(run_build(&args(&f, None, false)), Outcome::Failed);
    assert!(!f.dir.join("第1話").exists());
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`crates/s2v-cli/src/lib.rs` に次を追加してから実行する:
```rust
pub mod build;
pub mod proclog;

pub use build::{run_build, BuildArgs};
```

Run: `cargo test -p s2v-cli --test build_golden`
Expected: コンパイルエラー

- [ ] **Step 3: Process.log を実装する**

`crates/s2v-cli/src/proclog.rs`:
```rust
//! `Process.log`(処理の記録・警告・エラー)。警告とエラーは標準エラーにも出す。

use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct ProcLog {
    file: File,
    warnings: usize,
}

impl ProcLog {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        Ok(Self { file: File::create(path)?, warnings: 0 })
    }

    pub fn info(&mut self, msg: impl AsRef<str>) {
        let _ = writeln!(self.file, "[INFO] {}", msg.as_ref());
        println!("{}", msg.as_ref());
    }

    pub fn warn(&mut self, msg: impl AsRef<str>) {
        self.warnings += 1;
        let _ = writeln!(self.file, "[WARN] {}", msg.as_ref());
        eprintln!("[WARN] {}", msg.as_ref());
    }

    pub fn error(&mut self, msg: impl AsRef<str>) {
        let _ = writeln!(self.file, "[ERROR] {}", msg.as_ref());
        eprintln!("[ERROR] {}", msg.as_ref());
    }

    pub fn warning_count(&self) -> usize {
        self.warnings
    }
}
```

- [ ] **Step 4: `build` を実装する**

`crates/s2v-cli/src/build.rs`:
```rust
//! `script2video build`(設計書 5.2)。
//! 取り込み → 無音削除 → ラウドネス正規化 → 配置 → 音響処理 → ミックス → 出力。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Context;
use s2v_audio::detect::single_used_range;
use s2v_audio::input::{read_mono_48k, InputError};
use s2v_audio::loudness::{apply_edge_fades, apply_gain_db, decide_gain, measure_mono, Loudness};
use s2v_audio::AudioProcessor;
use s2v_core::ids::LineId;
use s2v_core::placement::{flatten, place, LineSource, MissingReason};
use s2v_core::time::{ceil_ms, floor_ms};
use s2v_core::voices::match_voice_files;
use s2v_core::{validate, Config, ScriptItem, ScriptParser};
use s2v_export::fcpxml::render_fcpxml;
use s2v_export::srt::render_srt;
use s2v_export::timeline_json::{write_timeline_json, CastInfo, LineAudioInfo, TakeInfo, TimelineInput};
use s2v_export::wavio::{write_f32_stereo, write_i24_stereo};
use s2v_export::{mix, MixRequest};

use crate::proclog::ProcLog;
use crate::work::{line_key, resampled_key, sha256_hex, LineKeyParams, NormMeta, WorkDir};
use crate::Outcome;

pub struct BuildArgs {
    pub script: PathBuf,
    pub voice_dir: PathBuf,
    pub config: Option<PathBuf>,
    pub no_cache: bool,
}

/// 実行して終了コードを返す。読み書きの失敗などの予期しないエラーは終了コード 2。
pub fn run_build(args: &BuildArgs) -> Outcome {
    match build(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

/// 正規化済みの台詞。
struct PreparedLine {
    /// 正規化・フェード後のモノラル(使用範囲)
    samples: Vec<f32>,
    meta: NormMeta,
    /// 収録ファイル(48kHz 変換後)の長さ
    file_len: usize,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn build(args: &BuildArgs) -> anyhow::Result<Outcome> {
    // 1. 設定
    let config = match &args.config {
        Some(p) => Config::from_file(p).with_context(|| format!("設定ファイルを読めません: {}", p.display()))?,
        None => Config::builtin(),
    };
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        for e in config_errors {
            eprintln!("[ERROR] {e}");
        }
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }

    // 2. 台本
    let text = std::fs::read_to_string(&args.script)
        .with_context(|| format!("台本を読めません(UTF-8 で保存してください): {}", args.script.display()))?;
    let mut parser = ScriptParser::new();
    let scenes = parser.parse_str(&text)?;
    let diag = validate(&scenes);
    let errors: Vec<_> = parser.errors().iter().chain(diag.errors.iter()).collect();
    if !errors.is_empty() {
        for e in errors {
            eprintln!("[ERROR] {} 行目: {}", e.line_no, e.message);
        }
        eprintln!("[ERROR] 出力していません");
        return Ok(Outcome::Failed);
    }

    // 3. 出力フォルダ・Process.log・work/
    let stem = args
        .script
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "script".to_string());
    let out_dir = args.script.parent().unwrap_or(Path::new(".")).join(&stem);
    std::fs::create_dir_all(&out_dir).with_context(|| format!("出力フォルダを作れません: {}", out_dir.display()))?;
    let mut log = ProcLog::create(&out_dir.join("Process.log"))?;
    for w in parser.warnings().iter().chain(diag.warnings.iter()) {
        log.warn(format!("{} 行目: {}", w.line_no, w.message));
    }
    let (mut work, note) = WorkDir::open(&out_dir.join("work"), args.no_cache)
        .with_context(|| format!("work/ に書き込めません: {}", out_dir.display()))?;
    if let Some(n) = note {
        log.info(n);
    }

    // 4. 照合
    let flat = flatten(&scenes);
    let ids: Vec<LineId> = flat
        .iter()
        .filter_map(|f| match f.item {
            ScriptItem::Speech { id, .. } => *id,
            _ => None,
        })
        .collect();
    let voices = match_voice_files(&args.voice_dir, &ids, config.voice.allow_flac)
        .with_context(|| format!("収録フォルダを読めません: {}", args.voice_dir.display()))?;
    for w in &voices.warnings {
        log.warn(w);
    }

    // 5. 台詞ごとの取り込み・無音削除・正規化
    let mut sources: HashMap<LineId, LineSource> = HashMap::new();
    let mut prepared: HashMap<LineId, PreparedLine> = HashMap::new();
    let mut takes: Vec<TakeInfo> = Vec::new();
    for id in &ids {
        let files = voices.files.get(id).cloned().unwrap_or_default();
        let names: Vec<String> = files.iter().map(|p| file_name(p)).collect();
        let result = match files.as_slice() {
            [] => Err(MissingReason::NoFile),
            [one] => prepare_line(*id, one, &config, &mut work, &mut log)?,
            _ => Err(MissingReason::Duplicate),
        };
        match result {
            Ok(line) => {
                sources.insert(*id, LineSource::Present { len: line.samples.len() as i64 });
                let head = floor_ms(line.meta.src_start as i64);
                let tail = ceil_ms(line.file_len as i64) - ceil_ms(line.meta.src_end as i64);
                takes.push(TakeInfo { id: id.file_stem(), files: names, trims: Some((head, tail)), missing: None });
                prepared.insert(*id, line);
            }
            Err(reason) => {
                let detail = match reason {
                    MissingReason::NoFile => "収録ファイルがありません".to_string(),
                    MissingReason::Duplicate => format!("同じ台詞のファイルが複数あります: {}", names.join(", ")),
                    _ => "収録ファイルを使えません".to_string(),
                };
                log.warn(format!("{}: 不足({})。仮の無音を置きます: {detail}", id.display(), reason.as_str()));
                sources.insert(*id, LineSource::Missing(reason));
                takes.push(TakeInfo { id: id.file_stem(), files: names, trims: None, missing: Some(reason) });
            }
        }
    }

    // 6. 配置
    let placement = place(&flat, &sources, config.voice.placeholder_ms_per_char);
    for w in &placement.warnings {
        log.warn(w);
    }
    if !placement.errors.is_empty() {
        for e in &placement.errors {
            log.error(e);
        }
        log.error("出力していません");
        return Ok(Outcome::Failed);
    }

    // 7. 音響処理
    let processor = AudioProcessor::new(config.audio.clone());
    log.info(format!("基準ゲイン(較正値): {:.4}", processor.calibration_gain()));
    let mut dialogue = Vec::new();
    for line in &placement.lines {
        let Some(p) = prepared.get(&line.id) else { continue };
        let fi = &flat[line.flat_index];
        let ScriptItem::Speech { cast_name, offset_params, scene_config, .. } = fi.item else {
            continue;
        };
        let cast = scenes[fi.scene_index]
            .casts
            .get(cast_name)
            .context("検証済みの役名が見つかりません")?
            .with_offsets(offset_params);
        dialogue.push((line.start, processor.render(&p.samples, &cast, scene_config)?));
    }

    // 8. ミックス
    let mixed = mix(MixRequest {
        dialogue,
        events: &placement.events,
        timeline_end: placement.end_sample(),
        bgm: &config.bgm,
        master_ceiling_db: config.mix.master_ceiling_db,
    });
    for w in &mixed.warnings {
        log.warn(w);
    }
    let tracks_dir = out_dir.join("tracks");
    write_f32_stereo(&tracks_dir.join("dialogue.wav"), &mixed.dialogue)?;
    write_f32_stereo(&tracks_dir.join("bgm.wav"), &mixed.bgm)?;
    write_f32_stereo(&tracks_dir.join("se.wav"), &mixed.se)?;
    write_i24_stereo(&out_dir.join("full_dialogue.wav"), &mixed.master)?;
    let total = mixed.master.len() as i64;

    // 9. timeline.json・字幕・FCPXML
    let mut casts: BTreeMap<String, CastInfo> = BTreeMap::new();
    for s in &scenes {
        for (name, c) in &s.casts {
            casts.insert(
                name.clone(),
                CastInfo { name: name.clone(), speaker: c.speaker_name.clone(), appearance: c.appearance.clone() },
            );
        }
    }
    let line_audio: HashMap<LineId, LineAudioInfo> = prepared
        .iter()
        .map(|(id, p)| {
            (
                *id,
                LineAudioInfo {
                    src_start: p.meta.src_start as i64,
                    src_end: p.meta.src_end as i64,
                    measured_lufs: p.meta.measured_lufs,
                    requested_gain_db: p.meta.requested_gain_db,
                    norm_gain_db: p.meta.norm_gain_db,
                    norm_clamped: p.meta.norm_clamped,
                },
            )
        })
        .collect();
    write_timeline_json(
        &out_dir.join("timeline.json"),
        &TimelineInput {
            script: file_name(&args.script),
            duration_samples: total,
            target_lufs: config.normalize.target_lufs,
            casts: casts.into_values().collect(),
            placement: &placement,
            takes,
            line_audio: &line_audio,
        },
    )?;
    std::fs::write(out_dir.join("subtitles.srt"), render_srt(&placement.lines))?;
    let track_paths = [
        ("dialogue", tracks_dir.join("dialogue.wav")),
        ("bgm", tracks_dir.join("bgm.wav")),
        ("se", tracks_dir.join("se.wav")),
    ];
    std::fs::write(
        out_dir.join("timeline.fcpxml"),
        render_fcpxml(&stem, total, &track_paths, config.export.fcpxml_2997fps),
    )?;
    work.finish()?;

    let missing = placement.lines.iter().filter(|l| l.missing.is_some()).count();
    log.info(format!(
        "台詞 {} 行(不足 {} 行)を出力しました: {}",
        placement.lines.len(),
        missing,
        out_dir.display()
    ));
    Ok(if log.warning_count() > 0 || missing > 0 { Outcome::Warnings } else { Outcome::Ok })
}

/// 1台詞を取り込み・無音削除・正規化する。不足なら Ok(Err(理由))、work/ に書けなければ Err。
fn prepare_line(
    id: LineId,
    path: &Path,
    config: &Config,
    work: &mut WorkDir,
    log: &mut ProcLog,
) -> anyhow::Result<Result<PreparedLine, MissingReason>> {
    let name = file_name(path);
    let src_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log.warn(format!("{}: 読み込めません: {name}: {e}", id.display()));
            return Ok(Err(MissingReason::Unreadable));
        }
    };

    // 48kHz モノラルへの変換(レベルは変えない)
    let rkey = resampled_key(&sha256_hex(&src_bytes));
    let rrel = format!("resampled/{}.wav", id.file_stem());
    let resampled = if work.lookup(&rrel, &rkey).is_some() {
        work.read_mono(&rrel)?
    } else {
        match read_mono_48k(path) {
            Ok(s) => {
                work.store(&rrel, &rkey, &s, None)?;
                s
            }
            Err(InputError::BadFormat(m)) => {
                log.warn(format!("{}: 形式が不正です: {name}: {m}", id.display()));
                return Ok(Err(MissingReason::BadFormat));
            }
            Err(InputError::Unreadable(m)) => {
                log.warn(format!("{}: 読み込めません: {name}: {m}", id.display()));
                return Ok(Err(MissingReason::Unreadable));
            }
        }
    };

    // 使用範囲(単独ファイルは無音削除だけで決める)
    let v = &config.voice;
    let Some(range) = single_used_range(&resampled, v.trim_threshold_db, v.trim_head_margin_ms, v.trim_tail_margin_ms) else {
        log.warn(format!("{}: 音がありません(全区間が {} dBFS 以下): {name}", id.display(), v.trim_threshold_db));
        work.release(&rrel);
        return Ok(Err(MissingReason::Silent));
    };

    let n = &config.normalize;
    let lkey = line_key(
        &rkey,
        &LineKeyParams {
            src_start: range.used.start as u64,
            src_end: range.used.end as u64,
            trim_threshold_db: v.trim_threshold_db,
            target_lufs: n.target_lufs,
            max_boost_db: n.max_boost_db,
            max_cut_db: n.max_cut_db,
            prefade: n.dump_prefade,
        },
    );
    let lrel = format!("lines/{}.wav", id.display());
    let prel = format!("lines_prefade/{}.wav", id.display());
    let hit = work.lookup(&lrel, &lkey).and_then(|e| e.norm);
    let prefade_ok = !n.dump_prefade || work.lookup(&prel, &lkey).is_some();
    if let (Some(meta), true) = (hit, prefade_ok) {
        for w in &meta.warnings {
            log.warn(format!("{}: {w}", id.display()));
        }
        let samples = work.read_mono(&lrel)?;
        return Ok(Ok(PreparedLine { samples, meta, file_len: resampled.len() }));
    }

    // ラウドネスの測定(測定範囲 = マージンを除いた発声部分、フェード前)
    let measured = match measure_mono(&resampled[range.measure.start..range.measure.end]) {
        Loudness::Lufs(l) => l,
        Loudness::Unmeasurable => {
            log.warn(format!("{}: ラウドネスを測定できません: {name}", id.display()));
            work.release(&rrel);
            return Ok(Err(MissingReason::Unmeasurable));
        }
    };
    let g = decide_gain(measured, n);
    let mut warnings = Vec::new();
    if g.clamped {
        warnings.push(format!(
            "正規化のゲインが制限に掛かりました(必要 {:+.1} dB、適用 {:+.1} dB)",
            g.requested_db, g.applied_db
        ));
    }
    let mut samples = resampled[range.used.start..range.used.end].to_vec();
    apply_gain_db(&mut samples, g.applied_db);
    if n.dump_prefade {
        work.store(&prel, &lkey, &samples, None)?;
    }
    apply_edge_fades(&mut samples);
    let meta = NormMeta {
        measured_lufs: measured,
        requested_gain_db: g.requested_db,
        norm_gain_db: g.applied_db,
        norm_clamped: g.clamped,
        measure_start: range.measure.start as u64,
        measure_end: range.measure.end as u64,
        src_start: range.used.start as u64,
        src_end: range.used.end as u64,
        warnings: warnings.clone(),
    };
    work.store(&lrel, &lkey, &samples, Some(meta.clone()))?;
    for w in &warnings {
        log.warn(format!("{}: {w}", id.display()));
    }
    Ok(Ok(PreparedLine { samples, meta, file_len: resampled.len() }))
}
```

`crates/s2v-cli/src/main.rs` の `Command::Build { .. }` の分岐を次にする:
```rust
        Command::Build { script, voice_dir, config, no_cache } => s2v_cli::run_build(&s2v_cli::BuildArgs {
            script,
            voice_dir,
            config,
            no_cache,
        }),
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p s2v-cli --test build_golden`
Expected: 5件 PASS

Run: `cargo test --workspace`
Expected: すべて PASS

- [ ] **Step 6: 手で動かして確認する**

Run(Bash):
```bash
TMP=$(mktemp -d)
cat > "$TMP/ep.txt" <<'EOF'
@scene 居間
@cast
A:田中

@script
#paragraph 朝
A:おはよう
EOF
cargo run -q -p s2v-cli -- number "$TMP/ep.txt"; echo "exit=$?"
mkdir "$TMP/voice" && cargo run -q -p s2v-cli -- build "$TMP/ep.txt" --voice-dir "$TMP/voice"; echo "exit=$?"
ls "$TMP/ep"
```
Expected: `number` は `exit=0`、`build` は不足の警告を出して `exit=1`。`ep/` に `full_dialogue.wav`、`tracks/`、`timeline.json`、`subtitles.srt`、`timeline.fcpxml`、`Process.log`、`work/` がある。

- [ ] **Step 7: Commit**

```bash
git add crates/s2v-cli
git commit -m "feat(cli): build コマンド(取り込みから出力まで、Process.log、終了コード)を統合する"
```

---

### Task 18: ①a 完了条件の確認ツールと README

**Files:**
- Create: `crates/s2v-cli/src/verify.rs`、`crates/s2v-cli/examples/verify_loudness.rs`、`README.md`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod verify;`)、`crates/s2v-cli/tests/build_golden.rs`

**Interfaces:**
- Consumes: Task 16 の `load_cache`・`CacheEntry`、Task 11 の `measure_mono`
- Produces: `verify::LineCheck { id: String, kind: CheckKind, passed: bool, detail: String }`、`verify::CheckKind::{ Measured, ShortLine, Clamped }`、`verify::verify_loudness(out_dir: &Path) -> anyhow::Result<Vec<LineCheck>>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/s2v-cli/tests/build_golden.rs` の末尾に追加する:
```rust
#[test]
fn verify_tool_passes_on_a_prefade_build() {
    let f = fixture(SCRIPT);
    for id in ["S01_L001", "S01_L002", "S03_L004"] {
        write_voice(&f.voices.join(format!("{id}.wav")));
    }
    let cfg = f.dir.join("prefade.toml");
    let builtin = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml")).unwrap();
    fs::write(&cfg, builtin.replace("dump_prefade = false", "dump_prefade = true")).unwrap();
    assert_eq!(run_build(&args(&f, Some(cfg), false)), Outcome::Warnings);

    let out = f.dir.join("第1話");
    assert_eq!(fs::read_dir(out.join("work").join("lines_prefade")).unwrap().count(), 3);
    let checks = s2v_cli::verify::verify_loudness(&out).unwrap();
    assert_eq!(checks.len(), 3);
    assert!(checks.iter().all(|c| c.passed), "{checks:?}");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

`lib.rs` に `pub mod verify;` を追加してから実行する。

Run: `cargo test -p s2v-cli --test build_golden verify_tool`
Expected: コンパイルエラー

- [ ] **Step 3: 確認ツールを実装する**

`crates/s2v-cli/src/verify.rs`:
```rust
//! ①a の完了条件(設計書 §12 項目8)の確認。`dump_prefade = true` で build した出力を調べる。
//! `work/lines_prefade/` の正規化後・フェード前の音声を、`cache.json` に記録した測定範囲で再測定する
//! (測定範囲は再検出しない)。

use std::path::Path;

use s2v_audio::loudness::{measure_mono, Loudness};

use crate::work::load_cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// 測定範囲が 400ms 以上: 目標の ±1 LU に入ること
    Measured,
    /// 測定範囲が 400ms 未満: 適用ゲインが式どおりであること
    ShortLine,
    /// ゲインが制限に掛かった: 判定の対象外(合格扱い)
    Clamped,
}

#[derive(Debug, Clone)]
pub struct LineCheck {
    pub id: String,
    pub kind: CheckKind,
    pub passed: bool,
    pub detail: String,
}

pub fn verify_loudness(out_dir: &Path) -> anyhow::Result<Vec<LineCheck>> {
    let timeline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("timeline.json"))?)?;
    let target = timeline["normalize"]["target_lufs"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("timeline.json に normalize.target_lufs がありません"))?;
    let work = out_dir.join("work");
    let entries = load_cache(&work)?;

    let mut checks = Vec::new();
    for (rel, entry) in &entries {
        let Some(id) = rel.strip_prefix("lines/").and_then(|n| n.strip_suffix(".wav")) else {
            continue;
        };
        let Some(meta) = &entry.norm else { continue };
        if meta.norm_clamped {
            checks.push(LineCheck { id: id.to_string(), kind: CheckKind::Clamped, passed: true, detail: "ゲインが制限に掛かったため対象外".into() });
            continue;
        }
        let measure_len = (meta.measure_end - meta.measure_start) as usize;
        if measure_len < 19_200 {
            let expected = target - meta.measured_lufs;
            let passed = (meta.norm_gain_db - expected).abs() < 1e-6;
            checks.push(LineCheck {
                id: id.to_string(),
                kind: CheckKind::ShortLine,
                passed,
                detail: format!("適用 {:+.3} dB / 式 {:+.3} dB", meta.norm_gain_db, expected),
            });
            continue;
        }
        let prefade = work.join("lines_prefade").join(format!("{id}.wav"));
        if !prefade.exists() {
            anyhow::bail!("{} がありません。[normalize] dump_prefade = true で build し直してください", prefade.display());
        }
        let mut r = hound::WavReader::open(&prefade)?;
        let samples: Vec<f32> = r.samples::<f32>().collect::<Result<_, _>>()?;
        let from = (meta.measure_start - meta.src_start) as usize;
        let to = (meta.measure_end - meta.src_start) as usize;
        let (passed, detail) = match measure_mono(&samples[from..to]) {
            Loudness::Lufs(l) => ((l - target).abs() <= 1.0, format!("{l:.2} LUFS(目標 {target:.1})")),
            Loudness::Unmeasurable => (false, "測定できません".to_string()),
        };
        checks.push(LineCheck { id: id.to_string(), kind: CheckKind::Measured, passed, detail });
    }
    Ok(checks)
}
```

`crates/s2v-cli/examples/verify_loudness.rs`:
```rust
//! 使い方: cargo run -p s2v-cli --example verify_loudness -- <台本名の出力フォルダ>
//! 事前に config.toml で `[normalize] dump_prefade = true` にして build しておく。

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("使い方: verify_loudness <出力フォルダ>");
        return ExitCode::from(2);
    };
    match s2v_cli::verify::verify_loudness(&dir) {
        Ok(checks) => {
            let mut failed = 0;
            for c in &checks {
                let mark = if c.passed { "OK " } else { "NG " };
                println!("{mark} {:<12} {:?} {}", c.id, c.kind, c.detail);
                if !c.passed {
                    failed += 1;
                }
            }
            println!("{} 件中 {} 件が不合格", checks.len(), failed);
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            ExitCode::from(2)
        }
    }
}
```

- [ ] **Step 4: README を書く**

リポジトリ直下に `README.md`:
````markdown
# Script2Video

台本と、肉声で収録した音声から、動画用の音声・字幕・タイムラインを作る。現在の実装は ①a(1台詞1ファイルの収録)まで。

- 台本の書式: [台本仕様.md](台本仕様.md)
- 設計: [docs/superpowers/specs/2026-10-12-script2video-design.md](docs/superpowers/specs/2026-10-12-script2video-design.md)

## 使い方

```bash
# 1. 台本に台詞IDとシーン番号を書き込む(台本名.bak・台本名.ids・台本名.html も出力)
cargo run --release -p s2v-cli -- number 第1話.txt

# 2. 台詞ごとに収録する(ファイル名は台詞IDの - を _ にしたもの: S01_L003.wav。モノラル)

# 3. 音声・字幕・タイムラインを作る(出力は 台本と同じフォルダの 第1話/)
cargo run --release -p s2v-cli -- build 第1話.txt --voice-dir 収録/
```

`build` のオプション: `--config 設定.toml`(省略時は同梱の `config.toml`)、`--no-cache`(中間ファイルを使わない)。

終了コード: 0=全件OK、1=警告・不足あり(出力は完成)、2=台本の構文エラー・設定値の範囲外など(出力なし)。

## 出力(台本名/)

| ファイル | 内容 |
|---|---|
| `full_dialogue.wav` | 台詞・BGM・SE の合成(24bit、マスターのソフトリミッタ後) |
| `tracks/dialogue.wav` `tracks/bgm.wav` `tracks/se.wav` | 動画編集ソフト用の別トラック(32bit 浮動小数、マスター前) |
| `timeline.json` | 後続工程が読む正本(schema_version 2) |
| `subtitles.srt` | 字幕 |
| `timeline.fcpxml` | Filmora 向け(3つのトラックを3レーンに配置) |
| `Process.log` | 処理の記録・警告・エラー |
| `work/` | 中間ファイル(削除してよい) |

## ①a の完了条件の確認

実収録の素材(複数の収録セッション・音量の異なる台詞を含む)で、次を行う。

1. 設定ファイルの `[normalize] dump_prefade = true` にして `build` する。
2. `cargo run -p s2v-cli --example verify_loudness -- 第1話` を実行し、すべて `OK` になることを確かめる。
````

- [ ] **Step 5: 全体の検証**

Run: `cargo fmt --check`
Expected: 差分なし(あれば `cargo fmt` を実行してから再確認する)

Run: `cargo test --workspace`
Expected: すべて PASS

Run: `cargo build --workspace`
Expected: 成功

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 警告なし(警告が出た場合は修正する。修正が設計判断を伴う場合は Human に報告する)

- [ ] **Step 6: Commit**

```bash
git add README.md crates/s2v-cli
git commit -m "feat(cli): ①a 完了条件の確認ツール(verify_loudness)と README を追加する"
```

---

## ①a の完了後(Human の判断)

- 実収録の素材での受け入れ試験(設計書 §12 項目8)は、Human が素材を用意して行う。Task 18 の `verify_loudness` で確認する。
- 既定値(-23 LUFS、-1.0 dBFS、-50 dBFS、30/80ms)の調整は、受け入れ試験の結果を見て Human が判断する。
- ①b(混合トラックのテイク)は、①a の完了後に別の実装計画を書く。
