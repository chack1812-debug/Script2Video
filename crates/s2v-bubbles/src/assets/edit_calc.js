// 編集の操作の純粋な計算(DOM は触らない)。つまみの当たり判定・数値の欄の解釈・配置の比較。
import { tailTip } from "./geom.js";

const KEYS = ["x", "y", "w", "h", "tail", "start_offset_ms", "end_offset_ms"];

/** 配置から比較・要求に使う 7 つのキーだけを取り出して複製する(尻尾が無ければ null)。 */
export function placementOf(p) {
  const out = {};
  for (const k of KEYS) out[k] = p[k] === undefined ? null : p[k];
  out.tail = p.tail ? { dx: p.tail.dx, dy: p.tail.dy } : null;
  if (out.start_offset_ms === null) out.start_offset_ms = 0;
  if (out.end_offset_ms === null) out.end_offset_ms = 0;
  return out;
}

/** 2 つの配置が同じか(キーと尻尾の値で比べる)。 */
export function samePlacement(a, b) {
  if (!a || !b) return a === b;
  return JSON.stringify(placementOf(a)) === JSON.stringify(placementOf(b));
}

/**
 * 当たったつまみ: "tail"(尻尾の先端)、"nw"|"ne"|"sw"|"se"(四隅)、"body"(本体)、無ければ null。
 * 位置は canvas の座標。grab は画面の px での反応の半径。
 */
export function hitHandle(p, cx, cy, scale, grab = 8) {
  const r = grab / Math.max(scale, 1e-9);
  const tip = tailTip(p);
  if (tip && Math.abs(cx - tip.x) <= r && Math.abs(cy - tip.y) <= r) return "tail";
  const corners = [
    ["nw", p.x, p.y],
    ["ne", p.x + p.w, p.y],
    ["sw", p.x, p.y + p.h],
    ["se", p.x + p.w, p.y + p.h],
  ];
  let best = null;
  let bestD = Infinity;
  for (const [name, hx, hy] of corners) {
    const dx = Math.abs(cx - hx);
    const dy = Math.abs(cy - hy);
    if (dx <= r && dy <= r && dx + dy < bestD) {
      best = name;
      bestD = dx + dy;
    }
  }
  if (best) return best;
  if (cx >= p.x && cx <= p.x + p.w && cy >= p.y && cy <= p.y + p.h) return "body";
  return null;
}

/** 位置・大きさ・尻尾の欄の整数の解釈。 */
export function parseIntField(text) {
  const s = typeof text === "string" ? text.trim() : String(text);
  if (!/^[+-]?\d+$/.test(s)) return { ok: false, reason: "整数で入力してください" };
  const v = Number(s);
  if (!Number.isSafeInteger(v) || Math.abs(v) > 1_000_000) return { ok: false, reason: "値が大きすぎます" };
  return { ok: true, value: v };
}

/**
 * 数値の欄 field に値 value を入れた新しい配置(base は変えない)。範囲の検査はサーバーに任せる。
 * field: x y w h tail_dx tail_dy start_offset_ms end_offset_ms。尻尾が無いのに尻尾の欄なら null。
 */
export function withField(base, field, value) {
  const next = placementOf(base);
  if (field === "tail_dx" || field === "tail_dy") {
    if (!next.tail) return null;
    next.tail[field === "tail_dx" ? "dx" : "dy"] = value;
    return next;
  }
  if (!KEYS.includes(field) || field === "tail") return null;
  next[field] = value;
  return next;
}
