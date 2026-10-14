//! ファイルの書き込みの共通の部品。

use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// 同じフォルダの一時ファイルに書いてから置き換える(途中で失敗しても元のファイルは壊れない)。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// ファイルの生のバイト列の SHA-256(64 文字の小文字の16進)。大きなファイルも少しずつ読む。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        std::fs::write(&p, "old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn sha256_file_matches_known_value() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        std::fs::write(&p, "abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 1 MiB を超える大きさでも、一度に読んだ値と同じ
        let big = vec![7u8; 3 * 1024 * 1024 + 5];
        std::fs::write(&p, &big).unwrap();
        assert_eq!(sha256_file(&p).unwrap(), crate::work::sha256_hex(&big));
    }
}
