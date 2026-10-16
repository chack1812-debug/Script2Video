// サーバー(bubbles)との通信。実際の応答を正規化した結果に変換する。DOM は触らない。
// 失敗は例外にせず {ok:false, status, errors:[…]} の値で返す(getState も同じ。例外を投げない)。
// 403 はセッション切れなので onSessionLost() を呼ぶ。

const SESSION_LOST = "セッションが切れました。bubbles を起動し直してください";
const NO_CONNECT = "サーバーに接続できません";

const fail = (status, errors) => ({ ok: false, status, errors });

export function createApi(fetchFn, onSessionLost) {
  /** fetch を呼ぶ。ネットワークの失敗は null を返す。 */
  async function call(url, init) {
    try {
      return await fetchFn(url, { credentials: "same-origin", ...init });
    } catch {
      return null;
    }
  }

  /** 失敗の応答(200 以外)を正規化する。 */
  async function failure(r) {
    if (r.status === 403) {
      onSessionLost();
      return fail(403, [SESSION_LOST]);
    }
    let body = null;
    try {
      body = await r.json();
    } catch {
      body = null;
    }
    if (r.status === 422 && body && Array.isArray(body.errors)) {
      return fail(422, body.errors);
    }
    const msg = body && typeof body.error === "string" ? body.error : `通信に失敗しました(${r.status})`;
    return fail(r.status, [msg]);
  }

  /** 要求を送り、200 なら ok(r) の結果、そうでなければ正規化した失敗を返す。 */
  async function request(url, init, ok) {
    const r = await call(url, init);
    if (r === null) return fail(0, [NO_CONNECT]);
    if (r.status !== 200) return failure(r);
    try {
      return await ok(r);
    } catch {
      return fail(r.status, [`通信に失敗しました(${r.status})`]);
    }
  }

  const post = (url, body, ok) =>
    request(
      url,
      { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) },
      ok,
    );

  return {
    getState: () => request("/api/state", { method: "GET" }, (r) => r.json()),
    render: (body) =>
      post("/api/render", body, async (r) => {
        const origin = (r.headers.get("X-S2V-Origin") || "0,0").split(",").map(Number);
        return {
          ok: true,
          png: await r.blob(),
          origin: [origin[0], origin[1]],
          clipped: r.headers.get("X-S2V-Clipped") === "1",
        };
      }),
    layout: (body) => post("/api/layout", body, async (r) => ({ ok: true, items: await r.json() })),
    save: (body) =>
      post("/api/save", body, async (r) => {
        const j = await r.json();
        return { ok: true, warnings: j.warnings || [] };
      }),
    frame: (cutId, n) =>
      request(`/media/frame/${encodeURIComponent(cutId)}?n=${n}`, { method: "GET" }, async (r) => ({
        ok: true,
        blob: await r.blob(),
      })),
    /** 状態コードの数値だけを返す(ネットワークの失敗は 0)。403 は onSessionLost も呼ぶ。 */
    async head(url) {
      const r = await call(url, { method: "HEAD" });
      if (r === null) return 0;
      if (r.status === 403) onSessionLost();
      return r.status;
    },
    quit: () => post("/api/quit", {}, async () => ({ ok: true })),
  };
}
