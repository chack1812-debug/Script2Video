# Script2Video 計画 2d-2(bubbles の画面) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 2d-1 の API の上に、吹き出し・ト書きを画面で動かし、大きさ・尻尾・表示時刻の offset を調整して保存できる画面の本体を作る。GUI と `render` の同じコマが一致すること(設計書 §9)を保つ。

**Architecture:** ブラウザの ES モジュールを、DOM に触れない純粋なモジュール(`geom.js`・`timeline.js`・`model.js`・`api.js`(fetch は差し込み))と、DOM のモジュール(`view.js`・`player.js`・`editor_ui.js`・`app.js`)に分ける。純粋なモジュールは Node の `node:test` で単体テストし、`cargo test` から `node --test` を呼ぶ。サーバーは埋め込みのファイルとパスを足すだけ(API は変えない)。画面全体は Chrome の自動操作で確かめる。

**Tech Stack:** 素の JavaScript(ES2020 の ES モジュール。外部のライブラリ・ビルドの道具なし)、HTML、CSS。Node 24(`node:test`、依存なし)。Rust(既存の `s2v-bubbles`)。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)§5.3(画面。今回の要件の正本)、§5.4(API の型の表が唯一の定義)、§2.3(ロック)、§8、§9。経緯は `docs/superpowers/specs/2026-10-16-script2video-bubbles-ui-design.md`。前提として計画 2d-1 が main にある。

## Global Constraints

- 応答・コメント・画面の文言・エラーメッセージ・コミットメッセージは日本語(既存のコードに合わせる)。
- **CSP**(`default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; …`)の下で動くこと。禁止: HTML のインラインのスクリプト・`<style>`・`style` 属性・イベントハンドラの属性(`on…=`)、JS の `innerHTML`・`outerHTML`・`insertAdjacentHTML`・`document.write`・`eval`・`new Function`・文字列を渡す `setTimeout`/`setInterval`・`setAttribute("style", …)`・`setAttribute("on…", …)`・`.style.cssText`、外部のライブラリ・CDN。許可: `element.style.<個別のプロパティ> = 値`(CSSOM)、`addEventListener`、`document.createElement`・`textContent`・`URL.createObjectURL`(blob の画像)。
- 台本由来の文字列(台詞・ト書き・シーン名・警告・エラーの理由)は `textContent` でだけ入れる。
- **時刻はコマ番号**。カットと項目のコマの範囲(半開区間)と重ねる順(`order`)はサーバーの値をそのまま使う。素材の中のコマ k: `d = max(n − n0, 0)`、`frames ≤ 0` なら 0、`d < frames` なら `d`、そうでなければ `looped` なら `d mod frames`、そうでなければ `frames − 1`(サーバーの `s2v_video::frames::asset_frame` と同じ)。音声の位置からのコマ: `n = min(max(floor(秒 × fps + 1e-9), 0), total_frames − 1)`。スライダーは `0..max(total_frames − 1, 0)`。2 コマのずれの比較は `|差| ≥ 2/fps − 1e-6`(浮動小数の誤差の許容)。
- **重ね合わせの位置**は `placed.x + image.dx`、`placed.y + image.dy`(`/api/render` の応答では `X-S2V-Origin: dx,dy`)。重ねる順は `order` の小さい順に下から。尻尾の中心は `x + floor(w/2)`、`y + floor(h/2)`、先端は `0 ≤ 先端 ≤ canvas の幅・高さ`(サーバーの検証と同じ)。
- **要求の配置**には `state.items[].kind` を `kind` として補う。`/api/render` の本文は `{id, manual:{ID:配置}}`、`/api/layout`・`/api/save` は `{manual:{ID:配置}}`。既存の応答の形: `/api/layout` は項目の配列(`state.items` と同じ要素)、`/api/render` は PNG とヘッダー `X-S2V-Origin: <dx>,<dy>`・`X-S2V-Clipped: 0|1`、エラーは 422 が `{errors:[…]}`、それ以外が `{error:"…"}`。
- 確定の直列化・layout の結果の反映・保存の一式・再生の状態・止まっているときのコマ・キーとドラッグ・素材のエラーとセッション・PNG の上限 300 件は、設計書 §5.3 の各項のとおり(下の対応表で、担当のタスクと確かめ方を決める)。
- Node のテストは `crates/s2v-bubbles/assets-test/*.test.mjs`。`cargo test` は Node が無ければ失敗し、`S2V_SKIP_NODE_TESTS=1` のときだけ飛ばす(禁止の検査は常に行う)。
- 検証コマンド(各タスクの最後に全部通す): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- `.superpowers/` の下と `.beads/interactions.jsonl` は git に加えない。コミットメッセージの末尾に、空行のあと実装者のモデルの `Co-Authored-By:` 行。

## Review Focus

1. **小さすぎる大きさにドラッグしてすぐ別の項目を動かす**: 1 つ目は直前の確定値に戻り、2 つ目は 1 つ目の未検証の値を含まずに確定する。→ Task 3 `queue_rejects_first_and_commits_second_without_it`、Task 9 の手順 3。
2. **保存の送信中に編集する / 数値の欄に入力した直後に Ctrl+S**: 送った一式だけが保存済みになる。入力中の値も保存に入る。→ Task 3 `save_marks_only_sent_snapshot_as_saved`・`save_runs_pending_flushers_first`、Task 9 の手順 9。
3. **台詞に `<script>alert(1)</script>` や `"` を含む台本**: どこでも文字として表示され、スクリプトは動かない。→ Task 1 `forbidden_checker_*`・`assets_have_no_dangerous_apis`、Task 9 の手順 1。
4. **停止中にカットの境目のコマ(n0−1 と n0)を行き来する**: 背景と吹き出しが必ずそのコマのものになる(古い応答で上書きされない)。→ Task 2 `cut_and_items_at_boundaries`、Task 3 `frame_requests_coalesce_and_drop_stale`・`stale_png_is_not_registered`、Task 9 の手順 6。
5. **長い動画の素材で再生 → 停止 → スライダー**: 動画の音が鳴らず、止めたコマで止まり、ループ・止めるが正しい。→ Task 2 `player_transitions`、Task 9 の手順 7。

## 設計書 §5.3 の各項と、担当・確かめ方の対応表

