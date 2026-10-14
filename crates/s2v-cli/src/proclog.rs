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

    /// 追記で開き、区切りの行 `== <command> <時刻>(UTC) ==` を書く(prompts・bubbles・render 用)。
    pub fn append(path: &Path, command: &str) -> anyhow::Result<Self> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let mut log = Self::from_writer(Box::new(f));
        let r = writeln!(log.out, "== {command} {} ==", utc_now()).and_then(|()| log.out.flush());
        if let Err(e) = r {
            log.write_error = Some(e);
        }
        Ok(log)
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

/// 1970-01-01 からの日数をグレゴリオ暦の (年, 月, 日) にする(Howard Hinnant の civil_from_days)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// 現在の UTC を `YYYY-MM-DD HH:MM:SS(UTC)` にする(ローカル時刻は標準ライブラリで取れない)。
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02}(UTC)",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
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
    fn append_keeps_previous_content_and_writes_a_header() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Process.log");
        std::fs::write(&p, "[INFO] build\n").unwrap();
        let mut log = ProcLog::append(&p, "render").unwrap();
        log.info("ok");
        drop(log);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("[INFO] build\n== render "), "{text}");
        assert!(text.ends_with("[INFO] ok\n"), "{text}");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn healthy_writer_passes_check() {
        let mut log = ProcLog::from_writer(Box::new(Vec::<u8>::new()));
        log.error("e");
        assert!(log.check().is_ok());
    }
}
