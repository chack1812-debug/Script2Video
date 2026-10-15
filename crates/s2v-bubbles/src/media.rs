//! メディア(素材・full_dialogue.wav)の固定と Range つきの配信部品(設計書 §5.4)。

use crate::range::{parse_range, RangeResult};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 共有モードで開いて固定したメディアファイル。
pub struct LockedFile {
    file: File,
    len: u64,
    content_type: &'static str,
}

/// 拡張子(大文字小文字を区別しない)から Content-Type を決める。
fn content_type_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "wav" => "audio/wav",
        _ => return None,
    })
}

/// 読み取り専用・`FILE_SHARE_READ` で開く。
/// 他のプロセスは読めるが、開いている間は書き込み・削除・改名ができない(Windows のみ)。
/// 書き込みのために開かれているファイルは共有違反で Err になる。
pub fn open_locked(path: &Path) -> io::Result<LockedFile> {
    let content_type = content_type_for(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "対応していない拡張子のメディア",
        )
    })?;
    let file = open_shared_read(path)?;
    let len = file.metadata()?.len();
    Ok(LockedFile {
        file,
        len,
        content_type,
    })
}

#[cfg(windows)]
fn open_shared_read(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ = 1(書き込み・削除の共有を与えない)
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
}

#[cfg(not(windows))]
fn open_shared_read(path: &Path) -> io::Result<File> {
    // ファイルの固定(書き込み・削除の拒否)は Windows の共有モードだけの機能。
    // それ以外の OS では通常の読み取りで開くのみ。
    std::fs::OpenOptions::new().read(true).open(path)
}

impl LockedFile {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// 位置を指定して読む(共有のカーソルを使わない)。短い読み出しもありうる。
    #[cfg(windows)]
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        use std::os::windows::fs::FileExt;
        self.file.seek_read(buf, offset)
    }

    #[cfg(not(windows))]
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        FileExt::read_at(&self.file, buf, offset)
    }
}

/// メディア応答の中身。`body` は読み出す範囲 `start..=end`(HEAD と 416 は None)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<(u64, u64)>,
}

/// Range ヘッダーに応じて 200 / 206 / 416 の応答を組み立てる。
pub fn media_response(
    file: &LockedFile,
    method_is_head: bool,
    range_header: Option<&str>,
) -> MediaResponse {
    let len = file.len();
    let h = |k: &str, v: String| (k.to_string(), v);
    let mut headers = vec![
        h("Content-Type", file.content_type().to_string()),
        h("Accept-Ranges", "bytes".to_string()),
    ];
    let (status, body) = match parse_range(range_header, len) {
        RangeResult::Full => {
            headers.push(h("Content-Length", len.to_string()));
            (200, (len > 0).then(|| (0, len - 1)))
        }
        RangeResult::Partial { start, end } => {
            headers.push(h("Content-Range", format!("bytes {start}-{end}/{len}")));
            headers.push(h("Content-Length", (end - start + 1).to_string()));
            (206, Some((start, end)))
        }
        RangeResult::Unsatisfiable => {
            headers.push(h("Content-Range", format!("bytes */{len}")));
            headers.push(h("Content-Length", "0".to_string()));
            (416, None)
        }
    };
    let body = if method_is_head { None } else { body };
    MediaResponse {
        status,
        headers,
        body,
    }
}

/// `start..=end` を順に読む `Read`。短い読み出しは繰り返す。
pub struct RangeReader<'a, T: ReadAt + ?Sized = LockedFile> {
    file: &'a T,
    next: u64,
    /// 読み終えた位置(含まない)。
    stop: u64,
}

/// 位置を指定した読み出し(テストで偽物を差し込めるようにする)。
pub trait ReadAt {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
}

impl ReadAt for LockedFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        LockedFile::read_at(self, buf, offset)
    }
}

pub fn reader(file: &LockedFile, start: u64, end: u64) -> RangeReader<'_> {
    reader_of(file, start, end)
}

