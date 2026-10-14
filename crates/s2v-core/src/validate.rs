//! 出力を止める問題(終了コード 2)の検査(設計書 5.7)。パースの後、配置の前に行う。

use std::collections::HashMap;

use crate::parser::ParseWarning;
use crate::types::{Scene, ScriptCommand, ScriptItem, SeAnchor};

#[derive(Debug, Default)]
pub struct Diagnostics {
    /// 出力を止める問題(終了コード 2)
    pub errors: Vec<ParseWarning>,
    /// 継続できる問題(終了コード 1)
    pub warnings: Vec<ParseWarning>,
}

/// 検査中のシーン(`#paragraph` から次の `#paragraph` まで)。
struct Block {
    line_no: usize,
    lines: usize,
    pause_ms: f64,
}

pub fn validate(scenes: &[Scene]) -> Diagnostics {
    let mut d = Diagnostics::default();
    let mut seen_lines: HashMap<u32, usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    let mut se_refs: Vec<(u32, usize)> = Vec::new();
    let mut block: Option<Block> = None;
    let mut warned_s00 = false;

    for item in scenes.iter().flat_map(|s| s.items.iter()) {
        match item {
            ScriptItem::Speech {
                id,
                line_no,
                overlap_ms,
                at,
                ..
            } => {
                if overlap_ms.is_some() && at.is_some() {
                    d.errors
                        .push(pw(*line_no, "overlap と at は同時に指定できません"));
                }
                if let Some(r) = at {
                    if !seen_lines.contains_key(&r.line) {
                        d.errors.push(pw(
                            *line_no,
                            format!(
                                "at= は前にある台詞だけを参照できます（L{:03} は前にありません）",
                                r.line
                            ),
                        ));
                    }
                }
                match id {
                    None => d.errors.push(pw(
                        *line_no,
                        "台詞IDがありません。先に `script2video number` を実行してください",
                    )),
                    Some(id) => {
                        if let Some(prev) = seen_lines.insert(id.line, *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!(
                                    "台詞番号 L{:03} が重複しています（{prev} 行目と同じ）",
                                    id.line
                                ),
                            ));
                        }
                    }
                }
                match block.as_mut() {
                    Some(b) => b.lines += 1,
                    None => {
                        if !warned_s00 {
                            d.warnings.push(pw(
                                *line_no,
                                "最初の #paragraph より前の台詞は、暗黙のシーン S00 に属します",
                            ));
                            warned_s00 = true;
                        }
                    }
                }
            }
            ScriptItem::Command(ScriptCommand::Paragraph { id, line_no, .. }) => {
                close_block(&mut block, &mut d);
                match id {
                    None => d.errors.push(pw(
                        *line_no,
                        "シーン番号がありません。先に `script2video number` を実行してください",
                    )),
                    Some(n) => {
                        if let Some(prev) = seen_scenes.insert(*n, *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!("シーン番号 S{n:02} が重複しています（{prev} 行目と同じ）"),
                            ));
                        }
                    }
                }
                block = Some(Block {
                    line_no: *line_no,
                    lines: 0,
                    pause_ms: 0.0,
                });
            }
            ScriptItem::Command(ScriptCommand::Pause(ms)) => {
                if let Some(b) = block.as_mut() {
                    b.pause_ms += ms;
                }
            }
            ScriptItem::Command(ScriptCommand::Se {
                anchor: Some(a),
                line_no,
                ..
            }) => {
                let r = match a {
                    SeAnchor::Start(r) | SeAnchor::End(r) => r,
                };
                se_refs.push((r.line, *line_no));
            }
            _ => {}
        }
    }
    close_block(&mut block, &mut d);

    for (line, line_no) in se_refs {
        if !seen_lines.contains_key(&line) {
            d.errors.push(pw(
                line_no,
                format!("#se の基準の台詞 L{line:03} が台本にありません"),
            ));
        }
    }
    d
}

fn close_block(block: &mut Option<Block>, d: &mut Diagnostics) {
    if let Some(b) = block.take() {
        if b.lines == 0 && b.pause_ms <= 0.0 {
            d.errors.push(pw(
                b.line_no,
                "台詞も #pause もないシーンです（無言のシーンには #pause が必要です）",
            ));
        }
    }
}

fn pw(line_no: usize, message: impl Into<String>) -> ParseWarning {
    ParseWarning {
        line_no,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ScriptParser;

    fn check(body: &str) -> Diagnostics {
        let text = format!("@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n{body}");
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(&text).unwrap();
        validate(&scenes)
    }

    fn messages(v: &[ParseWarning]) -> Vec<String> {
        v.iter().map(|w| w.message.clone()).collect()
    }

    #[test]
    fn valid_script_has_no_errors() {
        let d = check("#paragraph[S01] 朝\n[S01-L001] A:一\n[S01-L002] B(at=L001+100):二\n#se a.wav at=L002\n#paragraph[S02]\n#pause 1000\n");
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn duplicate_line_and_scene_numbers_are_errors() {
        let d = check(
            "#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] B:二\n#paragraph[S01]\n[S01-L002] A:三\n",
        );
        assert_eq!(d.errors.len(), 2, "{:?}", messages(&d.errors));
    }

    #[test]
    fn at_must_reference_an_earlier_line() {
        let d = check("#paragraph[S01]\n[S01-L001] A(at=L002):一\n[S01-L002] B(at=L002):二\n[S01-L003] A(at=L009):三\n");
        assert_eq!(d.errors.len(), 3, "{:?}", messages(&d.errors));
    }

    #[test]
    fn overlap_and_at_together_is_an_error() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B(overlap=100,at=L001):二\n");
        assert_eq!(d.errors.len(), 1);
        assert!(d.errors[0].message.contains("overlap"));
    }

    #[test]
    fn missing_ids_are_errors() {
        let d = check("#paragraph\nA:一\n");
        assert_eq!(d.errors.len(), 2, "{:?}", messages(&d.errors));
        assert!(d.errors.iter().all(|e| e.message.contains("number")));
    }

    #[test]
    fn scene_without_lines_needs_pause() {
        let ok = check("#paragraph[S01]\n#pause 500\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert!(ok.errors.is_empty(), "{:?}", messages(&ok.errors));
        let empty = check("#paragraph[S01]\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert_eq!(empty.errors.len(), 1);
        let zero = check("#paragraph[S01]\n#pause 0\n#paragraph[S02]\n[S02-L001] A:一\n");
        assert_eq!(zero.errors.len(), 1);
    }

    #[test]
    fn se_reference_to_unknown_line_is_an_error() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n#se a.wav end=L005\n");
        assert_eq!(d.errors.len(), 1);
    }

    #[test]
    fn lines_before_first_paragraph_warn_once() {
        let d = check("[S00-L001] A:一\n[S00-L002] B:二\n#paragraph[S01]\n[S01-L003] A:三\n");
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].message.contains("S00"));
    }
}
