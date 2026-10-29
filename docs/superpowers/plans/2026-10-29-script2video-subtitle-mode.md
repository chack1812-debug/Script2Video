# Script2Video 字幕モード Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `config.toml` の `[render] style = "subtitle"` で、吹き出しの代わりに最下段の横書き字幕(最大 3 行・左上に発言者名)で動画を仕上げる。4 行以上の台詞があれば `output.mp4` を作らず終了コード 1 で止める。

**Architecture:** 台詞は今までどおり `Item`(`ItemKind::Bubble`)のまま、`RenderConfig.style` が `Subtitle` のときだけ `place_items`(配置)と `draw_item`(描画)が字幕の経路へ分岐する。横書きの折り返しは、縦書きの `kinsoku::break_columns`(単位の長さの列を、上限の長さで分ける関数)を単位の「幅」で再利用する。`render` は配置の結果の問題一覧(`Placement.subtitle_problems`)を見て、あれば何も書かずに止まる。

**Tech Stack:** Rust(workspace: s2v-core / s2v-render / s2v-cli / s2v-bubbles)、rustybuzz、tiny-skia、ttf-parser。

**Spec:** `docs/superpowers/specs/2026-10-29-script2video-subtitle-mode-design.md`(承認済み)

## Global Constraints

- `style` が `"bubble"`(既定・省略時)のとき、動作・出力・既存テストを変えない。
- 切り替えは `config.toml` の `[render] style = "bubble" | "subtitle"` だけ。コマンドラインのオプションは作らない。
- 字幕は最下段・横書き・本文は最大 3 行(`SUBTITLE_MAX_LINES = 3`、設定項目にしない)。発言者名のラベルは行数に数えず、帯の上辺の左上に接して置く。発言者名は `LineEntry.cast`(役名)。空ならラベルなし。
- 4 行以上の台詞、または積み上げが画面に収まらない字幕があるときは、`output.mp4` を作らず(既存も変えず)、全件を `Process.log` に警告として書き、終了コード 1 で終わる。文言に「台詞が四行以上」を含める。
- ト書きは字幕モードでも現状どおり(縦書き・同じ枠・同じ自動配置)。字幕とト書きが重なれば警告(出力は作る)。
- `font_size` は吹き出し・ト書き・字幕で共通。新しい文字サイズの項目は足さない。`bubbles` 画面での文字サイズ設定は今回やらない。
- 字幕モードの `bubbles` は、台詞を出さずト書きだけを出す。保存では `bubbles.json` の台詞の手動配置を消さずに保持する。
- コメント・メッセージは日本語。コミットは日本語の `type: 説明` 形式(`git log` の既存の形式に合わせる)。**コミットと push は、ユーザーが求めたときだけ行う。** 各タスクの Commit の手順は「求められたとき」のものとして読む。

## Review Focus

- 3 行ちょうどの台詞は通り、4 行になる最小の台詞は止まる(境界)。
- 役名が空の台詞でラベルを出さず、帯の高さ・積み上げの位置が崩れない。
- 空白を含まない長い連続文字(英数字だけ)が帯の幅を超えるとき、無限ループや panic にならず、行数として数えられる。
- 同じ開始時刻の台詞が複数あるとき、`order` の順に決まった位置へ積まれ、実行ごとに変わらない。
- `style = "bubble"` の `config.toml`(`style` 省略を含む)で、`Placement` と描画が従来と同じ。`style = "Subtitle"` のような不正な値は設定の検査で分かる文言のエラーになる。

---

### Task 0: Beads の起票

**Files:** なし(`.beads/` が変わる)

- [ ] **Step 1: 起票する**

```bash
bd create --title="字幕モード(style=subtitle)の実装" --description="docs/superpowers/specs/2026-10-29-script2video-subtitle-mode-design.md を実装する。計画は docs/superpowers/plans/2026-10-29-script2video-subtitle-mode.md" --type=feature --priority=2
bd create --title="bubbles 画面で文字の大きさを設定できるようにする" --description="字幕モード設計書 §9 の後続。保存先(config.toml か bubbles.json か)と、文字サイズ変更で手動配置の枠に収まらなくなる問題を決めてから実装する。" --type=feature --priority=3
```

