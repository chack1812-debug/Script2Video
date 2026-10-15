// 座標・ドラッグ・当たり判定の計算(純粋な関数だけ。DOM は触らない)。
// 配置の形はサーバーの JSON と同じ {x, y, w, h, tail:{dx,dy}|null, start_offset_ms, end_offset_ms}。
// 返す配置は offset と(変えないときは)tail をそのまま引き継ぐ。

const clamp = (v, lo, hi) => Math.min(Math.max(v, lo), hi);

/** canvas を箱に縦横比を保って収め、中央に寄せる。 */
export function fitView(canvasW, canvasH, boxW, boxH) {
  const scale = Math.min(boxW / canvasW, boxH / canvasH);
  return {
    left: (boxW - canvasW * scale) / 2,
    top: (boxH - canvasH * scale) / 2,
    scale,
  };
}

/** 画面の座標 → canvas の座標(丸めない)。 */
export function toCanvas(px, py, view) {
  return { x: (px - view.left) / view.scale, y: (py - view.top) / view.scale };
}

/** canvas の座標 → 画面の座標。 */
export function toView(cx, cy, view) {
  return { x: view.left + cx * view.scale, y: view.top + cy * view.scale };
}

/** 四捨五入のうえ、1 ≤ w ≤ cw、1 ≤ h ≤ ch、画面の内側に収める。 */
export function clampRect(r, cw, ch) {
  const w = clamp(Math.round(r.w), 1, cw);
  const h = clamp(Math.round(r.h), 1, ch);
  const x = clamp(Math.round(r.x), 0, cw - w);
  const y = clamp(Math.round(r.y), 0, ch - h);
  return { x, y, w, h };
}

/** 尻尾の中心。サーバーの検証と同じく整数除算(floor)。 */
export function tailCenter(p) {
  return { x: p.x + Math.floor(p.w / 2), y: p.y + Math.floor(p.h / 2) };
}

/** 尻尾の先端(尻尾が無ければ null)。 */
export function tailTip(p) {
  if (!p.tail) return null;
  const c = tailCenter(p);
  return { x: c.x + p.tail.dx, y: c.y + p.tail.dy };
}

/** 先端が 0..=cw・0..=ch に収まるよう尻尾の dx・dy を詰める。尻尾が null ならそのまま。 */
export function clampTail(p, cw, ch) {
  if (!p.tail) return p;
  const c = tailCenter(p);
  const tx = clamp(c.x + p.tail.dx, 0, cw);
  const ty = clamp(c.y + p.tail.dy, 0, ch);
  return { ...p, tail: { dx: tx - c.x, dy: ty - c.y } };
}

/** 位置だけを動かす。 */
export function dragMove(start, dx, dy, cw, ch) {
  const r = clampRect({ x: start.x + dx, y: start.y + dy, w: start.w, h: start.h }, cw, ch);
  return clampTail({ ...start, ...r }, cw, ch);
}

/** 角を動かす。反対の角を固定し、大きさは 1 以上、画面の外に出さない。 */
export function dragCorner(start, corner, dx, dy, cw, ch) {
  let l = start.x;
  let t = start.y;
  let r = start.x + start.w;
  let b = start.y + start.h;
  const west = corner === "nw" || corner === "sw";
  const north = corner === "nw" || corner === "ne";
  if (west) l = clamp(Math.round(l + dx), 0, r - 1);
  else r = clamp(Math.round(r + dx), l + 1, cw);
  if (north) t = clamp(Math.round(t + dy), 0, b - 1);
  else b = clamp(Math.round(b + dy), t + 1, ch);
  const rect = clampRect({ x: l, y: t, w: r - l, h: b - t }, cw, ch);
  return clampTail({ ...start, ...rect }, cw, ch);
}

/** 尻尾の先端を canvas の座標 (px, py) に置いたときの {dx, dy}(中心からの相対)。 */
export function dragTail(start, px, py, cw, ch) {
  const c = tailCenter(start);
  const tx = clamp(Math.round(px), 0, cw);
  const ty = clamp(Math.round(py), 0, ch);
  return { dx: tx - c.x, dy: ty - c.y };
}

const NUDGE = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
};

/** 矢印キーで 1px(shift なら 10px)動かす。矢印キー以外は null。 */
export function nudge(p, key, shift, cw, ch) {
  const d = Object.prototype.hasOwnProperty.call(NUDGE, key) ? NUDGE[key] : null;
  if (!d) return null;
  const step = shift ? 10 : 1;
  return dragMove(p, d[0] * step, d[1] * step, cw, ch);
}

/** (cx, cy) を枠(端を含む)に含む項目のうち order が最大のものの ID。無ければ null。 */
export function hitTest(items, cx, cy) {
  let best = null;
  for (const it of items) {
    const p = it.placed;
    if (!p) continue;
    if (cx < p.x || cx > p.x + p.w || cy < p.y || cy > p.y + p.h) continue;
    if (best === null || it.order > best.order) best = it;
  }
  return best ? best.id : null;
}
