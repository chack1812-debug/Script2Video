# Script2Video シーンごとの表示方式(style=) 設計書

日付: 2026-11-07 / 対象: 台本の `#paragraph`、`build`・`render`・`bubbles`・`number`・`validate`、取扱説明書

## 1. 目的(なぜ)

台詞の表示方式(吹き出し/字幕)は、`config.toml` の `[render] style` で動画全体に 1 つだけ決まる(字幕モード設計書 2026-10-29)。
1 本の動画の中で、シーンによって吹き出しと字幕を使い分けたい。台本の側から、シーンごとに表示方式を指定できるようにする。

## 2. 目標・非目標・アンチゴール

### 目標
- `#paragraph` の行に `style=bubble` または `style=subtitle` を書き、そのシーンの台詞の表示方式を指定できる。
- `style=` を書かない `#paragraph` は、**直前のシーンの表示方式を引き継ぐ**(ユーザーの決定)。
- 最初のシーンで `style=` が無ければ、`config.toml` の `[render] style` に従う。
- `render` は、シーンごとに吹き出し・字幕を描き分けて 1 本の `output.mp4` を作る。
- `bubbles` 画面は、吹き出しのシーンの台詞だけを編集対象にする。

### 非目標(今回やらない)
- シーンの途中(`#cut` や台詞の途中)での切り替え。切り替えの単位は `#paragraph` のみ。
- 台詞 1 つごとの表示方式の指定。
- 台本の表示方式を `config.toml` で上書きする設定やコマンドラインのオプション。
- `@scene`(音響設定の単位)に表示方式を持たせること。

### アンチゴール
- `style=` を 1 つも書かない台本の `output.mp4` や `timeline.json` の中身が、今までと変わること(`style=` なしの既存の動作の維持)。
- 字幕のシーンで 4 行以上の台詞があるのに、黙って切り詰めて `output.mp4` を作ること(字幕モードの規則をシーン単位で守る)。
- 不正な `style=` の値を黙って無視すること。
- `number` が `style=` を消したり、順序を変えたりすること。

## 3. 現状(コードの事実)

- 台本仕様の「シーン」は `#paragraph` で区切る単位(`S01`…)。`@scene` は音響設定の単位で別物(台本仕様 §1・§2.8)。
- `#paragraph` の書式は `#paragraph[S02] 名称`。名称は自由文で、空白を含められる(`crates/s2v-core/src/parser.rs` の `parse_paragraph`)。コマンドは `ScriptCommand::Paragraph { id, name, line_no }`。
- `number` は `#paragraph` の直後が空か空白のとき、`#paragraph` と残りの間に `[Sxx]` を差し込む(`crates/s2v-core/src/numbering.rs`)。行のそれ以外の部分は触らない。
- `#se` は、行末の `キー=値` を後ろから読む既存の規則を持つ(`parse_se`)。
- 配置は `PlacedScene { id, name, index, silent, start, end, cuts, notes }`(`placement.rs`)。`timeline.json` の `SceneEntry` が同じ並びで持つ。`SCHEMA_VERSION` は現在 3。
- 表示方式は `cfg.style`(`RenderConfig`)を直接見ている箇所が複数ある: `s2v-render/src/scene.rs`(`missing_speaker_warnings`、`editable_items`、`is_subtitle`、約 444 行の分岐)、`s2v-bubbles/src/api.rs`(約 95 行)ほか。
- 項目は `build_items`(台詞とト書きを `Item` にする)→ `place_items` → `draw_item` → `compose` の順で作り、`render` と `bubbles` が同じ関数を使う。

## 4. 決めたこと(ユーザーの決定)

| 項目 | 決定 |
|------|------|
| 切り替えの単位 | シーン(`#paragraph`)ごと。`@scene` ではない |
| 指定の書き方 | `#paragraph` の行に `style=subtitle` / `style=bubble` を書く |
| 省略したとき | 直前のシーンの表示方式を引き継ぐ |
| 最初のシーンで省略したとき | 引き継ぐ元が無いので `config.toml` の `[render] style`(私の補足: 既存動作の維持のため) |

