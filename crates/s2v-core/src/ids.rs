//! 台詞ID(`S01-L003`)とシーン番号(`S02`)の書式(台本仕様 §1、§4)。

use serde::{Deserialize, Serialize};

/// 台詞ID。台詞番号 `line` は台本全体で一意。`scene` は採番時のシーン番号(表示用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineId {
    pub scene: u32,
    pub line: u32,
}

impl LineId {
    /// `S01-L003` の形式。シーンは2桁以上、台詞は3桁以上。
    pub fn display(&self) -> String {
        format!("S{:02}-L{:03}", self.scene, self.line)
    }

    /// 収録ファイル名の語幹 `S01_L003`。
    pub fn file_stem(&self) -> String {
        format!("S{:02}_L{:03}", self.scene, self.line)
    }

    /// `S01-L003` を読む(桁数は問わない)。
    pub fn parse(s: &str) -> Option<LineId> {
        Self::parse_with_sep(s, "-L")
    }

    /// 収録ファイル名の語幹 `S01_L003` を読む。範囲(`S01_L003-L007`)は None。
    pub fn parse_file_stem(s: &str) -> Option<LineId> {
        Self::parse_with_sep(s, "_L")
    }

    fn parse_with_sep(s: &str, sep: &str) -> Option<LineId> {
        let rest = s.strip_prefix('S')?;
        let (scene, line) = rest.split_once(sep)?;
        Some(LineId {
            scene: parse_digits(scene)?,
            line: parse_digits(line)?,
        })
    }
}

/// 台詞の参照 `L003+500`(`at=`、`#se at=/end=`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRef {
    pub line: u32,
    pub offset_ms: i64,
}

/// `L003`、`L003+500`、`L003-200`、`S01-L003+500` を読む。
pub fn parse_line_ref(s: &str) -> Option<LineRef> {
    let s = s.trim();
    let s = match s.split_once("-L") {
        Some((scene, rest)) if scene.starts_with('S') && parse_digits(&scene[1..]).is_some() => {
            rest
        }
        _ => s.strip_prefix('L')?,
    };
    let split = s.find(['+', '-']).unwrap_or(s.len());
    let line = parse_digits(&s[..split])?;
    let offset_ms = if split == s.len() {
        0
    } else {
        let sign: i64 = if s.as_bytes()[split] == b'-' { -1 } else { 1 };
        sign * parse_digits(&s[split + 1..])? as i64
    };
    Some(LineRef { line, offset_ms })
}

/// シーン番号 `S02`(2桁以上)。
pub fn format_scene_id(n: u32) -> String {
    format!("S{n:02}")
}

/// `S02` を読む。
pub fn parse_scene_id(s: &str) -> Option<u32> {
    parse_digits(s.strip_prefix('S')?)
}

/// 行頭の `[S01-L003]` を取り出す。台詞IDの書式でなければ (None, 元の文字列)。
pub fn split_line_id_prefix(content: &str) -> (Option<LineId>, &str) {
    if let Some(rest) = content.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            if let Some(id) = LineId::parse(&rest[..close]) {
                return (Some(id), rest[close + 1..].trim_start());
            }
        }
    }
    (None, content)
}

fn parse_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_id_display_and_file_stem() {
        let id = LineId { scene: 1, line: 3 };
        assert_eq!(id.display(), "S01-L003");
        assert_eq!(id.file_stem(), "S01_L003");
        let big = LineId {
            scene: 123,
            line: 4567,
        };
        assert_eq!(big.display(), "S123-L4567");
    }

    #[test]
    fn line_id_parse_accepts_any_digit_count() {
        assert_eq!(
            LineId::parse("S01-L003"),
            Some(LineId { scene: 1, line: 3 })
        );
        assert_eq!(
            LineId::parse("S1-L12345"),
            Some(LineId {
                scene: 1,
                line: 12345
            })
        );
        assert_eq!(LineId::parse("S01-L"), None);
        assert_eq!(LineId::parse("S01_L003"), None);
        assert_eq!(LineId::parse("S01-L003x"), None);
        assert_eq!(
            LineId::parse_file_stem("S01_L003"),
            Some(LineId { scene: 1, line: 3 })
        );
        assert_eq!(LineId::parse_file_stem("S01_L003-L007"), None);
    }

    #[test]
    fn scene_id_format_and_parse() {
        assert_eq!(format_scene_id(2), "S02");
        assert_eq!(parse_scene_id("S02"), Some(2));
        assert_eq!(parse_scene_id("S"), None);
        assert_eq!(parse_scene_id("X02"), None);
    }

    #[test]
    fn split_line_id_prefix_only_takes_valid_ids() {
        let (id, rest) = split_line_id_prefix("[S01-L003] A:おはよう");
        assert_eq!(id, Some(LineId { scene: 1, line: 3 }));
        assert_eq!(rest, "A:おはよう");
        let (id, rest) = split_line_id_prefix("[笑] A:おはよう");
        assert_eq!(id, None);
        assert_eq!(rest, "[笑] A:おはよう");
    }

    #[test]
    fn parse_line_ref_variants() {
        assert_eq!(
            parse_line_ref("L003+500"),
            Some(LineRef {
                line: 3,
                offset_ms: 500
            })
        );
        assert_eq!(
            parse_line_ref("L003"),
            Some(LineRef {
                line: 3,
                offset_ms: 0
            })
        );
        assert_eq!(
            parse_line_ref("L003-200"),
            Some(LineRef {
                line: 3,
                offset_ms: -200
            })
        );
        assert_eq!(
            parse_line_ref("S01-L003+10"),
            Some(LineRef {
                line: 3,
                offset_ms: 10
            })
        );
        assert_eq!(parse_line_ref("L+5"), None);
        assert_eq!(parse_line_ref("003+5"), None);
        assert_eq!(parse_line_ref("L003+"), None);
    }
}