- [ ] **Step 2: 1 つ目を `bd update <id> --claim` する**

---

### Task 1: 設定(`style` と字幕の項目)

**Files:**
- Modify: `crates/s2v-core/src/config.rs`(`RenderConfig`・`Default`・`validate_into`・テスト)
- Modify: `config.toml`(`[render]` に項目とコメント)

**Interfaces:**
- Produces: `s2v_core::config::RenderStyle { Bubble, Subtitle }`(`#[serde(rename_all = "lowercase")]`、`Default` は `Bubble`、`Copy`・`PartialEq`)。
- Produces: `RenderConfig` の新しいフィールド(名前・型・既定値は設計書 §5.3 の表のとおり): `style: RenderStyle`、`subtitle_text_color: String`、`subtitle_outline_color: String`、`subtitle_outline_width: f32`、`subtitle_band_fill: String`、`subtitle_band_opacity: f32`、`subtitle_band_stroke: String`、`subtitle_band_stroke_width: f32`、`subtitle_padding: u32`、`subtitle_margin_bottom: u32`、`speaker_text_color: String`、`speaker_fill: String`、`speaker_stroke: String`、`speaker_stroke_width: f32`、`speaker_padding: u32`。

- [ ] **Step 1: 失敗するテストを書く**(`config.rs` の `mod tests`)
  - `style_defaults_to_bubble_and_parses_subtitle`: `Config::from_toml("")` の `render.style == RenderStyle::Bubble`、`"[render]\nstyle = \"subtitle\""` で `Subtitle`、`"[render]\nstyle = \"Subtitle\""` は `from_toml` が Err。
  - `validate_reports_bad_subtitle_values`: `subtitle_text_color = "red"`、`subtitle_band_opacity = 1.5`、`subtitle_outline_width = -1.0`、`speaker_padding = 501` をそれぞれ入れると `validate()` の結果に `render.subtitle_text_color`・`render.subtitle_band_opacity`・`render.subtitle_outline_width`・`render.speaker_padding` を含む行が出る。既定値では出ない。
  - 既存の同梱 `config.toml` を読む `builtin()` のテストが通ること(Step 4 で確認)。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-core config` → コンパイルエラーまたは FAIL。
- [ ] **Step 3: 実装**: 上の型とフィールドを足し、`validate_into` に追加する。色は既存の色のループに足す(`#RRGGBB`)。`*_stroke_width`・`subtitle_outline_width` は `f_range(0.0, 50.0)`、`subtitle_band_opacity` は `f_range(0.0, 1.0)`、`*_padding`・`subtitle_margin_bottom` は `int_range(0, 500)`(margin_bottom は `0, 2000`)。`config.toml` の `[render]` に、既存の行と同じ書式のコメントつきで全項目を足し、`style = "bubble"` を既定で書く。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-core` → 全部 PASS。
- [ ] **Step 5: Commit**(求められたとき): `git add crates/s2v-core/src/config.rs config.toml` / `feat: 設定に style と字幕の項目を足す`

---

### Task 2: 横書きの折り返し(`hlayout`)

**Files:**
- Create: `crates/s2v-render/src/hlayout.rs`
- Modify: `crates/s2v-render/src/lib.rs`(`pub mod hlayout;`)

**Interfaces:**
- Consumes: `crate::font::FontFace`(`data()`・`index()`・`units_per_em`・`ascender`・`descender`)、`crate::kinsoku::{break_columns, is_no_start, is_no_end, UnitClass}`。
- Produces:
  - `pub struct HGlyph { pub glyph_id: u16, pub x: f32, pub y: f32 }`(単位の左端・ベースラインからの相対)
  - `pub struct HUnit { pub text: String, pub width: f32, pub glyphs: Vec<HGlyph> }`
  - `pub struct HLine { pub units: std::ops::Range<usize>, pub width: f32 }`
  - `pub struct HLayout { pub units: Vec<HUnit>, pub lines: Vec<HLine> }`
  - `pub fn hlayout(face: &FontFace, font_size: f32, text: &str, inner_w: f32) -> HLayout`: 書記素ごとに横書きで整形(`Direction::LeftToRight`、縦書きの機能・縦中横・回転は使わない)し、`break_columns(&classes, inner_w)` で行に分ける。禁則は `is_no_start`・`is_no_end` を単位の最初・最後の文字に使う(`layout.rs` の `layout` と同じ)。
  - 行の高さは `font_size`(行の間隔は呼び出し側が決める)。

