# Script2Video 計画 2d-2(bubbles の画面) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 2d-1 の API の上に、吹き出し・ト書きを画面で動かし、大きさ・尻尾・表示時刻の offset を調整して保存できる画面の本体を作る。GUI と `render` の同じコマが一致すること(設計書 §9)を保つ。

**Architecture:** ブラウザの ES モジュールを、DOM に触れない純粋なモジュール(`geom.js`・`timeline.js`・`model.js`)と、DOM・通信のモジュール(`api.js`・`view.js`・`app.js`)に分ける。純粋なモジュールは Node の `node:test` で単体テストし、`cargo test` から `node --test` を呼ぶ。サーバーは埋め込みのファイルとパスを足すだけ(API は変えない)。画面全体は Chrome の自動操作で確かめる。

**Tech Stack:** 素の JavaScript(ES2020 の ES モジュール。外部のライブラリ・ビルドの道具なし)、HTML、CSS。Node 24(`node:test`、依存なし)。Rust(既存の `s2v-bubbles`)。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)§5.3(画面。今回の要件の正本)、§5.4(API の型の表が唯一の定義)、§8、§9。経緯は `docs/superpowers/specs/2026-10-16-script2video-bubbles-ui-design.md`(以下「設計メモ」)。前提として計画 2d-1 が main にある。

## Global Constraints

- 応答・コメント・画面の文言・エラーメッセージ・コミットメッセージは日本語(既存のコードに合わせる)。
- CSP(`default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; …`)の下で動くこと。インラインのスクリプト・スタイル・イベントハンドラの属性(`onclick=` など)・`style=` の属性を使わない。`eval`・`new Function`・`innerHTML`・`outerHTML`・`insertAdjacentHTML`・`document.write` を使わない。外部のライブラリ・CDN を使わない。
- 台本由来の文字列(台詞・ト書き・シーン名・警告・エラーの理由)は `textContent` でだけ入れる。
- 時刻はコマ番号で扱う。カットと項目のコマの範囲(半開区間)、重ねる順(`order`)はサーバーの値をそのまま使う。素材の中のコマ k の式: `d = n − n0`、`d < frames` なら `k = d`、そうでなければ `looped` なら `k = d mod frames`、そうでなければ `k = frames − 1`(`frames ≤ 0` なら 0)。音声の位置からのコマは `n = min(floor(位置秒 × fps), total_frames − 1)`(負なら 0)。スライダーは `0..total_frames−1`。
- 重ね合わせの位置は `配置の左上(placed.x, placed.y) + 画像の原点のずれ(image.dx, image.dy)`(`/api/render` の応答では `X-S2V-Origin`)。重ねる順は `order` の小さい順に下から。
- 要求の配置には `state.items[].kind` を `kind` として補う。`/api/render` の本文は `{id, manual:{ID:配置}}`、`/api/layout`・`/api/save` は `{manual:{ID:配置}}`。
- 確定の直列化・layout の結果の反映・保存の一式・再生の状態・止まっているときのコマ・キーとドラッグ・素材のエラーとセッション・PNG の上限 300 件は、設計書 §5.3 の各項のとおり。
- Node のテストは `crates/s2v-bubbles/assets-test/*.test.mjs`。`cargo test` は Node が無ければ失敗し、`S2V_SKIP_NODE_TESTS=1` のときだけ飛ばす。
- 検証コマンド(各タスクの最後に全部通す): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- `.superpowers/` の下と `.beads/interactions.jsonl` は git に加えない。コミットメッセージの末尾に、空行のあと実装者のモデルの `Co-Authored-By:` 行。

## Review Focus

