//! エンジン起動処理のプロセス間排他ロック。
//!
//! Windows の named mutex はスレッドアフィニティ（`ReleaseMutex` は取得したスレッドから
//! 呼ぶ必要がある）を持つ。一方この臨界区間は `.await` をまたぎ、tokio のマルチスレッド
//! ランタイムでは `.await` の前後でワーカースレッドが移動しうるため named mutex は使えない。
//!
//! 代わりにロックファイルを `FILE_SHARE_NONE` で排他オープンする。スレッドアフィニティがなく、
//! プロセスが落ちれば OS がハンドルを閉じるためロックは自動的に解放される。

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::warn;

/// ロック取得のリトライ間隔。
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// `FILE_SHARE_NONE`。他プロセスが開いている間は共有違反で失敗する。
const FILE_SHARE_NONE: u32 = 0;

/// `ERROR_SHARING_VIOLATION`。他プロセスがロックを保持している状態。
const ERROR_SHARING_VIOLATION: i32 = 32;

/// 保持している間だけロックを持つ RAII ガード。drop するとハンドルが閉じてロックが解放される。
pub(crate) struct EngineStartupLock {
    _file: File,
}

/// ロックファイルのパス。Job Object 名と同じ `key` で揃える。
pub(crate) fn lock_path(key: &str) -> PathBuf {
    std::env::temp_dir().join(format!("script2voice_engine_{key}.lock"))
}

/// ロックの取得を1回だけ試みる。他プロセスが保持していれば `Ok(None)`。
pub(crate) fn try_acquire(key: &str) -> io::Result<Option<EngineStartupLock>> {
    match OpenOptions::new()
        .create(true)
        .write(true)
        .share_mode(FILE_SHARE_NONE)
        .open(lock_path(key))
    {
        Ok(file) => Ok(Some(EngineStartupLock { _file: file })),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `timeout` まで待ってロックを取得する。取れなければ `None`。
///
/// ブロッキングせず `tokio::time::sleep` でリトライするため、`.await` をまたいで保持してよい。
pub(crate) async fn acquire(key: &str, timeout: Duration) -> Option<EngineStartupLock> {
    let deadline = Instant::now() + timeout;
    loop {
        match try_acquire(key) {
            Ok(Some(lock)) => return Some(lock),
            Ok(None) => {}
            Err(e) => {
                // ロックファイルを開けない環境（権限等）では排他をあきらめて続行する。
                warn!(
                    "起動ロック {} を開けませんでした: {e}",
                    lock_path(key).display()
                );
                return None;
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_key(tag: &str) -> String {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        format!(
            "locktest_{}_{}_{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        )
    }

    #[test]
    fn second_acquire_fails_while_first_is_held() {
        let key = unique_key("held");
        let first = try_acquire(&key).unwrap();
        assert!(first.is_some(), "1回目は取得できること");
        assert!(
            try_acquire(&key).unwrap().is_none(),
            "保持中は取得できないこと"
        );
    }

    #[test]
    fn lock_is_released_when_guard_is_dropped() {
        // プロセスがクラッシュした場合も OS が同じようにハンドルを閉じるため、
        // このテストは「クラッシュ時にロックが残らない」ことの代理検証になる。
        let key = unique_key("release");
        let first = try_acquire(&key).unwrap().unwrap();
        drop(first);
        assert!(
            try_acquire(&key).unwrap().is_some(),
            "drop で解放されること"
        );
    }

    #[tokio::test]
    async fn acquire_gives_up_after_timeout() {
        let key = unique_key("timeout");
        let _held = try_acquire(&key).unwrap().unwrap();

        let start = Instant::now();
        assert!(acquire(&key, Duration::from_millis(300)).await.is_none());
        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "タイムアウトまで待つこと"
        );
    }

    #[tokio::test]
    async fn acquire_succeeds_once_the_holder_releases() {
        let key = unique_key("handoff");
        let held = try_acquire(&key).unwrap().unwrap();

        let key_for_task = key.clone();
        let waiter = tokio::spawn(async move {
            acquire(&key_for_task, Duration::from_secs(5))
                .await
                .is_some()
        });

        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(held);

        assert!(waiter.await.unwrap(), "保持者が解放したら取得できること");
    }
}
