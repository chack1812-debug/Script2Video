# シーンごとの表示方式(style=) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `#paragraph` に `style=bubble|subtitle` を書けるようにし、1 本の動画の中でシーンごとに吹き出しと字幕を使い分けて `render` できるようにする。

**Architecture:** 台本の `style=` をパーサが読み、`placement` が「省略したら直前を引き継ぐ」を解決して `PlacedScene.style` に持つ。`timeline.json` の `SceneEntry.style`(指定が無ければキーなし)として保存し、`render` と `bubbles` は `Item.style: Option<RenderStyle>` を見て項目ごとに吹き出し・字幕を選ぶ(`None` は `config.toml` の `cfg.style` に従う)。`cfg.style` を直接見ていた箇所を「項目の方式」を見る形に置き換える。

**Tech Stack:** Rust(workspace: s2v-core / s2v-timeline / s2v-export / s2v-render / s2v-bubbles / s2v-cli)、ffmpeg(統合テスト)。

**Spec:** `docs/superpowers/specs/2026-11-07-script2video-scene-style-design.md`

## Global Constraints

- 書式は `#paragraph[S02] 名称 style=subtitle`。**行末の空白区切りの最後の 1 語が `style=` で始まるときだけ**指定とみなす。値は小文字の `bubble` / `subtitle` のみ。それ以外はエラー(spec 5.1)。
- `style=` の無い `#paragraph` は直前の方式を引き継ぐ。最初のシーンで省略、または `#paragraph` より前の台詞は `config.toml` の `[render] style` に従う(spec 5.2・5.7)。
- `style=` を 1 つも書かない台本の `output.mp4` と `timeline.json` は今までと同じ。`timeline.json` の `style` が `null` のときはキーを書かない。`SCHEMA_VERSION` は 3 のまま(spec 5.4・5.7)。
- 吹き出しのシーンは今までの吹き出しの経路を通す。既存のテスト(golden・描画・`number`・`validate`)は変更せずに通ること。ただし構造体のフィールド追加に伴う機械的な追記(`style: None` など)は許す。
- 字幕のシーンは字幕モードの規則: 最大 3 行、4 行以上または積み上げ超過なら `output.mp4` を作らず終了コード 1(spec 5.3)。字幕の積み上げは字幕同士でのみ行う。
- `bubbles` は字幕のシーンの台詞を編集対象にせず、`bubbles.json` の手動配置は保存で消さない(spec 5.5)。
- `number` は `style=` を触らない(spec 5.6)。
- コミットは、実行する人が Human(ユーザー)から許可を得ている場合にだけ行う(プロジェクトの方針は「許可なくコミットしない」)。Commit のステップは「許可があれば」実行する。
- バージョンは最後の Task で 0.2.14 → 0.2.15 に上げる(直近のコミットの慣例)。

## Review Focus

仕様が暗に求めているが、どの Task のテストにも自然には出てこない入力。各行のテストは持ち主の Task に入れてある。

1. 名称の途中にある `style=`(例 `#paragraph 夏 style=x の海`)は指定にしない。名称の一部のまま(Task 1)。
2. `style=` だけ・`style=Bubble`・`style=foo` はエラーで `build` が止まる。黙って無視しない(Task 1)。
3. `style=subtitle` を書いた次のシーンで `style=` を省略すると字幕のまま。`style=bubble` で戻る(Task 2)。
4. 字幕のシーンと吹き出しのシーンが同時刻に重なる台詞でも、吹き出しが字幕の積み上げに数えられない(Task 3)。
5. 吹き出しのシーンの長い台詞(4 行以上)では `render` が止まらない。字幕のシーンの 4 行以上だけが止める(Task 5)。

---

### Task 1: パーサ — `#paragraph` の行末 `style=` を読む

**Files:**
- Modify: `crates/s2v-core/src/types.rs`(`ScriptCommand::Paragraph` にフィールド追加。約 86 行と、テスト内の構築 約 243 行)
- Modify: `crates/s2v-core/src/parser.rs`(`parse_paragraph` 約 686 行)
- Test: `crates/s2v-core/src/parser.rs` 内の `mod tests`、`crates/s2v-core/src/numbering.rs` 内のテスト

