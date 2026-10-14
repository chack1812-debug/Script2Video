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
                // 壊れた値の行は無視する(0 で上書きすると欠番が再利用されてしまう)。
                // 同じキーが複数回正しく読めた場合は、安全側として最大値を採る。
                let Ok(v) = v.trim().parse::<u32>() else {
                    continue;
                };
                match k.trim() {
                    "max_line" => r.max_line = r.max_line.max(v),
                    "max_scene" => r.max_scene = r.max_scene.max(v),
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

    // 使用済みの最大番号。新しい番号が必要になったときだけ checked_add で進める。
    let mut last_line = seen_lines
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(record.max_line);
    let mut last_scene = seen_scenes
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(record.max_scene);

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
                    let Some(n) = last_scene.checked_add(1) else {
                        return Err(exhausted("シーン番号"));
                    };
                    last_scene = n;
                    assigned_scenes += 1;
                    current_scene = n;
                    let at = indent_len + "#paragraph".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[{}]", format_scene_id(n)));
                    out.push_str(&raw[at..]);
                } else {
                    // `#paragraph` の直後が `[` だが `[Sxx]` として読めない(`[abc]` など): パーサが警告するので触らない。
                    // `#paragraphXYZ` のような別コマンドは classify が Other にするので、ここには来ない
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
                let Some(line) = last_line.checked_add(1) else {
                    return Err(exhausted("台詞番号"));
                };
                last_line = line;
                let id = LineId {
                    scene: current_scene,
                    line,
                };
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
            max_line: last_line,
            max_scene: last_scene,
        },
        assigned_lines,
        assigned_scenes,
        warnings,
    })
}

fn exhausted(kind: &str) -> Vec<ParseWarning> {
    vec![ParseWarning {
        line_no: 0,
        message: format!("{kind}が上限に達しました。これ以上採番できません"),
    }]
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

    fn script(body: &str) -> String {
        format!("{HEAD}{body}")
    }

    #[test]
    fn ids_record_ignores_corrupt_later_line() {
        let r = IdsRecord::parse("max_line=12\nmax_line=壊れた値\nmax_scene=3\nmax_scene=\n");
        assert_eq!(
            r,
            IdsRecord {
                max_line: 12,
                max_scene: 3
            }
        );
    }

    #[test]
    fn ids_record_corrupt_only_line_is_zero() {
        let r = IdsRecord::parse("max_line=壊れた値\nmax_scene=-1\n");
        assert_eq!(r, IdsRecord::default());
    }

    #[test]
    fn ids_record_duplicate_valid_lines_keep_largest() {
        let r = IdsRecord::parse("max_line=5\nmax_line=12\nmax_line=7\nmax_scene=9\nmax_scene=2\n");
        assert_eq!(
            r,
            IdsRecord {
                max_line: 12,
                max_scene: 9
            }
        );
    }

    #[test]
    fn exhausted_line_number_from_record_is_error() {
        let errs = number_script(
            &script("#paragraph[S01]\nA:一\n"),
            IdsRecord {
                max_line: u32::MAX,
                max_scene: 0,
            },
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("台詞番号"), "{}", errs[0].message);
    }

    #[test]
    fn exhausted_line_number_from_existing_id_is_error() {
        let errs = number_script(
            &script("#paragraph[S01]\n[S01-L4294967295] A:一\nB:二\n"),
            IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("台詞番号"), "{}", errs[0].message);
    }

    #[test]
    fn max_line_number_with_nothing_to_number_is_ok() {
        let src = script("#paragraph[S01]\n[S01-L4294967295] A:一\n");
        let n = number_script(&src, IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(
            n.record,
            IdsRecord {
                max_line: u32::MAX,
                max_scene: 1
            }
        );
        let n = number_script(
            &script("#paragraph[S01]\n"),
            IdsRecord {
                max_line: u32::MAX,
                max_scene: 0,
            },
        )
        .unwrap();
        assert_eq!(n.record.max_line, u32::MAX);
    }

    #[test]
    fn exhausted_scene_number_from_record_is_error() {
        let errs = number_script(
            &script("#paragraph\n"),
            IdsRecord {
                max_line: 0,
                max_scene: u32::MAX,
            },
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].message.contains("シーン番号"),
            "{}",
            errs[0].message
        );
    }

    #[test]
    fn exhausted_scene_number_from_existing_id_is_error() {
        let errs = number_script(
            &script("#paragraph[S4294967295]\n#paragraph\n"),
            IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].message.contains("シーン番号"),
            "{}",
            errs[0].message
        );
    }

    #[test]
    fn max_scene_number_with_nothing_to_number_is_ok() {
        let src = script("#paragraph[S4294967295]\n");
        let n = number_script(&src, IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(
            n.record,
            IdsRecord {
                max_line: 0,
                max_scene: u32::MAX
            }
        );
        let n = number_script(
            &script("[S01-L001] A:一\n"),
            IdsRecord {
                max_line: 0,
                max_scene: u32::MAX,
            },
        )
        .unwrap();
        assert_eq!(n.record.max_scene, u32::MAX);
    }

    #[test]
    fn paragraph_whitespace_between_tokens_is_preserved() {
        let n = run(
            "#paragraph\n#paragraph\t朝\n#paragraph  \t 名称\n#paragraph[S09] \t名称\n\tA:x\n",
            IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            script("#paragraph[S10]\n#paragraph[S11]\t朝\n#paragraph[S12]  \t 名称\n#paragraph[S09] \t名称\n\t[S09-L001] A:x\n")
        );
    }

    #[test]
    fn paragraph_like_command_is_untouched() {
        let src = script("#paragraphXYZ\n#paragraph[S01]\n");
        let n = number_script(&src, IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(n.assigned_scenes, 0);
    }

    #[test]
    fn numbering_continues_across_sections_and_scene_blocks() {
        let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph\nA:一\n@scene 台所\n@cast\nA:田中\n\n@script\n#paragraph\nA:二\n";
        let n = number_script(src, IdsRecord::default()).unwrap();
        assert_eq!(
            n.text,
            "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n@scene 台所\n@cast\nA:田中\n\n@script\n#paragraph[S02]\n[S02-L002] A:二\n"
        );
        assert_eq!(
            n.record,
            IdsRecord {
                max_line: 2,
                max_scene: 2
            }
        );
    }

    #[test]
    fn last_line_without_trailing_newline() {
        let n = run("#paragraph\nA:一", IdsRecord::default());
        assert_eq!(n.text, script("#paragraph[S01]\n[S01-L001] A:一"));
    }

    #[test]
    fn mixed_lf_and_crlf_are_preserved() {
        let n = run("#paragraph\r\nA:一\nB:二\r\n", IdsRecord::default());
        assert_eq!(
            n.text,
            script("#paragraph[S01]\r\n[S01-L001] A:一\n[S01-L002] B:二\r\n")
        );
    }

    #[test]
    fn empty_and_bom_only_inputs() {
        let n = number_script("", IdsRecord::default()).unwrap();
        assert_eq!(n.text, "");
        assert_eq!(n.record, IdsRecord::default());
        let n = number_script("\u{feff}", IdsRecord::default()).unwrap();
        assert_eq!(n.text, "\u{feff}");
        assert_eq!(n.assigned_lines, 0);
    }
}
