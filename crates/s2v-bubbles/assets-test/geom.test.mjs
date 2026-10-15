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