**Interfaces:**
- Produces: `ScriptCommand::Paragraph { id: Option<u32>, name: Option<String>, style: Option<s2v_core::config::RenderStyle>, line_no: usize }`。`style` は台本に書かれた値そのもの(引き継ぎは Task 2)。

- [ ] **Step 1: 失敗するテストを書く**(`parser.rs` の tests。既存の `parses_paragraph_with_name` と同じ書き方で、`parse_script` を使う)
  - `paragraph_style_trailing_token`: `#paragraph[S02] 港の朝 style=subtitle` → `name == Some("港の朝")`、`style == Some(RenderStyle::Subtitle)`、`errors()` が空。
  - `paragraph_style_without_name`: `#paragraph style=bubble` → `name == None`、`style == Some(Bubble)`。
  - `paragraph_without_style_is_none`: `#paragraph[S01] 朝` → `style == None`。
  - `paragraph_style_in_middle_of_name_is_part_of_name`(Review Focus 1): `#paragraph 夏 style=subtitle の海` → `name == Some("夏 style=subtitle の海")`、`style == None`、エラーなし。
  - `paragraph_style_bad_values_are_errors`(Review Focus 2): `style=`、`style=Bubble`、`style=foo` の各行で `errors().len() == 1`。メッセージに入力した値を含む。
- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-core paragraph_style` / Expected: コンパイルエラーまたは FAIL。
- [ ] **Step 3: 実装** — `ScriptCommand::Paragraph` に `style: Option<RenderStyle>` を足す。`parse_paragraph` は、名称の文字列(`arg`)を空白で分けた最後の 1 語が `style=` で始まるとき、その語を取り除いて値を `bubble`/`subtitle` に対応づける。値が不正なら `self.error(line_no, ...)`(`parse_cut` と同じ。指定は `None` として行は残す)。取り除いた残りを名称にする(残りが空なら名称なし)。名称の既存の検査(`]` やファイル名に使えない文字)は残りの名称にかける。`Paragraph` を構築・分解している箇所(`placement.rs`・`validate.rs`・`numbering.rs`・`script_html.rs`・テスト)は、`..` を使っていなければ追従させる。
- [ ] **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-core` / Expected: PASS(既存のテストを含む)。
- [ ] **Step 5: `number` が `style=` を保つテストを足す** — `numbering.rs` の既存テストと同じ書き方で、`#paragraph style=subtitle` に番号を付けると `#paragraph[S01] style=subtitle` になり、他の行は変わらないことを確かめる。Run: `cargo test -p s2v-core numbering` / Expected: PASS(実装の変更は不要のはず。落ちたら `numbering.rs` を直す)。
- [ ] **Step 6: Commit(許可があれば)** — `git add crates/s2v-core && git commit -m "feat: #paragraph の行末 style= を読む"`

---

### Task 2: 配置と timeline.json — 引き継ぎを解決して保存する

**Files:**
- Modify: `crates/s2v-core/src/placement.rs`(`PlacedScene` 約 109 行、`OpenScene` 約 172 行、`Paragraph` の分岐 約 237 行)
- Modify: `crates/s2v-timeline/src/lib.rs`(`SceneEntry` 約 39 行、`Timeline::validate`)
- Modify: `crates/s2v-export/src/timeline_json.rs`(`SceneEntry` を作る約 189 行)
- Test: 各ファイルの `mod tests`

**Interfaces:**
- Consumes: Task 1 の `ScriptCommand::Paragraph { style, .. }`。
- Produces: `PlacedScene.style: Option<RenderStyle>`(引き継ぎ解決後)。`SceneEntry.style: Option<String>`(`"bubble"` / `"subtitle"`。`#[serde(default, skip_serializing_if = "Option::is_none")]`)。