1. **小さすぎる大きさにドラッグしてすぐ別の項目を動かす**: 1 つ目は直前の確定値に戻り、2 つ目は 1 つ目の未検証の値を含まずに確定する(確定の直列化)。→ Task 3 のテスト `queue_rejects_first_and_commits_second_without_it`。
2. **保存の送信中に編集する**: 送った一式だけが保存済みになり、後の編集は未保存のまま(終了で確認が出る)。→ Task 3 のテスト `save_marks_only_sent_snapshot_as_saved`。
3. **台詞に `<script>alert(1)</script>` や `"` を含む台本**: 一覧・プレビュー・数値の欄・警告のどこでも文字として表示され、スクリプトは動かない。→ Task 1 のテスト `assets_have_no_dangerous_apis`、Task 6 の Chrome の確認。
4. **停止中にカットの境目のコマ(n0−1 と n0)をスライダーで行き来する**: 背景の素材と表示中の吹き出しが、そのコマのものに必ず切り替わる(古い応答で上書きされない)。→ Task 2 のテスト `cut_and_items_at_boundaries`、Task 3 のテスト `frame_requests_coalesce_and_drop_stale`、Task 6。
5. **長い動画の素材で再生 → 停止 → スライダー**: 動画の音が鳴らず(消音)、止めたコマで動画が止まり、ループする素材はループし、止める素材は最後のコマで止まる。→ Task 2 のテスト `video_sync_decisions`、Task 6。

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `crates/s2v-bubbles/src/assets/geom.js`(新規) | 画面と canvas の換算、ドラッグの計算(移動・四隅・尻尾)、画面の内側に収める、矢印キー | 1 |
| `crates/s2v-bubbles/assets-test/geom.test.mjs`(新規) | geom のテスト | 1 |
| `crates/s2v-bubbles/tests/node_assets.rs`(新規) | `node --test` の呼び出しと、JS・HTML の禁止の検査 | 1 |
| `crates/s2v-bubbles/src/server.rs` | 埋め込みのファイルとパスの追加 | 1(geom)・2・3・4(追加分) |
| `crates/s2v-bubbles/src/assets/timeline.js`、`assets-test/timeline.test.mjs`(新規) | コマから表示中のカットと項目、素材の中のコマ、音声の位置からのコマ、動画の合わせ方の判定 | 2 |
| `crates/s2v-bubbles/src/assets/model.js`、`assets-test/model.test.mjs`(新規) | 手動配置の一式・確定の待ち行列・保存の一式・layout の反映と PNG の破棄・offset の検証・frame の要求の集約・PNG の保持の上限 | 3 |
| `crates/s2v-bubbles/src/assets/api.js`・`view.js`・`app.js`・`index.html`・`app.css` | 通信・表示・入口(表示と再生) | 4 |
| 同上 | 操作(選択・ドラッグ・キー・数値の欄・自動に戻す・保存・終了) | 5 |
| `README.md`、Chrome の確認の手順と結果 | 画面の使い方と確認 | 6 |

並行: Task 1 の後、Task 2 と Task 3 は別のファイルなので並行できる(server.rs の埋め込みの追加は取り込み時に解消)。Task 4 は 2・3 の後、Task 5 は 4 の後、Task 6 は最後。

---

### Task 1: `geom.js`、Node のテストの仕組み、JS・HTML の禁止の検査

**Files:**
- Create: `crates/s2v-bubbles/src/assets/geom.js`、`crates/s2v-bubbles/assets-test/geom.test.mjs`、`crates/s2v-bubbles/tests/node_assets.rs`
- Modify: `crates/s2v-bubbles/src/server.rs`(`/geom.js` を埋め込みで配信。`text/javascript; charset=utf-8`)

