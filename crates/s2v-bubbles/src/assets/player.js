// 再生。<audio> を時計にし、requestAnimationFrame ごとに timeline.playerStep へ出来事を渡して、
// 返った actions を順に適用する(規則は timeline.js にあり、ここでは導き直さない)。DOM の要素は
// 渡されたものだけを使うので、node の偽物でも試せる。
import { cutAt, playerStep } from "./timeline.js";

const SEEKED_TIMEOUT_MS = 1500; // seeked が来ないときの保険(読み込み失敗など)

/**
 * o = {
 *   audio, video(要素か、今の動画の要素を返す関数。無ければ null), state, getN, setN, frames,
 *   showCut(cut, n): 背景を cut のものに替える, redraw(n): 止まっているときの描き直し,
 *   onFrame(n): 再生中のコマの更新(スライダー・重ね画像), onPlayingChange(bool),
 *   isLost?(), onAudioError?(), raf?, caf?, setTimer?, clearTimer?
 * }
 * 返す {play, stop, seek, onCutChange, tick}。
 */
export function createPlayer(o) {
  const { audio, state } = o;
  const getVideo = typeof o.video === "function" ? o.video : () => o.video;
  const raf = o.raf || ((f) => requestAnimationFrame(f));
  const caf = o.caf || ((h) => cancelAnimationFrame(h));
  const setTimer = o.setTimer || ((f, ms) => setTimeout(f, ms));
  const clearTimer = o.clearTimer || ((h) => clearTimeout(h));

  const cutsById = new Map();
  for (const sc of state.scenes || []) for (const c of sc.cuts || []) cutsById.set(c.id, c);

  const first = cutAt(state, o.getN());
  let st = { playing: false, n: o.getN(), cutId: first ? first.id : null };
  let handle = null; // rAF
  let token = 0; // seeked の待ちを無効にする世代
  let waiting = null; // { video, listener, timer }

  // 動画は常に消音(音は <audio> だけ)
  function video() {
    const v = getVideo();
    if (v) {
      v.muted = true;
      v.volume = 0;
    }
    return v || null;
  }

  function cancelWait() {
    token++;
    if (waiting) {
      waiting.video.removeEventListener("seeked", waiting.listener);
      clearTimer(waiting.timer);
      waiting = null;
    }
  }

  // seeked を待ってから描き直す(待つ間に別の操作があれば捨てる)
  function redrawAfterSeeked(n) {
    cancelWait();
    const v = video();
    if (!v) {
      o.redraw(n);
      return;
    }
    const my = token;
    const done = () => {
      if (my !== token) return;
      cancelWait();
      o.redraw(n);
    };
    waiting = { video: v, listener: done, timer: setTimer(done, SEEKED_TIMEOUT_MS) };
    v.addEventListener("seeked", done, { once: true });
  }

  function safe(fn) {
    try {
      return fn();
    } catch {
      return undefined; // メタデータの前の currentTime など。読み込み後に合わせ直される
    }
  }

  // actions を順に適用する。wait-seeked があれば true(描き直しは seeked の後)
  function apply(actions, n) {
    let wait = false;
    for (const a of actions) {
      switch (a.type) {
        case "audio-seek":
          safe(() => {
            audio.currentTime = a.time;
          });
          break;
        case "audio-play": {
          const p = safe(() => audio.play());
          if (p && typeof p.catch === "function") {
            p.catch(() => {
              stop();
              if (o.onAudioError) o.onAudioError();
            });
          }
          break;
        }
        case "audio-pause":
          safe(() => audio.pause());
          break;
        case "cut-change":
          o.showCut(a.cutId === null ? null : cutsById.get(a.cutId) || null, n);
          break;
        case "video-seek": {
          const v = video();
          if (v) {
            safe(() => {
              v.currentTime = a.time;
            });
          }
          break;
        }
        case "video-play": {
          const v = video();
          if (v) {
            const p = safe(() => v.play());
            if (p && typeof p.catch === "function") p.catch(() => {});
          }
          break;
        }
        case "video-pause": {
          const v = video();
          if (v) safe(() => v.pause());
          break;
        }
        case "wait-seeked":
          wait = true;
          break;
        default:
      }
    }
    return wait;
  }

  function setPlaying(p) {
    if (o.onPlayingChange) o.onPlayingChange(p);
  }

  function stopLoop() {
    if (handle !== null) {
      caf(handle);
      handle = null;
    }
  }

  function loop() {
    handle = null;
    if (!st.playing) return;
    tick();
    if (st.playing) handle = raf(loop);
  }

  function play() {
    if (st.playing || (o.isLost && o.isLost())) return;
    cancelWait();
    st = { ...st, n: o.getN() };
    // 最後のコマで押したら先頭から
    if (state.total_frames > 1 && st.n >= state.total_frames - 1) {
      const r = playerStep(state, st, { type: "seek", n: 0 });
      st = r.st;
      apply(r.actions, 0);
      o.setN(0);
    }
    // 再生中は frame を要求しない: 飛んでいる要求の結果も捨てる
    if (o.frames) o.frames.setCurrent(null, null);
    const r = playerStep(state, st, { type: "play" });
    st = r.st;
    apply(r.actions, st.n);
    setPlaying(true);
    o.onFrame(st.n);
    stopLoop();
    handle = raf(loop);
  }

  function stop() {
    stopLoop();
    if (!st.playing) return;
    const r = playerStep(state, st, { type: "stop" });
    st = r.st;
    setPlaying(false);
    if (apply(r.actions, st.n)) redrawAfterSeeked(st.n);
    else o.redraw(st.n);
  }

  function seek(n) {
    cancelWait();
    const r = playerStep(state, st, { type: "seek", n });
    st = r.st;
    o.setN(n);
    const wait = apply(r.actions, n);
    if (st.playing) o.onFrame(n);
    else if (wait) {
      // seeked を待つ間も、コマの表示と重ね画像は新しいコマにそろえる(背景は seeked の後)
      o.onFrame(n);
      redrawAfterSeeked(n);
    } else o.redraw(n);
  }

  function onCutChange(cut) {
    st = { ...st, cutId: cut ? cut.id : null };
    o.showCut(cut, st.n);
  }

  function tick() {
    if (!st.playing) return;
    const v = getVideo();
    const ready = v && v.readyState !== 0;
    const ev = {
      type: "tick",
      audioSec: audio.currentTime,
      videoSec: ready ? v.currentTime : NaN, // 読み込み前は「ずれ」と見なさない
      ended: !!audio.ended,
    };
    const before = st.n;
    const r = playerStep(state, st, ev);
    st = r.st;
    const wait = apply(r.actions, st.n);
    if (st.n !== before) o.setN(st.n);
    if (!st.playing) {
      // 音声の終わりで止まった
      stopLoop();
      setPlaying(false);
      if (wait) redrawAfterSeeked(st.n);
      else o.redraw(st.n);
    } else if (st.n !== before) {
      o.onFrame(st.n);
    }
  }

  // 音声が読めないとき(403 の確かめは app 側)
  if (audio.addEventListener) {
    audio.addEventListener("error", () => {
      stop();
      if (o.onAudioError) o.onAudioError();
    });
  }

  return { play, stop, seek, onCutChange, tick };
}