| §5.3 の項 | 担当 | 確かめ方 |
|---|---|---|
| 時刻はコマ番号、スライダーの範囲 | 2(計算)・5(DOM) | `cut_and_items_at_boundaries`・`frame_from_audio_and_slider` / Chrome 6 |
| 左の一覧・選択でそのコマへ | 5 | Chrome 2 |
| 中央: 縦横比・背景の fit と余白の色・重ねる順と原点 | 1(換算)・5 | `fit_view_*` / Chrome 5・6 |
| 動画は `<video>`、読み込みエラーで frame | 5・6 | Chrome 8 |
| 下: 再生/停止、音声の位置でコマ | 2(遷移)・6 | `player_transitions` / Chrome 7 |
| 右: 数値の欄 | 7 | Chrome 2 |
| 操作: 移動・四隅・尻尾・矢印キー・自動に戻す・Ctrl+S | 1(計算)・7・8 | `drag_*`・`nudge_*` / Chrome 2・3・4・10 |
| 重ね画像の取得(PNG の保持と破棄) | 3 | `changed_images`・`stale_png_is_not_registered`・`image_cache_*` |
| 再生中の動画の素材(消音・2 コマ・止める) | 2・6 | `player_transitions` / Chrome 7 |
| 要求の配置の型(kind を補う) | 3・4 | `with_kind`・`api_*` |
| 確定の直列化 | 3 | `queue_*` |
| layout の結果の反映 | 3 | `layout_replaces_all_items_and_drops_images`・`layout_failure_after_render_success_rejects` |
| 保存(保留中の入力・送った一式) | 3・8 | `save_*` / Chrome 9 |
| 再生の状態(遷移) | 2・6 | `player_transitions` / Chrome 7 |
| 止まっているときのコマ(seeked を待つ・frame の集約) | 2・3・6 | `player_transitions`・`frame_requests_coalesce_and_drop_stale` / Chrome 6・8 |
| キーとドラッグ(入力中の矢印キー・既定の動作・二重の確定・capture・cancel) | 7 | `last_queued_*` / Chrome 3・11 |
| 素材の読み込みエラーとセッション(HEAD で 403) | 4・5 | `api_session_lost_*` / Chrome 12 |
| PNG の保持の上限 300 | 3 | `image_cache_*` |
| `textContent` だけ | 1・5 | `assets_have_no_dangerous_apis` / Chrome 1 |
| 「終了」・`beforeunload` | 8 | Chrome 9 |

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `crates/s2v-bubbles/src/assets/geom.js`、`assets-test/geom.test.mjs` | 換算・ドラッグ・収める・矢印キー・ヒット判定 | 1 |
| `crates/s2v-bubbles/assets-test/forbidden.mjs`・`forbidden.test.mjs` | 禁止の検査の関数(Node で動く)とその陽性・陰性の見本 | 1 |
| `crates/s2v-bubbles/tests/node_assets.rs` | `node --test` の呼び出し(禁止の検査のテストも node で走る) | 1 |
| `crates/s2v-bubbles/src/server.rs` | 埋め込みのモジュールの配信 | 1(仕組みと geom)・2〜8(各自のファイルを1行ずつ) |
| `crates/s2v-bubbles/src/assets/timeline.js`、`assets-test/timeline.test.mjs` | コマから表示、素材の中のコマ、再生の状態遷移 | 2 |
| `crates/s2v-bubbles/src/assets/model.js`、`assets-test/model.test.mjs` | 手動配置・確定の待ち行列・保存・layout の反映・PNG の世代・frame の集約・PNG の上限 | 3 |
| `crates/s2v-bubbles/src/assets/api.js`、`assets-test/api.test.mjs` | fetch の変換(差し込み)・403 | 4 |
| `crates/s2v-bubbles/src/assets/view.js`・`app.js`・`index.html`・`app.css` | 一覧・プレビュー(背景・重ね)・スライダー・状態の表示 | 5 |
| `crates/s2v-bubbles/src/assets/player.js` | 再生(音声・動画・frame) | 6 |
| `crates/s2v-bubbles/src/assets/editor_ui.js` | 選択・ドラッグ・キー・数値の欄・自動に戻す | 7 |
| `app.js`・`editor_ui.js` | 保存・終了・`beforeunload`・保留中の入力 | 8 |
| `README.md` | 画面の使い方 | 9 |

並行: Task 1 の後、Task 2・3・4 は別のファイルなので並行できる(`server.rs` の配信の1行の追加は取り込み時に解消)。5 は 2・3・4 の後、6 と 7 は 5 の後に並行できる(`app.js` の結線の衝突は取り込み時に解消)、8 は 7 の後、9 は最後。

## DOM のモジュールの契約(Task 5〜8 が使う)

- `app.js` が状態を持つ: `state`(`/api/state`)、`editor`(`model.Editor`)、`cache`(`model.ImageCache`)、`frames`(`model.FrameFetcher`)、`ui = {n, selected, playing, dragging}`。他のモジュールは `app.js` から渡された関数と値だけを使う。
- `view.js`(Task 5)の公開: `mountView(root, state, handlers)`(`handlers = {onSelectItem(id), onSeek(n), onPlayToggle(), onSave(), onQuit(), onRevertAuto(id), onNumberCommit(id, field, value)}`)→ `{renderList(items, selected), renderFrame({n, cut, items, cache, selected}), setBackground(spec), setStatus(text), setWarnings(list), setDirty(bool), setNumbers(item|null), previewElement(), overlayLayer()}`。`setBackground(spec)` の `spec` は `{kind:"none"|"image"|"video"|"frame"|"unavailable", url?, fit, background}`。
- `player.js`(Task 6)の公開: `createPlayer({audio, video, state, getN, setN, frames, onSessionLost})` → `{play(), stop(), seek(n), onCutChange(cut), tick()}`(`timeline.playerStep` の結果を DOM に適用する)。
- `editor_ui.js`(Task 7・8)の公開: `attachEditing({view, editor, state, ui, player, cache, redraw, toast})` → `{flushPending(): Promise}`(矢印キーの待ちと入力中の数値の欄を `enqueue` する。保存・終了の前に呼ぶ)。

---

### Task 1: `geom.js`、Node のテストの仕組み、禁止の検査

**Files:**
- Create: `crates/s2v-bubbles/src/assets/geom.js`、`crates/s2v-bubbles/assets-test/geom.test.mjs`、`crates/s2v-bubbles/assets-test/forbidden.mjs`、`crates/s2v-bubbles/assets-test/forbidden.test.mjs`、`crates/s2v-bubbles/tests/node_assets.rs`
- Modify: `crates/s2v-bubbles/src/server.rs`(`/geom.js`。配信する埋め込みのモジュールを表(`&[(&str, &str)]` の「パス → 内容」)にし、以降のタスクは表に1行足すだけにする)

**Interfaces:**
- Produces(`geom.js`、純粋。`export`。配置の形はサーバーの JSON と同じ `{x, y, w, h, tail:{dx,dy}|null, start_offset_ms, end_offset_ms}`。関数は offset と(変えないときは)tail をそのまま引き継ぐ):
  - `fitView(canvasW, canvasH, boxW, boxH)` → `{left, top, scale}`(縦横比を保って箱に収め、中央寄せ)
  - `toCanvas(px, py, view)` → `{x, y}`(`(px − left)/scale` など。丸めない)、`toView(cx, cy, view)` → `{x, y}`
  - `clampRect(r, cw, ch)` → `{x, y, w, h}`(各値を四捨五入してから、`1 ≤ w ≤ cw`、`1 ≤ h ≤ ch`、`0 ≤ x ≤ cw − w`、`0 ≤ y ≤ ch − h`)
  - `tailCenter(p)` → `{x: p.x + floor(p.w/2), y: p.y + floor(p.h/2)}`、`tailTip(p)` → 中心 + 尻尾(尻尾が null なら null)
  - `clampTail(p, cw, ch)` → 配置(先端を `0..=cw`・`0..=ch` に収めるよう尻尾の dx・dy を詰める。null はそのまま)
  - `dragMove(start, dx, dy, cw, ch)` → 配置(x・y を動かして `clampRect`、その後 `clampTail`)
  - `dragCorner(start, corner, dx, dy, cw, ch)` → 配置(`corner` は `"nw"|"ne"|"sw"|"se"`。反対の角を固定、大きさは 1 以上、画面の外に出さない。その後 `clampTail`)
  - `dragTail(start, px, py, cw, ch)` → `{dx, dy}`(先端の canvas の座標 (px, py) を四捨五入し、`0..=cw`・`0..=ch` に収め、`tailCenter` からの相対)
  - `nudge(p, key, shift, cw, ch)` → 配置(`"ArrowLeft"|"ArrowRight"|"ArrowUp"|"ArrowDown"`。1px、shift なら 10px。`clampRect` と `clampTail`)。それ以外のキーは null
  - `hitTest(items, cx, cy)` → 項目 ID か null(`items` は `{id, placed, image, order}` の配列。枠 `placed` の矩形(端を含む)に (cx, cy) が入る項目のうち `order` の最大のもの)
- Produces(`assets-test/forbidden.mjs`。Node 専用): `checkJs(source)` → 違反の文字列の配列、`checkHtml(source)` → 違反の文字列の配列。規則は Global Constraints の禁止。判定はコメントを除いてから行う(`//` の行コメントと `/* */`。文字列の中の `//` は除かない簡易な処理でよいが、陰性の見本で誤検出しないこと)。HTML は `<` から `>` のタグの中だけを見て、大文字小文字を区別せず、属性名 `on[a-z]+`・`style`、`<style`、`src` の無い `<script`、`src` が `/` で始まらない、または `//` で始まる(同じオリジンでない)`<script` を違反とする。

- [ ] **Step 1: 失敗するテストを書く**