- [ ] **Step 1: 失敗するテストを書く**(`hlayout.rs` の `mod tests`。フォントは `layout.rs` のテストと同じ `tests/fonts/test-vrt2.ttf`、`font_size = 40.0`)
  - `short_text_is_one_line`: `"港朝"` を `inner_w = 400.0` で組むと `lines.len() == 1`、行の幅は約 80。
  - `wraps_at_inner_width`: `"港朝港朝港"`(各約 40)を `inner_w = 100.0` で組むと `lines.len() == 3`(2+2+1 文字)。
  - `line_start_prohibited_char_is_pulled_back`: `"港朝、"` を `inner_w = 80.0` で組むと、2 行目の先頭が「、」にならない(`lines[1]` の最初の単位の text が `"、"` でない)。
  - `long_unbreakable_run_does_not_hang`: 400 文字の `"港"` を `inner_w = 100.0` で組んでも戻り、`lines.len() >= 100`。
  - `empty_text_has_no_lines`: `""` で `lines.is_empty()`。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-render hlayout` → FAIL(未定義)。
- [ ] **Step 3: 実装**: `layout.rs` の `shape_units`・`group_by_grapheme` の整形の手順を参考にするが、`shape_run` は private なので、必要なら `layout.rs` 側の関数を `pub(crate)` にして再利用する(縦書きの挙動は変えない)。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-render` → 既存を含め全部 PASS。
- [ ] **Step 5: Commit**(求められたとき): `feat: 字幕用の横書きの折り返しを足す`

---

### Task 3: 字幕の描画(`subtitle`)

**Files:**
- Create: `crates/s2v-render/src/subtitle.rs`
- Modify: `crates/s2v-render/src/lib.rs`(`pub mod subtitle;`)

**Interfaces:**
- Consumes: Task 1 の設定、Task 2 の `hlayout`、`crate::draw::Overlay`(`dx`・`dy`・`pixmap`)。
- Produces:
  - `pub const SUBTITLE_MAX_LINES: usize = 3;`
  - `pub struct SubtitleMeasure { pub lines: usize, pub band_h: u32, pub label_h: u32 }`: `label_h` は帯の上辺より上に出るラベルの高さ(ラベルなしなら 0)。
  - `pub fn band_width(cfg: &RenderConfig) -> u32`: `cfg.width - 2 * cfg.margin`。
  - `pub fn measure(face: &FontFace, cfg: &RenderConfig, text: &str, speaker: Option<&str>) -> SubtitleMeasure`: 本文は `hlayout(face, cfg.font_size, text, band_width(cfg) - 2 * cfg.subtitle_padding)`。`band_h = 行数 × font_size + (行数-1) × font_size × line_spacing + 2 × subtitle_padding`(切り上げ)。ラベルは `speaker` が `Some` で空でないときだけ高さを持つ(`font_size + 2 × speaker_padding`)。
  - `pub fn draw_subtitle(face: &FontFace, cfg: &RenderConfig, text: &str, speaker: Option<&str>, band_w: u32, band_h: u32) -> Result<Overlay, String>`: 帯(`subtitle_band_fill` を `subtitle_band_opacity` で塗り、`subtitle_band_stroke_width > 0` なら枠線)、本文(`subtitle_outline_width > 0` なら `subtitle_outline_color` で縁取りしてから `subtitle_text_color` で塗る、左揃え、行ごとに上から)、ラベル(帯の左上に接し、ラベルの下辺が帯の上辺。`speaker_fill`・`speaker_stroke`・`speaker_text_color`)を描く。`Overlay.dx = 0`、`Overlay.dy = -label_h`(ラベルの分、画像は帯の上に伸びる)。
