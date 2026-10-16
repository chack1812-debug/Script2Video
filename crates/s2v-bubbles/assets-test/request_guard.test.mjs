import test from "node:test";
import assert from "node:assert/strict";
import { ImageRequestTracker, acceptFrame } from "../src/assets/request_guard.js";
import { FrameFetcher } from "../src/assets/model.js";

test("同じ ID・版の実行中の要求は重ねない", () => {
  const t = new ImageRequestTracker();
  assert.equal(t.shouldRequest("A", 0), true);
  assert.equal(t.shouldRequest("A", 0), false);
});

test("完了(成功・失敗)の後は、同じ版でも再び要求できる(退避・失敗の再試行)", () => {
  const t = new ImageRequestTracker();
  assert.equal(t.shouldRequest("A", 0), true);
  t.done("A", 0);
  assert.equal(t.shouldRequest("A", 0), true);
});

test("新しい版の要求は、古い版の実行中に邪魔されない", () => {
  const t = new ImageRequestTracker();
  assert.equal(t.shouldRequest("A", 0), true);
  assert.equal(t.shouldRequest("A", 1), true);
  assert.equal(t.shouldRequest("B", 0), true);
});

test("acceptFrame は今のカットとコマが一致するときだけ受け入れる", () => {
  assert.equal(acceptFrame({ cutId: "C", n: 3 }, { cutId: "C", n: 3 }), true);
  assert.equal(acceptFrame({ cutId: "D", n: 3 }, { cutId: "C", n: 3 }), false);
  assert.equal(acceptFrame({ cutId: "C", n: 4 }, { cutId: "C", n: 3 }), false);
  assert.equal(acceptFrame({ cutId: null, n: 3 }, { cutId: "C", n: 3 }), false);
  assert.equal(acceptFrame(null, { cutId: "C", n: 3 }), false);
});

test("FrameFetcher.clear の後は、取得中の応答を渡さない", async () => {
  let resolve;
  const ff = new FrameFetcher(() => new Promise((r) => { resolve = r; }));
  const got = [];
  ff.setCurrent("C", 1);
  ff.request("C", 1, (b) => got.push(b), (e) => got.push(e));
  ff.clear();
  resolve({ ok: true, blob: "b" });
  await new Promise((r) => setTimeout(r, 0));
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(got, []);
});