`assets-test/geom.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import * as g from "../src/assets/geom.js";

const W = 1920, H = 1080;
const p = (o) => ({ x: 100, y: 100, w: 200, h: 300, tail: null, start_offset_ms: 7, end_offset_ms: -3, ...o });
const box = (r) => ({ x: r.x, y: r.y, w: r.w, h: r.h });

test("fit_view_and_round_trip", () => {
  const v = g.fitView(1920, 1080, 960, 600);
  assert.deepEqual(v, { left: 0, top: 30, scale: 0.5 });
  const c = g.toCanvas(480, 300, v);
  assert.deepEqual(c, { x: 960, y: 540 });
  assert.deepEqual(g.toView(c.x, c.y, v), { x: 480, y: 300 });
  const tall = g.fitView(1920, 1080, 400, 1000); // 縦に長い箱: 横で決まる
  assert.equal(tall.scale, 400 / 1920);
  assert.equal(tall.left, 0);
});

test("clamp_rect", () => {
  assert.deepEqual(g.clampRect({ x: -5, y: 2000, w: 10.6, h: 0 }, W, H), { x: 0, y: 1079, w: 11, h: 1 });
  assert.deepEqual(g.clampRect({ x: 0, y: 0, w: 5000, h: 5000 }, W, H), { x: 0, y: 0, w: 1920, h: 1080 });
});

test("tail_center_uses_floor_like_the_server", () => {
  assert.deepEqual(g.tailCenter(p({ w: 201, h: 301 })), { x: 200, y: 250 });
  assert.deepEqual(g.tailTip(p({ tail: { dx: 5, dy: -7 } })), { x: 205, y: 243 });
  assert.equal(g.tailTip(p()), null);
});

test("drag_corner", () => {
  const s = p();
  assert.deepEqual(box(g.dragCorner(s, "se", 50, 20, W, H)), { x: 100, y: 100, w: 250, h: 320 });
  assert.deepEqual(box(g.dragCorner(s, "nw", 500, 500, W, H)), { x: 299, y: 399, w: 1, h: 1 });
  assert.deepEqual(box(g.dragCorner(s, "nw", -500, -500, W, H)), { x: 0, y: 0, w: 300, h: 400 });
  assert.deepEqual(box(g.dragCorner(s, "se", 5000, 5000, W, H)), { x: 100, y: 100, w: 1820, h: 980 });
  assert.deepEqual(box(g.dragCorner(s, "ne", 30, -40, W, H)), { x: 100, y: 60, w: 230, h: 340 });
  assert.deepEqual(box(g.dragCorner(s, "sw", -30, 40, W, H)), { x: 70, y: 100, w: 230, h: 340 });
  assert.equal(g.dragCorner(s, "se", 1, 1, W, H).start_offset_ms, 7);
});

test("drag_move_clamps_tail_tip", () => {
  const s = p({ tail: { dx: 0, dy: 200 } }); // 中心 (200,250)、先端 (200,450)
  const m = g.dragMove(s, 0, 2000, W, H);
  assert.equal(m.y, 780);
  assert.deepEqual(g.tailTip(m), { x: 200, y: 1080 }); // 先端は 0..=H に詰める
  assert.equal(m.end_offset_ms, -3);
});

test("drag_tail", () => {
  const s = p(); // 中心 (200, 250)
  assert.deepEqual(g.dragTail(s, 250.4, 600.6, W, H), { dx: 50, dy: 351 });
  assert.deepEqual(g.dragTail(s, -50, 5000, W, H), { dx: -200, dy: 830 }); // 先端 (0, 1080)
  assert.deepEqual(g.dragTail(s, 5000, -5, W, H), { dx: 1720, dy: -250 }); // 先端 (1920, 0)
});

test("nudge", () => {
  assert.equal(g.nudge(p(), "ArrowLeft", false, W, H).x, 99);
  assert.equal(g.nudge(p(), "ArrowDown", true, W, H).y, 110);
  assert.equal(g.nudge(p({ x: 0 }), "ArrowLeft", true, W, H).x, 0);
  assert.equal(g.nudge(p({ x: 1720 }), "ArrowRight", false, W, H).x, 1720);
  assert.equal(g.nudge(p(), "a", false, W, H), null);
});

test("hit_test_picks_topmost_under_the_point", () => {
  const items = [
    { id: "N", order: 0, placed: p({ x: 0, y: 0, w: 500, h: 500 }) },
    { id: "A", order: 2, placed: p({ x: 100, y: 100, w: 100, h: 100 }) },
    { id: "B", order: 1, placed: p({ x: 150, y: 150, w: 100, h: 100 }) },
  ];
  assert.equal(g.hitTest(items, 160, 160), "A"); // 3 つが重なる: order 最大
  assert.equal(g.hitTest(items, 240, 240), "B"); // A の外、B と N
  assert.equal(g.hitTest(items, 400, 400), "N");
  assert.equal(g.hitTest(items, 600, 600), null);
  assert.equal(g.hitTest(items, 200, 200), "A"); // 端を含む
});
```

`assets-test/forbidden.test.mjs`(検査そのものの見本):

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { checkJs, checkHtml } from "./forbidden.mjs";

test("forbidden_checker_detects_violations", () => {
  for (const bad of ["a.innerHTML = x", "e.outerHTML=1", "e.insertAdjacentHTML('a', s)", "document.write(s)",
    "eval(s)", "new Function('x')", "setTimeout('f()', 1)", "setInterval(\"f()\", 1)",
    "e.setAttribute('style', 'x')", "e.setAttribute(\"onclick\", f)", "e.style.cssText = s"]) {
    assert.ok(checkJs(bad).length > 0, bad);
  }
  for (const bad of ["<div onclick=\"x\">", "<DIV\nONLOAD=x>", "<p style = \"a\">", "<STYLE>p{}</STYLE>",
    "<script>alert(1)</script>", "<script src=\"https://cdn.example/x.js\"></script>", "<script src=\"//cdn.example/x.js\"></script>"]) {
    assert.ok(checkHtml(bad).length > 0, bad);
  }
});

test("forbidden_checker_allows_good_code", () => {
  for (const ok of ["e.style.left = '1px'", "e.textContent = s", "setTimeout(() => f(), 300)",
    "// innerHTML は使わない(コメント)", "/* eval( */ const a = 1;", "e.addEventListener('click', f)"]) {
    assert.deepEqual(checkJs(ok), [], ok);
  }
  for (const ok of ["<script type=\"module\" src=\"/app.js\"></script>", "<link rel=\"stylesheet\" href=\"/app.css\">",
    "<p>style や onclick という言葉は本文なら可</p>", "<button id=\"save\" type=\"button\">保存</button>"]) {
    assert.deepEqual(checkHtml(ok), [], ok);
  }
});

test("assets_have_no_dangerous_apis", () => {
  const dir = new URL("../src/assets/", import.meta.url);
  for (const f of readdirSync(dir)) {
    const s = readFileSync(new URL(f, dir), "utf8");
    if (f.endsWith(".js")) assert.deepEqual(checkJs(s), [], f);
    if (f.endsWith(".html")) assert.deepEqual(checkHtml(s), [], f);
  }
});
```

`tests/node_assets.rs`:

```rust
//! 画面の JS のテスト(Node の node:test。禁止の検査も含む)を cargo test から呼ぶ(設計書 §5.3・§8)。
//! Node が無ければ失敗する。S2V_SKIP_NODE_TESTS=1 のときだけ飛ばす。

