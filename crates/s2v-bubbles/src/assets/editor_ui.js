// 編集の操作(選択・ドラッグ・矢印キー・数値の欄・自動に戻す)。DOM を触るが、計算は geom.js と edit_calc.js に任せる。
// 確定は model.Editor.enqueue に渡し(直列化はそちら)、結果を画面に反映する。
import { dragCorner, dragMove, dragTail, fitView, hitTest, tailTip, toCanvas, toView } from "./geom.js";
import { hitHandle } from "./edit_calc.js";
import { EditSession } from "./edit_session.js";
import { itemsAt } from "./timeline.js";

const FORM_TAGS = new Set(["INPUT", "TEXTAREA", "BUTTON", "SELECT"]);
const CORNERS = ["nw", "ne", "sw", "se"];

function div(cls) {
  const e = document.createElement("div");
  e.className = cls;
  return e;
}

/**
 * 編集の操作を結ぶ。戻り値:
 *  flushPending(): 矢印キーの待ちと入力中の数値の欄を確定の待ち行列へ入れて、その完了を待つ。
 *  sync(): 選択中の項目のつまみ・輪郭を今の状態に合わせて描き直す(redraw の後に app.js が呼ぶ)。
 *  commitNumber(id, field, text) / revertAuto(id): 数値の欄・「自動に戻す」からの呼び出し。
 */