**Interfaces:**
- Produces(`geom.js`。すべて純粋な関数。`export`):
  - `toCanvas(px, py, view)`・`toView(cx, cy, view)`(`view = {left, top, scale}`。`toCanvas` は `{x: (px−left)/scale, y: (py−top)/scale}` を返す。四捨五入はしない)
  - `fitView(canvasW, canvasH, boxW, boxH)` → `{left, top, scale}`(canvas の縦横比を保って箱に収める。中央寄せ)
  - `clampRect(r, canvasW, canvasH)` → `{x, y, w, h}`(`w`・`h` は 1 以上 canvas 以下の整数、`0 ≤ x ≤ canvasW − w`、y も同様。入力は丸めてから収める)
  - `dragMove(start, dx, dy, canvas)` → 新しい配置(x・y だけを動かし、`clampRect` で収める。尻尾は相対なので動かさないが、先端が画面の外に出るなら尻尾の dx・dy を画面の内側に収まるよう詰める)
  - `dragCorner(start, corner, dx, dy, canvas)` → 新しい配置(`corner` は `"nw"|"ne"|"sw"|"se"`。反対側の角を固定し、大きさは 1 以上。画面の外には出さない)
  - `dragTail(start, px, py)` → `{dx, dy}`(先端の canvas の座標 (px, py) から、吹き出しの中心からの相対の整数。先端は画面の内側に収める)
  - `nudge(p, key, shift, canvas)` → 新しい配置(`key` は `"ArrowLeft"` など。1px、shift なら 10px。端で止まる)
  - `tailTip(p)` → `{x, y}`(中心 + 尻尾の相対。尻尾が null なら null)
  - 配置の形はサーバーの JSON と同じ `{x, y, w, h, tail:{dx,dy}|null, start_offset_ms, end_offset_ms}`(関数は start_offset_ms・end_offset_ms をそのまま引き継ぐ)

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/geom.test.mjs`)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import * as g from "../src/assets/geom.js";

const canvas = { w: 1920, h: 1080 };
const p = (o) => ({ x: 100, y: 100, w: 200, h: 300, tail: null, start_offset_ms: 0, end_offset_ms: 0, ...o });

test("toCanvas と toView は往復する", () => {
  const view = g.fitView(1920, 1080, 960, 600); // 横に収まる: scale 0.5、上下の余白
  assert.equal(view.scale, 0.5);
  assert.equal(view.left, 0);
  assert.equal(view.top, (600 - 540) / 2);
  const c = g.toCanvas(480, 300, view);
  const v = g.toView(c.x, c.y, view);
  assert.deepEqual(v, { x: 480, y: 300 });
});

test("clampRect は大きさと位置を画面の内側に収める", () => {
  assert.deepEqual(g.clampRect({ x: -5, y: 2000, w: 10.6, h: 0 }, 1920, 1080), { x: 0, y: 1079, w: 11, h: 1 });
  assert.deepEqual(g.clampRect({ x: 0, y: 0, w: 5000, h: 5000 }, 1920, 1080), { x: 0, y: 0, w: 1920, h: 1080 });
});

test("dragCorner は反対の角を固定し、負の大きさにも画面の外にもならない", () => {
  const s = p();
  assert.deepEqual(pick(g.dragCorner(s, "se", 50, 20, canvas)), { x: 100, y: 100, w: 250, h: 320 });
  assert.deepEqual(pick(g.dragCorner(s, "nw", 500, 500, canvas)), { x: 299, y: 399, w: 1, h: 1 }); // 反対の角(300,400)の手前で止まる
  assert.deepEqual(pick(g.dragCorner(s, "nw", -500, -500, canvas)), { x: 0, y: 0, w: 300, h: 400 });
  assert.deepEqual(pick(g.dragCorner(s, "se", 5000, 5000, canvas)), { x: 100, y: 100, w: 1820, h: 980 });
});

test("dragMove は端で止まり、尻尾の先端を画面の内側に詰める", () => {
  const s = p({ tail: { dx: 0, dy: 200 } }); // 中心 (200,250)、先端 (200,450)
  const m = g.dragMove(s, 0, 2000, canvas);
  assert.equal(m.y, 1080 - 300);
  const tip = g.tailTip(m);
  assert.ok(tip.y <= 1079, JSON.stringify(m));
  assert.equal(m.start_offset_ms, 0);
});

test("dragTail は先端を画面の内側に収めた相対の整数", () => {
  const s = p(); // 中心 (200, 250)
  assert.deepEqual(g.dragTail(s, 250.4, 600.6), { dx: 50, dy: 351 });
  assert.deepEqual(g.dragTail(s, -50, 5000), { dx: -200, dy: 829 }); // 先端 (0, 1079)
});

test("nudge は 1px・10px 動き、端で止まる", () => {
  assert.equal(g.nudge(p(), "ArrowLeft", false, canvas).x, 99);
  assert.equal(g.nudge(p(), "ArrowDown", true, canvas).y, 110);
  assert.equal(g.nudge(p({ x: 0 }), "ArrowLeft", true, canvas).x, 0);
  assert.equal(g.nudge(p({ x: 1720 }), "ArrowRight", false, canvas).x, 1720);
});

function pick(r) { return { x: r.x, y: r.y, w: r.w, h: r.h }; }
```

`tests/node_assets.rs`:

```rust
//! 画面の JS のテスト(Node の node:test)と、JS・HTML の禁止の検査(設計書 §5.3・§8)。
//! Node が無ければ失敗する。S2V_SKIP_NODE_TESTS=1 のときだけ node のテストを飛ばす(禁止の検査は常に行う)。

#[test]
fn node_unit_tests_pass() {
    if std::env::var("S2V_SKIP_NODE_TESTS").as_deref() == Ok("1") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("node")
        .arg("--test")
        .arg(dir.join("assets-test"))
        .current_dir(dir)
        .output()
        .expect("node を起動できません(PATH に Node が必要です。飛ばすなら S2V_SKIP_NODE_TESTS=1)");
    assert!(out.status.success(), "node --test が失敗しました\n{}\n{}",
        String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn assets_have_no_dangerous_apis() {
    // src/assets の全ての .js について、禁止の語が無いこと(コメントも含めて検査する)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assets");
    let banned = ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "eval(", "new Function", "setTimeout(\"", "setInterval(\""];
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) == Some("js") {
            let s = std::fs::read_to_string(&p).unwrap();
            for b in banned { assert!(!s.contains(b), "{} に {b}", p.display()); }
        }
    }
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
    assert!(!html.contains(" on"), "index.html にイベントハンドラの属性");     // onclick= など(属性の前の空白で判定)
    assert!(!html.contains("style="), "index.html に style 属性");
    assert!(!html.contains("<style"), "index.html に style 要素");
    for (i, _) in html.match_indices("<script") {
        let tag_end = html[i..].find('>').unwrap() + i;
        assert!(html[i..tag_end].contains("src="), "index.html にインラインのスクリプト");
    }
}
```