#[test]
fn node_unit_tests_pass() {
    if std::env::var("S2V_SKIP_NODE_TESTS").as_deref() == Ok("1") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("node")
        .args(["--test", "assets-test/"])
        .current_dir(dir)
        .output()
        .expect("node を起動できません(PATH に Node が必要です。飛ばすなら S2V_SKIP_NODE_TESTS=1)");
    assert!(
        out.status.success(),
        "node --test が失敗しました\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
```

注: 禁止の検査を Node の側に置くので、`S2V_SKIP_NODE_TESTS=1` のときは禁止の検査も飛ぶ。ただし既存の Rust のテスト `embedded_assets_have_no_inner_html`(2d-1)は残し、全ての `.js` を対象に広げる(Node が無くても最低限の検査が残る)。

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-bubbles --test node_assets` / Expected: FAIL(geom.js・forbidden.mjs が無い)。
- [ ] **Step 3: 実装する** — `geom.js`・`forbidden.mjs` を書く。`server.rs` の埋め込みを表にして `/geom.js` を足す。
- [ ] **Step 4: 通ることを確かめる** — Run: `node --test crates/s2v-bubbles/assets-test/`、`cargo test -p s2v-bubbles`
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面の座標とドラッグの計算、Node のテストの仕組み、禁止の検査を加える`

---

### Task 2: `timeline.js`(コマから表示、素材の中のコマ、再生の状態遷移)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/timeline.js`、`crates/s2v-bubbles/assets-test/timeline.test.mjs`
- Modify: `crates/s2v-bubbles/src/server.rs`(表に `/timeline.js`)

**Interfaces:**
- Consumes: `/api/state` の JSON(`scenes[].cuts[].{id, frames:[n0,n1], asset:{url, kind:"png"|"jpeg"|"video", looped, frames, frame_url}|null}`、`items[].{id, kind, frames:[s,e], order, placed, image, clipped, manual}`、`fps`、`total_frames`)。
- Produces(純粋。`export`):
  - `cutAt(state, n)` → カットか null(`n0 ≤ n < n1`。長さ 0 のカットは選ばない)
  - `itemsAt(items, n)` → `s ≤ n < e` の項目を `order` の小さい順に
  - `assetFrame(n, n0, frames, looped)` → k(Global Constraints の式)
  - `frameFromAudio(sec, fps, total)` → n、`sliderMax(total)` → `max(total − 1, 0)`
  - `playerStep(st, ev)` → `{st, actions}`。状態 `st = {playing, n, cutId}`。出来事 `ev`:
    - `{type:"play"}`: `playing = true`。actions: `audio-seek(n/fps)`、`audio-play`、今のカットが動画なら `video-seek(k/fps)` と(止める素材の最後でなければ)`video-play`。
    - `{type:"stop"}`: `playing = false`。actions: `audio-pause`、`video-pause`、今のカットが動画なら `video-seek(k/fps)` と `wait-seeked`。
    - `{type:"seek", n}`(スライダー・一覧): n とカットを新しい値にする。再生中なら `audio-seek(n/fps)`、カットが変われば `cut-change`、新しいカットが動画なら `video-seek(k/fps)` と、止める素材の最後なら `video-pause`、それ以外は `video-play`。停止中なら `cut-change`(変われば)と、動画なら `video-seek(k/fps)`・`wait-seeked`。
    - `{type:"tick", audioSec, videoSec, ended}`(再生中の `requestAnimationFrame`): `ended` なら `playing = false`、`n = total − 1`、カット = `cutAt(total − 1)`、actions は `audio-pause`・`video-pause`、カットが変わっていれば `cut-change`、新しいカットが動画なら `video-seek(k/fps)`(k はコマ `total − 1` の素材の中のコマ)・`wait-seeked`。そうでなければ `n = frameFromAudio(audioSec)`。カットが変われば `cut-change` と、動画なら `video-seek(k/fps)`・`video-play`(止める素材の最後なら `video-pause` と最後のコマへの `video-seek`)。カットが同じで動画なら: 止める素材の最後(`!looped` かつ `d ≥ frames − 1`)で `video-pause` と最後のコマへの `video-seek`、それ以外で `|videoSec − k/fps| ≥ 2/fps − 1e-6` なら `video-seek(k/fps)`(と再生中なら `video-play`)。
    - 動画の素材は常に消音(`player.js` が `muted = true` にする)。actions は配列で、`{type:"audio-seek", time}`・`{type:"audio-play"}`・`{type:"audio-pause"}`・`{type:"video-seek", time}`・`{type:"video-play"}`・`{type:"video-pause"}`・`{type:"wait-seeked"}`・`{type:"cut-change", cutId}`。
  - `playerStep` は `state`(`/api/state`)を `st` とは別の引数で受ける: `playerStep(state, st, ev)`。

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/timeline.test.mjs`)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import * as t from "../src/assets/timeline.js";

const state = {
  fps: 30, total_frames: 300,
  scenes: [{ id: "S01", cuts: [
    { id: "C1", frames: [0, 100], asset: { kind: "png" } },
    { id: "C0", frames: [100, 100], asset: null },
    { id: "C3", frames: [100, 300], asset: { kind: "video", looped: false, frames: 60 } },
  ] }],
};
const types = (r) => r.actions.map((a) => a.type);

test("cut_and_items_at_boundaries", () => {
  assert.equal(t.cutAt(state, 99).id, "C1");
  assert.equal(t.cutAt(state, 100).id, "C3");
  assert.equal(t.cutAt(state, 299).id, "C3");
  assert.equal(t.cutAt(state, 300), null);
  const items = [{ id: "L2", frames: [50, 100], order: 2 }, { id: "N1", frames: [0, 100], order: 0 }, { id: "L1", frames: [99, 150], order: 1 }];
  assert.deepEqual(t.itemsAt(items, 99).map((i) => i.id), ["N1", "L1", "L2"]);
  assert.deepEqual(t.itemsAt(items, 100).map((i) => i.id), ["L1"]);
  assert.deepEqual(t.itemsAt(items, 49).map((i) => i.id), ["N1"]);
});

test("asset_frame_matches_the_server", () => {
  // crates/s2v-video/src/frames.rs の asset_frame_freezes_or_loops と同じ値
  assert.equal(t.assetFrame(100, 100, 60, false), 0);
  assert.equal(t.assetFrame(159, 100, 60, false), 59);
  assert.equal(t.assetFrame(160, 100, 60, false), 59);
  assert.equal(t.assetFrame(300, 100, 60, false), 59);
  assert.equal(t.assetFrame(160, 100, 60, true), 0);
  assert.equal(t.assetFrame(225, 100, 60, true), 5);
  assert.equal(t.assetFrame(105, 100, 0, false), 0);
  assert.equal(t.assetFrame(90, 100, 60, false), 0); // d = max(n − n0, 0)
});

test("frame_from_audio_and_slider", () => {
  assert.equal(t.frameFromAudio(0, 30, 300), 0);
  assert.equal(t.frameFromAudio(1 / 30 - 1e-6, 30, 300), 0);
  assert.equal(t.frameFromAudio(1 / 30, 30, 300), 1);
  assert.equal(t.frameFromAudio(3 * (1 / 30), 30, 300), 3); // 浮動小数の誤差でも 3
  assert.equal(t.frameFromAudio(999, 30, 300), 299);
  assert.equal(t.frameFromAudio(-1, 30, 300), 0);
  assert.equal(t.sliderMax(300), 299);
  assert.equal(t.sliderMax(0), 0);
});

test("player_transitions", () => {
  // 停止中に C3 の n=110 へ: シークして seeked を待つ
  let r = t.playerStep(state, { playing: false, n: 0, cutId: "C1" }, { type: "seek", n: 110 });
  assert.deepEqual(types(r), ["cut-change", "video-seek", "wait-seeked"]);
  assert.deepEqual(r.actions[1], { type: "video-seek", time: 10 / 30 });
  // 位置の差がゼロでも再生を始める(play は位置差で判定しない)
  r = t.playerStep(state, r.st, { type: "play" });
  assert.deepEqual(types(r), ["audio-seek", "audio-play", "video-seek", "video-play"]);
  assert.equal(r.st.playing, true);
  // 再生中: 1 コマのずれは何もしない、2 コマ(浮動小数の誤差つき)で合わせ直す
  const st = { playing: true, n: 110, cutId: "C3" };
  r = t.playerStep(state, st, { type: "tick", audioSec: 110 / 30, videoSec: 10 / 30 + 1 / 30, ended: false });
  assert.deepEqual(types(r), []);
  r = t.playerStep(state, st, { type: "tick", audioSec: 110 / 30, videoSec: 10 / 30 + 2 / 30, ended: false });
  assert.deepEqual(types(r), ["video-seek", "video-play"]);
  // 止める素材の最後以降は一時停止して最後のコマ
  r = t.playerStep(state, { playing: true, n: 199, cutId: "C3" }, { type: "tick", audioSec: 200 / 30, videoSec: 1.9, ended: false });
  assert.deepEqual(types(r), ["video-pause", "video-seek"]);
  assert.deepEqual(r.actions[1], { type: "video-seek", time: 59 / 30 });
  // カットの切り替え(C1 → C3)
  r = t.playerStep(state, { playing: true, n: 99, cutId: "C1" }, { type: "tick", audioSec: 100 / 30, videoSec: 0, ended: false });
  assert.deepEqual(types(r), ["cut-change", "video-seek", "video-play"]);
  // 停止: 音声と動画を止め、今のコマへシークして待つ
  r = t.playerStep(state, { playing: true, n: 120, cutId: "C3" }, { type: "stop" });
  assert.deepEqual(types(r), ["audio-pause", "video-pause", "video-seek", "wait-seeked"]);
  assert.equal(r.st.playing, false);
  // 音声の終わり: 停止して最後のコマ。C1(n=99)から終わっても、最後のコマの C3 に切り替えて k = min(299−100, 59) = 59 で止まる
  r = t.playerStep(state, { playing: true, n: 99, cutId: "C1" }, { type: "tick", audioSec: 10, videoSec: 0, ended: true });
  assert.equal(r.st.playing, false);
  assert.equal(r.st.n, 299);
  assert.equal(r.st.cutId, "C3");
  assert.deepEqual(types(r), ["audio-pause", "video-pause", "cut-change", "video-seek", "wait-seeked"]);
  assert.deepEqual(r.actions[3], { type: "video-seek", time: 59 / 30 });
  // 再生中のシーク(C1 → C3 の先頭): 動画もシークして再生する
  r = t.playerStep(state, { playing: true, n: 50, cutId: "C1" }, { type: "seek", n: 100 });
  assert.deepEqual(types(r), ["audio-seek", "cut-change", "video-seek", "video-play"]);
  assert.equal(r.st.cutId, "C3");
  // 停止中の連続シーク: それぞれシークして待つ
  r = t.playerStep(state, { playing: false, n: 120, cutId: "C3" }, { type: "seek", n: 121 });
  assert.deepEqual(types(r), ["video-seek", "wait-seeked"]);
  // ループする素材
  const loop = { ...state, scenes: [{ id: "S", cuts: [{ id: "L", frames: [100, 300], asset: { kind: "video", looped: true, frames: 60 } }] }] };
  r = t.playerStep(loop, { playing: true, n: 164, cutId: "L" }, { type: "tick", audioSec: 165 / 30, videoSec: 1.9, ended: false });
  assert.deepEqual(r.actions[0], { type: "video-seek", time: 5 / 30 });
});
```

注: テストの `videoSec: 10/30 + 2/30` は `0.39999999999999997` になり、`10/30` との差 `0.06666666666666665` が `2/30`(`0.06666666666666667`)よりわずかに小さい(Node で確認済み)。Global Constraints の `− 1e-6` の許容で合わせ直しの対象になることを確かめる。

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、`node --test crates/s2v-bubbles/assets-test/` で通す。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面のコマと表示、再生の状態遷移を加える`

---

### Task 3: `model.js`(手動配置、確定の待ち行列、保存、layout の反映、PNG の世代、frame の集約、PNG の上限)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/model.js`、`crates/s2v-bubbles/assets-test/model.test.mjs`
- Modify: `crates/s2v-bubbles/src/server.rs`(表に `/model.js`)

**Interfaces:**
- Consumes: Task 4 の `api.js` が返す形(このタスクでは偽の API を使う): `render(body)` → `{ok:true, png:Blob|Uint8Array, origin:[dx,dy], clipped:bool}` か `{ok:false, status, errors:[文字列]}`、`layout(body)` → `{ok:true, items:[…]}` か `{ok:false, status, errors}`、`save(body)` → `{ok:true, warnings:[…]}` か `{ok:false, status, errors}`。
- Produces(`export`。DOM・fetch に触れない):
  - `withKind(manual, items)` → 要求用の一式(各配置に `kind` を足した新しいオブジェクト。元は変えない。配置のキーは `x, y, w, h, tail, start_offset_ms, end_offset_ms, kind` だけにする)
  - `validateOffset(v)` → `{ok:true, value}` か `{ok:false, reason}`(整数・`|v| ≤ 60000`。数字の文字列も受ける)
  - `changedImages(oldItems, newItems)` → PNG を捨てる ID(`placed` の x・y・w・h・tail、`clipped`、`image` のどれかが違う項目と、新しく現れた項目)
  - `class Editor`(`constructor(api, state)`):
    - `items`(表示の正本。起動時は `state.items`)、`confirmed`(確定済みの一式。起動時は `state.items[].manual` が null でないものから作る)、`saved`(保存済みの一式の正規化した JSON 文字列。起動時は `confirmed` と同じ)
    - `addPendingFlusher(fn)`(`fn() → Promise`。保存・終了の前に呼ぶ保留中の入力の確定。Task 7 が矢印キーと数値の欄を登録する)
    - `enqueue(id, placementOrNull)` → Promise<`{ok:true, changed:[ID]}` | `{rejected:true, id, errors}`>(1 つずつ処理。一式 = `confirmed` の写し + この編集(null なら ID を消す)。`render({id, manual: withKind(一式, items)})` が ok → `layout({manual: withKind(一式, items)})` が ok のときだけ `confirmed = 一式`、`items = layout の items`、`changed = changedImages(古い items, 新しい items)`。null(自動に戻す)のときも render(その項目、自動の配置)と layout の両方を呼ぶ。どちらかが ok でなければ何も変えずに `rejected`)
    - `lastQueued(id)` → 最後に待ち行列へ入れた値(処理中・未処理を含む。無ければ `confirmed[id]`。自動に戻したなら null)
    - `flush()` → 待ち行列が空になるまで待つ
    - `save()` → Promise(全ての `addPendingFlusher` の関数を呼んで待つ → `flush()` → `confirmed` の写しを送る → ok なら `saved = 送った写し`)
    - `isDirty()` → `confirmed` と `saved` が違えば true
  - `class FrameFetcher`(`constructor(fetchFrame)`。`fetchFrame(cutId, n) → Promise<{ok, blob?}>`): `setCurrent(cutId, n)`、`request(cutId, n, onResult)`(同時に1件だけ。処理中の間の要求は最新の1件だけを残して1件目の後に送る。応答は、そのときの `setCurrent` の値と一致するときだけ `onResult(blob)` を呼ぶ)
  - `class ImageCache`(`constructor(max = 300, revoke)`。項目ごとの版で古い応答を捨てる): `version(id)` → 整数(初めは 0)、`get(id)` → URL か undefined(使った順を更新)、`set(id, url, requestedVersion)` → 真偽(`requestedVersion` が今の `version(id)` と同じときだけ登録して true。違えば登録せず `revoke(url)` して false。上限を超えたら使った順の古いものを捨てて `revoke`。同じ ID の置き換えでも古い URL を `revoke`)、`drop(ids)`(各 ID の URL を `revoke` して消し、`version` を 1 増やす)。変わっていない項目は `drop` されないので、その URL は使い続ける

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/model.test.mjs`)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { Editor, FrameFetcher, ImageCache, withKind, validateOffset, changedImages } from "../src/assets/model.js";

const P = (o = {}) => ({ x: 0, y: 0, w: 100, h: 100, tail: null, start_offset_ms: 0, end_offset_ms: 0, ...o });
const item = (id, kind, placed, extra = {}) => ({ id, kind, frames: [0, 10], order: 0, placed, image: { dx: 0, dy: 0, w: placed.w, h: placed.h }, clipped: false, manual: null, ...extra });
const baseItems = () => [item("A", "bubble", P()), item("B", "note", P({ x: 200 }))];

// 手で解決できる Promise
function deferred() { let resolve; const p = new Promise((r) => (resolve = r)); return { p, resolve }; }
const tick = () => new Promise((r) => setImmediate(r));

test("with_kind", () => {
  const m = { A: P() };
  const r = withKind(m, baseItems());
  assert.equal(r.A.kind, "bubble");
  assert.equal(m.A.kind, undefined);
  assert.deepEqual(Object.keys(r.A).sort(), ["end_offset_ms", "h", "kind", "start_offset_ms", "tail", "w", "x", "y"]);
});

test("validate_offset", () => {
  assert.deepEqual(validateOffset("120"), { ok: true, value: 120 });
  assert.deepEqual(validateOffset(-60000), { ok: true, value: -60000 });
  for (const bad of [60001, "1.5", "abc", "", null]) assert.equal(validateOffset(bad).ok, false, String(bad));
});

test("starts_from_existing_manual", () => {
  const items = baseItems();
  items[0].manual = P({ x: 5 });
  const ed = new Editor({}, { items });
  assert.equal(ed.confirmed.A.x, 5);
  assert.equal(ed.isDirty(), false);
});

test("queue_rejects_first_and_commits_second_without_it", async () => {
  // Review Focus 1
  const calls = [];
  const api = {
    render: async (b) => { calls.push(["render", b]); return b.id === "A" ? { ok: false, status: 422, errors: ["収まりません"] } : { ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }; },
    layout: async (b) => { calls.push(["layout", b]); return { ok: true, items: baseItems() }; },
  };
  const ed = new Editor(api, { items: baseItems() });
  const a = ed.enqueue("A", P({ w: 1, h: 1 }));
  const b = ed.enqueue("B", P({ x: 300 }));
  const ra = await a, rb = await b;
  assert.equal(ra.rejected, true);
  assert.deepEqual(ra.errors, ["収まりません"]);
  assert.equal(rb.ok, true);
  assert.equal(ed.confirmed.A, undefined);
  assert.equal(ed.confirmed.B.x, 300);
  // A の render の後に layout は呼ばれない。B の render と layout はどちらも A を含まない
  assert.deepEqual(calls.map(([k, b]) => k + ":" + (b.id || "")), ["render:A", "render:B", "layout:"]);
  for (const [, body] of calls.slice(1)) assert.equal(body.manual.A, undefined);
});

test("queue_processes_one_at_a_time", async () => {
  let running = 0, max = 0;
  const d = [deferred(), deferred()];
  let i = 0;
  const api = {
    render: async () => { running++; max = Math.max(max, running); await d[i++].p; running--; return { ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }; },
    layout: async () => ({ ok: true, items: baseItems() }),
  };
  const ed = new Editor(api, { items: baseItems() });
  const a = ed.enqueue("A", P({ x: 1 })), b = ed.enqueue("B", P({ x: 2 }));
  await tick(); d[0].resolve(); await a; await tick(); d[1].resolve(); await b;
  assert.equal(max, 1);
});

test("layout_replaces_all_items_and_drops_images", async () => {
  const after = baseItems();
  after[0].placed = P({ x: 50 }); after[0].frames = [3, 9]; after[0].order = 1; // A の位置・表示時刻・順が変わる
  const api = { render: async () => ({ ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }), layout: async () => ({ ok: true, items: after }) };
  const ed = new Editor(api, { items: baseItems() });
  const r = await ed.enqueue("A", P({ x: 50 }));
  assert.deepEqual(r.changed, ["A"]);
  assert.deepEqual(ed.items[0].frames, [3, 9]);
  assert.equal(ed.items[0].order, 1);
  assert.equal(ed.isDirty(), true);
});

test("layout_failure_after_render_success_rejects", async () => {
  const api = { render: async () => ({ ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }), layout: async () => ({ ok: false, status: 422, errors: ["逆転"] }) };
  const ed = new Editor(api, { items: baseItems() });
  const r = await ed.enqueue("A", P({ start_offset_ms: 9999 }));
  assert.equal(r.rejected, true);
  assert.equal(ed.confirmed.A, undefined);
  assert.equal(ed.items[0].placed.x, 0); // 表示の正本も変えない
});

test("revert_to_auto_removes_from_manual", async () => {
  const items = baseItems(); items[0].manual = P({ x: 5 });
  const bodies = [];
  const api = { render: async (b) => { bodies.push(b); return { ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }; }, layout: async (b) => { bodies.push(b); return { ok: true, items: baseItems() }; } };
  const ed = new Editor(api, { items });
  const r = await ed.enqueue("A", null);
  assert.equal(r.ok, true);
  assert.equal(ed.confirmed.A, undefined);
  for (const b of bodies) assert.equal(b.manual.A, undefined);
  assert.equal(ed.isDirty(), true);
});

test("last_queued_includes_pending", async () => {
  const d = deferred();
  const api = { render: async () => { await d.p; return { ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }; }, layout: async () => ({ ok: true, items: baseItems() }) };
  const ed = new Editor(api, { items: baseItems() });
  const q = ed.enqueue("A", P({ x: 9 }));
  assert.equal(ed.lastQueued("A").x, 9);   // 処理中
  assert.equal(ed.confirmed.A, undefined);
  d.resolve(); await q;
  assert.equal(ed.lastQueued("A").x, 9);   // 確定後は confirmed
});

test("save_marks_only_sent_snapshot_as_saved", async () => {
  // Review Focus 2
  const sd = deferred();
  const saves = [];
  const api = {
    render: async () => ({ ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }),
    layout: async () => ({ ok: true, items: baseItems() }),
    save: async (b) => { saves.push(b); await sd.p; return { ok: true, warnings: [] }; },
  };
  const ed = new Editor(api, { items: baseItems() });
  await ed.enqueue("A", P({ x: 10 }));
  const saving = ed.save();
  await tick(); await tick();
  assert.equal(saves.length, 1);
  await ed.enqueue("B", P({ x: 400 }));   // 保存の送信中の編集
  sd.resolve(); await saving;
  assert.equal(saves[0].manual.A.x, 10);
  assert.equal(saves[0].manual.B, undefined);
  assert.equal(ed.isDirty(), true);
});

test("save_runs_pending_flushers_first", async () => {
  // 数値の欄に入力した直後の Ctrl+S: 保留中の入力の確定が保存の前に待ち行列に入り、保存に含まれる
  const saves = [];
  const api = { render: async () => ({ ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }), layout: async () => ({ ok: true, items: baseItems() }), save: async (b) => { saves.push(b); return { ok: true, warnings: [] }; } };
  const ed = new Editor(api, { items: baseItems() });
  ed.addPendingFlusher(() => ed.enqueue("A", P({ x: 77 })));
  await ed.save();
  assert.equal(saves[0].manual.A.x, 77);
  assert.equal(ed.isDirty(), false);
});

test("changed_images", () => {
  const old = baseItems(), nw = baseItems();
  nw[1].placed.x = 250;
  nw.push(item("C", "bubble", P()));
  assert.deepEqual(changedImages(old, nw).sort(), ["B", "C"]);
});

test("frame_requests_coalesce_and_drop_stale", async () => {
  // Review Focus 4
  const d = [];
  const asked = [];
  const ff = new FrameFetcher((cut, n) => { asked.push(n); const x = deferred(); d.push(x); return x.p; });
  const got = [];
  ff.setCurrent("C", 10);
  ff.request("C", 10, (b) => got.push(["10", b]));
  ff.setCurrent("C", 13);
  ff.request("C", 11, (b) => got.push(["11", b]));
  ff.request("C", 12, (b) => got.push(["12", b]));
  ff.request("C", 13, (b) => got.push(["13", b]));
  assert.deepEqual(asked, [10]);                     // 同時に1件
  d[0].resolve({ ok: true, blob: "b10" }); await tick(); await tick();
  assert.deepEqual(asked, [10, 13]);                 // 待っていた中の最新だけ
  assert.deepEqual(got, []);                         // 10 は今の n(13)と違うので捨てる
  d[1].resolve({ ok: true, blob: "b13" }); await tick(); await tick();
  assert.deepEqual(got, [["13", "b13"]]);
});

test("stale_png_is_not_registered", () => {
  // A の PNG を要求した後(版 0)、確定で A が変わり drop された(版 1)。遅れて返った版 0 の応答は登録しない
  const revoked = [];
  const c = new ImageCache(10, (u) => revoked.push(u));
  const v0 = c.version("A");
  c.drop(["A"]);
  assert.equal(c.set("A", "old", v0), false);
  assert.equal(c.get("A"), undefined);
  assert.deepEqual(revoked, ["old"]);
  assert.equal(c.set("A", "new", c.version("A")), true);
  assert.equal(c.get("A"), "new");
});

test("unchanged_images_are_reused_after_commit", async () => {
  // 確定で B だけが変わったとき、A の PNG は捨てずに使い続ける
  const after = baseItems(); after[1].placed = P({ x: 260 });
  const api = { render: async () => ({ ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }), layout: async () => ({ ok: true, items: after }) };
  const ed = new Editor(api, { items: baseItems() });
  const c = new ImageCache(10, () => {});
  c.set("A", "uA", c.version("A")); c.set("B", "uB", c.version("B"));
  const r = await ed.enqueue("B", P({ x: 260 }));
  c.drop(r.changed);
  assert.equal(c.get("A"), "uA");
  assert.equal(c.get("B"), undefined);
});

test("image_cache_limit_and_revoke", () => {
  const revoked = [];
  const c = new ImageCache(2, (u) => revoked.push(u));
  c.set("A", "u1", 0); c.set("B", "u2", 0); c.get("A"); c.set("C", "u3", 0);
  assert.deepEqual(revoked, ["u2"]);
  c.set("A", "u4", 0);
  assert.deepEqual(revoked, ["u2", "u1"]);
  c.drop(["C"]);
  assert.deepEqual(revoked, ["u2", "u1", "u3"]);
});
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面の手動配置・確定の待ち行列・保存・PNG の世代と上限・frame の集約を加える`

---

### Task 4: `api.js`(fetch の変換と 403)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/api.js`、`crates/s2v-bubbles/assets-test/api.test.mjs`
- Modify: `crates/s2v-bubbles/src/server.rs`(表に `/api.js`)

**Interfaces:**
- Produces: `createApi(fetchFn, onSessionLost)` → `{getState(), render(body), layout(body), save(body), frame(cutId, n), head(url), quit()}`。`fetchFn` は `fetch` と同じ形(テストで差し込む)。全ての要求は `credentials: "same-origin"`、POST は `Content-Type: application/json`。
  - 変換: `render` の 200 → `{ok:true, png: await r.blob(), origin:[dx,dy](X-S2V-Origin を数値に), clipped: X-S2V-Clipped === "1"}`。`layout` の 200 → `{ok:true, items: 本文の配列}`。`save` の 200 → `{ok:true, warnings}`。`getState` の 200 → JSON。`frame` の 200 → `{ok:true, blob}`。`head(url)` → 状態コードの数値(`method: "HEAD"`)。
  - エラー: 422 → `{ok:false, status:422, errors: 本文.errors}`。その他(400・404・409・500・503)→ `{ok:false, status, errors:[本文.error か "通信に失敗しました(状態コード)"]}`。403 → `onSessionLost()` を呼び、`{ok:false, status:403, errors:["セッションが切れました。bubbles を起動し直してください"]}`。ネットワークの失敗(fetch の reject)→ `{ok:false, status:0, errors:["サーバーに接続できません"]}`。

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/api.test.mjs`。実際のサーバーの応答の形の見本を偽の fetch で返す)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { createApi } from "../src/assets/api.js";

const resp = (status, body, headers = {}) => new Response(typeof body === "string" ? body : JSON.stringify(body), { status, headers });

test("api_render_converts_headers", async () => {
  const api = createApi(async () => new Response(new Uint8Array([137, 80, 78, 71]), { status: 200, headers: { "Content-Type": "image/png", "X-S2V-Origin": "-3,12", "X-S2V-Clipped": "1" } }), () => {});
  const r = await api.render({ id: "A", manual: {} });
  assert.equal(r.ok, true);
  assert.deepEqual(r.origin, [-3, 12]);
  assert.equal(r.clipped, true);
  assert.equal((await r.png.arrayBuffer()).byteLength, 4);
});

test("api_layout_returns_items_array", async () => {
  const api = createApi(async () => resp(200, [{ id: "A" }]), () => {});
  assert.deepEqual(await api.layout({ manual: {} }), { ok: true, items: [{ id: "A" }] });
});

test("api_errors_are_normalized", async () => {
  let api = createApi(async () => resp(422, { errors: ["a", "b"] }), () => {});
  assert.deepEqual(await api.save({ manual: {} }), { ok: false, status: 422, errors: ["a", "b"] });
  api = createApi(async () => resp(503, { error: "混んでいます" }), () => {});
  assert.deepEqual(await api.frame("C", 3), { ok: false, status: 503, errors: ["混んでいます"] });
  api = createApi(async () => { throw new TypeError("network"); }, () => {});
  assert.deepEqual(await api.getState().catch((e) => e), { ok: false, status: 0, errors: ["サーバーに接続できません"] });
});

test("api_session_lost_on_403", async () => {
  let lost = 0;
  const api = createApi(async () => resp(403, { error: "x" }), () => lost++);
  const r = await api.layout({ manual: {} });
  assert.equal(r.status, 403);
  assert.equal(lost, 1);
  assert.equal(await api.head("/media/asset/C"), 403);
});

test("api_requests_use_same_origin_and_json", async () => {
  const seen = [];
  const api = createApi(async (url, init) => { seen.push([url, init]); return resp(200, { warnings: [] }); }, () => {});
  await api.save({ manual: { A: { x: 1 } } });
  const [url, init] = seen[0];
  assert.equal(url, "/api/save");
  assert.equal(init.method, "POST");
  assert.equal(init.credentials, "same-origin");
  assert.equal(init.headers["Content-Type"], "application/json");
  assert.deepEqual(JSON.parse(init.body), { manual: { A: { x: 1 } } });
});
```

`getState` の失敗は例外ではなく同じ形の値を返す(テストは `.catch` を付けているが、返り値でもよい。実装でどちらかに決め、テストをそれに合わせる)。

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面の通信の変換と 403 の扱いを加える`

---

### Task 5: 表示(一覧・プレビュー・スライダー・状態)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/view.js`
- Modify: `crates/s2v-bubbles/src/assets/app.js`(仮の画面を置き換える)、`index.html`(`<script type="module" src="/app.js">`、一覧・プレビュー・数値の欄・スライダー・ボタンの骨組み)、`app.css`、`crates/s2v-bubbles/src/server.rs`(表に `/view.js`)、`crates/s2v-bubbles/tests/http.rs`

**Interfaces:**
- Consumes: Task 1〜4。「DOM のモジュールの契約」の `mountView`。
- 振る舞い(設計書 §5.3):
  - 起動: `createApi(fetch, onSessionLost)` → `getState()` → `Editor`・`ImageCache(300, URL.revokeObjectURL)`・`FrameFetcher(api.frame)` を作り、一覧・スライダー(`max = sliderMax`)を描く。
  - コマの描画 `redraw(n)`: `cutAt` → 背景(画像は `<img>` に `fit` を `object-fit` で、余白の色を `background`、動画は `<video muted playsinline preload="auto">`、素材なしは「素材なし」の文字)→ `itemsAt(editor.items, n)` を `order` の順に、各項目の PNG(`cache.get(id)`。無ければ `v = cache.version(id)` を覚えて `api.render({id, manual: withKind(editor.confirmed, editor.items)})` を要求し、返ったとき `cache.set(id, URL.createObjectURL(png), v)` が true なら描き直す。PNG の原点のずれは要求の応答の `origin` ではなく `editor.items` の `image` を使う(layout の結果と同じ値))を `placed.x + image.dx` などの位置に重ねる(位置と大きさは `fitView` の scale を掛けて `element.style.left` などで設定)。
  - 素材の読み込みエラー: `<img>`・`<video>` の `error` で `api.head(url)` → 403 ならセッション切れの表示、動画ならそのカットを frame に切り替え(`frames.setCurrent` と `frames.request` で `<img>` に blob を表示)、frame も失敗なら「プレビューできません」。
  - セッション切れ: 状態の表示に文を出し、以降の要求と再生を止める(`ui.sessionLost = true`)。
  - 一覧: シーン → カット → 項目。手動配置の印(`editor.confirmed` にある)・切り詰めの印(`clipped`)。台本の文字は `textContent` だけ。
- [ ] **Step 1: テストを書く** — 2d-1 の既存のテスト `embedded_assets_have_no_inner_html`(`<script src="/app.js"` を要求している)を、`<script type="module" src="/app.js">` を要求するように直す。`tests/http.rs` に `embedded_modules_are_served`(`/geom.js`・`/timeline.js`・`/model.js`・`/api.js`・`/view.js`・`/app.js` が `text/javascript; charset=utf-8` で 200(クッキーあり)、クッキーなしで 403)と `index_loads_app_as_module`(`/` の本文に `type="module"` と `src="/app.js"`)。
- [ ] **Step 2〜4**: 実装し、`cargo test -p s2v-bubbles` で通す。手で `script2video bubbles <sample> --no-open` を起動して開き、開発者ツールのコンソールに CSP の違反・エラーが無いこと、全てのモジュールが読み込まれたこと(ネットワークの表)を報告に書く。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面に一覧・プレビュー・スライダーを加える`

---

### Task 6: 再生(`player.js`)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/player.js`
- Modify: `app.js`(結線)、`view.js`(再生/停止のボタン)、`server.rs`(表に `/player.js`)

**Interfaces:**
- Consumes: Task 2 の `playerStep`、Task 3 の `FrameFetcher`、Task 5。「DOM のモジュールの契約」の `createPlayer`。
- 振る舞い: `<audio src="/media/audio" preload="auto">`。`play()`・`stop()`・`seek(n)` は `playerStep` に出来事を渡し、返った actions を順に DOM に適用する(`video-seek` は `video.currentTime = time`、`wait-seeked` は `seeked` の出来事を待ってから `redraw`。`cut-change` は背景を替える)。再生中は `requestAnimationFrame` で `tick`(`audio.currentTime`・`video.currentTime`・`audio.ended`)。動画は常に `muted = true`。frame に切り替えたカットでは、停止中のコマの移動で `FrameFetcher.request` を使い、再生中は frame を要求しない(最後に表示した画像のまま)。ドラッグの開始で再生を止める(Task 7 が `stop()` を呼ぶ)。
- [ ] **Step 1〜4**: 実装し、`cargo test -p s2v-bubbles` で通す。手で、動画の素材の台本(ループする・止める)で再生・停止・スライダーを操作し、動画の音が出ないこと、止めたコマで止まること、ループ・止めるの見え方を報告に書く。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面に再生と動画の素材の同期を加える`

---

### Task 7: 編集の操作(`editor_ui.js`)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/editor_ui.js`
- Modify: `app.js`(結線)、`view.js`(つまみ・輪郭・数値の欄)、`app.css`、`server.rs`(表に `/editor_ui.js`)

**Interfaces:**
- Consumes: Task 1(`dragMove`・`dragCorner`・`dragTail`・`nudge`・`hitTest`・`toCanvas`・`fitView`)、Task 3(`Editor.enqueue`・`lastQueued`・`addPendingFlusher`、`validateOffset`)、Task 5・6。「DOM のモジュールの契約」の `attachEditing`。
- 振る舞い(設計書 §5.3「操作」「キーとドラッグ」):
  - 選択: 一覧のクリックはその項目の表示開始のコマへ移動(`player.seek`)して選択。プレビューのクリックは `hitTest(表示中の項目, canvas の座標)`。空白なら選択を外す。
  - ドラッグ: 選択中の項目の本体・四隅のつまみ・尻尾の先端のつまみで `pointerdown` → `setPointerCapture`、再生中なら `player.stop()`、輪郭の要素だけを動かす(ドラッグ中は選択とコマを変えない)。`pointerup` で `enqueue(id, 新しい配置)`。`pointercancel` で輪郭を捨てて元に戻す(確定しない)。
  - 矢印キー: `document` の `keydown`。フォーカスが `input`・`textarea`・`button`・`select` のときは何もしない(既定の動作を優先)。それ以外で選択中なら `nudge` して輪郭を動かし、`preventDefault`。最後の押下から 300ms 後に `enqueue`。待ちの確定は `addPendingFlusher` に登録する(保存・終了の前に即座に確定)。
  - 数値の欄: x・y・w・h・尻尾 dx・dy・開始と終了の offset。Enter(`keydown`)と `change` の両方で確定を試み、値が `lastQueued(id)` と同じなら何もしない。offset は `validateOffset`(不正なら理由を出して欄を戻す)。入力中の欄の確定も `addPendingFlusher` に登録する。
  - 確定の結果: `rejected` なら理由を表示し、輪郭と数値の欄を確定値に戻す。成功なら `cache.drop(changed)` して `redraw`。
  - 「自動配置に戻す」: `enqueue(id, null)`。
- [ ] **Step 1〜4**: 実装し、`cargo test -p s2v-bubbles` で通す。手で、移動・四隅・尻尾・矢印キー(押し続け)・数値の欄(Enter と Tab)・自動に戻す・小さすぎる大きさの復帰を操作し、結果を報告に書く。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面にドラッグ・キー・数値の入力・自動に戻すの操作を加える`

---

### Task 8: 保存・終了・`beforeunload`

**Files:**
- Modify: `app.js`、`view.js`

**Interfaces:**
- Consumes: Task 3(`Editor.save`・`flush`・`isDirty`)、Task 4(`quit`)、Task 7(`addPendingFlusher` に登録済み)。
- 振る舞い: 保存はボタンと Ctrl+S(`keydown` で `ctrlKey && key === "s"`、`preventDefault`)。`editor.save()` の結果を表示(成功: 「保存しました」と警告、422: 理由の一覧、その他: 短い文)。保存中はボタンを押せなくする。「終了」: `editor.flush()`(保留中の入力も含めるため、`save` と同じく pending flusher を呼んでから)→ `isDirty()` なら `confirm("保存していない変更があります。終了しますか")` → `api.quit()` → 「終了しました」。`beforeunload` は `isDirty()` のときだけ `preventDefault`。未保存の表示(`setDirty`)は確定と保存のたびに更新する。
- [ ] **Step 1〜4**: 実装し、`cargo test -p s2v-bubbles` で通す。手で、数値の欄に入力した直後の Ctrl+S が新しい値を保存すること、保存の後に編集して終了すると確認が出ること、保存してから終了すると確認が出ないことを確かめ、`bubbles.json` の中身を報告に書く。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面に保存と終了を加える`

---

### Task 9: Chrome の自動操作による確認と README(コントローラーが実施)

**Files:**
- Modify: `README.md`(bubbles の節に画面の操作を書き、仮の画面の注記を消す)

- [ ] **Step 1: 確認用の台本を作る** — 台詞に `<script>alert(1)</script>`、`"` と改行、長い台詞(切り詰め)を含み、シーン 2 つ(画像のカット、ループする動画のカット、止める動画のカット、ブラウザで再生できない素材(ffmpeg で作る HEVC の `.mov`。作れなければ理由を記録し、frame への切り替えは `<video>` の src を壊した素材で代わりに確かめる)のカット)と、ト書き・重なる吹き出し・尻尾つきの吹き出しを持つ台本。`build` を通す。
- [ ] **Step 2: Chrome で確かめる**(claude-in-chrome。`bubbles --no-open` の URL を開く。各項目で画面を撮り、手順と結果を記録する):
  1. 台本の文字がどこでも文字として表示され、`alert` が出ない(Review Focus 3)。
  2. 一覧の選択でそのコマへ移る。ドラッグで移動・大きさ・尻尾を変え、数値の欄で offset を変えて保存 → `bubbles.json` に入る。
  3. 小さすぎる大きさにすると元に戻り理由が出る。続けて別の項目を動かすと、それだけが確定する(Review Focus 1)。Enter の後に Tab で抜けても二重に確定しない。
  4. 自動の項目が、手動の項目を避けて動く(layout の反映と PNG の取り直し)。
  5. 尻尾つきの吹き出しの PNG の位置(原点のずれ)、重なりの順(ト書きが下)。
  6. カットの境目のコマ(n0−1・n0)で背景と吹き出しが切り替わり、素早く行き来しても古い表示が残らない(Review Focus 4)。contain と cover(設定を変えて起動し直す)。
  7. 再生 → 停止 → スライダー: 動画の音が出ない、止めたコマで止まる、ループ・最後のコマで止める、音声の終わりで最後のコマ。再生中の一覧の選択・スライダーの操作(Review Focus 5)。
  8. 再生できない素材のカットが frame の画像に切り替わる。
  9. 数値の欄に入力した直後の Ctrl+S が新しい値を保存する。未保存で「終了」すると確認が出る。保存してから終了すると確認が出ない(Review Focus 2)。
  10. 矢印キー(押し続け・Shift)と、数値の欄に入力中の矢印キーが枠を動かさないこと。
  11. ドラッグの途中で `pointercancel`(ウィンドウの外に出る・タブを替える)で元に戻ること。
  12. 403: ブラウザでクッキーを消して操作すると「セッションが切れました」が出て、要求と再生が止まる。
- [ ] **Step 3: GUI と render の一致** — 2. で保存した状態で、境目を含む 3 つのコマの GUI の画面を撮る → `bubbles` を「終了」で終える(ロックを外す。設計書 §2.3)→ `render` → 出力のその 3 つのコマ(`ffmpeg -vf select=eq(n\,<n>)`)を GUI の画面と並べて、吹き出しの位置・大きさ・出入りを目で比べ、結果を記録する。
- [ ] **Step 4: 見つかった不具合** — Beads に起票し、実装者に直させてから、その項目を確かめ直す。
- [ ] **Step 5: README を更新しコミット** — `docs: bubbles の画面の使い方を README に書く`
- [ ] **Step 6: 手動の確認(Human)** — sample の台本で `build` → `bubbles` → 配置を調整して保存 → 終了 → `render` し、画面で見た配置と `output.mp4` が一致することを目で確かめてもらう。
