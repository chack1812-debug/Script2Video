# 合成音声の取り込み Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `@cast` にエンジン名を書いた役の台詞を `build` が音声合成し、肉声と同じ処理経路に流す。

**Architecture:** パーサーが台詞ごとに「合成指示(`SynthSpec`)」を持たせる(`@cast` の再定義が以後の台詞にだけ効くようにするため)。`build` は収録ファイル照合の前に「合成工程」を挟み、`voices/S01_L003.wav` を作る。合成記録 `voices/synth.json` で再合成の要否を決め、置き換えるファイルは `voices/backup/` へ退避する。エンジン層は Script2Voice-Rust版 `s2v-engines` をコピーして `SynthRequest` 受け取りに改造する。

**Tech Stack:** Rust(workspace)、tokio(current_thread の `block_on` で非同期エンジンを呼ぶ)、reqwest、wiremock(テスト)、sha2(既存)。

**Spec:** `docs/superpowers/specs/2026-10-28-script2video-synth-voice-design.md`

## Global Constraints

- 合成は `build` で行う。`number` は変更しない。
- オプション名は `voice=rec`。退避先は `voices/backup/`、ファイル名は `S01_L003.<YYYYMMDDTHHMMSS>.wav`。合成記録は `voices/synth.json`。
- 合成した音声は `voices/S01_L003.wav`(モノラル・48kHz)。以降は肉声と同じ経路(無音削除・正規化・配置・重なり)。
- エンジン名は `voicevox` / `aivis` / `xtts` のみ。エンジン名の無い役の挙動は変えない。
- 肉声役の `'単語|読み'` は廃止のまま。合成役のみ有効(区切りは半角 `|`)。
- `s2v-engines` は Script2Voice-Rust版からコピー(パス依存にしない)。流用元コミットを `PORTING.md` に追記する。
- コミット・push は利用者の許可がある場合のみ。計画の各「Commit」ステップは、許可を得てから実行する。
- 作業の追跡は Beads(`bd`、prefix `s2vid`)。TodoWrite は使わない。

## Review Focus

1. `@cast` を台本の途中で再定義 → 再定義より前の台詞の声は変わらず、後の台詞だけ新定義(Task 2)
2. 合成役の台詞で `voices/S01_L003.wav` が人の収録(合成記録に無い/ハッシュ不一致) → 退避してから合成(Task 5)
3. 合成記録と一致 → エンジンを起動せず再合成もしない(Task 5, 6)
4. エンジンが起動できない/合成失敗 → その台詞は「不足」で仮の無音、`build` 全体は継続しエラー報告(Task 6)
5. `voice=rec` の台詞 → ファイルがあれば一切触らない、無ければ従来どおり不足(Task 5)
6. 台詞テキストに `'東京|とうきょう'` → エンジンへは「とうきょう」、字幕・吹き出しは「東京」(Task 2)

---

## File Structure

| ファイル | 役割 |
|---|---|
| `crates/s2v-engines/`(新規、コピー+改造) | エンジン呼び出し。`SynthRequest` を受け取る |
| `crates/s2v-core/src/cast.rs`(変更) | `Cast` に `synth: Option<SynthSpec>` |
| `crates/s2v-core/src/types.rs`(変更) | `ScriptItem::Speech` に `synth: Option<SynthSpec>` と `voice_rec: bool` |
| `crates/s2v-core/src/parser.rs`(変更) | `@cast` のエンジン書式、`voice=rec`、読み指定、台詞への `SynthSpec` 付与 |
| `crates/s2v-core/src/config.rs`(変更) | `[voicevox]` `[aivis]` `[xtts]` `[http]` 設定 |
| `crates/s2v-core/src/validate.rs`(変更) | エンジン名・`voice=rec` の検査 |
| `crates/s2v-cli/src/synth.rs`(新規) | 合成工程(判定・記録・退避・合成の実行) |
| `crates/s2v-cli/src/build.rs`(変更) | 照合の前に合成工程を呼ぶ |
| `台本仕様.md`、`README.md`、`PORTING.md`、`config.toml`(変更) | 仕様・設定の反映 |

---

### Task 0: Beads の起票

**Files:** なし(`bd` のみ)

- [ ] **Step 1: エピックと各タスクを起票する**