`node_assets.rs` の `index.html` の ` on` の判定が本文の日本語の文で誤検出しないこと(index.html は英字の属性しか持たない)を確かめ、誤検出するなら属性の正規表現的な判定(`<` から `>` の中だけを見る)に直す。

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-bubbles --test node_assets` / Expected: `node_unit_tests_pass` が geom.js の import の失敗で FAIL。
- [ ] **Step 3: 実装する** — `geom.js` を Interfaces どおりに書く。`server.rs` の振り分けに `/geom.js` を足す(`include_str!`)。
- [ ] **Step 4: 通ることを確かめる** — Run: `node --test crates/s2v-bubbles/assets-test`、`cargo test -p s2v-bubbles --test node_assets`
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面の座標とドラッグの計算と、Node のテストの仕組みを加える`

---

### Task 2: `timeline.js`(コマから表示、素材の中のコマ、再生の判定)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/timeline.js`、`crates/s2v-bubbles/assets-test/timeline.test.mjs`
- Modify: `crates/s2v-bubbles/src/server.rs`(`/timeline.js`)

**Interfaces:**
- Consumes: `/api/state` の JSON(設計書 §5.4。`scenes[].cuts[].{id, frames:[n0,n1], asset:{url, kind, looped, frames, frame_url}|null}`、`items[].{id, kind, frames:[s,e], order, placed, image, clipped, manual}`、`fps`、`total_frames`)。
- Produces(純粋。`export`):
  - `cutAt(state, n)` → カットの JSON か null(`n0 ≤ n < n1` のカット。長さ 0 のカットは選ばない)
  - `itemsAt(items, n)` → `s ≤ n < e` の項目を `order` の小さい順に並べた配列
  - `assetFrame(n, n0, frames, looped)` → k(Global Constraints の式。サーバーの `s2v_video::frames::asset_frame` と同じ値)
  - `frameFromAudio(sec, fps, total)` → n(Global Constraints の式)
  - `videoTime(k, fps)` → 秒(`k / fps`)
  - `videoAction({playing, n, cut, videoTime, duration, fps})` → `{type:"none"}`・`{type:"seek", time}`・`{type:"pause-at", time}`・`{type:"play-from", time}` のどれか。規則(設計書 §5.3「再生の状態」「止まっているときのコマ」): 停止中は常に `seek`(k/fps へ)。再生中: 素材の中のコマが止める素材の最後(`!looped` かつ `d ≥ frames − 1`)なら `pause-at`(最後のコマの時刻)。それ以外で、動画の位置と k/fps の差が 2 コマ(`2 / fps` 秒)以上なら `play-from`(k/fps)。それ以外は `none`。
  - `sliderMax(total)` → `max(total − 1, 0)`

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/timeline.test.mjs`)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import * as t from "../src/assets/timeline.js";

const state = {
  fps: 30, total_frames: 300,
  scenes: [{ id: "S01", cuts: [
    { id: "S01-C01", frames: [0, 100], asset: null },
    { id: "S01-C02", frames: [100, 100], asset: null },          // 長さ 0
    { id: "S01-C03", frames: [100, 300], asset: { kind: "video", looped: false, frames: 60 } },
  ] }],
};

test("cut_and_items_at_boundaries", () => {
  assert.equal(t.cutAt(state, 99).id, "S01-C01");
  assert.equal(t.cutAt(state, 100).id, "S01-C03"); // 長さ 0 のカットは選ばない
  assert.equal(t.cutAt(state, 299).id, "S01-C03");
  assert.equal(t.cutAt(state, 300), null);
  const items = [
    { id: "L2", frames: [50, 100], order: 2 },
    { id: "N1", frames: [0, 100], order: 0 },
    { id: "L1", frames: [99, 150], order: 1 },
  ];
  assert.deepEqual(t.itemsAt(items, 99).map((i) => i.id), ["N1", "L1", "L2"]);
  assert.deepEqual(t.itemsAt(items, 100).map((i) => i.id), ["L1"]); // 半開区間
  assert.deepEqual(t.itemsAt(items, 49).map((i) => i.id), ["N1"]);
});

test("assetFrame はサーバーと同じ値", () => {
  // crates/s2v-video/src/frames.rs の asset_frame_freezes_or_loops と同じ値
  assert.equal(t.assetFrame(100, 100, 60, false), 0);
  assert.equal(t.assetFrame(159, 100, 60, false), 59);
  assert.equal(t.assetFrame(160, 100, 60, false), 59);
  assert.equal(t.assetFrame(300, 100, 60, false), 59);
  assert.equal(t.assetFrame(160, 100, 60, true), 0);
  assert.equal(t.assetFrame(225, 100, 60, true), 5);
  assert.equal(t.assetFrame(105, 100, 0, false), 0);
});

test("frameFromAudio と sliderMax", () => {
  assert.equal(t.frameFromAudio(0, 30, 300), 0);
  assert.equal(t.frameFromAudio(1 / 30 - 1e-9, 30, 300), 0);
  assert.equal(t.frameFromAudio(1 / 30, 30, 300), 1);
  assert.equal(t.frameFromAudio(999, 30, 300), 299);
  assert.equal(t.frameFromAudio(-1, 30, 300), 0);
  assert.equal(t.sliderMax(300), 299);
  assert.equal(t.sliderMax(0), 0);
});

test("video_sync_decisions", () => {
  const cut = { frames: [100, 300], asset: { looped: false, frames: 60 } };
  const base = { fps: 30, cut, duration: 2 };
  // 停止中は常にシーク
  assert.deepEqual(t.videoAction({ ...base, playing: false, n: 110, videoTime: 0.333 }), { type: "seek", time: 10 / 30 });
  // 再生中、1 コマのずれは何もしない、2 コマのずれは合わせ直す
  assert.deepEqual(t.videoAction({ ...base, playing: true, n: 110, videoTime: 10 / 30 + 1 / 30 }), { type: "none" });
  assert.deepEqual(t.videoAction({ ...base, playing: true, n: 110, videoTime: 10 / 30 + 2 / 30 }), { type: "play-from", time: 10 / 30 });
  // 止める素材の最後のコマ以降は一時停止
  assert.deepEqual(t.videoAction({ ...base, playing: true, n: 200, videoTime: 1.9 }), { type: "pause-at", time: 59 / 30 });
  // ループする素材はループ後の位置へ
  const loop = { ...base, cut: { frames: [100, 300], asset: { looped: true, frames: 60 } } };
  assert.deepEqual(t.videoAction({ ...loop, playing: true, n: 165, videoTime: 1.9 }), { type: "play-from", time: 5 / 30 });
});
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、`node --test crates/s2v-bubbles/assets-test` と `cargo test -p s2v-bubbles --test node_assets` で通す。`server.rs` に `/timeline.js`。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面のコマと表示・再生の判定を加える`