- 字形の輪郭を経路にする処理は `draw.rs` の `GlyphPath` を `pub(crate)` にして再利用する。

- [ ] **Step 1: 失敗するテストを書く**(`subtitle.rs` の `mod tests`。既定の `RenderConfig` に `font_size = 40.0`、テストフォント)
  - `measure_counts_body_lines_only`: 3 行になる本文で `measure(..).lines == 3`、`speaker = Some("A")` を足しても `lines` は 3 のまま、`label_h > 0`。`speaker = None` と `Some("")` は `label_h == 0`。
  - `measure_boundary_three_vs_four_lines`: 帯の内側の幅がちょうど N 文字分になる `cfg` で、3N 文字は `lines == 3`、3N+1 文字は `lines == 4`。
  - `draw_places_band_and_label`: `draw_subtitle(.., Some("A"), band_w, band_h)` の `Overlay.dy == -label_h`、`pixmap` の高さは `band_h + label_h`、帯の内側の中央付近の画素のアルファが `subtitle_band_opacity` に応じた値(`opacity = 1.0` なら 255)、ラベルの塗りの色が左上の画素に出る。
  - `outline_width_zero_draws_no_outline`: `subtitle_outline_width = 0.0` と `3.0` で結果の画像が異なる。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-render subtitle` → FAIL。
- [ ] **Step 3: 実装**: 上のシグネチャのとおり。色は `RenderConfig::rgb` で読む(検査済みなので `unwrap_or` の既定色を使う。`Style::bubble` と同じ流儀)。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-render` → PASS。
- [ ] **Step 5: Commit**(求められたとき): `feat: 字幕の測定と描画を足す`

---

### Task 4: 配置と描画の分岐(`scene`)

**Files:**
- Modify: `crates/s2v-render/src/scene.rs`(`Item`・`build_items`・`Placement`・`place_items`・`draw_item`・テスト)
- Modify: `crates/s2v-cli/src/render.rs`(テスト内の `Placement { placed, clipped }` の分解に `..` を足す。他の `Item`・`Placement` のリテラルがあれば追随)

**Interfaces:**
- Consumes: Task 1〜3。`autolayout::auto_layout`(ト書き)。
- Produces:
  - `Item` に `pub speaker: Option<String>` を足す。`build_items` は台詞に `LineEntry.cast`(空なら `None`)を入れ、ト書きは `None`。
  - `pub enum SubtitleProblem { TooManyLines { lines: usize }, Overflow }` と `pub struct SubtitleIssue { pub id: String, pub speaker: Option<String>, pub text: String, pub problem: SubtitleProblem }`。
  - `Placement` に `pub subtitle_issues: Vec<SubtitleIssue>` を足す(吹き出しモードでは常に空)。
  - `pub fn editable_items(items: Vec<Item>, cfg: &RenderConfig) -> Vec<Item>`: `cfg.style == Subtitle` なら `ItemKind::Bubble` を除く。それ以外は何もしない(`bubbles` 画面用)。
  - `place_items` の署名は変えない。`cfg.style == Subtitle` のとき、台詞(`ItemKind::Bubble`)は手動配置(`manual`)を無視して `place_subtitles` で置き、ト書きは今までの経路(`auto_layout`)に台詞を渡さずに置く。
  - `draw_item` の署名は変えない。`cfg.style == Subtitle` の台詞は `subtitle::draw_subtitle` で描く(`placed.w`・`placed.h` を帯の大きさとして渡す)。

