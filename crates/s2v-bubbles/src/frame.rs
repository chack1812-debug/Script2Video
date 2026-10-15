//! ブラウザで再生できない素材の1コマを ffmpeg で PNG にして取り出すワーカー(設計書 §5.4 `/media/frame`)。
//!
//! 専用スレッドを1本だけ持ち、使用中の要求は待たせずに `Busy` で断る。
//! 10 秒で打ち切り、出力は 32 MiB まで。

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// ffmpeg を待つ上限。
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// ffmpeg の標準出力(PNG)の上限。
pub const FRAME_MAX_BYTES: usize = 32 << 20;

/// 標準エラーの末尾として返す行数。
const STDERR_TAIL_LINES: usize = 20;
/// 標準エラーとして保持する最大のバイト数(これ以上は読み捨てる)。
const STDERR_MAX_BYTES: u64 = 1 << 20;
/// 子プロセスの状態を見る間隔。
const POLL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// 別のコマを取り出している最中。
    Busy,
    /// 時間切れ(ffmpeg は強制終了した)。
    Timeout,
    /// 出力が上限を超えた(ffmpeg は強制終了した)。
    TooLarge,
    /// 起動できない、終了コードが 0 でない、中断された、など。
    Failed(String),
}

/// 素材の中のコマ `k` を PNG として標準出力へ出す ffmpeg の引数。
///
/// 入力の前の `-ss` でシークするので、先頭から復号せず長い素材でも速い(ffmpeg 既定の正確なシーク)。
/// 色の指定(`matrix`・`range`)は `cut_args` の動画の入力の scale と同じ値。
pub fn frame_args(input: &str, k: i64, fps: u32, matrix: &str, range: &str) -> Vec<String> {
    let secs = k as f64 / f64::from(fps);
    let v = |s: &str| s.to_string();
    vec![
        v("-nostdin"),
        v("-hide_banner"),
        v("-v"),
        v("error"),
        v("-threads"),
        v("2"),
        // TODO(2d-1 取り込み時): s2v_video::ffmpeg::PROTOCOL_WHITELIST を使う
        v("-protocol_whitelist"),
        v("file"),
        v("-ss"),
        format!("{secs:.6}"),
        v("-i"),
        input.to_string(),
        v("-vf"),
        format!(
            "scale=in_color_matrix={matrix}:in_range={range}:out_color_matrix=bt709:out_range=tv,format=rgb24"
        ),
        v("-frames:v"),
        v("1"),
        v("-f"),
        v("image2pipe"),
        v("-vcodec"),
        v("png"),
        // 出力はパイプ(標準出力)なので `-n`(上書きしない)は付けない。
        // Global Constraints の `-n` は、ファイルに書く呼び出しのためのもの。
        v("-"),
    ]
}

type FrameResult = Result<Vec<u8>, FrameError>;

struct Job {
    args: Vec<String>,
    reply: Sender<FrameResult>,
}

pub struct FrameWorker {
    /// 閉じるとワーカーのスレッドが終わる。
    tx: Option<Sender<Job>>,
    busy: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FrameWorker {
    pub fn start(ffmpeg: String) -> FrameWorker {
        Self::start_with(ffmpeg, FRAME_TIMEOUT, FRAME_MAX_BYTES)
    }

    /// 実行ファイル・時間切れ・出力の上限を指定して始める(テストで差し替える)。
    pub fn start_with(ffmpeg: String, timeout: Duration, max_bytes: usize) -> FrameWorker {
        let (tx, rx) = channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s) = (busy.clone(), stop.clone());
        let handle = thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let result = if s.load(Ordering::SeqCst) {
                    Err(FrameError::Failed("停止中です".into()))
                } else {
                    run_job(&ffmpeg, &job.args, timeout, max_bytes, &s)
                };
                // 返事を受け取った側がすぐ次を出せるよう、使用中の印は返事より先に外す
                b.store(false, Ordering::SeqCst);
                let _ = job.reply.send(result);
            }
        });
        FrameWorker {
            tx: Some(tx),
            busy,
            stop,
            handle: Some(handle),
        }
    }

    /// 使用中なら待たずに `Err(Busy)`。受け付けたら結果を受け取る口を返す。
    pub fn try_submit(&self, args: Vec<String>) -> Result<Receiver<FrameResult>, FrameError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(FrameError::Busy);
        }
        let (reply, rx) = channel();
        let sent = self
            .tx
            .as_ref()
            .map(|tx| tx.send(Job { args, reply }).is_ok())
            .unwrap_or(false);
        if !sent {
            self.busy.store(false, Ordering::SeqCst);
            return Err(FrameError::Failed("ワーカーが止まっています".into()));
        }
        Ok(rx)
    }

    /// 走っている ffmpeg を止めてスレッドを終える。
    pub fn shutdown(self) {
        // 後始末は Drop が行う
        drop(self);
    }
}

impl Drop for FrameWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 子を強制終了して回収し、読み取りのスレッドも必ず回収してからエラーを返す。
fn abort(
    child: &mut Child,
    out: Option<JoinHandle<Vec<u8>>>,
    err: JoinHandle<Vec<u8>>,
    e: FrameError,
) -> FrameResult {
    let _ = child.kill();
    let _ = child.wait();
    if let Some(h) = out {
        let _ = h.join();
    }
    let _ = err.join();
    Err(e)
}