- [ ] **Step 1: 失敗するテストを書く**
  - `placement.rs`: `scene_style_is_inherited_until_changed`(Review Focus 3)— 3 つの `#paragraph`(1 つ目 `style=subtitle`、2 つ目 省略、3 つ目 `style=bubble`)と、最初に `style=` の無い `#paragraph` を 1 つ前に置いた 4 シーンで、`PlacedScene.style` が `[None, Some(Subtitle), Some(Subtitle), Some(Bubble)]`。
  - `s2v-timeline`: `scene_style_is_omitted_when_none` — `style: None` の `SceneEntry` を JSON にするとキー `style` が無い。`Some("subtitle")` ならキーがある。キーの無い JSON を読むと `None`。
  - `s2v-timeline`: `validate_rejects_unknown_scene_style` — `style: Some("foo")` の `Timeline` は `validate()` が `Err`(既存の「§3.6 の不変条件」の書き方に合わせる)。
  - `timeline_json.rs`: 既存の組み立てのテストの書き方で、`PlacedScene.style = Some(Subtitle)` のとき `SceneEntry.style == Some("subtitle")`。
- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-core -p s2v-timeline -p s2v-export` / Expected: FAIL またはコンパイルエラー。
- [ ] **Step 3: 実装** — 配置のループに「現在の方式」(`Option<RenderStyle>`)を持たせ、`Paragraph` で `style` が `Some` なら更新し、新しいシーンにその時点の値を持たせる。最初の `#paragraph` より前の暗黙のシーン(S00)は `None`。`SceneEntry` にフィールドを足して、`timeline_json.rs` で `RenderStyle` → 文字列に変換する(`Bubble` → `"bubble"`、`Subtitle` → `"subtitle"`)。`Timeline::validate` は `style` が `"bubble"`・`"subtitle"` 以外なら不変条件の違反として報告する。`SceneEntry` / `PlacedScene` をリテラルで作っているテストやヘルパーは `style: None` を足す(`rg "SceneEntry \{|PlacedScene \{"` で探す)。
- [ ] **Step 4: 通ることを確かめる** — Run: `cargo test --workspace --exclude s2v-cli` / Expected: PASS。さらに、`style=` の無い台本の既存の golden(`crates/s2v-cli/tests/build_golden.rs`)が変わらないこと — Run: `cargo test -p s2v-cli --test build_golden` / Expected: PASS(golden ファイルは更新しない)。
- [ ] **Step 5: Commit(許可があれば)** — `git commit -m "feat: シーンの表示方式を引き継いで timeline.json に保存する"`

---

### Task 3: render の項目 — 項目ごとの表示方式

**Files:**
- Modify: `crates/s2v-render/src/scene.rs`(`Item` 約 18 行、`build_items` 約 32 行、`missing_speaker_warnings` 約 105 行、`editable_items` 約 170 行、`is_subtitle` 約 182 行、`place_items` 約 347・444 行、`draw_item` 約 486 行)
- Modify: `crates/s2v-core/src/config.rs`(約 600 行)
- Modify: `crates/s2v-cli/src/prepare.rs`(約 156 行)
- Test: `crates/s2v-render/src/scene.rs` の `mod tests`、`crates/s2v-core/src/config.rs` の `mod tests`

**Interfaces:**
- Consumes: Task 2 の `SceneEntry.style: Option<String>`。`LineEntry.scene`(例 `"S01"`)と `SceneEntry.id` を突き合わせて台詞の所属シーンを決める。ト書きは `SceneEntry.notes` のシーン。
- Produces:
  - `Item.style: Option<RenderStyle>`(`None` は `cfg.style` に従う)。
  - `pub fn item_style(item: &Item, cfg: &RenderConfig) -> RenderStyle`(`item.style.unwrap_or(cfg.style)`)。
  - `fn is_subtitle(item: &Item, cfg: &RenderConfig) -> bool` — 署名は今のまま、中身を `item_style(item, cfg) == Subtitle && item.kind == Bubble` に変える。
  - `pub fn editable_items(items: Vec<Item>, cfg: &RenderConfig) -> Vec<Item>` — 署名は今のまま。字幕の台詞(`is_subtitle`)だけを除く。
  - `pub fn missing_speaker_warnings(face, items, cfg)` — 署名は今のまま。字幕の台詞だけ対象にする。
  - `pub fn RenderConfig::subtitle_body_error(&self) -> Option<String>` — 約 600 行の本文幅の検査を取り出したもの(`validate` はこれを `style == Subtitle` のときだけ呼ぶ。メッセージは今と同じ)。
  - `pub fn has_subtitle(items: &[Item], cfg: &RenderConfig) -> bool`。