export function attachEditing({ view, editor, state, ui, player, cache, redraw, toast }) {
  const box = view.previewElement();
  const overlay = view.overlayLayer();
  const cw = state.canvas.w;
  const ch = state.canvas.h;
  const inputs = view.numberInputs();

  // --- つまみと輪郭(overlay の中。pointer-events は none。操作は box で受ける) ---
  const outline = div("drag-outline");
  const handles = {};
  for (const c of CORNERS) handles[c] = div("handle corner");
  handles.tail = div("handle tail");
  outline.hidden = true;
  overlay.appendChild(outline);
  for (const h of Object.values(handles)) {
    h.hidden = true;
    overlay.appendChild(h);
  }

  // 確定するまで見せる配置(ドラッグ中・矢印キー中・確定の待ち中)。{ id, placed }
  let shadow = null;
  let drag = null; // { id, kind, start, sx, sy, pointerId }

  const itemById = (id) => editor.items.find((it) => it.id === id) || null;
  const selectedVisible = () => {
    if (!ui.selected) return null;
    return itemsAt(editor.items, ui.n).find((it) => it.id === ui.selected) || null;
  };
  const curFit = () => view.currentFit() || fitView(cw, ch, Math.max(box.clientWidth, 1), Math.max(box.clientHeight, 1));

  function place(el, left, top, w, h, fit) {
    const q = toView(left, top, fit);
    el.style.left = q.x + "px";
    el.style.top = q.y + "px";
    if (w !== undefined) {
      el.style.width = w * fit.scale + "px";
      el.style.height = h * fit.scale + "px";
    }
  }

  function sync() {
    const item = ui.sessionLost ? null : selectedVisible();
    if (!item) {
      outline.hidden = true;
      for (const h of Object.values(handles)) h.hidden = true;
      return;
    }
    const fit = curFit();
    const showing = shadow && shadow.id === item.id;
    const p = showing ? shadow.placed : item.placed;
    outline.hidden = !showing;
    if (showing) place(outline, p.x, p.y, p.w, p.h, fit);
    const pts = { nw: [p.x, p.y], ne: [p.x + p.w, p.y], sw: [p.x, p.y + p.h], se: [p.x + p.w, p.y + p.h] };
    for (const c of CORNERS) {
      handles[c].hidden = false;
      place(handles[c], pts[c][0], pts[c][1], undefined, undefined, fit);
    }
    const tip = tailTip(p);
    handles.tail.hidden = !tip;
    if (tip) place(handles.tail, tip.x, tip.y, undefined, undefined, fit);
  }

  if (typeof ResizeObserver === "function") new ResizeObserver(() => sync()).observe(box);

  // --- 反映 ---
  function refresh(keepFocusedInput) {
    view.renderList(editor.items, ui.selected);
    const active = document.activeElement;
    const keep = keepFocusedInput && active && [...inputs.values()].includes(active) ? { el: active, v: active.value } : null;
    view.setNumbers(itemById(ui.selected));
    if (keep && !keep.el.disabled) keep.el.value = keep.v;
    view.setDirty(editor.isDirty());
    redraw(ui.n);
  }

  /** 配置を確定の待ち行列へ入れ、結果を反映する。placement が null なら自動に戻す。 */
  async function commit(id, placement, { fromNumber = false } = {}) {
    if (ui.sessionLost) return;
    const token = placement ? { id, placed: placement } : null;
    if (token) shadow = token;
    const r = await editor.enqueue(id, placement);
    if (shadow === token) shadow = null;
    if (ui.sessionLost) return;
    if (r.rejected) toast("確定できません: " + (r.errors || []).join(" / "));
    else cache.drop(r.changed);
    // 失敗したときも、輪郭・数値の欄を確定値へ戻す
    refresh(!fromNumber);
  }

  // 編集の状態(矢印の待ち・基準値・二重の確定の判定)は edit_session.js
  const session = new EditSession({
    editor,
    cw,
    ch,
    hooks: {
      commit,
      preview: (id, placed) => {
        shadow = { id, placed };
        sync();
      },
      discard: () => {
        shadow = null;
        sync();
      },
    },
    activeInput: () => {
      const active = document.activeElement;
      for (const [field, el] of inputs) if (el === active && ui.selected) return { id: ui.selected, field, text: el.value };
      return null;
    },
  });

  function select(id) {
    session.beforeOther();
    ui.selected = id;
    refresh(false);
  }

  // --- プレビューのクリックとドラッグ ---
  const canvasPoint = (e) => {
    const r = box.getBoundingClientRect();
    return toCanvas(e.clientX - r.left, e.clientY - r.top, curFit());
  };

  function dragPlaced(d, c) {
    const dx = c.x - d.sx;
    const dy = c.y - d.sy;
    if (d.kind === "body") return dragMove(d.start, dx, dy, cw, ch);
    if (d.kind === "tail") return { ...d.start, tail: dragTail(d.start, c.x, c.y, cw, ch) };
    return dragCorner(d.start, d.kind, dx, dy, cw, ch);
  }

  box.addEventListener("pointerdown", (e) => {
    if (ui.sessionLost || e.button !== 0 || drag) return;
    // 数値の欄にフォーカスが残らないよう、入力を確定して外す(以後の矢印キーを項目へ渡す)
    session.releaseInput(() => document.activeElement.blur());
    const c = canvasPoint(e);
    const sel = selectedVisible();
    const kind = sel ? hitHandle(shadow && shadow.id === sel.id ? shadow.placed : sel.placed, c.x, c.y, curFit().scale) : null;
    if (sel && kind) {
      e.preventDefault();
      // capture できなければドラッグを始めない(pointerup を取りこぼすため)
      try {
        box.setPointerCapture(e.pointerId);
      } catch {
        return;
      }
      if (player && player.stop) player.stop();
      drag = { id: sel.id, kind, start: session.dragStart(sel), sx: c.x, sy: c.y, pointerId: e.pointerId };
      ui.dragging = true;
      return;
    }
    // ドラッグではなく選択: 重なっていれば前面の項目、空白なら選択を外す
    select(hitTest(itemsAt(editor.items, ui.n), c.x, c.y));
  });

  box.addEventListener("pointermove", (e) => {
    if (drag) {
      if (e.pointerId !== drag.pointerId) return;
      const p = dragPlaced(drag, canvasPoint(e));
      shadow = { id: drag.id, placed: p };
      sync();
      return;
    }
    const sel = selectedVisible();
    const c = canvasPoint(e);
    const k = sel ? hitHandle(sel.placed, c.x, c.y, curFit().scale) : null;
    box.style.cursor = k === "body" ? "move" : k ? "pointer" : "";
  });

  function endDrag(e, apply) {
    if (!drag || e.pointerId !== drag.pointerId) return;
    const d = drag;
    drag = null;
    ui.dragging = false;
    try {
      box.releasePointerCapture(e.pointerId);
    } catch {
      // すでに解放されている
    }
    const placed = apply ? dragPlaced(d, canvasPoint(e)) : null;
    if (!session.dragEnd(itemById(d.id), placed)) {
      shadow = null; // 取り消し、または動いていない: 輪郭を捨てて元に戻す
      sync();
    }
  }
  box.addEventListener("pointerup", (e) => endDrag(e, true));
  box.addEventListener("pointercancel", (e) => endDrag(e, false));

  // --- 矢印キー ---
  document.addEventListener("keydown", (e) => {
    if (ui.sessionLost || ui.dragging) return;
    const t = e.target;
    if (t && FORM_TAGS.has(t.tagName)) return; // フォームの部品は自分の既定の動作を優先
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (!ui.selected) return;
    const item = itemById(ui.selected);
    if (!item) return;
    const key = e.key;
    if (!key.startsWith("Arrow")) return;
    e.preventDefault();
    if (player && player.stop) player.stop();
    session.arrowKey(item, key, e.shiftKey);
  });

  // --- 数値の欄 ---
  function commitNumber(id, field, text) {
    if (ui.sessionLost) return Promise.resolve();
    const r = session.commitNumber(id, field, text);
    if (r.kind === "commit") return r.promise;
    if (r.kind === "invalid") toast(r.reason);
    if (ui.selected === id) refresh(false); // 不正・同じ値: 欄を確定値に戻す(表記ゆれも整える)
    return Promise.resolve();
  }

  for (const [field, input] of inputs) {
    input.addEventListener("keydown", (e) => {
      if (e.key !== "Enter") return;
      e.preventDefault();
      if (ui.selected) commitNumber(ui.selected, field, input.value);
    });
  }

  function revertAuto(id) {
    if (ui.sessionLost) return Promise.resolve();
    return session.revert(id) || Promise.resolve();
  }

  // 保存・終了の前に、待っている入力を確定する
  editor.addPendingFlusher(() => session.flushArrow());
  editor.addPendingFlusher(() => session.flushInput());

  async function flushPending() {
    await session.flushAll();
    await editor.flush();
  }

  return { flushPending, sync, commitNumber, revertAuto };
}
