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

test("player_branches_pinned", () => {
  // (a) 再生中の同じカットのシーク: cut-change なしで動画をシークして再生
  let r = t.playerStep(state, { playing: true, n: 110, cutId: "C3" }, { type: "seek", n: 150 });
  assert.deepEqual(types(r), ["audio-seek", "video-seek", "video-play"]);
  // (b) 停止中の tick は何も出さない
  const stopped = { playing: false, n: 110, cutId: "C3" };
  r = t.playerStep(state, stopped, { type: "tick", audioSec: 5, videoSec: 0, ended: false });
  assert.deepEqual(types(r), []);
  assert.deepEqual(r.st, stopped);
  // (c) カットが変わり、新しいカットが止める素材の最後(C3 の n=200)
  r = t.playerStep(state, { playing: true, n: 99, cutId: "C1" }, { type: "tick", audioSec: 200 / 30, videoSec: 0, ended: false });
  assert.deepEqual(types(r), ["cut-change", "video-pause", "video-seek"]);
  assert.deepEqual(r.actions[2], { type: "video-seek", time: 59 / 30 });
  // (d) 同じカットで最後のコマに止まっていれば何も出さない。ずれていれば止めて置き直す
  const atEnd = { playing: true, n: 199, cutId: "C3" };
  r = t.playerStep(state, atEnd, { type: "tick", audioSec: 200 / 30, videoSec: 59 / 30, ended: false });
  assert.deepEqual(types(r), []);
  r = t.playerStep(state, atEnd, { type: "tick", audioSec: 200 / 30, videoSec: 1.5, ended: false });
  assert.deepEqual(types(r), ["video-pause", "video-seek"]);
});

test("redraw_mode_keeps_background_while_playing", () => {
  assert.equal(t.redrawMode(true), "overlay");
  assert.equal(t.redrawMode(false), "full");
});
