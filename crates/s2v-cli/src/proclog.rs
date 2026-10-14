//! `Process.log`(処理の記録・警告・エラー)。警告とエラーは標準エラーにも出す。
//!
//! 書き込みの失敗は最初の 1 件を覚えておき、`check` で報告する(記録できなかった実行は失敗とする)。

use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct ProcLog {
    out: Box<dyn Write>,
    warnings: usize,
    write_error: Option<std::io::Error>,
}

impl ProcLog {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        Ok(Self::from_writer(Box::new(File::create(path)?)))
    }

    pub fn from_writer(out: Box<dyn Write>) -> Self {
        Self {
            out,
            warnings: 0,
            write_error: None,
        }
    }

    fn put(&mut self, level: &str, msg: &str) {
        let r = writeln!(self.out, "[{level}] {msg}").and_then(|()| self.out.flush());
        if let Err(e) = r {
            if self.write_error.is_none() {
                self.write_error = Some(e);
            }
        }
    }

    pub fn info(&mut self, msg: impl AsRef<str>) {
        self.put("INFO", msg.as_ref());
        println!("{}", msg.as_ref());
    }

    pub fn warn(&mut self, msg: impl AsRef<str>) {
        self.warnings += 1;
        self.put("WARN", msg.as_ref());
        eprintln!("[WARN] {}", msg.as_ref());
    }

    pub fn error(&mut self, msg: impl AsRef<str>) {
        self.put("ERROR", msg.as_ref());
        eprintln!("[ERROR] {}", msg.as_ref());
    }

    pub fn warning_count(&self) -> usize {
        self.warnings
    }

    /// これまでに書き込みの失敗があれば Err にする。
    pub fn check(&self) -> anyhow::Result<()> {
        match &self.write_error {
            None => Ok(()),
            Some(e) => Err(anyhow::anyhow!("Process.log に書き込めません: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("書き込み失敗"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_failure_is_reported_by_check() {
        let mut log = ProcLog::from_writer(Box::new(Failing));
        assert!(log.check().is_ok());
        log.warn("x");
        assert_eq!(log.warning_count(), 1);
        let e = log.check().unwrap_err().to_string();
        assert!(e.contains("書き込み失敗"), "{e}");
    }

    #[test]
    fn healthy_writer_passes_check() {
        let mut log = ProcLog::from_writer(Box::new(Vec::<u8>::new()));
        log.error("e");
        assert!(log.check().is_ok());
    }
}