fn reader_of<T: ReadAt + ?Sized>(file: &T, start: u64, end: u64) -> RangeReader<'_, T> {
    RangeReader {
        file,
        next: start,
        stop: end.saturating_add(1),
    }
}

impl<T: ReadAt + ?Sized> Read for RangeReader<'_, T> {
    /// 1 回の呼び出しで、要求の長さ(残りの範囲と buf の小さい方)を満たすまで
    /// `read_at` を繰り返す。途中で 0 が返り範囲が残っていれば UnexpectedEof。
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.next >= self.stop || buf.is_empty() {
            return Ok(0);
        }
        let remain = self.stop - self.next;
        let want = buf.len().min(usize::try_from(remain).unwrap_or(usize::MAX));
        let mut got = 0;
        while got < want {
            match self.file.read_at(&mut buf[got..want], self.next) {
                // 範囲の途中でファイルが終わった(固定していれば起きない)
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    got += n;
                    self.next += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(got)
    }
}

/// メディアのスレッドの同時数の上限。
pub struct Slots {
    max: usize,
    used: AtomicUsize,
}

/// 枠の保持。drop で返す。
pub struct SlotGuard {
    slots: Arc<Slots>,
}

impl Slots {
    pub fn new(max: usize) -> Arc<Slots> {
        Arc::new(Slots {
            max,
            used: AtomicUsize::new(0),
        })
    }

