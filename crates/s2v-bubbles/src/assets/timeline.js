// コマ番号から表示するカット・項目・素材の中のコマを求める計算と、再生の状態遷移(純粋な関数だけ。DOM は触らない)。
// 時刻はコマ番号。範囲は半開区間 [開始, 終了)。

/** コマ n のカット(n0 ≤ n < n1)。長さ 0 のカットは選ばない。無ければ null。 */
export function cutAt(state, n) {
  for (const scene of state.scenes || []) {
    for (const cut of scene.cuts || []) {
      if (cut.frames[0] <= n && n < cut.frames[1]) return cut;
    }
  }
  return null;
}

/** コマ n に出ている項目(s ≤ n < e)を order の小さい順に。 */
export function itemsAt(items, n) {
  return items
    .filter((it) => it.frames[0] <= n && n < it.frames[1])
    .sort((a, b) => a.order - b.order);
}

/** 素材の中のコマ k。サーバーの s2v_video::frames::asset_frame と同じ式。 */
export function assetFrame(n, n0, frames, looped) {
  const d = Math.max(n - n0, 0);
  if (frames <= 0) return 0;
  if (d < frames) return d;
  return looped ? d % frames : frames - 1;
}

/** 音声の位置(秒)からのコマ。浮動小数の誤差を 1e-9 で吸収し、範囲に収める。 */
export function frameFromAudio(sec, fps, total) {
  const n = Math.floor(sec * fps + 1e-9);
  return Math.min(Math.max(n, 0), Math.max(total - 1, 0));
}

/** スライダーの最大値。 */
export function sliderMax(total) {
  return Math.max(total - 1, 0);
}

// カットが動画のとき、コマ n での素材の中のコマ k と「止める素材の最後か」。動画でなければ null。
function videoAt(cut, n) {
  if (!cut || !cut.asset || cut.asset.kind !== "video") return null;
  const { looped, frames } = cut.asset;
  const k = assetFrame(n, cut.frames[0], frames, looped);
  const d = Math.max(n - cut.frames[0], 0);
  return { k, atEnd: !looped && d >= frames - 1 };
}

const idOf = (cut) => (cut ? cut.id : null);

/**
 * 再生の状態遷移。st = {playing, n, cutId}、ev は play / stop / seek / tick。
 * 返す {st, actions} の actions は player.js が DOM に適用する。
 */
export function playerStep(state, st, ev) {
  const fps = state.fps;
  const total = state.total_frames;
  const actions = [];
  const secOf = (k) => k / fps;

  switch (ev.type) {
    case "play": {
      const v = videoAt(cutAt(state, st.n), st.n);
      actions.push({ type: "audio-seek", time: secOf(st.n) }, { type: "audio-play" });
      if (v) {
        actions.push({ type: "video-seek", time: secOf(v.k) });
        if (!v.atEnd) actions.push({ type: "video-play" });
      }
      return { st: { ...st, playing: true }, actions };
    }
    case "stop": {
      const v = videoAt(cutAt(state, st.n), st.n);
      actions.push({ type: "audio-pause" }, { type: "video-pause" });
      if (v) actions.push({ type: "video-seek", time: secOf(v.k) }, { type: "wait-seeked" });
      return { st: { ...st, playing: false }, actions };
    }
    case "seek": {
      const cut = cutAt(state, ev.n);
      const v = videoAt(cut, ev.n);
      if (st.playing) actions.push({ type: "audio-seek", time: secOf(ev.n) });
      if (idOf(cut) !== st.cutId) actions.push({ type: "cut-change", cutId: idOf(cut) });
      if (v) {
        actions.push({ type: "video-seek", time: secOf(v.k) });
        if (!st.playing) actions.push({ type: "wait-seeked" });
        else actions.push({ type: v.atEnd ? "video-pause" : "video-play" });
      }
      return { st: { ...st, n: ev.n, cutId: idOf(cut) }, actions };
    }
    case "tick": {
      if (!st.playing) return { st, actions };
      if (ev.ended) {
        // 音声の終わり: 停止して最後のコマ(そのコマの素材の中のコマで止める)
        const n = Math.max(total - 1, 0);
        const cut = cutAt(state, n);
        const v = videoAt(cut, n);
        actions.push({ type: "audio-pause" }, { type: "video-pause" });
        if (idOf(cut) !== st.cutId) actions.push({ type: "cut-change", cutId: idOf(cut) });
        if (v) actions.push({ type: "video-seek", time: secOf(v.k) }, { type: "wait-seeked" });
        return { st: { playing: false, n, cutId: idOf(cut) }, actions };
      }
      const n = frameFromAudio(ev.audioSec, fps, total);
      const cut = cutAt(state, n);
      const v = videoAt(cut, n);
      if (idOf(cut) !== st.cutId) {
        actions.push({ type: "cut-change", cutId: idOf(cut) });
        if (v) {
          if (v.atEnd) actions.push({ type: "video-pause" }, { type: "video-seek", time: secOf(v.k) });
          else actions.push({ type: "video-seek", time: secOf(v.k) }, { type: "video-play" });
        }
      } else if (v) {
        if (v.atEnd) {
          // 止める素材の最後: 一時停止して最後のコマに置く
          actions.push({ type: "video-pause" }, { type: "video-seek", time: secOf(v.k) });
        } else if (Math.abs(ev.videoSec - secOf(v.k)) >= 2 / fps - 1e-6) {
          // 2 コマ以上のずれ(浮動小数の誤差の許容つき)で合わせ直す
          actions.push({ type: "video-seek", time: secOf(v.k) }, { type: "video-play" });
        }
      }
      return { st: { ...st, n, cutId: idOf(cut) }, actions };
    }
    default:
      return { st, actions };
  }
}
