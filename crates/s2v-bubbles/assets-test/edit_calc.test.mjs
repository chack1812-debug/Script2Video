import { test } from "node:test";
import assert from "node:assert/strict";
import { hitHandle, parseIntField, withField, samePlacement, placementOf } from "../src/assets/edit_calc.js";

const p = (o) => ({ x: 100, y: 100, w: 200, h: 100, tail: null, start_offset_ms: 0, end_offset_ms: 0, ...o });

test("hit_handle_priority_and_regions", () => {
  const withTail = p({ tail: { dx: 0, dy: 150 } }); // 先端 (200, 300)
  assert.equal(hitHandle(withTail, 202, 298, 1), "tail");
  assert.equal(hitHandle(withTail, 102, 98, 1), "nw");
  assert.equal(hitHandle(withTail, 298, 202, 1), "se");
  assert.equal(hitHandle(withTail, 300, 100, 1), "ne");
  assert.equal(hitHandle(withTail, 100, 200, 1), "sw");
  assert.equal(hitHandle(withTail, 200, 150, 1), "body");
  assert.equal(hitHandle(withTail, 600, 600, 1), null);
  assert.equal(hitHandle(p(), 200, 300, 1), null); // 尻尾が無ければ先端はない
});

test("hit_handle_grab_radius_scales_with_view", () => {
  // scale 0.5 なら画面 8px は canvas の 16
  assert.equal(hitHandle(p(), 100 - 15, 100 - 15, 0.5), "nw");
  assert.equal(hitHandle(p(), 100 - 15, 100 - 15, 2), null);
});

test("parse_int_field", () => {
  assert.deepEqual(parseIntField(" 42 "), { ok: true, value: 42 });
  assert.deepEqual(parseIntField("-7"), { ok: true, value: -7 });
  for (const bad of ["", "1.5", "abc", "1e3", "0x10", "99999999999"]) assert.equal(parseIntField(bad).ok, false, bad);
});

test("with_field_and_same_placement", () => {
  const base = p({ tail: { dx: 5, dy: 6 } });
  assert.equal(withField(base, "x", 150).x, 150);
  assert.deepEqual(withField(base, "tail_dy", -9).tail, { dx: 5, dy: -9 });
  assert.equal(base.tail.dy, 6); // 元は変えない
  assert.equal(withField(p(), "tail_dx", 1), null);
  assert.equal(withField(base, "bogus", 1), null);
  assert.equal(withField(base, "end_offset_ms", 30).end_offset_ms, 30);
  assert.ok(samePlacement(base, withField(base, "x", 100)));
  assert.ok(!samePlacement(base, withField(base, "w", 201)));
  assert.ok(samePlacement(p(), { ...p(), extra: 1, kind: "bubble" }));
  assert.deepEqual(Object.keys(placementOf(base)).sort(), ["end_offset_ms", "h", "start_offset_ms", "tail", "w", "x", "y"]);
});