fn run_job(
    ffmpeg: &str,
    args: &[String],
    timeout: Duration,
    max_bytes: usize,
    stop: &AtomicBool,
) -> FrameResult {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FrameError::Failed(format!("ffmpeg を起動できません: {e}")))?;

    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    // 標準出力は上限 + 1 バイトまで読む(超えたことが分かれば足りる)
    let out_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut buf);
        buf
    });
    let err_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(STDERR_MAX_BYTES).read_to_end(&mut buf);
        // 上限を超えた分も詰まらないよう読み捨てる
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        buf
    });

    let started = Instant::now();
    let mut out_reader = Some(out_reader);
    let mut data: Option<Vec<u8>> = None;

    loop {
        if stop.load(Ordering::SeqCst) {
            return abort(
                &mut child,
                out_reader,
                err_reader,
                FrameError::Failed("中断されました".into()),
            );
        }
        if data.is_none() && out_reader.as_ref().is_some_and(|h| h.is_finished()) {
            data = out_reader.take().and_then(|h| h.join().ok());
        }
        if data.as_ref().is_some_and(|d| d.len() > max_bytes) {
            return abort(&mut child, out_reader, err_reader, FrameError::TooLarge);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let data = match data.take() {
                    Some(d) => d,
                    None => out_reader
                        .take()
                        .and_then(|h| h.join().ok())
                        .unwrap_or_default(),
                };
                let err = err_reader.join().unwrap_or_default();
                if data.len() > max_bytes {
                    return Err(FrameError::TooLarge);
                }
                if !status.success() {
                    return Err(FrameError::Failed(stderr_tail(&err)));
                }
                return Ok(data);
            }
            Ok(None) => {}
            Err(e) => {
                return abort(
                    &mut child,
                    out_reader,
                    err_reader,
                    FrameError::Failed(format!("ffmpeg の状態を取得できません: {e}")),
                );
            }
        }
        if started.elapsed() >= timeout {
            return abort(&mut child, out_reader, err_reader, FrameError::Timeout);
        }
        thread::sleep(POLL);
    }
}

fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    let tail = lines[start..].join("\n");
    if tail.trim().is_empty() {
        "ffmpeg が失敗しました(標準エラーは空)".to_string()
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_args_are_exact() {
        let a = frame_args("C:/a b/S01.mov", 5, 30, "bt709", "tv");
        let expected: Vec<&str> = vec![
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-threads",
            "2",
            "-protocol_whitelist",
            "file",
            "-ss",
            "0.166667",
            "-i",
            "C:/a b/S01.mov",
            "-vf",
            "scale=in_color_matrix=bt709:in_range=tv:out_color_matrix=bt709:out_range=tv,format=rgb24",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ];
        assert_eq!(a, expected);
    }

    #[test]
    fn frame_args_use_given_color() {
        let a = frame_args("a.mp4", 0, 24, "bt601", "pc");
        assert!(a.contains(&"0.000000".to_string()));
        assert!(a
            .iter()
            .any(|s| s.starts_with("scale=in_color_matrix=bt601:in_range=pc:")));
    }

    #[cfg(windows)]
    fn ps(script: &str) -> Vec<String> {
        vec!["-NoProfile".into(), "-Command".into(), script.into()]
    }

    #[cfg(windows)]
    #[test]
    fn busy_worker_rejects_immediately() {
        let w = FrameWorker::start_with("powershell".into(), Duration::from_secs(20), 1 << 20);
        let _rx = w.try_submit(ps("Start-Sleep 3")).expect("受け付ける");
        let t = Instant::now();
        let r = w.try_submit(ps("Start-Sleep 3"));
        assert!(matches!(r, Err(FrameError::Busy)));
        assert!(
            t.elapsed() < Duration::from_millis(200),
            "{:?}",
            t.elapsed()
        );
        w.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn timeout_kills_the_process() {
        let w = FrameWorker::start_with("powershell".into(), Duration::from_millis(300), 1 << 20);
        let t = Instant::now();
        let rx = w.try_submit(ps("Start-Sleep 20")).unwrap();
        assert_eq!(rx.recv().unwrap(), Err(FrameError::Timeout));
        assert!(t.elapsed() < Duration::from_secs(10));
        // 打ち切ったあとも次を受け付ける
        // (短い時間切れでは powershell の起動が間に合わないので、受け付けたことと再び時間切れになることを見る)
        let rx = w.try_submit(ps("Start-Sleep 20")).expect("次を受け付ける");
        assert_eq!(rx.recv().unwrap(), Err(FrameError::Timeout));
        w.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn too_large_output_is_rejected() {
        let w = FrameWorker::start_with("powershell".into(), Duration::from_secs(20), 1024);
        let rx = w
            .try_submit(ps("[Console]::Out.Write(('a' * 1048576))"))
            .unwrap();
        assert_eq!(rx.recv().unwrap(), Err(FrameError::TooLarge));
        w.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn nonzero_exit_reports_stderr_tail() {
        let w = FrameWorker::start_with("powershell".into(), Duration::from_secs(20), 1 << 20);
        let rx = w
            .try_submit(ps("[Console]::Error.WriteLine('boom'); exit 3"))
            .unwrap();
        match rx.recv().unwrap() {
            Err(FrameError::Failed(m)) => assert!(m.contains("boom"), "{m}"),
            other => panic!("{other:?}"),
        }
        w.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn shutdown_kills_running_process() {
        let w = FrameWorker::start_with("powershell".into(), Duration::from_secs(60), 1 << 20);
        let rx = w.try_submit(ps("Start-Sleep 30")).unwrap();
        let t = Instant::now();
        w.shutdown();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(matches!(rx.recv().unwrap(), Err(FrameError::Failed(_))));
    }

    #[test]
    fn missing_executable_fails() {
        let w = FrameWorker::start_with(
            "s2v-no-such-exe-xyz".into(),
            Duration::from_secs(5),
            1 << 20,
        );
        let rx = w.try_submit(vec![]).unwrap();
        assert!(matches!(rx.recv().unwrap(), Err(FrameError::Failed(_))));
        w.shutdown();
    }
}
