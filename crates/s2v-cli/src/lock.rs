//! 台本フォルダの排他ロック(映像工程設計書 §2.3)。プロセスが終われば OS が外す。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub const LOCK_FILE: &str = ".script2video.lock";

pub struct ScriptLock {
    _file: File,
}

impl ScriptLock {
    pub fn acquire(script_dir: &Path) -> anyhow::Result<ScriptLock> {
        let path = script_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("ロックファイルを開けません: {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(ScriptLock { _file: file }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "別の script2video が実行中です(同じ台本フォルダで同時に実行できません): {}",
                script_dir.display()
            ),
            Err(TryLockError::Error(e)) => {
                anyhow::bail!("ロックを取れません: {}: {e}", path.display())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_first_is_held() {
        let d = tempfile::tempdir().unwrap();
        let first = ScriptLock::acquire(d.path()).unwrap();
        let e = ScriptLock::acquire(d.path()).err().unwrap().to_string();
        assert!(e.contains("別の script2video が実行中"), "{e}");
        drop(first);
        assert!(ScriptLock::acquire(d.path()).is_ok());
    }

    #[test]
    fn leftover_lock_file_does_not_block() {
        // Review Focus 4: 異常終了で残ったロックファイル
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(LOCK_FILE), "").unwrap();
        assert!(ScriptLock::acquire(d.path()).is_ok());
    }
}
