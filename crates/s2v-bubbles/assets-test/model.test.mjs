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

test("frame_cut_id_mismatch_is_dropped", async () => {
  const d = [];
  const ff = new FrameFetcher(() => { const x = deferred(); d.push(x); return x.p; });
  const got = [];
  ff.setCurrent("C", 10);
  ff.request("C", 10, (b) => got.push(["C", b]));
  ff.setCurrent("D", 10);
  ff.request("D", 10, (b) => got.push(["D", b]));
  d[0].resolve({ ok: true, blob: "bC" }); await tick(); await tick();
  assert.deepEqual(got, []);
  d[1].resolve({ ok: true, blob: "bD" }); await tick(); await tick();
  assert.deepEqual(got, [["D", "bD"]]);
});

test("frame_failure_calls_on_error_only_when_current", async () => {
  const d = [];
  const asked = [];
  const ff = new FrameFetcher((cut, n) => { asked.push(n); const x = deferred(); d.push(x); return x.p; });
  const results = [], errors = [];
  ff.setCurrent("C", 1);
  ff.request("C", 1, (b) => results.push(b), (e) => errors.push(["1", e]));
  ff.setCurrent("C", 2);
  ff.request("C", 2, (b) => results.push(b), (e) => errors.push(["2", e]));
  d[0].resolve({ ok: false, status: 500, errors: ["x"] }); await tick(); await tick();
  assert.deepEqual(errors, []);            // 古い失敗は捨てる
  assert.deepEqual(asked, [1, 2]);         // 失敗の後も次の要求は送られる
  d[1].resolve({ ok: false, status: 404, errors: ["無い"] }); await tick(); await tick();
  assert.deepEqual(errors, [["2", { status: 404, errors: ["無い"] }]]);
  assert.deepEqual(results, []);
});

test("frame_exception_calls_on_error", async () => {
  const ff = new FrameFetcher(async () => { throw new Error("切れた"); });
  const errors = [];
  ff.setCurrent("C", 1);
  ff.request("C", 1, () => assert.fail(), (e) => errors.push(e));
  await tick(); await tick();
  assert.deepEqual(errors, [{ status: 0, errors: ["切れた"] }]);
});

test("queue_survives_api_throw", async () => {
  let n = 0;
  const api = {
    render: async () => { if (n++ === 0) throw new Error("boom"); return { ok: true, png: new Uint8Array([1]), origin: [0, 0], clipped: false }; },
    layout: async () => ({ ok: true, items: baseItems() }),
  };
  const ed = new Editor(api, { items: baseItems() });
  const a = ed.enqueue("A", P({ x: 1 })), b = ed.enqueue("B", P({ x: 2 }));
  assert.equal((await a).rejected, true);
  assert.equal((await b).ok, true);
  assert.equal(ed.confirmed.A, undefined);
});
