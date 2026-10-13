//! `script2video number` の採番(台本仕様 §4)。台本を行単位で書き換え、ID 以外のバイトは変えない。

use std::collections::HashMap;

use crate::ids::{format_scene_id, parse_scene_id, split_line_id_prefix, LineId};
use crate::parser::ParseWarning;

/// `台本名.ids`: 使用済みの最大番号(削除した番号を再利用しないための記録)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdsRecord {
    pub max_line: u32,
    pub max_scene: u32,
}

impl IdsRecord {
    /// `.ids` の内容を読む。読めない行は無視する。
    pub fn parse(text: &str) -> Self {
        let mut r = Self::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().parse::<u32>().unwrap_or(0);
                match k.trim() {
                    "max_line" => r.max_line = v,
                    "max_scene" => r.max_scene = v,
                    _ => {}
                }
            }
        }
        r
    }

    pub fn to_text(&self) -> String {
        format!(
            "# script2video number が使用した最大番号(削除した番号を再利用しないための記録)\nmax_line={}\nmax_scene={}\n",
            self.max_line, self.max_scene
        )
    }
}

#[derive(Debug)]
pub struct Numbered {
    pub text: String,
    pub record: IdsRecord,
    pub assigned_lines: usize,
    pub assigned_scenes: usize,
    pub warnings: Vec<ParseWarning>,
}

enum Kind<'a> {
    Other,
    /// `#paragraph` の直後の文字列(`[S02] 名称` など)
    Paragraph {
        after: &'a str,
    },
    Speech {
        id: Option<LineId>,
    },
}

/// 台本に台詞IDとシーン番号を書き込む。ID の重複があれば何も変えずに Err を返す。
pub fn number_script(text: &str, record: IdsRecord) -> Result<Numbered, Vec<ParseWarning>> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => ("\u{feff}", b),
        None => ("", text),
    };
    let lines: Vec<&str> = body.split_inclusive('\n').collect();

    // 1回目: 既存の番号を集め、重複を検出する
    let mut errors = Vec::new();
    let mut seen_lines: HashMap<u32, usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    let mut section = String::new();
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let (content, _) = split_content(raw);
        update_section(&mut section, content);
        if section != "@script" {
            continue;
        }
        match classify(content) {
            Kind::Paragraph { after } => {
                if let Some(n) = existing_scene(after) {
                    if let Some(p) = seen_scenes.insert(n, line_no) {
                        errors.push(ParseWarning {
                            line_no,
                            message: format!(
                                "シーン番号 {} が重複しています（{p} 行目と同じ）",
                                format_scene_id(n)
                            ),
                        });
                    }
                }
            }
            Kind::Speech { id: Some(id) } => {
                if let Some(p) = seen_lines.insert(id.line, line_no) {
                    errors.push(ParseWarning {
                        line_no,
                        message: format!(
                            "台詞番号 L{:03} が重複しています（{p} 行目と同じ）",
                            id.line
                        ),
                    });
                }
            }
            _ => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut next_line = seen_lines
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(record.max_line)
        + 1;
    let mut next_scene = seen_scenes
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(record.max_scene)
        + 1;

    // 2回目: 番号のない行に書き込む
    let mut out = String::with_capacity(text.len() + 256);
    out.push_str(bom);
    let mut section = String::new();
    let mut current_scene = 0u32;
    let (mut assigned_lines, mut assigned_scenes) = (0usize, 0usize);
    let mut warnings = Vec::new();
    let mut warned_s00 = false;
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let (content, indent_len) = split_content(raw);
        update_section(&mut section, content);
        if section != "@script" {
            out.push_str(raw);
            continue;
        }
        match classify(content) {
            Kind::Paragraph { after } => {
                if let Some(n) = existing_scene(after) {
                    current_scene = n;
                    out.push_str(raw);
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let n = next_scene;
                    next_scene += 1;
                    assigned_scenes += 1;
                    current_scene = n;
                    let at = indent_len + "#paragraph".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[{}]", format_scene_id(n)));
                    out.push_str(&raw[at..]);
                } else {
                    // `[` で始まるが書式が不正: パーサが警告するので、ここでは触らない
                    out.push_str(raw);
                }
            }
            Kind::Speech { id: Some(_) } => out.push_str(raw),
            Kind::Speech { id: None } => {
                if current_scene == 0 && !warned_s00 {
                    warnings.push(ParseWarning {
                        line_no,
                        message: "最初の #paragraph より前の台詞は、暗黙のシーン S00 に属します"
                            .to_string(),
                    });
                    warned_s00 = true;
                }
                let id = LineId {
                    scene: current_scene,
                    line: next_line,
                };
                next_line += 1;
                assigned_lines += 1;
                out.push_str(&raw[..indent_len]);
                out.push_str(&format!("[{}] ", id.display()));
                out.push_str(&raw[indent_len..]);
            }
            Kind::Other => out.push_str(raw),
        }
    }

    Ok(Numbered {
        text: out,
        record: IdsRecord {
            max_line: next_line - 1,
            max_scene: next_scene - 1,
        },
        assigned_lines,
        assigned_scenes,
        warnings,
    })
}

