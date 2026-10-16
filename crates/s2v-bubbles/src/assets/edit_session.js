// 編集の状態の管理(DOM に触れない)。矢印キーの待ち・保留中の入力・二重の確定の判定・
// 新しい操作の前の保留の確定・次の操作の基準値の決定を持つ。タイマーは差し込む。
// hooks = { commit(id, placement|null, {fromNumber}) -> Promise, preview(id, placed), discard() }
import { nudge } from "./geom.js";
import { parseIntField, placementOf, samePlacement, withField } from "./edit_calc.js";
import { validateOffset } from "./model.js";

export class EditSession {
  constructor({ editor, cw, ch, hooks, setTimer = (f, ms) => setTimeout(f, ms), clearTimer = (h) => clearTimeout(h), delayMs = 300, activeInput = () => null }) {
    this.editor = editor;
    this.cw = cw;
    this.ch = ch;
    this.hooks = hooks;
    // ブラウザの setTimeout は this が window 以外だと投げるため、既定はアロー関数で包む
    this._set = setTimer;
    this._clear = clearTimer;
    this.delayMs = delayMs;
    this.activeInput = activeInput; // () => { id, field, text } | null(入力中の欄)
    this.arrow = null; // { id, placed, timer }
  }

  item(id) {
    return this.editor.items.find((it) => it.id === id) || null;
  }

  /** 次の操作の基準: 保留中の矢印の値、無ければ最後に待ち行列へ入れた手動の値、無ければ今の配置。 */
  baseOf(item) {
    if (this.arrow && this.arrow.id === item.id) return placementOf(this.arrow.placed);
    return placementOf(this.editor.lastQueued(item.id) || item.placed);
  }

  /** 最後に待ち行列へ入れた値(無ければ今の配置)と同じなら true。 */
  isUnchanged(item, next) {
    return samePlacement(next, this.editor.lastQueued(item.id) || item.placed);
  }

  /** 矢印キー 1 回。動かせたら新しい配置、矢印キー以外は null。最後の押下から delayMs 後に 1 回だけ確定する。 */
  arrowKey(item, key, shift) {
    const next = nudge(this.baseOf(item), key, shift, this.cw, this.ch);
    if (!next) return null;
    if (this.arrow && this.arrow.id !== item.id) this.flushArrow();
    const a = this.arrow || { id: item.id, placed: null, timer: null };
    if (a.timer !== null) this._clear(a.timer);
    a.id = item.id;
    a.placed = next;
    a.timer = this._set(() => this.flushArrow(), this.delayMs);
    this.arrow = a;
    this.hooks.preview(item.id, next);
    return next;
  }

  /** 保留中の矢印の編集を今すぐ enqueue する。 */
  flushArrow() {
    const a = this.arrow;
    if (!a) return Promise.resolve();
    this.arrow = null;
    if (a.timer !== null) this._clear(a.timer);
    const item = this.item(a.id);
    if (!item || this.isUnchanged(item, a.placed)) {
      this.hooks.discard();
      return Promise.resolve();
    }
    return this.hooks.commit(a.id, a.placed, {});
  }

  /** ドラッグの開始: 保留を先に確定し、基準を最後に待ち行列へ入れた値にする。 */
  dragStart(item) {
    this.flushArrow();
    return this.baseOf(item);
  }

  /** ドラッグの結果を確定する。変化が無ければ null(輪郭を捨てるのは呼び出し側)。 */
  dragEnd(item, placed) {
    if (!item || !placed || this.isUnchanged(item, placed)) return null;
    return this.hooks.commit(item.id, placed, {});
  }

  /**
   * 数値の欄の確定。{kind:"invalid",reason} | {kind:"unchanged"} | {kind:"commit", promise}。
   * Enter の後の change など、同じ値の 2 回目は unchanged になる。
   */
  commitNumber(id, field, text) {
    const item = this.item(id);
    if (!item) return { kind: "unchanged" };
    const parsed = field.endsWith("_ms") ? validateOffset(text) : parseIntField(text);
    if (!parsed.ok) return { kind: "invalid", reason: parsed.reason };
    this.flushArrow();
    const next = withField(this.baseOf(item), field, parsed.value);
    if (!next || this.isUnchanged(item, next)) return { kind: "unchanged" };
    return { kind: "commit", promise: this.hooks.commit(id, next, { fromNumber: true }) };
  }

  /** 自動に戻す。すでに自動なら null。 */
  revert(id) {
    if (this.arrow && this.arrow.id === id) {
      this._clear(this.arrow.timer);
      this.arrow = null;
      this.hooks.discard();
    } else {
      this.flushArrow();
    }
    if (!this.editor.lastQueued(id)) return null;
    return this.hooks.commit(id, null, {});
  }

  /** 選択の変更など、別の操作の前に保留を確定する。 */
  beforeOther() {
    return this.flushArrow();
  }

  /** 入力中の欄を確定する(無ければ何もしない)。 */
  flushInput() {
    const a = this.activeInput();
    if (!a) return Promise.resolve();
    const r = this.commitNumber(a.id, a.field, a.text);
    return r.kind === "commit" ? r.promise : Promise.resolve();
  }

  /** 保存・終了の前: 保留中の矢印と入力中の欄を確定の待ち行列へ入れる。 */
  async flushAll() {
    await Promise.all([this.flushArrow(), this.flushInput()]);
  }
}