---

### Task 3: `model.js`(手動配置の一式、確定の待ち行列、保存、layout の反映、frame の集約、PNG の上限)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/model.js`、`crates/s2v-bubbles/assets-test/model.test.mjs`
- Modify: `crates/s2v-bubbles/src/server.rs`(`/model.js`)

**Interfaces:**
- Produces(`export`。DOM・fetch に触れない。通信は差し込む `api` オブジェクト経由):
  - `withKind(manual, items)` → 要求用の一式(各配置に `items` の `kind` を足した新しいオブジェクト。元は変えない)
  - `validateOffset(v)` → `{ok:true, value}` か `{ok:false, reason}`(整数で `|v| ≤ 60000`。文字列の数字も受ける。それ以外は理由の日本語)
  - `changedImages(oldItems, newItems)` → PNG を捨てる項目の ID の配列(`placed`(x・y・w・h・tail)か `clipped` か `image` が違う項目、新しく現れた項目)
  - `class Editor`(`constructor(api, state)`。`api` は `{render(body) → Promise<{ok, status, png?, origin?, clipped?, errors?}>, layout(body) → Promise<{ok, status, items?, errors?}>, save(body) → Promise<{ok, status, warnings?, errors?}>}`):
    - `confirmed`(確定済みの手動配置の一式)、`saved`(保存済みの一式。比較用の正規化した JSON 文字列)、`items`(表示の正本)
    - `enqueue(id, placementOrNull)` → Promise(待ち行列に入れる。`null` は自動に戻す。処理は1つずつ: 一式 = `confirmed` の写し + この編集、`render({id, manual: withKind(一式)})` と `layout({manual: withKind(一式)})` の両方が ok なら `confirmed = 一式`、`items = layout の items`、`changedImages` を返す。どちらかが ok でなければ `confirmed` は変えず、`{rejected: true, id, errors}` を返す)
    - `lastQueued(id)` → 最後に待ち行列へ入れた値(処理中・未処理を含む。無ければ `confirmed[id]`)。二重の確定の判定に使う
    - `flush()` → 待ち行列が空になるまで待つ Promise
    - `save()` → Promise(`flush()` の後、`confirmed` の写しを送り、ok なら `saved = 送った写し`。送った後に確定した編集は未保存のまま)
    - `isDirty()` → `confirmed` と `saved` が違えば true
  - `class FrameFetcher`(`constructor(fetchFrame)`。`fetchFrame(cutId, n) → Promise<blob|Error>`): `request(cutId, n, onResult)`(同時に1件だけ。処理中の間の要求は最新の1件だけを残す。応答は、そのときの「今のカットと n」(`setCurrent(cutId, n)` で設定)と一致するときだけ `onResult` に渡す)
  - `class ImageCache`(`constructor(max = 300, revoke)`): `get(id)`、`set(id, url)`(上限を超えたら使った順の古いものを捨て、`revoke(url)` を呼ぶ。同じ ID の置き換えでも古い URL を `revoke`)、`drop(ids)`

