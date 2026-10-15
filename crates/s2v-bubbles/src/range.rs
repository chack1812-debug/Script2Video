//! メディア応答用の HTTP Range ヘッダーの解釈(設計書 §5.4、RFC 9110 の単一範囲)。

/// Range の解釈結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeResult {
    /// 全体を返す(ヘッダー無し)。
    Full,
    /// 一部を返す。`end` は含む。
    Partial { start: u64, end: u64 },
    /// 満たせない範囲(416)。
    Unsatisfiable,
}

/// `Range` ヘッダーを、長さ `len` のファイルに対して解釈する。
pub fn parse_range(header: Option<&str>, len: u64) -> RangeResult {
    let Some(header) = header else {
        return RangeResult::Full;
    };
    parse_bytes_range(header.trim(), len).unwrap_or(RangeResult::Unsatisfiable)
}

/// 数字だけからなる文字列を u64 として読む(符号・空・あふれは None)。
fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_bytes_range(spec: &str, len: u64) -> Option<RangeResult> {
    if len == 0 {
        return None;
    }
    let spec = spec.strip_prefix("bytes=")?;
    // 複数範囲の `,` は数字の検査で弾かれる
    let (first, last) = spec.split_once('-')?;
    let (start, end) = if first.is_empty() {
        // 末尾から n バイト
        let n = digits(last)?;
        if n == 0 {
            return None;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let start = digits(first)?;
        let end = if last.is_empty() {
            len - 1
        } else {
            digits(last)?.min(len - 1)
        };
        (start, end)
    };
    if start >= len || start > end {
        return None;
    }
    Some(RangeResult::Partial { start, end })
}

#[cfg(test)]
mod tests {
    use super::RangeResult::*;
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 100), Full);
        assert_eq!(
            parse_range(Some("bytes=0-9"), 100),
            Partial { start: 0, end: 9 }
        );
        assert_eq!(
            parse_range(Some("bytes=90-"), 100),
            Partial { start: 90, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=90-500"), 100),
            Partial { start: 90, end: 99 }
        ); // 切り詰め
        assert_eq!(
            parse_range(Some("bytes=-10"), 100),
            Partial { start: 90, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=-500"), 100),
            Partial { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some(" bytes=0-0 "), 100),
            Partial { start: 0, end: 0 }
        );
        for bad in [
            "bytes=-0",
            "bytes=100-",
            "bytes=100-200",
            "bytes=5-4",
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes=a-b",
            "bytes=",
            "bytes=-",
            "bytes=99999999999999999999-",
            "bytes=0-99999999999999999999",
            "bytes = 0-1",
            "bytes=+1-2",
        ] {
            assert_eq!(parse_range(Some(bad), 100), Unsatisfiable, "{bad}");
        }
        assert_eq!(parse_range(Some("bytes=0-0"), 0), Unsatisfiable); // 空のファイル
        assert_eq!(parse_range(None, 0), Full);
    }
}