署名を変えないので、既存のテスト(`sub_cfg(RenderStyle::Subtitle)` で `style: None` の項目を使うもの)は変更せずに通る。

- [ ] **Step 1: 失敗するテストを書く**(`scene.rs` の tests。既存のヘルパー `line`・`sub_cfg`・`build_items` を使い、`SceneEntry.style` を `Some("subtitle")` にした `Timeline` を作る)
  - `build_items_carries_scene_style`: 台詞・ト書きとも、所属シーンの `style` が `Item.style` に入る。`style: None` のシーンは `None`。
  - `scene_style_overrides_config`: `cfg.style = Bubble` でも `Item.style = Some(Subtitle)` の台詞は `is_subtitle` が真。`cfg.style = Subtitle` でも `Some(Bubble)` の台詞は偽(`item_style` の確認)。
  - `editable_items_drops_only_subtitle_lines`: `cfg.style = Bubble`、字幕シーンの台詞 1 件・吹き出しシーンの台詞 1 件・ト書き 1 件 → 字幕シーンの台詞だけが除かれる。
  - `mixed_scenes_do_not_stack_bubbles_into_subtitles`(Review Focus 4): 同じ時刻に重なる台詞 2 件(吹き出しシーン 1・字幕シーン 1)で、字幕の `subtitle_issues` が空、字幕は最下段(`subtitle_margin_bottom` の位置)に置かれ、吹き出しは通常の自動配置の対象のまま(`Placement.placed` に両方ある)。
  - `missing_speaker_warnings_only_for_subtitle_scene_lines`: 役名にフォントに無い文字がある台詞が、吹き出しシーンなら警告なし、字幕シーンなら警告あり。
  - `config.rs`: `subtitle_body_error_matches_validate` — `subtitle_padding` を大きくして本文幅が `font_size` を下回る設定では `subtitle_body_error()` が `Some`、`validate()` は `style = Subtitle` のときだけ同じメッセージを含み、`Bubble` のときは含まない(今の挙動の維持)。
- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-render -p s2v-core` / Expected: FAIL またはコンパイルエラー。
- [ ] **Step 3: 実装** — `Item.style` を足し、`build_items` が所属シーンの `style` 文字列を `RenderStyle` に直して入れる(`tl.scenes` から シーン ID → 方式 の対応表を先に作る)。`is_subtitle`・`editable_items`・`missing_speaker_warnings` を `item_style` ベースにする。`place_items` の 444 行の `if cfg.style == Subtitle` は `if has_subtitle(items, cfg)` に変える(字幕の台詞が 1 件も無いときに字幕の処理を通さないので、吹き出しだけの台本の経路は今までと同じ)。`warn_subtitle_note_overlap` はそのまま(字幕を置いたときだけ働く)。`config.rs` は `subtitle_body_error` を取り出す。`prepare.rs` は項目を作った後、`has_subtitle(&items, cfg)` が真で `cfg.style != Subtitle` のとき `cfg.subtitle_body_error()` を調べ、あれば既存のエラーの返し方で止める。`Item` をリテラルで作っているテストは `style: None` を足す。
- [ ] **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-render -p s2v-core` / Expected: PASS(既存の字幕・吹き出しのテストを含む)。
- [ ] **Step 5: Commit(許可があれば)** — `git commit -m "feat: render が項目ごとの表示方式で吹き出しと字幕を描き分ける"`