- [ ] **Step 1: 失敗するテストを書く**(`assets-test/model.test.mjs`。偽の API は Promise をテストから解決できるようにする)

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { Editor, FrameFetcher, ImageCache, withKind, validateOffset, changedImages } from "../src/assets/model.js";

const P = (o = {}) => ({ x: 0, y: 0, w: 100, h: 100, tail: null, start_offset_ms: 0, end_offset_ms: 0, ...o });
const items = [{ id: "A", kind: "bubble", placed: P(), image: { dx: 0, dy: 0, w: 100, h: 100 }, clipped: false },
               { id: "B", kind: "note", placed: P({ x: 200 }), image: { dx: 0, dy: 0, w: 100, h: 100 }, clipped: false }];
const state = () => ({ items: structuredClone(items) });

// 偽の API: 呼ばれた要求を記録し、テストが応答を決める
function fakeApi(decide) {
  const calls = [];
  const api = {
    calls,
    render: async (b) => { calls.push(["render", b]); return decide("render", b); },
    layout: async (b) => { calls.push(["layout", b]); return decide("layout", b); },
    save: async (b) => { calls.push(["save", b]); return decide("save", b); },
  };
  return api;
}

test("withKind は kind を補い、元を変えない", () => {
  const m = { A: P() };
  const r = withKind(m, items);
  assert.equal(r.A.kind, "bubble");
  assert.equal(m.A.kind, undefined);
});

test("validateOffset", () => {
  assert.deepEqual(validateOffset("120"), { ok: true, value: 120 });
  assert.equal(validateOffset(60001).ok, false);
  assert.equal(validateOffset("1.5").ok, false);
  assert.equal(validateOffset("abc").ok, false);
});

test("queue_rejects_first_and_commits_second_without_it", async () => {
  // Review Focus 1: A の小さすぎる大きさは render が 422。B はその後に確定し、B の要求に A の未検証の値が入らない
  const api = fakeApi((kind, b) => {
    if (kind === "render" && b.id === "A") return { ok: false, status: 422, errors: ["文字が収まりません"] };
    if (kind === "layout") return { ok: true, status: 200, items: state().items };
    return { ok: true, status: 200, png: new Uint8Array([1]), origin: [0, 0], clipped: false };
  });
  const ed = new Editor(api, state());
  const a = ed.enqueue("A", P({ w: 1, h: 1 }));
  const b = ed.enqueue("B", P({ x: 300 }));
  assert.equal((await a).rejected, true);
  await b;
  assert.equal(ed.confirmed.A, undefined);
  assert.equal(ed.confirmed.B.x, 300);
  const bCalls = api.calls.filter(([k, body]) => k === "layout" && body.manual.B);
  assert.ok(bCalls.every(([, body]) => !body.manual.A), "B の要求に A の未検証の値が入った");
});

test("save_marks_only_sent_snapshot_as_saved", async () => {
  // Review Focus 2
  let releaseSave;
  const api = fakeApi((kind) => {
    if (kind === "save") return new Promise((r) => { releaseSave = () => r({ ok: true, status: 200, warnings: [] }); });
    if (kind === "layout") return { ok: true, status: 200, items: state().items };
    return { ok: true, status: 200, png: new Uint8Array([1]), origin: [0, 0], clipped: false };
  });
  const ed = new Editor(api, state());
  await ed.enqueue("A", P({ x: 10 }));
  const saving = ed.save();
  await new Promise((r) => setImmediate(r));
  await ed.enqueue("B", P({ x: 400 }));   // 保存の送信中の編集
  releaseSave();
  await saving;
  assert.equal(ed.isDirty(), true, "送った後の編集が保存済みになった");
  const sent = api.calls.find(([k]) => k === "save")[1].manual;
  assert.equal(sent.A.x, 10);
  assert.equal(sent.B, undefined);
});

test("save は保留中の確定を待ってから送る", async () => {
  // enqueue の直後に save を呼ぶと、その編集の確定の後に送られる
});

test("lastQueued は処理中・未処理の値を返す", async () => {
  // render を遅らせ、enqueue の直後に lastQueued(id) が入れた値になること。処理後は confirmed の値
});

test("changedImages", () => {
  const nw = structuredClone(items);
  nw[1].placed.x = 250;
  nw.push({ id: "C", placed: P(), image: { dx: 0, dy: 0, w: 1, h: 1 }, clipped: false });
  assert.deepEqual(changedImages(items, nw).sort(), ["B", "C"]);
});