/// 改行を除き、前後の空白を除いた内容と、行頭の字下げの長さ(バイト)を返す。
fn split_content(raw: &str) -> (&str, usize) {
    let no_eol = raw.trim_end_matches(['\n', '\r']);
    let content = no_eol.trim_start();
    let indent_len = no_eol.len() - content.len();
    (content.trim_end(), indent_len)
}

fn update_section(section: &mut String, content: &str) {
    if content.starts_with('@') {
        *section = content.split_whitespace().next().unwrap_or("").to_string();
    }
}

fn classify(content: &str) -> Kind<'_> {
    if content.is_empty() || content.starts_with('@') {
        return Kind::Other;
    }
    if let Some(rest) = content.strip_prefix('#') {
        if let Some(after) = rest.strip_prefix("paragraph") {
            if after.is_empty() || after.starts_with('[') || after.starts_with(char::is_whitespace)
            {
                return Kind::Paragraph { after };
            }
        }
        return Kind::Other;
    }
    if content.chars().all(|c| c.is_ascii_digit()) {
        return Kind::Other;
    }
    let (id, rest) = split_line_id_prefix(content);
    if id.is_some() {
        return Kind::Speech { id };
    }
    match rest.split_once(':') {
        Some((role, _)) if !role.trim().is_empty() => Kind::Speech { id: None },
        _ => Kind::Other,
    }
}

/// `[S02] 名称` からシーン番号を読む。
fn existing_scene(after: &str) -> Option<u32> {
    let inner = after.strip_prefix('[')?;
    let close = inner.find(']')?;
    parse_scene_id(&inner[..close])
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n";

    fn run(body: &str, record: IdsRecord) -> Numbered {
        number_script(&format!("{HEAD}{body}"), record).unwrap()
    }

    #[test]
    fn assigns_line_and_scene_numbers() {
        let n = run(
            "#paragraph 朝\nA:おはよう\nB:おはよう\n#paragraph\nA:またね\n",
            IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            format!("{HEAD}#paragraph[S01] 朝\n[S01-L001] A:おはよう\n[S01-L002] B:おはよう\n#paragraph[S02]\n[S02-L003] A:またね\n")
        );
        assert_eq!(n.assigned_lines, 3);
        assert_eq!(n.assigned_scenes, 2);
        assert_eq!(
            n.record,
            IdsRecord {
                max_line: 3,
                max_scene: 2
            }
        );
    }

    #[test]
    fn keeps_existing_ids_and_continues_after_the_maximum() {
        let n = run(
            "#paragraph[S03] 朝\n[S03-L007] A:一\nB:二\n#paragraph\nA:三\n",
            IdsRecord::default(),
        );
        assert!(
            n.text
                .contains("[S03-L007] A:一\n[S03-L008] B:二\n#paragraph[S04]\n[S04-L009] A:三\n"),
            "{}",
            n.text
        );
    }

    #[test]
    fn record_prevents_reuse_of_deleted_numbers() {
        let n = run(
            "#paragraph[S01]\n[S01-L001] A:一\nB:二\n",
            IdsRecord {
                max_line: 10,
                max_scene: 4,
            },
        );
        assert!(n.text.contains("[S01-L011] B:二"), "{}", n.text);
        assert_eq!(
            n.record,
            IdsRecord {
                max_line: 11,
                max_scene: 4
            }
        );
    }

    #[test]
    fn duplicates_are_errors_and_nothing_is_written() {
        let errs = number_script(
            &format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] B:二\n#paragraph[S01]\n"),
            IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn crlf_and_bom_are_preserved() {
        let src = "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph 朝\r\n  A:おはよう\r\n";
        let n = number_script(src, IdsRecord::default()).unwrap();
        assert_eq!(
            n.text,
            "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01] 朝\r\n  [S01-L001] A:おはよう\r\n"
        );
    }

    #[test]
    fn lines_outside_script_and_commands_are_untouched() {
        let n = run(
            "# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n",
            IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            format!("{HEAD}# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n")
        );
        assert_eq!(n.assigned_lines, 0);
    }

    #[test]
    fn lines_before_first_paragraph_get_s00_and_a_warning() {
        let n = run("A:前置き\n#paragraph\nB:本編\n", IdsRecord::default());
        assert!(
            n.text
                .contains("[S00-L001] A:前置き\n#paragraph[S01]\n[S01-L002] B:本編\n"),
            "{}",
            n.text
        );
        assert_eq!(n.warnings.len(), 1);
    }

    #[test]
    fn bracket_text_is_numbered_once() {
        let first = run("#paragraph\nA:[笑]それは:どうかな\n", IdsRecord::default());
        assert!(
            first.text.contains("[S01-L001] A:[笑]それは:どうかな\n"),
            "{}",
            first.text
        );
        let second = number_script(&first.text, first.record).unwrap();
        assert_eq!(second.text, first.text);
        assert_eq!(second.assigned_lines, 0);
    }

    #[test]
    fn ids_record_round_trip() {
        let r = IdsRecord {
            max_line: 12,
            max_scene: 3,
        };
        assert_eq!(IdsRecord::parse(&r.to_text()), r);
        assert_eq!(IdsRecord::parse("ゴミ\n"), IdsRecord::default());
    }
}