---

### Task 4: bubbles — シーンごとに字幕の台詞を編集対象から外す

**Files:**
- Modify: `crates/s2v-bubbles/src/api.rs`(`items_for` 約 86 行、`is_subtitle_style` 約 94 行、使用箇所 約 518・537 行)
- Test: `crates/s2v-bubbles/src/api.rs` の `mod tests`(既存の `subtitle_session`・`subtitle_style_*` の近く)

**Interfaces:**
- Consumes: Task 3 の `item_style`・`editable_items`・`Item.style`。
- Produces: `fn is_subtitle_line(s: &Session, id: &str) -> bool`(`is_subtitle_style` の置き換え。台詞 ID の所属シーンの方式が字幕か。`s.timeline` から ID を探し、`Item.style` と同じ規則で方式を決める。既存の `s.cfg` を使う)。

- [ ] **Step 1: 失敗するテストを書く** — `mixed_style_state_has_bubble_scene_lines_and_all_notes`: `cfg.style = Bubble` のセッションで、シーン S01 が字幕・S02 が吹き出しの `timeline` を使い、`state` に S02 の台詞とすべてのト書きが出て、S01 の台詞が出ない。`mixed_style_save_keeps_subtitle_scene_manual_placement`: `bubbles.json` に S01 の台詞の手動配置があるとき、S02 の台詞を保存しても S01 の台詞の配置が保存後のファイルに残る(既存の `subtitle_style_save_keeps_line_entries` と同じ書き方)。
- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-bubbles mixed_style` / Expected: FAIL。
- [ ] **Step 3: 実装** — `items_for` は変更なし(`editable_items` が項目ごとに除く)。`is_subtitle_style(s)` を使っている 2 箇所(枠に収まるかの検査・保存で手動配置を残す処理)を、台詞 ID ごとの `is_subtitle_line(s, id)` に置き換える。`api.rs` の `RenderStyle` の import が余ったら整理する。
- [ ] **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles` / Expected: PASS(既存の `subtitle_style_*` を含む)。
- [ ] **Step 5: Commit(許可があれば)** — `git commit -m "feat: bubbles が字幕のシーンの台詞を編集対象から外す"`

---

### Task 5: 統合テスト — 混在した台本を render する

**Files:**
- Test: `crates/s2v-cli/tests/render_ffmpeg.rs`(既存の `project`・`skip`・`RED`・`SUBTITLE`・`frame_rgb` などを使う。`subtitle_style_*` のテストの近く)
- Test: `crates/s2v-cli/tests/number_cli.rs` または `validate` を呼ぶ既存のテスト(不正な値の確認)

**Interfaces:**
- Consumes: Task 1〜4。新しい実装は無い(通らなければ該当 Task を直す)。

- [ ] **Step 1: テストを書く**(ffmpeg が無い環境では `skip()` で飛ばす既存の書き方)
  - `mixed_styles_render_one_video`: 2 シーン(`#paragraph[S01] 朝 style=subtitle` / `#paragraph[S02] 昼 style=bubble`)の台本を `config.toml` の `style` 既定(吹き出し)で `render` し、成功して `output.mp4` ができる。S01 の台詞の時刻のコマの最下段付近に白い字幕の画素があり、S02 の台詞の時刻のコマには吹き出しの白い枠がある(`count_near(&buf, [255,255,255], 3)` の領域の違いで確かめる。既存の `subtitle_style_*` と `overlong_auto_item_*` の確かめ方を参考にする)。
  - `subtitle_scene_four_lines_refuses_but_bubble_scene_long_line_does_not`(Review Focus 5): 4 行になる長い台詞が S01(字幕)にあるとき `Outcome::Warnings` で `output.mp4` を作らず、警告に `S01-Lxxx: 台詞が四行以上` が出る。同じ長さの台詞が S02(吹き出し)だけにある台本では止まらない(警告があっても `output.mp4` はできる)。
  - `no_style_script_is_unchanged`: `style=` を書かない台本の `timeline.json` に `"style"` というキーが無い(`serde_json::Value` で全シーンを確かめる)。
  - `bad_style_value_stops_build`: `#paragraph[S01] 朝 style=foo` で `validate` / `build` がエラー(終了コード 1)になり、メッセージに `style` の値が出る。既存の `validate` / `build` を呼ぶテストの書き方に合わせる。