- [ ] **Step 1: 失敗するテストを書く**(`scene.rs` の `mod tests`。既存の `timeline()` などのヘルパーを使う)
  - `subtitle_style_places_lines_at_bottom_and_notes_as_before`: `style = Subtitle` の `place_items` で、台詞の `Placed` が `x == margin`、`w == width - 2*margin`、下端 `y + h == height - subtitle_margin_bottom`。ト書きの `Placed` は `style = Bubble` のときと同じ。
  - `overlapping_lines_stack_upward_in_order`: 同時に出る 2 つの台詞で、`order` の早い方が下、遅い方がその上(ラベルの高さと `bubble_gap` を避ける)。時間が重ならない 2 つは同じ位置。
  - `four_line_text_is_reported`: 4 行になる台詞で `subtitle_issues` に `TooManyLines { lines: 4 }`、`id`・`speaker`・`text` が入る。3 行ちょうどは空。
  - `stack_beyond_screen_is_reported_as_overflow`: 同時に出る台詞を、積むと画面の上端を超える数だけ作ると、超えた台詞に `Overflow`。
  - `line_note_overlap_warns`: 字幕とト書きの矩形が時間も位置も重なると、`warnings` に重なりの文言が 1 件。
  - `bubble_style_is_unchanged`: `style = Bubble` で `subtitle_issues` が空、`placed` は既存テストと同じ。
  - `editable_items_drops_lines_only_in_subtitle_style`。
  - `draw_item_uses_subtitle_in_subtitle_style`: `Overlay.dy < 0`(speaker あり)で、吹き出しモードの同じ項目とは `pixmap` の大きさが違う。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-render scene` → FAIL。
- [ ] **Step 3: 実装**: `place_subtitles`(private)は、台詞を `(start_ms, order)` の順に見て、下端を `height - subtitle_margin_bottom` にそろえた位置から始め、時間が重なる配置済みの台詞(ラベルを含む上端 `y - label_h`)と重ならない一番低い位置を探す(候補は「重なる台詞の上端 − `bubble_gap`」)。ラベルを含む上端が画面の上(0 未満)になる台詞は `Overflow` にして、位置は画面内に収まる最後の候補に置く(報告のためで、描画はされない)。行数が `SUBTITLE_MAX_LINES` を超える台詞は `TooManyLines`(置く位置は 3 行の高さを仮定せず、測定した高さのまま)。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-render -p s2v-cli --lib` → PASS(`render.rs` のテストが通ること)。
- [ ] **Step 5: Commit**(求められたとき): `feat: 字幕モードの配置と描画を足す`

---

### Task 5: `render` の停止と終了コード

**Files:**
- Modify: `crates/s2v-cli/src/render.rs`(`render`・`render_main`)
- Test: `crates/s2v-cli/tests/render_ffmpeg.rs`(ffmpeg を使う統合テストの流儀に合わせる。ffmpeg が無い環境での扱いは既存のテストと同じ)

**Interfaces:**
- Consumes: `Prepared.placement.subtitle_issues`(Task 4)。
- Produces: `render_main` が `Result<RenderEnd, String>` を返す。`enum RenderEnd { Written, Refused }`(private)。

- [ ] **Step 1: 失敗するテストを書く**(`render_ffmpeg.rs`。既存の台本・素材の用意のヘルパーを使う)
  - `subtitle_style_with_four_line_line_refuses_and_exits_1`: `style = "subtitle"` の config で、4 行になる台詞を含む台本を `run_render` すると `Outcome::Warnings`、`output.mp4` が作られない(事前に置いた既存の `output.mp4` の内容も変わらない)、`Process.log` に台詞 ID と「台詞が四行以上」と行数が書かれる。該当が 2 件あれば 2 件とも書かれる。
  - `subtitle_style_renders_with_three_line_line`: 3 行以下だけの台本で `Outcome::Ok`(または警告のみ)、`output.mp4` ができる。
  - `bubble_style_default_is_unchanged`: 既存の render のテストが通ること(新規テストは不要)。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-cli --test render_ffmpeg subtitle` → FAIL。
- [ ] **Step 3: 実装**: `render_main` の `prepare` の直後(作業フォルダを消す手順 8 より前)で、`subtitle_issues` が空でなければ、各件を `log.warn`(`TooManyLines` は `{id} {役名}: 台詞が四行以上({n} 行)です。台詞を分割して台本を作り直してください: {本文}`、`Overflow` は `{id}: 同時に出る字幕が画面に収まりません`)で書き、`log.warn("output.mp4 は作っていません(変更もしていません)")` を書いて `Ok(RenderEnd::Refused)` を返す。`render` は `Refused` のとき `log.check()` を確かめて `Outcome::Warnings` を返す(`Written` は今までどおり)。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-cli` → PASS(ffmpeg が無い環境ではスキップされる既存の流儀に従う)。
- [ ] **Step 5: Commit**(求められたとき): `feat: render が四行以上の台詞で止まるようにする`