## 5. 仕様

### 5.1 台本の書式
- 書式: `#paragraph[S02] 名称 style=subtitle`。`[S02]` と名称は今までどおり省略できる。
- **`style=値` は行末のトークンとして読む**(`#se` の行末 `キー=値` の規則に合わせる。**設計者の判断**)。行末の空白区切りの最後の 1 語が `style=` で始まるときだけ、表示方式の指定とみなす。名称は残りの部分。
  - 例: `#paragraph[S02] 港の朝 style=subtitle` → 名称「港の朝」、表示方式 字幕。
  - 例: `#paragraph style=bubble` → 名称なし、表示方式 吹き出し。
  - 名称の途中に `style=` があっても指定にはしない(名称の一部)。
- 値は `bubble` か `subtitle`(小文字のみ)。それ以外(`style=` と値が空、`style=Bubble`、`style=foo`)は**エラー**。`validate` と `build` が報告し、`build` は止まる。
- 1 行に `style=` が複数あるときは、行末の 1 つだけが指定。残りは名称に含まれる。
- `style=` を書けるのは `#paragraph` のみ。他のコマンドに書いても、今までどおり本文や引数として扱う。

### 5.2 表示方式の決まり方(引き継ぎ)
- 表示方式は台本の並びに従って決める。`style=` を書いた `#paragraph` から、その値に切り替わる。`style=` の無い `#paragraph` は直前の `#paragraph` の方式のまま。
- 最初の `#paragraph` より前に台詞がある場合(`#paragraph` が無いシーン 0 相当)は、`config.toml` の方式。
- 台本全体で `style=` が 1 つも無いときは、全シーンが `config.toml` の方式 = 今までと同じ。
- 吹き出しに戻したいときは `style=bubble` と明示する。
- 決定した結果は `timeline.json` の各シーンに保存する(5.4)。`render` と `bubbles` は台本を読み直さず `timeline.json` だけを見る(今までの構造どおり)。

### 5.3 render の動作
- 項目(`Item`)ごとに、その台詞が属するシーンの表示方式を持たせる。`cfg.style` を直接見ている箇所は、項目ごとの方式を見るようにする。**詳細な持たせ方は実装計画で決める**。
- 吹き出しのシーンの台詞は、今までの吹き出しの経路をそのまま通る。字幕の処理を通さない(既存の golden・描画テストがそのまま通ることで担保する)。
- 字幕のシーンの台詞は、字幕モードの規則に従う:
  - 画面最下段に横書き、最大 3 行。4 行以上の台詞があるときは `output.mp4` を作らずに終了コード 1。台詞 ID を報告する。
  - 同時に出る字幕は後から始まる台詞を上に積む。積み上げが画面に収まらないときも、終了コード 1 で止める。
  - 発言者名は `subtitle_show_speaker` に従う。文字色などの設定項目は `[render]` の既存のものを全シーン共通で使う(シーンごとの見た目の指定はしない)。
  - 字幕の重ね合わせ判定(積み上げ)は字幕同士でのみ行う。吹き出しは字幕の積み上げに数えない。
- ト書きは、どちらの方式のシーンでも今までどおり出る。ト書きと字幕の重なりは、字幕のシーンだけ警告する(既存の規則)。
- 台詞の表示時刻の補正(`start_offset_ms`・`end_offset_ms`)は方式によらず効く。

### 5.4 timeline.json
- `SceneEntry` に表示方式を保存する: `"style": "bubble" | "subtitle" | null`。`null` は台本で一度も指定がなく、`config.toml` に従う(5.2 の最初のシーンの扱い)。引き継ぎは build 時に解決して、指定があった後のシーンには値を入れる。
- 追加の項目は読み込み時に省略可(省略 = `null`)とし、**`SCHEMA_VERSION` は 3 のままにできるか**を実装計画で確認する。既存の `timeline.json` は `build` をやり直さなくても読め、方式は `config.toml` に従う。互換が保てないなら 4 に上げ、版の不一致時は「`build` をやり直してください」と案内する(既存の仕組み)。
- 台本の `style=` を変えたら `script_sha256` が変わるので、`render` の鮮度チェック(`freshness.rs`)が `build` のやり直しを促す。既存の仕組みのままでよい。

