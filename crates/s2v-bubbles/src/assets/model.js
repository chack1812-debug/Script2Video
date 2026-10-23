// 手動配置・確定の待ち行列・保存・PNG の保持・frame の集約。
// DOM にも fetch にも触れない。API は呼び出し側から渡される。

const PLACEMENT_KEYS = ["x", "y", "w", "h", "tail", "start_offset_ms", "end_offset_ms"];

// 配置から要求に使うキーだけを取り出す(尻尾が無ければ null)。
function pickPlacement(p) {
  const out = {};
  for (const k of PLACEMENT_KEYS) out[k] = p[k] === undefined ? null : p[k];
  return out;
}

// 手動の一式に、各項目の kind を足した新しい一式を返す(元は変えない)。
export function withKind(manual, items) {
  const kinds = new Map(items.map((it) => [it.id, it.kind]));
  const out = {};
  for (const [id, p] of Object.entries(manual)) {
    out[id] = { ...pickPlacement(p), kind: kinds.get(id) };
  }
  return out;
}

// 開始・終了のずれ(ミリ秒)の検査。整数で |v| ≤ 60000。数字の文字列も受ける。
export function validateOffset(v) {
  let n;
  if (typeof v === "number") n = v;
  else if (typeof v === "string" && /^-?\d+$/.test(v.trim())) n = Number(v.trim());
  else return { ok: false, reason: "整数で入力してください" };
  if (!Number.isInteger(n)) return { ok: false, reason: "整数で入力してください" };
  if (Math.abs(n) > 60000) return { ok: false, reason: "±60000 の範囲で入力してください" };
  return { ok: true, value: n };
}

function sameImage(a, b) {
  if (!a || !b) return a === b;
  return a.dx === b.dx && a.dy === b.dy && a.w === b.w && a.h === b.h;
}

function samePlaced(a, b) {
  return a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h && JSON.stringify(a.tail ?? null) === JSON.stringify(b.tail ?? null);
}

// 確定の前後で PNG を捨てる ID: 見た目が違う項目と、新しく現れた項目。
export function changedImages(oldItems, newItems) {
  const old = new Map(oldItems.map((it) => [it.id, it]));
  const out = [];
  for (const n of newItems) {
    const o = old.get(n.id);
    if (!o || !samePlaced(o.placed, n.placed) || o.clipped !== n.clipped || !sameImage(o.image, n.image)) out.push(n.id);
  }
  return out;
}

// 一式を比較用の文字列にする(ID 順・キー順を固定)。
function normalize(manual) {
  const out = {};
  for (const id of Object.keys(manual).sort()) out[id] = pickPlacement(manual[id]);
  return JSON.stringify(out);
}

function copyManual(manual) {
  const out = {};
  for (const [id, p] of Object.entries(manual)) out[id] = { ...p, tail: p.tail ? { ...p.tail } : p.tail };
  return out;
}

function errorsOf(r) {
  return r && Array.isArray(r.errors) && r.errors.length ? r.errors : ["失敗しました"];
}

export class Editor {
  // isLost: セッションが切れているかを返す関数(保存の直前の確認に使う)
  constructor(api, state, isLost = () => false) {
    this.api = api;
    this.isLost = isLost;
    this.items = state.items;
    this.confirmed = {};
    for (const it of state.items) if (it.manual) this.confirmed[it.id] = pickPlacement(it.manual);
    this.saved = normalize(this.confirmed);
    this._flushers = [];
    this._tail = Promise.resolve();
    this._busy = 0; // 待ち行列に入っていて未完了の件数
    this._queued = new Map(); // id → { value, count }: 最後に待ち行列へ入れた値
  }

  addPendingFlusher(fn) {
    this._flushers.push(fn);
  }

  // 最後に待ち行列へ入れた値(処理中・未処理を含む)。無ければ確定済みの値。
  lastQueued(id) {
    const q = this._queued.get(id);
    if (q) return q.value;
    return this.confirmed[id] ?? null;
  }

  enqueue(id, placement) {
    const value = placement ? pickPlacement(placement) : null;
    const q = this._queued.get(id) || { value: null, count: 0 };
    q.value = value;
    q.count++;
    this._queued.set(id, q);
    this._busy++;
    const run = async () => {
      try {
        return await this._commit(id, value);
      } finally {
        this._busy--;
        q.count--;
        if (q.count === 0 && this._queued.get(id) === q) this._queued.delete(id);
      }
    };
    const p = this._tail.then(run, run);
    this._tail = p.catch(() => {});
    return p;
  }

