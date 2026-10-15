// 入口。状態(state・editor・cache・frames・ui)を持ち、view.js に描かせる。
// 再生(player.js)は Task 6、編集(editor_ui.js)は Task 7・8 がここに繋ぐ(下の ext と redraw が口)。
import { createApi } from "./api.js";
import { Editor, FrameFetcher, ImageCache, withKind } from "./model.js";
import { assetFrame, cutAt, itemsAt } from "./timeline.js";
import { mountView } from "./view.js";

const SESSION_LOST_TEXT = "セッションが切れました。bubbles を起動し直してください";

async function main() {
  const ui = { n: 0, selected: null, playing: false, dragging: false, sessionLost: false };
  // 後続のタスクが差し込む口(player: Task 6、editing: Task 7・8)
  const ext = { player: null, editing: null };
  let view = null;

  const setStatus = (text) => {
    if (view) view.setStatus(text);
    else document.getElementById("status").textContent = text;
  };

  const onSessionLost = () => {
    ui.sessionLost = true;
    ui.playing = false;
    setStatus(SESSION_LOST_TEXT);
  };

  const api = createApi((url, init) => fetch(url, init), onSessionLost);
  const got = await api.getState();
  if (got.ok === false) {
    setStatus((got.errors || ["状態を取得できません"]).join(" / "));
    return;
  }
  const state = got;
  const editor = new Editor(api, state);
  const cache = new ImageCache(300, (u) => URL.revokeObjectURL(u));
  const frames = new FrameFetcher((cutId, n) => api.frame(cutId, n));

  // --- 背景 ---
  const frameMode = new Set(); // 動画を読み込めず frame に切り替えたカットの ID
  let frameShown = null; // { cutId, n, url }: 表示中の frame

  const bgSpec = (kind, extra) => ({ kind, fit: state.fit, background: state.background, ...extra });

  function showFrame(cut, n) {
    frames.setCurrent(cut.id, n);
    if (frameShown && frameShown.cutId === cut.id && frameShown.n === n) {
      view.setBackground(bgSpec("frame", { url: frameShown.url }));
      return;
    }
    if (ui.sessionLost) return;
    frames.request(
      cut.id,
      n,
      (blob) => {
        const url = URL.createObjectURL(blob);
        if (frameShown) URL.revokeObjectURL(frameShown.url);
        frameShown = { cutId: cut.id, n, url };
        view.setBackground(bgSpec("frame", { url }));
      },
      () => view.setBackground(bgSpec("unavailable")),
    );
  }

  // 素材の読み込みエラー: HEAD で 403 ならセッション切れ(api が onSessionLost を呼ぶ)
  async function onAssetError(cut, kind) {
    const status = await api.head(cut.asset.url);
    if (status === 403 || ui.sessionLost) return;
    if (kind === "video" && cut.asset.frame_url) {
      frameMode.add(cut.id);
      redraw(ui.n);
    } else {
      view.setBackground(bgSpec("unavailable"));
    }
  }

  function updateBackground(cut, n) {
    if (!cut || !cut.asset) {
      view.setBackground(bgSpec("none"));
      return;
    }
    const a = cut.asset;
    if (a.kind !== "video") {
      view.setBackground(bgSpec("image", { url: a.url, onError: () => onAssetError(cut, "image") }));
      return;
    }
    if (frameMode.has(cut.id)) {
      showFrame(cut, n);
      return;
    }
    // 止まっているときは素材の中のコマ k の時刻に置く(再生中の同期は player.js)
    const k = assetFrame(n, cut.frames[0], a.frames, a.looped);
    view.setBackground(bgSpec("video", { url: a.url, time: k / state.fps, onError: () => onAssetError(cut, "video") }));
  }

  // --- 重ね画像 ---
  const requested = new Set(); // 要求済みの「ID@版」(失敗しても同じ版では繰り返さない)

  function requestImage(item) {
    if (ui.sessionLost) return;
    const v = cache.version(item.id);
    const key = item.id + "@" + v;
    if (requested.has(key)) return;
    requested.add(key);
    api.render({ id: item.id, manual: withKind(editor.confirmed, editor.items) }).then((r) => {
      if (!r.ok) {
        setStatus("重ね画像を取得できません: " + (r.errors || []).join(" / "));
        return;
      }
      // 版が変わっていれば set が false を返し、blob URL も解放される
      if (cache.set(item.id, URL.createObjectURL(r.png), v)) redraw(ui.n);
    });
  }

  // --- コマの描画 ---
  function redraw(n) {
    ui.n = n;
    view.setFrame(n);
    const cut = cutAt(state, n);
    updateBackground(cut, n);
    view.renderFrame({
      n,
      cut,
      items: itemsAt(editor.items, n),
      cache,
      selected: ui.selected,
      onMissing: requestImage,
    });
  }

  function refreshChrome() {
    view.renderList(editor.items, ui.selected);
    view.setNumbers(editor.items.find((it) => it.id === ui.selected) || null);
    view.setDirty(editor.isDirty());
  }

  // --- 操作 ---
  const handlers = {
    onSelectItem(id) {
      ui.selected = id;
      const it = editor.items.find((x) => x.id === id);
      refreshChrome();
      if (it && !(it.frames[0] <= ui.n && ui.n < it.frames[1])) seek(it.frames[0]);
      else redraw(ui.n);
    },
    onSeek: (n) => seek(n),
    onPlayToggle() {
      if (ui.sessionLost) return;
      if (ext.player) ui.playing ? ext.player.stop() : ext.player.play();
    },
    async onSave() {
      if (ui.sessionLost) return;
      if (ext.editing) await ext.editing.flushPending();
      const r = await editor.save();
      if (r.ok) {
        view.setWarnings([...state.warnings, ...r.warnings]);
        setStatus("保存しました");
      } else {
        setStatus("保存できません: " + (r.errors || []).join(" / "));
      }
      view.setDirty(editor.isDirty());
    },
    async onQuit() {
      const r = await api.quit();
      setStatus(r.ok ? "終了しました。このタブは閉じてかまいません" : (r.errors || []).join(" / "));
    },
    onRevertAuto() {},
    onNumberCommit() {},
  };

  function seek(n) {
    if (ext.player) ext.player.seek(n);
    redraw(n);
  }

  view = mountView(document.body, state, handlers);
  view.setWarnings(state.warnings);
  setStatus(
    "シーン " + state.scenes.length + " 件、項目 " + state.items.length + " 件、警告 " + state.warnings.length + " 件",
  );
  refreshChrome();
  redraw(0);
  if (ui.sessionLost) setStatus(SESSION_LOST_TEXT);
}

main();
