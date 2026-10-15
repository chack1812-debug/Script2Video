import { test } from "node:test";
import assert from "node:assert/strict";
import { decideQuit, formatSaveResult, QUIT_CONFIRM_TEXT } from "../src/assets/save_quit.js";
import { Editor } from "../src/assets/model.js";

function base(over = {}) {
  const log = [];
  const o = {
    flushers: () => { log.push("flushers"); return [Promise.resolve()]; },
    flush: async () => { log.push("flush"); },
    isDirty: () => false,
    confirm: (t) => { log.push("confirm:" + t); return true; },
    quit: async () => { log.push("quit"); return { ok: true }; },
    sessionLost: () => false,
    ...over,
  };
  return { o, log };
}

test("format_save_ok_with_warnings", () => {
  const f = formatSaveResult({ ok: true, warnings: ["w1"] });
  assert.equal(f.ok, true);
  assert.equal(f.status, "保存しました");
  assert.deepEqual(f.lines, ["w1"]);
});

test("format_save_422_lists_reasons", () => {
  const f = formatSaveResult({ ok: false, status: 422, errors: ["a", "b"] });
  assert.equal(f.ok, false);
  assert.deepEqual(f.lines, ["保存できません: a", "保存できません: b"]);
});

test("format_save_other_error_is_short", () => {
  const f = formatSaveResult({ ok: false, status: 500, errors: ["サーバーのエラー"] });
  assert.equal(f.status, "保存できません(状態コード 500)");
  assert.deepEqual(f.lines, []);
  assert.equal(formatSaveResult({ ok: false, status: 0, errors: ["x"] }).status, "サーバーに接続できません");
  assert.equal(formatSaveResult({ ok: false, status: 403, errors: ["x"] }).status, "セッションが切れました。bubbles を起動し直してください");
});

test("quit_waits_for_saving_in_progress", async () => {
  let release;
  const saving = new Promise((r) => { release = r; });
  const { o, log } = base({ pendingSave: () => saving });
  const p = decideQuit(o);
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(log, []);
  release();
  assert.equal((await p).result, "quit");
  assert.deepEqual(log, ["flushers", "flush", "quit"]);
});

test("quit_clean_does_not_confirm", async () => {
  const { o, log } = base();
  assert.equal((await decideQuit(o)).result, "quit");
  assert.deepEqual(log, ["flushers", "flush", "quit"]);
});

test("quit_dirty_confirms_after_flush", async () => {
  let dirty = false;
  const { o, log } = base({ isDirty: () => dirty, flush: async () => { dirty = true; } });
  assert.equal((await decideQuit(o)).result, "quit");
  assert.deepEqual(log, ["flushers", "confirm:" + QUIT_CONFIRM_TEXT, "quit"]);
});

test("quit_cancelled_does_not_send", async () => {
  const { o, log } = base({ isDirty: () => true, confirm: () => false });
  assert.equal((await decideQuit(o)).result, "cancelled");
  assert.ok(!log.includes("quit"));
});

test("quit_session_lost_does_not_send", async () => {
  const { o, log } = base({ sessionLost: () => true });
  assert.equal((await decideQuit(o)).result, "lost");
  assert.deepEqual(log, []);
});

test("quit_session_lost_during_flush_does_not_send", async () => {
  let lost = false;
  const { o, log } = base({ flush: async () => { lost = true; }, sessionLost: () => lost });
  assert.equal((await decideQuit(o)).result, "lost");
  assert.ok(!log.includes("quit"));
});

test("quit_failure_reports_errors", async () => {
  const { o } = base({ quit: async () => ({ ok: false, errors: ["x"] }) });
  const r = await decideQuit(o);
  assert.equal(r.result, "failed");
  assert.deepEqual(r.errors, ["x"]);
});

test("editor_run_pending_flushers_calls_all", async () => {
  const ed = new Editor({}, { items: [] });
  const called = [];
  ed.addPendingFlusher(() => { called.push(1); });
  ed.addPendingFlusher(async () => { called.push(2); });
  await ed.runPendingFlushers();
  assert.deepEqual(called, [1, 2]);
});

test("quit_session_lost_during_confirm_does_not_send", async () => {
  let lost = false;
  const { o, log } = base({ isDirty: () => true, confirm: () => { lost = true; return true; }, sessionLost: () => lost });
  assert.equal((await decideQuit(o)).result, "lost");
  assert.ok(!log.includes("quit"));
});

test("save_session_lost_during_flusher_does_not_send", async () => {
  let lost = false;
  let sent = 0;
  const api = { save: async () => { sent++; return { ok: true, warnings: [] }; } };
  const ed = new Editor(api, { items: [] }, () => lost);
  ed.addPendingFlusher(() => { lost = true; });
  const r = await ed.save();
  assert.equal(sent, 0);
  assert.equal(r.ok, false);
  assert.equal(r.status, 403);
});