```bash
cd "D:/UDS/宇摩コードワークス/Script2Video"
bd create --title="合成音声の取り込み(@cast のエンジン指定)" --type=feature --priority=2 --description="設計書 docs/superpowers/specs/2026-10-28-script2video-synth-voice-design.md。エンジン付きの役の台詞を build が合成する"
```
続けてタスク 1〜7 を `--type=task` で起票し(タイトルは各 Task 見出し)、`bd dep add` で依存を張る: 2→1、3→2、5→3、6→4,5、7→2〜6。

- [ ] **Step 2: 確認** — `bd list --status=open` に8件が並ぶこと。

---

### Task 1: s2v-engines のコピーと SynthRequest 化

**Files:**
- Create: `crates/s2v-engines/`(流用元 `D:\UDS\宇摩コードワークス\Script2Voice-Rust版\crates\s2v-engines` を丸ごとコピー)
- Modify: `Cargo.toml`(workspace members に追加)、`crates/s2v-engines/Cargo.toml`(`s2v-core` 依存を外す)、`PORTING.md`

**Interfaces:**
- Produces(`s2v_engines`):
  ```rust
  pub struct SynthRequest {
      pub speaker_name: String,
      pub style: String,                       // 既定 "ノーマル" は呼び出し側で補う
      pub language: String,                    // 既定 "ja"
      pub params: HashMap<String, serde_json::Value>, // エンジン固有パラメータ
  }
  #[async_trait] pub trait Engine {
      async fn activate(&self) -> anyhow::Result<()>;
      async fn synthesize(&self, text: &str, req: &SynthRequest, output: &Path) -> anyhow::Result<()>;
      fn is_request_valid(&self, req: &SynthRequest) -> bool { true }
      fn terminate(&self) {}
  }
  impl EngineManager { pub async fn synthesize(&self, engine: &str, text: &str, req: &SynthRequest, out: &Path) -> anyhow::Result<()>; }
  pub fn build_engine_manager(cfg: &EnginesConfig) -> EngineManager;
  ```
  `EnginesConfig` は Task 3 で `s2v-core::config` に定義する。この Task では `build_engine_manager` を `s2v-engines` 内の引数付き関数として作らず、Task 3 に回してよい(その場合は登録処理を呼び出し側に残す)。

- [ ] **Step 1: 失敗するテストを書く** — コピー後の `http_engine.rs` テストの `dummy_cast()` を `dummy_request()` に置換し、`resolve_style_id` が `req.style` を使うことを `style_comes_from_request` として確認するテスト(`style="あまあま"` を持つ話者でそのスタイルのIDが解決される)を追加。XTTS は `req.language` が `tts_to_audio` の payload に入る `language_comes_from_request`。
- [ ] **Step 2: 失敗確認** — `cargo test -p s2v-engines` が `SynthRequest` 未定義でコンパイルエラーになること。
- [ ] **Step 3: 実装** — `Cast` 依存を `SynthRequest` に置換する。`cast.params.get("style")`→`req.style`、`cast.params.get("language")`→`req.language`、`cast.speaker_name`→`req.speaker_name`。それ以外の `params` はエンジンのクエリへマージする既存動作を維持。`EngineManager::synthesize` はエンジン名を引数で受ける。
- [ ] **Step 4: 通過確認** — `cargo test -p s2v-engines` が全て PASS。
- [ ] **Step 5:** `PORTING.md` に「`s2v-engines` を `55cffe4` からコピーし、`Cast` 依存を `SynthRequest` に置換した」を追記。
- [ ] **Step 6: Commit** — `feat: s2v-engines をコピーして SynthRequest で受け取る形にする`

---

### Task 2: パーサー(@cast のエンジン書式・voice=rec・読み指定・台詞ごとの SynthSpec)

**Files:**
- Modify: `crates/s2v-core/src/cast.rs`、`types.rs`、`parser.rs`
- Test: 各ファイル末尾の `#[cfg(test)]`

