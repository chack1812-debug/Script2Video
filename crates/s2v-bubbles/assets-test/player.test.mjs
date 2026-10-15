import { test } from "node:test";
import assert from "node:assert/strict";
import { createPlayer } from "../src/assets/player.js";

const state = {
  fps: 30, total_frames: 300,
  scenes: [{ id: "S01", cuts: [
    { id: "C1", frames: [0, 100], asset: { kind: "png" } },
    { id: "C3", frames: [100, 300], asset: { kind: "video", looped: false, frames: 60 } },
  ] }],
};

function fakeEl() {
  const L = {};
  return {
    log: [], currentTime: 0, muted: false, volume: 1, ended: false, readyState: 4, paused: true,
    addEventListener(t, f) { (L[t] ||= []).push(f); },
    removeEventListener(t, f) { L[t] = (L[t] || []).filter((x) => x !== f); },
    fire(t) { for (const f of [...(L[t] || [])]) f(); },
    listeners: (t) => (L[t] || []).length,
    play() { this.log.push("play"); this.paused = false; return Promise.resolve(); },
    pause() { this.log.push("pause"); this.paused = true; },
  };
}

function setup(startN = 0) {
  const audio = fakeEl();
  const vid = fakeEl();
  const calls = [];
  let n = startN;
  let rafFn = null;
  let timer = null;
  const p = createPlayer({
    audio, video: () => vid, state, getN: () => n, setN: (x) => { n = x; },
    frames: { setCurrent: (c, k) => calls.push(["frames.setCurrent", c, k]) },
    showCut: (cut, k) => calls.push(["showCut", cut && cut.id, k]),
    redraw: (k) => calls.push(["redraw", k]),
    onFrame: (k) => calls.push(["onFrame", k]),
    onPlayingChange: (b) => calls.push(["playing", b]),
    raf: (f) => { rafFn = f; return 1; }, caf: () => { rafFn = null; },
    setTimer: (f) => { timer = f; return 2; }, clearTimer: () => { timer = null; },
  });
  return { p, audio, vid, calls, getN: () => n, runRaf: () => { const f = rafFn; rafFn = null; f(); }, hasRaf: () => rafFn !== null, fireTimer: () => timer && timer() };
}

test("video_is_always_muted_and_play_starts_clock", () => {
  const t = setup(120);
  t.p.play();
  assert.equal(t.vid.muted, true);
  assert.equal(t.vid.volume, 0);
  assert.equal(t.audio.currentTime, 4); // 120 / 30
  assert.equal(t.vid.currentTime, 20 / 30);
  assert.deepEqual(t.audio.log, ["play"]);
  assert.deepEqual(t.vid.log, ["play"]);
  assert.ok(t.calls.some((c) => c[0] === "playing" && c[1] === true));
  assert.ok(t.calls.some((c) => c[0] === "frames.setCurrent" && c[1] === null)); // frame の結果を捨てる
  assert.ok(t.hasRaf());
});

test("tick_advances_frame_and_stop_waits_for_seeked_then_redraws", () => {
  const t = setup(120);
  t.p.play();
  t.audio.currentTime = 5; // コマ 150
  t.vid.currentTime = 20 / 30; // 素材の 50 コマ目のはずが 20 のまま → 2 コマ以上のずれ
  t.runRaf();
  assert.equal(t.getN(), 150);
  assert.equal(t.vid.currentTime, 50 / 30); // 合わせ直し
  t.calls.length = 0;
  t.p.stop();
  assert.deepEqual(t.audio.log, ["play", "pause"]);
  assert.equal(t.vid.currentTime, 50 / 30); // 止めたコマの素材の位置
  assert.ok(!t.calls.some((c) => c[0] === "redraw")); // seeked の前は描き直さない
  assert.equal(t.vid.listeners("seeked"), 1);
  t.vid.fire("seeked");
  assert.deepEqual(t.calls.filter((c) => c[0] === "redraw"), [["redraw", 150]]);
  assert.equal(t.vid.listeners("seeked"), 0);
  assert.equal(t.hasRaf(), false);
});

test("seek_while_stopped_on_video_waits_and_a_newer_seek_cancels_the_wait", () => {
  const t = setup(0);
  t.p.seek(130);
  assert.ok(t.calls.some((c) => c[0] === "showCut" && c[1] === "C3")); // 背景の切り替え
  assert.equal(t.vid.currentTime, 30 / 30);
  assert.ok(!t.calls.some((c) => c[0] === "redraw"));
  t.p.seek(50); // 画像のカット: 待ちは捨てて即描き直す
  assert.equal(t.vid.listeners("seeked"), 0);
  t.vid.fire("seeked");
  assert.deepEqual(t.calls.filter((c) => c[0] === "redraw"), [["redraw", 50]]);
});

test("seeked_timeout_still_redraws", () => {
  const t = setup(0);
  t.p.seek(130);
  t.fireTimer();
  assert.deepEqual(t.calls.filter((c) => c[0] === "redraw"), [["redraw", 130]]);
});

test("cut_change_while_playing_swaps_background_and_seeks_video", () => {
  const t = setup(90);
  t.p.play();
  t.audio.currentTime = 101 / 30; // コマ 101: C3 に入る
  t.runRaf();
  assert.ok(t.calls.some((c) => c[0] === "showCut" && c[1] === "C3"));
  assert.ok(t.calls.some((c) => c[0] === "onFrame" && c[1] === 101));
  assert.equal(t.vid.currentTime, 1 / 30);
});

test("audio_end_stops_at_last_frame", () => {
  const t = setup(290);
  t.p.play();
  t.audio.ended = true;
  t.audio.currentTime = 10;
  t.runRaf();
  assert.equal(t.getN(), 299);
  assert.ok(t.calls.some((c) => c[0] === "playing" && c[1] === false));
  assert.equal(t.vid.currentTime, 59 / 30); // 止める素材の最後のコマ
  t.vid.fire("seeked");
  assert.deepEqual(t.calls.filter((c) => c[0] === "redraw"), [["redraw", 299]]);
  assert.equal(t.hasRaf(), false);
});

test("play_at_last_frame_restarts_from_head", () => {
  const t = setup(299);
  t.p.play();
  assert.equal(t.getN(), 0);
  assert.equal(t.audio.currentTime, 0);
});

test("lost_session_does_not_start", () => {
  const audio = fakeEl();
  const p = createPlayer({
    audio, video: () => null, state, getN: () => 0, setN() {}, isLost: () => true,
    showCut() {}, redraw() {}, onFrame() {}, onPlayingChange() { throw new Error("started"); }, raf: () => 1,
  });
  p.play();
  assert.deepEqual(audio.log, []);
});

test("stop_without_video_redraws_immediately", () => {
  const t = setup(10);
  t.p.play();
  t.calls.length = 0;
  t.p.stop();
  assert.deepEqual(t.calls.filter((c) => c[0] === "redraw"), [["redraw", 10]]);
});
