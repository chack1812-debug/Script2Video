//! `Process.log`(処理の記録・警告・エラー)。警告とエラーは標準エラーにも出す。

use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct ProcLog {
    file: File,
    warnings: usize,
}

impl ProcLog {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            file: File::create(path)?,
            warnings: 0,
        })
    }

    pub fn info(&mut self, msg: impl AsRef<str>) {
        let _ = writeln!(self.file, "[INFO] {}", msg.as_ref());
        println!("{}", msg.as_ref());
    }

    pub fn warn(&mut self, msg: impl AsRef<str>) {
        self.warnings += 1;
        let _ = writeln!(self.file, "[WARN] {}", msg.as_ref());
        eprintln!("[WARN] {}", msg.as_ref());
    }

    pub fn error(&mut self, msg: impl AsRef<str>) {
        let _ = writeln!(self.file, "[ERROR] {}", msg.as_ref());
        eprintln!("[ERROR] {}", msg.as_ref());
    }

    pub fn warning_count(&self) -> usize {
        self.warnings
    }
}