  // 1 件の確定: render と layout の両方が ok のときだけ状態を変える。
  async _commit(id, value) {
    const next = copyManual(this.confirmed);
    if (value) next[id] = value;
    else delete next[id];
    try {
      const r = await this.api.render({ id, manual: withKind(next, this.items) });
      if (!r || !r.ok) return { rejected: true, id, errors: errorsOf(r) };
      const png = r.png;
      const l = await this.api.layout({ manual: withKind(next, this.items) });
      if (!l || !l.ok) return { rejected: true, id, errors: errorsOf(l) };
      const changed = changedImages(this.items, l.items);
      this.confirmed = next;
      this.items = l.items;
      return { ok: true, changed, png };
    } catch (e) {
      return { rejected: true, id, errors: [String(e && e.message ? e.message : e)] };
    }
  }

  // 保留中の入力(矢印キー・数値の欄)の確定を待ち行列へ入れ、その呼び出しの完了を待つ。
  runPendingFlushers() {
    return Promise.all(this._flushers.map((f) => f()));
  }

  // 待ち行列が空になるまで待つ。
  async flush() {
    while (this._busy > 0) await this._tail;
  }

  // 保留中の入力を確定し、待ち行列を空にしてから、その時点の一式だけを送る。
  async save() {
    await this.runPendingFlushers();
    await this.flush();
    if (this.isLost()) {
      return { ok: false, status: 403, errors: ["セッションが切れました。bubbles を起動し直してください"] };
    }
    const sent = copyManual(this.confirmed);
    const r = await this.api.save({ manual: withKind(sent, this.items) });
    if (r && r.ok) this.saved = normalize(sent);
    return r;
  }

  isDirty() {
    return normalize(this.confirmed) !== this.saved;
  }
}

// 停止中のコマの取得: 同時に 1 件、待っている間は最新の 1 件だけ、古い応答は捨てる。
export class FrameFetcher {
  constructor(fetchFrame) {
    this._fetch = fetchFrame;
    this._cur = null;
    this._busy = false;
    this._waiting = null;
  }

  setCurrent(cutId, n) {
    this._cur = { cutId, n };
  }

  // 今のコマが frame を要らなくなった(別のカット・画像・素材なし・再生中)ときに、取得中の応答を無効にする。
  clear() {
    this._cur = null;
  }

  request(cutId, n, onResult, onError) {
    const req = { cutId, n, onResult, onError };
    if (this._busy) {
      this._waiting = req;
      return;
    }
    this._start(req);
  }

  async _start(req) {
    this._busy = true;
    let res;
    try {
      res = await this._fetch(req.cutId, req.n);
    } catch (e) {
      res = { ok: false, status: 0, errors: [String(e && e.message ? e.message : e)] };
    }
    this._busy = false;
    // 今のコマと一致する応答だけを渡す(古い成功も失敗も捨てる)
    if (this._cur && this._cur.cutId === req.cutId && this._cur.n === req.n) {
      if (res && res.ok) req.onResult(res.blob);
      else if (req.onError) req.onError({ status: (res && res.status) || 0, errors: errorsOf(res) });
    }
    const next = this._waiting;
    this._waiting = null;
    if (next) this._start(next);
  }
}

// 重ね画像(blob URL)の保持。項目ごとの版で古い応答を捨て、上限を超えたら使った順に捨てる。
export class ImageCache {
  constructor(max = 300, revoke = () => {}) {
    this._max = max;
    this._revoke = revoke;
    this._map = new Map(); // id → url(挿入順 = 使った順)
    this._ver = new Map();
  }

  version(id) {
    return this._ver.get(id) || 0;
  }

  get(id) {
    if (!this._map.has(id)) return undefined;
    const url = this._map.get(id);
    this._map.delete(id);
    this._map.set(id, url);
    return url;
  }

  set(id, url, requestedVersion) {
    if (requestedVersion !== this.version(id)) {
      this._revoke(url);
      return false;
    }
    if (this._map.has(id)) {
      const old = this._map.get(id);
      this._map.delete(id);
      if (old !== url) this._revoke(old);
    }
    this._map.set(id, url);
    while (this._map.size > this._max) {
      const oldest = this._map.keys().next().value;
      this._revoke(this._map.get(oldest));
      this._map.delete(oldest);
    }
    return true;
  }

  drop(ids) {
    for (const id of ids) {
      if (this._map.has(id)) {
        this._revoke(this._map.get(id));
        this._map.delete(id);
      }
      this._ver.set(id, this.version(id) + 1);
    }
  }
}
