// 非同期の応答を受け入れるかどうかの判断(純粋関数・小さな状態)。DOM には触れない。

// 重ね画像の要求の追跡: 実行中のものだけを覚える。完了(成功も失敗も)で忘れるので、
// キャッシュから捨てられた PNG や、失敗した要求は、次の描画で再び要求できる。
// キーは「ID:版」。新しい版の要求は、古い版の実行中の要求に邪魔されない。
export class ImageRequestTracker {
  constructor() {
    this._inflight = new Set();
  }

  _key(id, version) {
    return id + ":" + version;
  }

  // 要求してよければ true を返し、実行中として記録する。
  shouldRequest(id, version) {
    const k = this._key(id, version);
    if (this._inflight.has(k)) return false;
    this._inflight.add(k);
    return true;
  }

  // 要求の完了(成功・失敗どちらでも)。
  done(id, version) {
    this._inflight.delete(this._key(id, version));
  }
}

// frame の応答を今の表示に出してよいか。current は今のカットとコマ({ cutId, n }。無ければ null)。
export function acceptFrame(current, response) {
  return !!current && !!response && current.cutId === response.cutId && current.n === response.n;
}