    /// 空きがあれば取る(比較交換の繰り返し)。無ければ None(呼び出し側が 503 を返す)。
    pub fn try_acquire(self: &Arc<Self>) -> Option<SlotGuard> {
        let mut cur = self.used.load(Ordering::Acquire);
        loop {
            if cur >= self.max {
                return None;
            }
            match self
                .used
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Some(SlotGuard {
                        slots: Arc::clone(self),
                    })
                }
                Err(actual) => cur = actual,
            }
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.slots.used.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        (dir, path)
    }

    fn header<'a>(r: &'a MediaResponse, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn media_response_statuses() {
        let bytes: Vec<u8> = (0..100u8).collect();
        let (_d, path) = temp_file("a.mp4", &bytes);
        let f = open_locked(&path).unwrap();

        let r = media_response(&f, false, None);
        assert_eq!(r.status, 200);
        assert_eq!(header(&r, "Content-Length"), Some("100"));
        assert_eq!(header(&r, "Accept-Ranges"), Some("bytes"));
        assert_eq!(header(&r, "Content-Type"), Some("video/mp4"));
        assert_eq!(r.body, Some((0, 99)));

        let r = media_response(&f, false, Some("bytes=10-19"));
        assert_eq!(r.status, 206);
        assert_eq!(header(&r, "Content-Range"), Some("bytes 10-19/100"));
        assert_eq!(header(&r, "Content-Length"), Some("10"));
        assert_eq!(header(&r, "Accept-Ranges"), Some("bytes"));
        assert_eq!(r.body, Some((10, 19)));

        let r = media_response(&f, false, Some("bytes=200-"));
        assert_eq!(r.status, 416);
        assert_eq!(header(&r, "Content-Range"), Some("bytes */100"));
        assert_eq!(header(&r, "Content-Length"), Some("0"));
        assert_eq!(header(&r, "Accept-Ranges"), Some("bytes"));
        assert_eq!(r.body, None);

        let head = media_response(&f, true, Some("bytes=10-19"));
        let get = media_response(&f, false, Some("bytes=10-19"));
        assert_eq!(head.status, 206);
        assert_eq!(head.headers, get.headers);
        assert_eq!(head.body, None);

        let head = media_response(&f, true, None);
        assert_eq!(head.status, 200);
        assert_eq!(header(&head, "Content-Length"), Some("100"));
        assert_eq!(head.body, None);

        let (_d2, empty) = temp_file("e.mp4", b"");
        let f = open_locked(&empty).unwrap();
        let r = media_response(&f, false, None);
        assert_eq!(r.status, 200);
        assert_eq!(header(&r, "Content-Length"), Some("0"));
        assert_eq!(r.body, None);
    }

    #[test]
    fn content_types_by_extension() {
        for (name, ct) in [
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.JPEG", "image/jpeg"),
            ("a.mov", "video/quicktime"),
            ("a.webm", "video/webm"),
            ("a.wav", "audio/wav"),
        ] {
            let (_d, p) = temp_file(name, b"x");
            assert_eq!(open_locked(&p).unwrap().content_type(), ct, "{name}");
        }
    }

    #[test]
    fn reader_returns_exact_bytes() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        let (_d, path) = temp_file("a.wav", &bytes);
        let f = open_locked(&path).unwrap();
        let mut out = Vec::new();
        reader(&f, 10, 19).read_to_end(&mut out).unwrap();
        assert_eq!(out, (10..=19u8).collect::<Vec<_>>());
        // 末尾まで
        let mut out = Vec::new();
        reader(&f, 250, 255).read_to_end(&mut out).unwrap();
        assert_eq!(out, (250..=255u8).collect::<Vec<_>>());
    }

    /// 1 回に 3 バイトまでしか返さない偽物。`limit` 到達で 0 を返す。
    struct Short {
        data: Vec<u8>,
        limit: usize,
    }
    impl ReadAt for Short {
        fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
            let off = offset as usize;
            let end = self.data.len().min(self.limit);
            if off >= end {
                return Ok(0);
            }
            let n = buf.len().min(3).min(end - off);
            buf[..n].copy_from_slice(&self.data[off..off + n]);
            Ok(n)
        }
    }

    #[test]
    fn single_read_fills_buf_despite_short_reads() {
        let src = Short {
            data: (0..100u8).collect(),
            limit: 100,
        };
        let mut r = reader_of(&src, 10, 29);
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf).unwrap(), 8);
        assert_eq!(buf.to_vec(), (10..18u8).collect::<Vec<_>>());
        // 残りの範囲(12 バイト)が buf より小さいときは残りだけ
        let mut buf = [0u8; 32];
        assert_eq!(r.read(&mut buf).unwrap(), 12);
        assert_eq!(buf[..12].to_vec(), (18..30u8).collect::<Vec<_>>());
        assert_eq!(r.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn zero_in_middle_is_unexpected_eof() {
        let src = Short {
            data: (0..100u8).collect(),
            limit: 15,
        };
        let mut buf = [0u8; 20];
        let e = reader_of(&src, 10, 29).read(&mut buf).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn unknown_extension_is_rejected() {
        let (_d, path) = temp_file("a.txt", b"hello");
        let e = open_locked(&path).err().expect("拡張子が不明なら Err");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(windows)]
    #[test]
    fn locked_asset_cannot_be_overwritten_or_deleted() {
        let (dir, path) = temp_file("a.png", b"original");
        let other = dir.path().join("b.png");
        let f = open_locked(&path).unwrap();

        assert!(std::fs::write(&path, b"new").is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(std::fs::rename(&path, &other).is_err());
        assert!(!other.exists());
        // ffmpeg のような別の読み取りは通る
        let mut s = String::new();
        std::fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .expect("別ハンドルの読み取りは許される")
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "original");
        // 配信される内容は起動時のまま
        let mut buf = [0u8; 8];
        let n = f.read_at(&mut buf, 0).unwrap();
        assert_eq!(&buf[..n], b"original");

        drop(f);
        std::fs::remove_file(&path).expect("閉じたら削除できる");
    }

    #[cfg(windows)]
    #[test]
    fn file_open_for_writing_elsewhere_cannot_be_locked() {
        let (_d, path) = temp_file("a.png", b"original");
        let _w = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(open_locked(&path).is_err());
    }

    #[test]
    fn slots_limit() {
        let s = Slots::new(2);
        let a = s.try_acquire().unwrap();
        let _b = s.try_acquire().unwrap();
        assert!(s.try_acquire().is_none());
        drop(a);
        assert!(s.try_acquire().is_some());
    }
}
