import { test } from "node:test";
import assert from "node:assert/strict";
import { EditSession } from "../src/assets/edit_session.js";
import { Editor } from "../src/assets/model.js";

const P = (o) => ({ x: 100, y: 100, w: 200, h: 100, tail: null, start_offset_ms: 0, end_offset_ms: 0, ...o });
const mkItem = (id) => ({ id, kind: "bubble", frames: [0, 100], order: 0, placed: P(), manual: null, image: { dx: 0, dy: 0, w: 1, h: 1 } });

function setup() {
  const items = [mkItem("a"), mkItem("b")];
  const sent = [];
  const api = {
    render: async () => ({ ok: true }),
    layout: async ({ manual }) => ({
      ok: true,
      items: items.map((it) => (manual[it.id] ? { ...it, placed: { ...manual[it.id] }, manual: manual[it.id] } : it)),
    }),
    save: async () => ({ ok: true }),
  };
  const editor = new Editor(api, { items });
  const timers = [];
  let active = null;
  const hooks = {
    commit: (id, placement, opts) => {
      sent.push({ id, placement, opts });
      return editor.enqueue(id, placement);
    },
    preview: () => {},
    discard: () => {},
  };
  const s = new EditSession({
    editor,
    cw: 1920,
    ch: 1080,
    hooks,
    setTimer: (f) => {
      timers.push(f);
      return timers.length - 1;
    },
    clearTimer: (i) => {
      timers[i] = null;
    },
    activeInput: () => active,
  });
  return {
    s,
    editor,
    sent,
    timers,
    setActive: (a) => {
      active = a;
    },
  };
}
const fire = (timers) =>
  timers.forEach((f, i) => {
    if (f) {
      timers[i] = null;
      f();
    }
  });

test("arrow_keys_three_presses_enqueue_once", async () => {
  const { s, editor, sent, timers } = setup();
  const it = editor.items[0];
  s.arrowKey(it, "ArrowRight", false);
  s.arrowKey(it, "ArrowRight", false);
  s.arrowKey(it, "ArrowDown", true);
  assert.equal(sent.length, 0);
  fire(timers);
  await editor.flush();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].placement.x, 102);
  assert.equal(sent[0].placement.y, 110);
});

test("drag_after_arrow_flushes_arrow_first_and_bases_on_it", async () => {
  const { s, editor, sent, timers } = setup();
  const it = editor.items[0];
  s.arrowKey(it, "ArrowRight", true); // x=110(保留中)
  const base = s.dragStart(it);
  assert.equal(sent.length, 1); // 矢印の値が先に enqueue
  assert.equal(sent[0].placement.x, 110);
  assert.equal(base.x, 110); // ドラッグの基準は矢印の値
  fire(timers); // 古いタイマーは何もしない
  assert.equal(sent.length, 1);
  await s.dragEnd(it, { ...base, x: 150 });
  assert.equal(sent.length, 2);
  assert.equal(editor.confirmed.a.x, 150); // ドラッグの結果が上書きされない
});

test("enter_then_change_enqueues_once", async () => {
  const { s, editor, sent } = setup();
  const r1 = s.commitNumber("a", "x", "250");
  const r2 = s.commitNumber("a", "x", "250");
  assert.equal(r1.kind, "commit");
  assert.equal(r2.kind, "unchanged");
  await r1.promise;
  assert.equal(sent.length, 1);
  assert.equal(editor.confirmed.a.x, 250);
});

test("number_unchanged_on_auto_item_and_invalid_input", () => {
  const { s, sent } = setup();
  assert.equal(s.commitNumber("a", "x", "100").kind, "unchanged"); // Tab で通り過ぎても手動にならない
  assert.equal(s.commitNumber("a", "start_offset_ms", "99999").kind, "invalid");
  assert.equal(s.commitNumber("a", "w", "abc").kind, "invalid");
  assert.equal(sent.length, 0);
});

test("flush_all_enqueues_pending_arrow_and_typing_field", async () => {
  const { s, editor, sent, setActive } = setup();
  s.arrowKey(editor.items[0], "ArrowLeft", false); // a: x=99
  setActive({ id: "b", field: "y", text: "300" });
  await s.flushAll();
  await editor.flush();
  assert.deepEqual(sent.map((x) => x.id).sort(), ["a", "b"]);
  assert.equal(editor.confirmed.a.x, 99);
  assert.equal(editor.confirmed.b.y, 300);
});

test("revert_drops_pending_arrow", () => {
  const { s, editor, sent } = setup();
  s.arrowKey(editor.items[0], "ArrowLeft", false);
  assert.equal(s.revert("a"), null); // 自動のまま: 何も送らない
  assert.equal(s.arrow, null);
  assert.equal(sent.length, 0);
});

// ブラウザの setTimeout は this が window 以外だと TypeError: Illegal invocation になる。
// Node は投げないので、同じ検査をする関数に差し替えて、タイマーを差し込まない既定の経路を確かめる。
test("タイマーを差し込まない既定でも、矢印キーは this つきの呼び出しで投げない", async () => {
  const origSet = globalThis.setTimeout;
  const origClear = globalThis.clearTimeout;
  const strict = (orig) =>
    function (...args) {
      if (this !== undefined && this !== globalThis) throw new TypeError("Illegal invocation");
      return orig(...args);
    };
  globalThis.setTimeout = strict(origSet);
  globalThis.clearTimeout = strict(origClear);
  try {
    const items = [mkItem("a")];
    const editor = new Editor({ render: async () => ({ ok: true }), layout: async () => ({ ok: true, items }), save: async () => ({ ok: true }) }, { items });
    const s = new EditSession({
      editor,
      cw: 1920,
      ch: 1080,
      hooks: { commit: () => Promise.resolve(), preview: () => {}, discard: () => {} },
      delayMs: 1,
    });
    assert.ok(s.arrowKey(items[0], "ArrowRight", false));
    assert.ok(s.arrowKey(items[0], "ArrowRight", false)); // 2 回目は clear も通る
    s.revert("a"); // 保留を捨てる(clear)
  } finally {
    globalThis.setTimeout = origSet;
    globalThis.clearTimeout = origClear;
  }
});