---

### Task 6: `bubbles` の字幕モード

**Files:**
- Modify: `crates/s2v-bubbles/src/api.rs`(`items_for`・`save_with`・テスト)

**Interfaces:**
- Consumes: `s2v_render::scene::editable_items`(Task 4)。

- [ ] **Step 1: 失敗するテストを書く**(`api.rs` の `mod tests`。既存のセッション作成のヘルパーを使う)
  - `subtitle_style_state_has_notes_only`: `cfg.style = Subtitle` のセッションの `/api/state` の `items` に `kind == "note"` だけが並ぶ。
  - `subtitle_style_save_keeps_line_entries`: 読み込んだ `bubbles.json` に台詞(`S01-L003` など)の手動配置があるとき、ト書きだけを含む `manual` で `save` しても、保存後の `bubbles.json` に台詞の項目が残る。吹き出しモードの既存の保存のテストは変えない。
- [ ] **Step 2: 実行して失敗を確認**: `cargo test -p s2v-bubbles api` → FAIL。
- [ ] **Step 3: 実装**: `items_for` の結果を `editable_items` に通す。`save_with` で、`cfg.style == Subtitle` のとき、`loaded` のうち `kind == Bubble` で `manual` に無いものを `items` に足す(「timeline に無い ID の引き継ぎ」と同じ場所)。
- [ ] **Step 4: 実行して成功を確認**: `cargo test -p s2v-bubbles` → PASS(HTTP と JS のテストを含む。Node が必要な既存テストの扱いは既存のまま)。
- [ ] **Step 5: Commit**(求められたとき): `feat: bubbles の字幕モードでは台詞を出さず保存で保持する`

---

### Task 7: ドキュメント・全体の検証・Codex レビュー

**Files:**
- Modify: `README.md`、`docs/Operating Manual.html`(字幕モードの切り替え・設定項目・4 行以上で止まること・終了コード 1 の意味)、`台本仕様.md`(台詞の長さの目安を足すなら)、`docs/superpowers/specs/2026-10-14-script2video-video-design.md` の終了コードの節に 1 行(字幕モードの停止も終了コード 1)

- [ ] **Step 1: ドキュメントを更新する**(既存の書き方・用語に合わせる)。
- [ ] **Step 2: 全体の検証**: `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` がすべて通る。結果を貼る(通らない項目は通らないと書く)。
- [ ] **Step 3: Codex に厳しくレビューさせる**(ユーザーの指示)。作業ツリーの差分に対して実行する:

```bash
codex exec review --uncommitted "字幕モード(docs/superpowers/specs/2026-10-29-script2video-subtitle-mode-design.md)の実装を、できるだけ厳しく指摘してください。仕様との不一致、style=bubble の既存の出力が変わる箇所、4行以上・積み上げ超過での停止(終了コード1・output.mp4を作らない・既存を変えない)の穴、境界(3行ちょうど/4行)、役名が空の台詞、長い連続文字、同時刻の台詞、bubbles.json の台詞の手動配置の保持、panic や未検査の入力、テストの不足を重点に。指摘は重大度順に、ファイルと行を付けて日本語で。"
```

  指摘を 1 件ずつ検証し、正しいものは修正して再実行する。採らない指摘は理由を記録する。重大な指摘が残らなくなるまで繰り返す。
- [ ] **Step 4: 手動確認の依頼**: 字幕の位置・読みやすさ・ラベルの見た目は Human が確認する(`bd` に確認用の issue を起票して渡す)。
- [ ] **Step 5: Beads を更新する**: 実装の issue は、Human の確認が済むまで close しない。`git status` を見せて、コミット案を報告し、承認を待つ。
