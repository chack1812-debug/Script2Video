// 画面の表示(DOM)。状態は持たず、app.js から渡された値を描くだけ。
// 台本由来の文字列は textContent だけで入れる(HTML として解釈させない)。
import { fitView, toView } from "./geom.js";
import { sliderMax } from "./timeline.js";

const NUMBER_FIELDS = [
  ["x", "X"],
  ["y", "Y"],
  ["w", "幅"],
  ["h", "高さ"],
  ["tail_dx", "尻尾 dx"],
  ["tail_dy", "尻尾 dy"],
  ["start_offset_ms", "開始のずれ(ms)"],
  ["end_offset_ms", "終了のずれ(ms)"],
];

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
}

/**
 * 画面を組み立てる。root の中の要素は id で探す(index.html が骨組みを持つ)。
 * handlers = {onSelectItem, onSeek, onPlayToggle, onSave, onQuit, onRevertAuto, onNumberCommit}
 */
export function mountView(root, state, handlers) {
  const $ = (id) => root.querySelector("#" + id);
  const box = $("preview");
  const statusEl = $("status");
  const dirtyEl = $("dirty");
  const listEl = $("list");
  const warnEl = $("warnings");
  const numbersEl = $("numbers");
  const slider = $("seek");
  const frameLabel = $("frame-label");
  const playBtn = $("play");
  const canvasW = state.canvas.w;
  const canvasH = state.canvas.h;

  // --- プレビューの箱: 背景の舞台(canvas の範囲)と重ねる層 ---
  const stage = el("div", "stage");
  const overlay = el("div", "overlay");
  box.appendChild(stage);
  box.appendChild(overlay);

  let fit = fitView(canvasW, canvasH, Math.max(box.clientWidth, 1), Math.max(box.clientHeight, 1));
  let lastFrame = null;
  let bgKey = null;
  let bgEl = null; // <img> か <video>

  function applyFit() {
    fit = fitView(canvasW, canvasH, Math.max(box.clientWidth, 1), Math.max(box.clientHeight, 1));
    stage.style.left = fit.left + "px";
    stage.style.top = fit.top + "px";
    stage.style.width = canvasW * fit.scale + "px";
    stage.style.height = canvasH * fit.scale + "px";
  }

  // --- 背景 ---
  // spec = {kind:"none"|"image"|"video"|"frame"|"unavailable", url?, fit, background, time?, onError?}
  function setBackground(spec) {
    stage.style.backgroundColor = spec.background || "";
    const key = spec.kind + "|" + (spec.cutId || "") + "|" + (spec.url || "");
    if (key !== bgKey) {
      bgKey = key;
      clear(stage);
      bgEl = null;
      if (spec.kind === "image" || spec.kind === "frame") {
        bgEl = el("img", "bg");
        bgEl.alt = "";
        if (spec.onError) bgEl.addEventListener("error", () => spec.onError("image"));
        bgEl.src = spec.url;
        stage.appendChild(bgEl);
      } else if (spec.kind === "video") {
        bgEl = el("video", "bg");
        bgEl.muted = true;
        bgEl.setAttribute("playsinline", "");
        bgEl.preload = "auto";
        if (spec.onError) bgEl.addEventListener("error", () => spec.onError("video"));
        bgEl.src = spec.url;
        stage.appendChild(bgEl);
      } else {
        stage.appendChild(el("div", "bg-note", spec.kind === "unavailable" ? "プレビューできません" : "素材なし"));
      }
    }
    if (bgEl) {
      bgEl.style.objectFit = spec.fit === "cover" ? "cover" : "contain";
      if (spec.kind === "video" && typeof spec.time === "number" && Math.abs(bgEl.currentTime - spec.time) > 1e-6) {
        try {
          bgEl.currentTime = spec.time;
        } catch {
          // メタデータの前は設定できない。読み込み後に合わせ直される
        }
      }
    }
  }

  /** 背景のメディアの読み込みを止める(セッション切れの後)。表示は最後のまま。 */
  function releaseBackground() {
    if (bgEl) {
      bgEl.removeAttribute("src");
      if (bgEl.tagName === "VIDEO") bgEl.load();
    }
    bgKey = null;
  }

  // --- 重ね画像 ---
  const layerImgs = new Map(); // id → <img>
  let selBox = null;

  function drawOverlay() {
    if (!lastFrame) return;
    const { items, cache, selected, onMissing } = lastFrame;
    const alive = new Set();
    for (const it of items) {
      alive.add(it.id);
      const url = cache ? cache.get(it.id) : undefined;
      if (url === undefined && onMissing) onMissing(it);
      let img = layerImgs.get(it.id);
      if (!url || !it.image || it.image.w <= 0 || it.image.h <= 0) {
        if (img) {
          img.remove();
          layerImgs.delete(it.id);
        }
        continue;
      }
      if (!img) {
        img = el("img", "ov");
        img.alt = "";
        layerImgs.set(it.id, img);
        overlay.appendChild(img);
      }
      if (img.getAttribute("src") !== url) img.src = url;
      // 位置は placed + image の原点(サーバーの合成と同じ値)。scale を掛けて箱の座標へ
      const p = toView(it.placed.x + it.image.dx, it.placed.y + it.image.dy, fit);
      img.style.left = p.x + "px";
      img.style.top = p.y + "px";
      img.style.width = it.image.w * fit.scale + "px";
      img.style.height = it.image.h * fit.scale + "px";
      img.style.zIndex = String(it.order);
    }
    for (const [id, img] of [...layerImgs]) {
      if (!alive.has(id)) {
        img.remove();
        layerImgs.delete(id);
      }
    }
    // 選択中の項目の枠
    const sel = selected ? items.find((it) => it.id === selected) : null;
    if (sel && sel.placed) {
      if (!selBox) {
        selBox = el("div", "sel");
        overlay.appendChild(selBox);
      }
      const q = toView(sel.placed.x, sel.placed.y, fit);
      selBox.style.left = q.x + "px";
      selBox.style.top = q.y + "px";
      selBox.style.width = sel.placed.w * fit.scale + "px";
      selBox.style.height = sel.placed.h * fit.scale + "px";
    } else if (selBox) {
      selBox.remove();
      selBox = null;
    }
  }

  // args = {n, cut, items, cache, selected, onMissing?}(items は itemsAt の結果)
  function renderFrame(args) {
    lastFrame = args;
    drawOverlay();
  }

  // 箱の大きさが変わったら、換算をやり直して描き直す
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      applyFit();
      drawOverlay();
    }).observe(box);
  }
  applyFit();

  // --- 一覧 ---
  function renderList(items, selected) {
    clear(listEl);
    const byCut = new Map();
    const byScene = new Map();
    const cutOf = (it) => {
      for (const sc of state.scenes)
        for (const c of sc.cuts) if (c.frames[0] <= it.frames[0] && it.frames[0] < c.frames[1]) return c.id;
      return null;
    };
    for (const it of items) {
      const cid = cutOf(it);
      const m = cid ? byCut : byScene;
      const key = cid || it.scene;
      if (!m.has(key)) m.set(key, []);
      m.get(key).push(it);
    }
    const itemRow = (it) => {
      const b = el("button", "row item" + (it.id === selected ? " selected" : ""));
      b.type = "button";
      b.appendChild(el("span", "id", it.id));
      b.appendChild(el("span", "text", it.text));
      if (it.manual) b.appendChild(el("span", "mark manual", "手動"));
      if (it.clipped) b.appendChild(el("span", "mark clipped", "切詰"));
      b.addEventListener("click", () => handlers.onSelectItem(it.id));
      return b;
    };
    for (const sc of state.scenes) {
      const sec = el("section", "scene");
      sec.appendChild(el("h3", "scene-title", sc.id + (sc.name ? " " + sc.name : "")));
      for (const c of sc.cuts) {
        const cb = el("button", "row cut");
        cb.type = "button";
        cb.appendChild(el("span", "id", c.id));
        cb.appendChild(
          el("span", "text", "コマ " + c.frames[0] + "-" + c.frames[1] + (c.asset ? " " + c.asset.kind : " 素材なし")),
        );
        cb.addEventListener("click", () => handlers.onSeek(c.frames[0]));
        sec.appendChild(cb);
        for (const it of byCut.get(c.id) || []) sec.appendChild(itemRow(it));
      }
      // どのカットの範囲にも入らない項目(長さ 0 など)はシーンの末尾に
      for (const it of byScene.get(sc.id) || []) sec.appendChild(itemRow(it));
      listEl.appendChild(sec);
    }
  }

  // --- 状態・警告・保存の印 ---
  function setStatus(text) {
    statusEl.textContent = text;
  }

  function setWarnings(list) {
    clear(warnEl);
    for (const w of list) warnEl.appendChild(el("li", "warn", w));
  }

  function setDirty(dirty) {
    dirtyEl.textContent = dirty ? "未保存の変更あり" : "";
  }

  // --- 数値の欄(確定の処理は Task 7 が onNumberCommit を受ける) ---
  const inputs = new Map();
  let numbersId = null;
  const numTitle = el("div", "num-title");
  numbersEl.appendChild(numTitle);
  for (const [field, label] of NUMBER_FIELDS) {
    const row = el("label", "num-row");
    row.appendChild(el("span", "num-label", label));
    const input = el("input", "num-input");
    input.type = "text";
    input.inputMode = "numeric";
    input.addEventListener("change", () => {
      if (numbersId) handlers.onNumberCommit(numbersId, field, input.value);
    });
    inputs.set(field, input);
    row.appendChild(input);
    numbersEl.appendChild(row);
  }
  const revertBtn = el("button", "revert", "自動に戻す");
  revertBtn.type = "button";
  revertBtn.addEventListener("click", () => {
    if (numbersId) handlers.onRevertAuto(numbersId);
  });
  numbersEl.appendChild(revertBtn);

  function setNumbers(item) {
    numbersId = item ? item.id : null;
    numTitle.textContent = item ? item.id : "項目を選んでください";
    for (const [field] of NUMBER_FIELDS) {
      const input = inputs.get(field);
      const p = item ? item.placed : null;
      const isTail = field === "tail_dx" || field === "tail_dy";
      const off = !item || (isTail && !p.tail);
      input.disabled = off;
      if (off) input.value = "";
      else if (isTail) input.value = String(field === "tail_dx" ? p.tail.dx : p.tail.dy);
      else input.value = String(p[field] || 0);
    }
    revertBtn.disabled = !(item && item.manual);
  }
  setNumbers(null);

  // --- スライダー・ボタン ---
  const max = sliderMax(state.total_frames);
  slider.min = "0";
  slider.max = String(max);
  slider.step = "1";
  slider.value = "0";
  slider.addEventListener("input", () => handlers.onSeek(Number(slider.value)));
  playBtn.addEventListener("click", () => handlers.onPlayToggle());
  $("save").addEventListener("click", () => handlers.onSave());
  $("quit").addEventListener("click", () => handlers.onQuit());

  /** スライダーとコマの表示を n に合わせる(イベントは起こさない)。 */
  function setFrame(n) {
    slider.value = String(n);
    frameLabel.textContent = "コマ " + n + " / " + max + "(" + (n / state.fps).toFixed(2) + " 秒)";
  }
  function setPlaying(playing) {
    playBtn.textContent = playing ? "停止" : "再生";
  }
  setFrame(0);
  setPlaying(false);

  return {
    renderList,
    renderFrame,
    setBackground,
    releaseBackground,
    setStatus,
    setWarnings,
    setDirty,
    setNumbers,
    setFrame,
    setPlaying,
    /** 数値の欄の入力要素(field → input)。editor_ui.js がキー操作を足す。 */
    numberInputs: () => inputs,
    previewElement: () => box,
    overlayLayer: () => overlay,
    /** 動画の背景の要素(無ければ null)。再生(player.js)が使う。 */
    videoElement: () => (bgEl && bgEl.tagName === "VIDEO" ? bgEl : null),
    /** 今の箱に対する換算({left, top, scale})。 */
    currentFit: () => fit,
  };
}