**Interfaces:**
- Produces(`s2v_core`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
  pub struct SynthSpec {
      pub engine: String,          // "voicevox" | "aivis" | "xtts"(検証は Task 4)
      pub speaker_name: String,
      pub style: String,           // 省略時 "ノーマル"。xtts は "default"
      pub language: String,        // 省略時 "ja"
      pub params: BTreeMap<String, serde_json::Value>, // エンジン固有。BTreeMap は記録のハッシュを安定させるため
  }
  // Cast に追加:     #[serde(default)] pub synth: Option<SynthSpec>,
  // Speech に追加:   pub synth: Option<SynthSpec>,   // 合成役の台詞のみ Some
  //                  pub voice_rec: bool,            // 行内 voice=rec
  //                  pub synth_text: Option<String>, // 読み置換後(合成役のみ)
  ```
  `Speech.text` は表示用(読み指定は「単語」側)。

**仕様の確定事項(パーサー):**
- 書式判定: `remain`(最初の `:` の後ろ)に `:` を含み、かつ 2 つ目の要素に `,` を含む → エンジン書式 `話者名:style,エンジン名,language=…,k=v…`。`style` が空なら既定。エンジン名が `voicevox/aivis/xtts` 以外でも `SynthSpec` は作り、検証(Task 4)でエラーにする。
- 台詞の所属: 台詞を読んだ時点の `self.casts[role]` の `synth` を `Speech.synth` に複製する(再定義は以後の台詞にだけ効く)。

- [ ] **Step 1: 失敗するテストを書く**(`parser.rs`)
  - `cast_with_engine_sets_synth`: `"@scene s\n@cast\nA:ずんだもん:あまあま,voicevox,pan=10\n@script\nA:こんにちは\n"` → 台詞の `synth == Some(SynthSpec{engine:"voicevox", speaker_name:"ずんだもん", style:"あまあま", language:"ja", params:{}})`、`Cast.pan == 10.0`。
  - `cast_without_engine_has_no_synth`: `"A:話者,pan=0"` → `synth == None`、警告なし(従来の「Script2Voice 形式」警告が出ない)。
  - `xtts_language_and_params`: `"E:Ana:default,xtts,language=en,speed=1.2"` → `language=="en"`、`params["speed"]==1.2`。
  - `recast_applies_only_to_later_lines`(Review Focus 1): `@cast` で A を voicevox で定義 → 台詞1、同じ `@scene` 内で再び `@cast` で A を肉声役(エンジン無し)に再定義 → 台詞2。台詞1 `synth.is_some()`、台詞2 `synth.is_none()`。
  - `voice_rec_option`: `A(voice=rec):台詞` → `voice_rec==true`、`offset_params` に `voice` を含まない。肉声役に書いても `voice_rec==true` で警告なし。
  - `reading_markup_for_synth_role`(Review Focus 6): `A:'東京|とうきょう'へ行く` → `text=="東京へ行く"`、`synth_text==Some("とうきょうへ行く")`。肉声役では `text` は `'東京|とうきょう'へ行く` のまま、`synth_text==None`。
  - `fullwidth_bar_is_not_a_separator`: `'東京｜とうきょう'` は置換しない。
- [ ] **Step 2: 失敗確認** — `cargo test -p s2v-core parser::` が新テストで FAIL。
- [ ] **Step 3: 実装** — `parse_cast_line` のエンジン書式対応(既存の「Script2Voice 形式」拒否は、エンジン書式として受理できなかった場合だけ残す)。`parse_speech_params` で `voice=rec` を取り出す。読み置換関数 `fn split_reading(raw: &str) -> (String /*表示*/, String /*合成*/)` を `parser.rs` に置く(Script2Voice-Rust版 `parser.rs` の `'単語|読み'` 処理を参照して挙動を合わせる)。`ScriptItem::Speech` 構築箇所を全て更新(`cargo build --workspace` が通るまで)。
- [ ] **Step 4: 通過確認** — `cargo test -p s2v-core` 全て PASS。`cargo build --workspace` が通る(`Speech` のパターンマッチ箇所の追従)。
- [ ] **Step 5: Commit** — `feat: @cast のエンジン書式と voice=rec、合成役の読み指定を解析する`

---

### Task 3: 設定(エンジンの接続先)

**Files:**
- Modify: `crates/s2v-core/src/config.rs`、`config.toml`
- Modify: `crates/s2v-engines`(`build_engine_manager(&EnginesConfig)` をここに置く)

**Interfaces:**
- Produces: `s2v_core::config::{EngineUrl, HttpConfig, EnginesConfig}`、`Config` に `#[serde(default)] pub engines: EnginesConfig`。`EnginesConfig { voicevox: EngineUrl, aivis: EngineUrl, xtts: EngineUrl, http: HttpConfig }`。`EngineUrl`/`HttpConfig` は流用元 `config.rs` と同じフィールド・同じ既定値(`http`: connect 10s / request 180s)。設定のテーブル名は流用元と同じ `[voicevox]` `[aivis]` `[xtts]` `[http]`(`config.toml` 直下)。**3つとも省略可**(エンジンを使わない台本で設定が要らないように)。

- [ ] **Step 1: 失敗するテスト** — `config.rs`: `engines_default_when_absent`(設定なしで `Config` が読めて、`voicevox.url == "http://127.0.0.1:50021"`、`aivis.url == "http://127.0.0.1:10101"`、`xtts.url == "http://localhost:8020"`)、`engines_override_from_toml`(`[voicevox] url="http://x:1"` が反映される)。
- [ ] **Step 2: 失敗確認** — `cargo test -p s2v-core config::` FAIL。
- [ ] **Step 3: 実装** — 上記の型を追加。`config.toml` に、流用元の説明コメントごと `[voicevox]` `[aivis]` `[xtts]` を追加(XTTS の `exe_path`/`args` は例としてコメントアウトし、特定の利用者のパスを入れない)。`s2v_engines::build_engine_manager` を `EnginesConfig` 受けで実装(流用元 `src/lib.rs:43-91` を移植)。
- [ ] **Step 4: 通過確認** — `cargo test --workspace` PASS。
- [ ] **Step 5: Commit** — `feat: 音声合成エンジンの接続設定を追加する`

---

### Task 4: 検証(validate)

**Files:** Modify `crates/s2v-core/src/validate.rs`

**Interfaces:** Consumes: Task 2 の `Speech.synth` / `voice_rec`。既存の検証関数の流儀(エラー/警告の返し方)に合わせる。

- [ ] **Step 1: 失敗するテスト**
  - `unknown_engine_is_error`: `A:話者:x,vioce,pan=0` のようにエンジン名が `voicevox/aivis/xtts` でない → エラー(終了コード 2 になる種類)。メッセージに行番号とエンジン名を含む。
  - `unknown_voice_option_value_is_warning`: `A(voice=recc):…` → 警告「voice の値は rec のみ」、台詞は合成対象のまま。
  - `engine_params_on_non_synth_role_warn`: 肉声役の `@cast` に `style` 相当を書いた場合の既存動作(未知パラメータ警告)が変わらない。
- [ ] **Step 2〜4:** FAIL → 実装 → `cargo test -p s2v-core validate::` PASS。
- [ ] **Step 5: Commit** — `feat: エンジン名と voice オプションを検証する`

---

### Task 5: 合成工程の判定・記録・退避(エンジン抜きの純粋部分)

**Files:**
- Create: `crates/s2v-cli/src/synth.rs`(`lib.rs` に `pub mod synth;`)
- Test: 同ファイル末尾

**Interfaces:**
- Produces:
  ```rust
  pub enum Action { Skip, Synthesize { backup: Option<PathBuf> } }
  pub struct SynthJob { pub id: LineId, pub out: PathBuf, pub text: String, pub engine: String, pub req: SynthRequest }
  pub struct SynthPlan { pub jobs: Vec<SynthJob>, pub backups: Vec<(PathBuf, PathBuf)> /*(元, 退避先)*/, pub warnings: Vec<String> }
  pub fn plan(items: &[(LineId, &ScriptItem)], voice_dir: &Path, record: &SynthRecord, now: &str) -> anyhow::Result<SynthPlan>;
  pub struct SynthRecord { /* voices/synth.json: BTreeMap<LineId文字列, Entry{engine,text,params_hash,file_sha256}> */ }
  impl SynthRecord { pub fn load(voice_dir:&Path)->anyhow::Result<Self>; pub fn save(&self, voice_dir:&Path)->anyhow::Result<()>; }
  pub fn backup_name(id: LineId, now: &str) -> String; // "S01_L003.20261028T153012.wav"
  ```
  `plan` は **副作用なし**(ファイルを動かさない・書かない)。退避の実行と記録の保存は Task 6 が行う。`now` は `YYYYMMDDTHHMMSS` の文字列(テストで固定できるよう引数にする)。`SynthRecord` の JSON には `version: 1` を持たせる。

- [ ] **Step 1: 失敗するテスト**(tempdir 使用)
  - `no_file_means_synthesize`: ファイル無し → `jobs.len()==1`、`backups` 空。
  - `matching_record_means_skip`(Review Focus 3): 記録のエンジン・文・パラメータのハッシュ・ファイルのSHA-256 が全て一致 → `jobs` 空。
  - `changed_text_means_resynthesize_with_backup`: 台詞の文を変えた → `jobs.len()==1`、`backups==[(S01_L003.wav, voices/backup/S01_L003.20261028T153012.wav)]`。
  - `foreign_file_is_backed_up`(Review Focus 2): 記録に無い `S01_L003.wav`(人の収録)→ 退避つきで合成、警告に「合成記録にないファイルを退避」を含む。
  - `file_hash_mismatch_is_backed_up`: 記録はあるがファイルの中身が違う(人が差し替えた)→ 退避つきで合成。
  - `voice_rec_is_untouched`(Review Focus 5): `voice_rec==true` → `jobs`/`backups` 空。合成記録に同じIDがあり、かつファイルのハッシュが記録と一致するときのみ警告「合成の残りです」。
  - `non_synth_role_is_untouched`: `synth==None` → 何もしない。
  - `id_less_speech_is_skipped_with_warning`: IDの無い台詞 → ジョブにしない(`number` を先に実行する旨の警告)。
  - `backup_name_format`: `backup_name(S01-L003, "20261028T153012") == "S01_L003.20261028T153012.wav"`。
  - `same_second_backup_gets_suffix`: 退避先が既に存在するとき `.1` を付けて衝突を避ける。
  - `record_roundtrip`: `save` → `load` で同じ内容。壊れた JSON は `Err`(呼び出し側が作り直す扱いは Task 6)。
- [ ] **Step 2: 失敗確認** — `cargo test -p s2v-cli synth::` FAIL。
- [ ] **Step 3: 実装** — `params_hash` は `SynthSpec` の style/language/params と `speaker_name` を `serde_json` で整形(BTreeMap のため順序固定)して SHA-256 を 16 進で記録。`sha2` は workspace 既存。ファイルのハッシュは `work.rs` の `sha256_hex` と同じ実装を使う(重複させず、公開されていなければ `pub(crate)` にして共有)。
- [ ] **Step 4: 通過確認** — `cargo test -p s2v-cli synth::` PASS。
- [ ] **Step 5: Commit** — `feat: 合成の要否判定と合成記録・退避の計画を作る`

---

### Task 6: 合成の実行と build への組み込み

**Files:**
- Modify: `crates/s2v-cli/src/synth.rs`、`crates/s2v-cli/src/build.rs`、`crates/s2v-cli/Cargo.toml`(`s2v-engines`・`tokio` 依存)
- Test: `synth.rs`(wiremock で VOICEVOX 風の偽サーバ)、`build.rs` の統合テスト

**Interfaces:**
- Consumes: Task 1 `EngineManager`、Task 3 `build_engine_manager`、Task 5 `plan`/`SynthRecord`。
- Produces:
  ```rust
  pub struct SynthOutcome { pub failed: HashSet<LineId> /*合成に失敗した台詞*/, pub synthesized: usize, pub skipped: usize }
  pub fn run(plan: SynthPlan, record: &mut SynthRecord, manager: &EngineManager, voice_dir: &Path, log: &mut ProcLog) -> SynthOutcome;
  ```
  `run` は `tokio::runtime::Builder::new_current_thread()` の `block_on` で実行する(`build` は同期のまま)。

**振る舞い:**
- `jobs` が空ならエンジンを起動しない(`activate` を呼ばない)。
- 必要なエンジンだけ `activate`。起動に失敗したエンジンの台詞は全て `failed` に入れ、`log.error` で報告して継続。
- 各ジョブ: 既存ファイルがあれば退避へ移動(`rename`)→ 合成 → 出力が 48kHz モノラル WAV でなければ変換(エンジン出力のサンプルレートは異なるため、`s2v-audio` の既存の変換処理を使う。無ければ `build` が後段で 48kHz 化するので、書き出しはエンジン出力のままでよい — 実装者は既存の取り込みがモノラル・任意レートを受けることを確認する)→ 記録更新。
- 合成に失敗した台詞: 退避した元ファイルがあれば**元の場所へ戻す**(失敗で音声を失わない)。`failed` に入れる。
- 全ジョブ後、`record.save`。`synth.json` が壊れていたら破棄して作り直し、`log.warn`。
- `build.rs` の `produce` で、`match_voice_files` の直前に `synth::plan` → `synth::run` を呼ぶ。`failed` の台詞は、その台詞に有効なファイルが残っていなければ既存の「不足」経路(`MissingReason::NoFile`)へ流れる(特別な分岐は作らない)。`voices` フォルダが無く、かつ合成対象があるときは `voices/` を作る(合成対象が無ければ従来のエラーのまま)。

- [ ] **Step 1: 失敗するテスト**
  - `synthesizes_and_records`(wiremock): VOICEVOX 風 `/speakers`、`/audio_query`、`/synthesis`(固定の短い WAV を返す)を立て、合成役1台詞 → `voices/S01_L003.wav` ができ、`synth.json` に記録される。
  - `second_run_does_not_call_engine`(Review Focus 3): 同じ入力で2回目 → モックへのリクエストが増えない(`received_requests` の数が1回目と同じ)。
  - `engine_down_marks_failed_and_continues`(Review Focus 4): 接続先に何も無い → `failed` にその台詞、`run` は `Err` にならず他の台詞は処理される。
  - `failure_restores_backup`: 既存の肉声ファイルがあり合成が 500 エラー → 肉声ファイルが元の場所に残る(退避は戻される)。
  - `build_integration_mixed_script`(`build.rs`): 肉声役 B(`voices/S01_L002.wav` あり)と合成役 A(モック)の台本で `run_build` → 終了コード 0 or 1、`output` の `timeline.json` に両台詞が入る。
- [ ] **Step 2: 失敗確認** — `cargo test -p s2v-cli synth:: build::` FAIL。
- [ ] **Step 3: 実装** — 上記。`build.rs` の既存テスト(`log_write_failure_after_output_removes_final_artifacts` など)が壊れないこと。
- [ ] **Step 4: 通過確認** — `cargo test --workspace` PASS。
- [ ] **Step 5: Commit** — `feat: build で合成役の台詞を音声合成する`

---

### Task 7: 仕様書・README・実機確認

**Files:** Modify `台本仕様.md`、`README.md`、`docs/Operating Manual.html`(該当箇所のみ)

- [ ] **Step 1: 台本仕様.md を更新** — §2.4 の「エンジン…は廃止」を、設計書 §3.1 の内容(合成役/肉声役、書式、有効範囲)に置き換える。§2.5 に `voice=rec` と合成役の読み指定。§3 に「合成音声(§3.7 新設)」: 判定表(設計書 §3.3)、退避(§3.4)、合成記録(§3.5)。§6「Script2Voice からの変更点」の @cast・読み指定・音声の元の行を更新。
- [ ] **Step 2: README/取扱説明書** — 合成役の書き方と `config.toml` のエンジン設定を追記。
- [ ] **Step 3: 全体テスト** — `cargo test --workspace` と `cargo clippy --workspace` が通る。
- [ ] **Step 4: 実機確認(Human)** — 実際の VOICEVOX を起動し、`sample/` を参考に合成役1+肉声役1の台本で `number` → `build`。合成音声が混ざった `full_dialogue.wav` を聴いて確認してもらう。台詞を1つ編集して再 `build` し、`voices/backup/` に退避ファイルができることを確認してもらう。Beads に「手動確認(Human)」として起票する。
- [ ] **Step 5: Commit** — `docs: 合成音声の仕様を台本仕様・README に反映する`

---

## Self-Review

- **Spec coverage:** 設計書 §3.1(Task 2)、§3.2(Task 2)、§3.3 の判定表(Task 5、6)、§3.4 退避(Task 5、6)、§3.5 記録(Task 5)、§4 実装方針(Task 1〜4)、設計判断 1〜8(Global Constraints に反映)。設計書 §3.3 の「テイクが覆う合成役の台詞は合成しない」は、現状①a が範囲ファイルを未対応(`voices.rs` が警告のみ)なので**今回は実装しない**。①b でテイクを実装する際の課題として Beads に別途起票する。
- **Type consistency:** `SynthSpec`(Task 2)→ Task 5 の `SynthJob.req: SynthRequest` へは `SynthSpec` から変換(speaker_name/style/language/params)。`EngineManager::synthesize(engine, text, req, out)` は Task 1 と Task 6 で同じ。
- **Proportion:** 設計書より長いが、各 Task はインターフェースとテストの確定が中心。
