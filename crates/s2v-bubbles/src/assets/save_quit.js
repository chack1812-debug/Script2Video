// 保存と終了の判断(DOM に触れない純粋な部分)。app.js が結線する。

export const QUIT_CONFIRM_TEXT = "保存していない変更があります。終了しますか";
export const QUIT_DONE_TEXT = "終了しました。このタブは閉じてかまいません";
export const SESSION_LOST_QUIT_TEXT = "セッションが切れているため、終了の操作は送りません。bubbles を起動し直してください";

/**
 * api.save の結果を表示用にまとめる。
 * status: 状態欄の短い文。lines: 警告の一覧へ足す行(成功なら警告、422 なら理由の一覧)。
 */
export function formatSaveResult(r) {
  if (r && r.ok) return { ok: true, status: "保存しました", lines: Array.isArray(r.warnings) ? r.warnings : [] };
  const errors = r && Array.isArray(r.errors) && r.errors.length ? r.errors : [];
  if (r && r.status === 422) {
    return { ok: false, status: "保存できません(" + errors.length + " 件の理由を下に示します)", lines: errors.map((e) => "保存できません: " + e) };
  }
  // 422 以外は理由を並べず、固定の短い文にする
  const code = r && typeof r.status === "number" ? r.status : 0;
  let text;
  if (code === 0) text = "サーバーに接続できません";
  else if (code === 403) text = "セッションが切れました。bubbles を起動し直してください";
  else text = "保存できません(状態コード " + code + ")";
  return { ok: false, status: text, lines: [] };
}

/**
 * 終了の手順。保留中の入力の確定 → 待ち行列を空に → 未保存なら確認 → 終了の送信。
 * 戻り値の result: "lost"(セッション切れで送らない)/ "cancelled" / "quit" / "failed"(errors つき)。
 */
export async function decideQuit({ pendingSave = () => null, flushers, flush, isDirty, confirm, quit, sessionLost }) {
  if (sessionLost()) return { result: "lost" };
  // 進行中の保存があれば、その終わりを待ってから(その結果で未保存を判定する)
  const saving = pendingSave();
  if (saving) await saving;
  if (sessionLost()) return { result: "lost" };
  await Promise.all(flushers());
  await flush();
  if (sessionLost()) return { result: "lost" };
  if (isDirty() && !confirm(QUIT_CONFIRM_TEXT)) return { result: "cancelled" };
  // 確認のダイアログの間に切れたかもしれないので、送る直前にもう一度確かめる
  if (sessionLost()) return { result: "lost" };
  const r = await quit();
  if (r && r.ok) return { result: "quit" };
  return { result: "failed", errors: r && r.errors ? r.errors : ["失敗しました"] };
}