- [ ] **Step 2: 実行する** — Run: `cargo test -p s2v-cli --test render_ffmpeg mixed_styles subtitle_scene_four_lines no_style_script bad_style_value`(別々に実行してもよい) / Expected: PASS。
- [ ] **Step 3: 全体を確かめる** — Run: `cargo test --workspace` と `cargo clippy --workspace --all-targets -- -D warnings` と `cargo fmt --all -- --check` / Expected: すべて成功(既存のテストが 1 つも変わっていない)。
- [ ] **Step 4: Commit(許可があれば)** — `git commit -m "test: シーンごとの表示方式の統合テストを足す"`

---

### Task 6: 文書とバージョン

**Files:**
- Modify: `台本仕様.md`(§2.8 の `#paragraph`、§6 の変更点の表の「台詞の表示」)
- Modify: `config.toml`(`[render] style` のコメント)
- Modify: `README.md`、`docs/Operating Manual.html`(字幕モードの説明の近く。版の表記 `v0.2.14・2026-11-06` を `v0.2.15・2026-11-07` に)
- Modify: 各 `crates/*/Cargo.toml` の `version`(0.2.14 → 0.2.15)と `Cargo.lock`

- [ ] **Step 1: 台本仕様を書く** — §2.8 に、`#paragraph[S02] 名称 style=subtitle` の書式、値(`bubble` / `subtitle`)、省略したら直前を引き継ぐこと、最初のシーンの省略は `config.toml` の `style` に従うこと、名称の最後の語を `style=` で始められないこと、不正な値はエラーになることを書く。§6 の「台詞の表示」の行を更新する。例は spec の 5.1 の 3 つを使う。
- [ ] **Step 2: `config.toml` と README と取扱説明書に反映する** — `[render] style` のコメントに「台本の `#paragraph` の `style=` が優先する」を足す。README と取扱説明書の字幕モードの説明に、台本での切り替え方(引き継ぎの規則を含む)と、字幕のシーンでは `bubbles` 画面に台詞が出ないことを足す。
- [ ] **Step 3: バージョンを上げる** — 9 つの `Cargo.toml` を 0.2.15 にして `cargo build --workspace` で `Cargo.lock` を更新する。
- [ ] **Step 4: 確かめる** — Run: `cargo test --workspace` / Expected: PASS。`rg "0\.2\.14"` が残っていない(履歴の文書を除く)。
- [ ] **Step 5: Commit(許可があれば)** — `git commit -m "docs: シーンごとの表示方式 style= を説明する。バージョンを 0.2.15 にする"`

---

## Self-Review

- **Spec coverage:** 5.1 書式 → Task 1。5.2 引き継ぎ → Task 2。5.3 render → Task 3・5。5.4 timeline.json → Task 2。5.5 bubbles → Task 4。5.6 number → Task 1 Step 5、validate/build のエラー → Task 1・5、文書 → Task 6。5.7 の 3 点(キー省略・S00 の扱い・字幕設定の検証の拡張)→ Task 2・3。`script_html` への表示は spec が任意としたので**やらない**(YAGNI。必要なら別の issue)。
- **Type consistency:** `RenderStyle`(core)を `Paragraph.style` / `PlacedScene.style` / `Item.style` で共通に使い、`s2v-timeline` は依存を増やさないため `SceneEntry.style: Option<String>`(Task 2 で変換、Task 3 で戻す)。`item_style` / `is_subtitle` / `has_subtitle` / `subtitle_body_error` / `is_subtitle_line` の名前は Task 3・4 で一致。
- **Proportion:** コードの本体は署名と検証内容だけにとどめた。