test("frame_requests_coalesce_and_drop_stale", async () => {
  // Review Focus 4: 1 件目の処理中に n=11,12,13 を要求すると、2 件目は n=13 だけ。
  // 1 件目の応答が返った時点で今のカット・n が違えば onResult を呼ばない
});

test("ImageCache は上限で古いものを捨てて revoke する", () => {
  const revoked = [];
  const c = new ImageCache(2, (u) => revoked.push(u));
  c.set("A", "u1"); c.set("B", "u2"); c.get("A"); c.set("C", "u3");
  assert.deepEqual(revoked, ["u2"]);
  c.set("A", "u4");
  assert.deepEqual(revoked, ["u2", "u1"]);
});
```

(本文がコメントだけのテストは、上の偽の API の要領で具体的に書く。)

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す。`server.rs` に `/model.js`。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面の手動配置・確定の待ち行列・保存・frame の集約・PNG の上限を加える`

---

### Task 4: 表示と再生(`api.js`・`view.js`・`app.js`・`index.html`・`app.css`)

**Files:**
- Create: `crates/s2v-bubbles/src/assets/api.js`、`view.js`
- Modify: `crates/s2v-bubbles/src/assets/app.js`(仮の画面を置き換える)、`index.html`、`app.css`、`crates/s2v-bubbles/src/server.rs`(`/api.js`・`/view.js`)

**Interfaces:**
- Consumes: Task 1〜3。
- Produces:
  - `api.js`: `getState()`、`render(body)`・`layout(body)`・`save(body)`(Task 3 の `api` の形を返す。`render` は応答の blob と `X-S2V-Origin`・`X-S2V-Clipped` を読む)、`frame(cutId, n)`、`head(url)` → 状態コード、`quit()`。全ての `fetch` は `credentials: "same-origin"`。403 は共通の処理(`onSessionLost` を呼ぶ)。
  - `view.js`: 一覧(シーン → カット → 項目。手動配置の印・切り詰めの印)、プレビュー(canvas の縦横比の箱。背景の `<img>`・`<video muted playsinline>`・frame の `<img>`・「素材なし」・「プレビューできません」、重ねる `<img>` の層)、スライダー(`max = sliderMax`)、再生/停止のボタン、数値の欄、警告と状態の表示。DOM の要素は `document.createElement` と `textContent` だけで作る。位置は CSS の `transform`・`left`・`top` をスクリプトから `element.style` に設定してよい(CSP は style 属性の文字列を禁じるが、CSSOM の設定は許す)。
  - `app.js`: 起動(`getState` → `Editor`・`ImageCache`・`FrameFetcher` を作る)、コマの移動(設計書 §5.3 の手順)、再生(`<audio>` と `requestAnimationFrame`、`videoAction` の結果を `<video>` に適用)、素材の読み込みエラー(`head(url)` で 403 か判定 → セッション切れ、それ以外の動画は frame へ)。
- 背景の収め方: `fit` が contain なら `object-fit: contain`、cover なら `cover`、余白の色は `background`(CSSOM で設定)。

