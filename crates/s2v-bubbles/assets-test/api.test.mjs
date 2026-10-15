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
  assert.equal(lost, 2);
});

test("api_all_requests_are_same_origin", async () => {
  const seen = [];
  const f = async (url, init) => {
    seen.push([url, init]);
    return url.startsWith("/media/") && init.method === "GET" ? new Response(new Uint8Array([1]), { status: 200 }) : url === "/api/layout" ? resp(200, []) : url === "/api/render" ? new Response(new Uint8Array([1]), { status: 200, headers: { "X-S2V-Origin": "0,0" } }) : resp(200, {});
  };
  const api = createApi(f, () => {});
  await api.getState();
  await api.render({});
  await api.layout({});
  await api.save({});
  await api.frame("C", 1);
  await api.head("/media/asset/C");
  await api.quit();
  assert.equal(seen.length, 7);
  for (const [url, init] of seen) {
    assert.equal(init.credentials, "same-origin", url);
    if (init.method === "POST") assert.equal(init.headers["Content-Type"], "application/json", url);
  }
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

test("api_state_and_frame_success_shapes", async () => {
  let api = createApi(async () => resp(200, { cuts: [] }), () => {});
  assert.deepEqual(await api.getState(), { cuts: [] });
  const seen = [];
  api = createApi(async (url, init) => { seen.push([url, init]); return new Response(new Uint8Array([1, 2]), { status: 200 }); }, () => {});
  const r = await api.frame("C 1", 3);
  assert.equal(r.ok, true);
  assert.equal(r.blob.size, 2);
  assert.equal(seen[0][0], "/media/frame/C%201?n=3");
  assert.equal(seen[0][1].credentials, "same-origin");
});

test("api_head_network_failure_is_zero", async () => {
  const api = createApi(async () => { throw new TypeError("x"); }, () => {});
  assert.equal(await api.head("/media/asset/C"), 0);
});