### 5.5 bubbles 画面
- 吹き出しのシーンの台詞とすべてのト書きを編集対象にする。字幕のシーンの台詞は編集対象にしない(今回の字幕モードの規則を、シーン単位に変える)。
- `bubbles.json` の字幕のシーンの台詞の手動配置は、保存のときに消さずに保持する。そのシーンを吹き出しに戻したときに使える。
- 画面の描画(プレビュー)では、字幕のシーンの台詞は表示しないか、字幕として表示する。**どちらにするかは実装計画で決める**。表示しないほうが、今までの字幕モードの `bubbles` 画面と揃う。

### 5.6 number・validate・その他
- `number` は `style=` を含む行を触らない。`#paragraph style=subtitle` に番号を付けると `#paragraph[S01] style=subtitle` になる。`style=` を書いた行の `[Sxx]` の位置・形式は今までと同じ。
- `validate` は 5.1 の不正な値を `ParseWarning` ではなくエラーとして報告する。
- `script_html`(台本の閲覧用 HTML)は、`#paragraph` の見出しに表示方式を出す(出さない選択もある: 実装計画で決める)。
- 取扱説明書・README・台本仕様(§2.8 の `#paragraph`)・`config.toml` のコメントに、`style=` の書き方と引き継ぎの規則を書く。`[render] style` のコメントに「台本の `style=` が優先する」を足す。

### 5.7 設計者の判断(レビューで確認した点)
- `timeline.json` の `style` が `null` のときはキーを書かない(`skip_serializing_if`)。`style=` なしの台本の `timeline.json` が今までと同じになり、`SCHEMA_VERSION` は 3 のままにできる。
- 最初の `#paragraph` より前の台詞は `config.toml` の方式に従う。
- `config.rs` の字幕設定の検証(約 600 行。今は `style == Subtitle` のときだけ `subtitle_padding` などを検証)は、台本に字幕のシーンがあるときも効くよう、条件を広げる。

## 6. 制約・既存との整合
- `render` と `bubbles` が `Item` を同じ関数で作る構造を保つ。
- 既存のテスト(golden・描画・`number`・`validate`)は変更せずに通ること。新しい挙動は新しいテストで確かめる。
- 映像の見た目の確認(字幕と吹き出しの切り替わり、読みやすさ)は Human が行う。

## 7. 受け入れ条件
- `#paragraph style=subtitle` のシーンで台詞が字幕として出て、`style=bubble` のシーンで吹き出しとして出る 1 本の動画ができる。
- `style=` の無いシーンが直前の方式を引き継ぐ。最初のシーンで省略すると `config.toml` に従う。
- `style=` を 1 つも書かない台本で、出力(`output.mp4`・`timeline.json`)が今までと同じ。
- 不正な値(`style=foo`)がエラーになり、`build` が止まる。
- 字幕のシーンで 4 行以上の台詞があると、`output.mp4` を作らずに終了コード 1。吹き出しのシーンの台詞の長さでは止まらない。
- `number` が `style=` を保持する。
- `bubbles` で保存しても、字幕のシーンの台詞の手動配置が `bubbles.json` に残る。
- 取扱説明書・README・台本仕様・`config.toml` のコメントに説明がある。

## 8. 実装者に任せること
- `Item` に表示方式を持たせる方法と、`cfg.style` を見ている各箇所の置き換え方。
- `timeline.json` の版の扱い(5.4)。
- `bubbles` 画面での字幕シーンの台詞の見せ方(5.5)。
- `script_html` への表示(5.6)。
- エラーメッセージの文面。