- [ ] **Step 1: テストを書く** — DOM の部分は Node で単体テストしない(Task 6 の Chrome で確かめる)。`node_assets.rs` の `assets_have_no_dangerous_apis` が新しいファイルも検査することを確かめる。Rust の `tests/http.rs` に、`/api.js`・`/view.js`・`/geom.js`・`/timeline.js`・`/model.js` が `text/javascript; charset=utf-8` で 200(クッキーあり)、クッキーなしで 403 のテストを足す(`embedded_modules_are_served`)。
- [ ] **Step 2〜4**: 実装し、`cargo test -p s2v-bubbles` で通す。手で `script2video bubbles <sample>` を起動し、ブラウザの開発者ツールに CSP の違反とエラーが出ないことを確かめ、報告に書く(画面のスクリーンショットは不要)。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面に一覧・プレビュー・スライダー・再生を加える`

---

### Task 5: 操作(選択・ドラッグ・キー・数値の欄・自動に戻す・保存・終了)

**Files:**
- Modify: `crates/s2v-bubbles/src/assets/app.js`、`view.js`、`app.css`

**Interfaces:**
- Consumes: Task 1(`dragMove`・`dragCorner`・`dragTail`・`nudge`・`toCanvas`)、Task 3(`Editor.enqueue`・`lastQueued`・`save`・`flush`・`isDirty`、`validateOffset`)、Task 4。
- 規則(設計書 §5.3 の「操作」「確定の直列化」「保存」「キーとドラッグ」):
  - 選択: 一覧のクリックはその項目の表示開始のコマへ移動して選択。プレビューのクリックは、表示中の項目のうち `order` の最も大きい(最前面の)ものを選ぶ。
  - ドラッグ: 選択中の項目の本体(移動)・四隅のつまみ(大きさ)・尻尾の先端のつまみ。`pointerdown` で `setPointerCapture`、ドラッグ中は輪郭の要素だけを動かす(選択・コマを変えない、再生中なら止める)。`pointerup` で `enqueue`。`pointercancel` で輪郭を捨てて元に戻す。
  - 矢印キー: フォーカスが数値の欄・ボタン以外にあるとき、選択中の項目を `nudge`。`preventDefault` でスクロールを抑える。押している間は輪郭だけを動かし、最後の押下から 300ms 後に `enqueue`。
  - 数値の欄: x・y・w・h・尻尾 dx・dy・開始と終了の offset。Enter か `change`(フォーカスの喪失)で、値が `lastQueued(id)` と同じなら何もしない。offset は `validateOffset`。
  - 確定の結果: `rejected` なら理由を表示して輪郭と数値の欄を確定値に戻す。成功なら `changedImages` の PNG を `ImageCache.drop`、現在のコマを描き直す。
  - 「自動配置に戻す」: `enqueue(id, null)`。
  - 保存: ボタンと Ctrl+S(`preventDefault`)。保留中の矢印キーの待ちを即座に `enqueue` してから `Editor.save()`。結果を表示(422 は理由の一覧)。
  - 終了: 保留中の入力を `enqueue` → `flush()` → `isDirty()` なら `confirm` → `quit()` → 「終了しました」。`beforeunload` は `isDirty()` のときだけ `preventDefault`。
- [ ] **Step 1〜4**: 実装し、`cargo test -p s2v-bubbles` で通す(純粋な部分は Task 1・3 のテストが担う)。手で起動し、移動・大きさ・尻尾・数値・自動に戻す・保存・終了を一通り操作して、開発者ツールにエラーが出ないことと `bubbles.json` の中身を報告に書く。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の画面にドラッグ・キー・数値の入力・保存・終了の操作を加える`

---

### Task 6: Chrome の自動操作による確認と README(コントローラーが実施)

**Files:**
- Create: `crates/s2v-bubbles/tests/fixtures/ui_check/`(確認用の台本と素材を作るスクリプトの説明。素材は実行時に作る)
- Modify: `README.md`(bubbles の節に画面の操作を書く。仮の画面の注記を消す)

- [ ] **Step 1: 確認用の台本を作る** — 台詞に `<script>alert(1)</script>`、`"` と改行、長い台詞(切り詰め)を含み、シーン 2 つ(画像のカット、ループする動画のカット、止める動画のカット、ブラウザで再生できない素材(ffmpeg で作る HEVC の `.mov`)のカット)と、ト書き・重なる吹き出しを持つ台本。`build` を通す。
- [ ] **Step 2: Chrome で確かめる**(claude-in-chrome。`bubbles --no-open` の URL を開く。各項目で画面を撮る):
  1. 台本の文字がどこでも文字として表示され、`alert` が出ない(Review Focus 3)。
  2. ドラッグで移動・大きさ・尻尾を変え、数値の欄で offset を変えて保存 → `bubbles.json` に入る。
  3. 小さすぎる大きさにすると元に戻り、理由が出る。続けて別の項目を動かすと、それだけが確定する(Review Focus 1)。
  4. 自動の項目が、手動の項目を避けて動く(配置の結果の反映と PNG の取り直し)。
  5. 尻尾つきの吹き出しの PNG の位置(原点のずれ)、重なりの順(ト書きが下)。
  6. カットの境目のコマ(n0−1・n0)で背景と吹き出しが切り替わる(Review Focus 4)。contain と cover(設定を変えて起動し直す)。
  7. 再生 → 停止 → スライダー: 動画の音が出ない、止めたコマで止まる、ループする素材はループ、止める素材は最後のコマで止まる(Review Focus 5)。
  8. 再生できない素材のカットが frame の画像に切り替わる。
  9. 未保存で「終了」すると確認が出る。保存してから終了すると確認が出ない。
  10. GUI と render の一致: 2. で保存した後に `render` を実行し、境目を含む 3 つのコマについて、GUI のスクリーンショットと出力のコマ(`ffmpeg -vf select=eq(n\,<n>)`)を並べて目で比べる(吹き出しの位置・大きさ・出入り)。
- [ ] **Step 3: 見つかった不具合** — 不具合は Beads に起票し、実装者に直させてから、その項目を確かめ直す。
- [ ] **Step 4: README を更新しコミット** — `docs: bubbles の画面の使い方を README に書く`
- [ ] **Step 5: 手動の確認(Human)** — sample の台本で `build` → `bubbles` → 配置を調整して保存 → `render` し、画面で見た配置と `output.mp4` が一致することを目で確かめてもらう。
