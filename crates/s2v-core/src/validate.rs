//! 出力を止める問題(終了コード 2)の検査(設計書 5.7)。パースの後、配置の前に行う。

use std::collections::HashMap;

use crate::ids::{format_scene_id, LineId, LineRef};
use crate::parser::ParseWarning;
use crate::types::{Scene, ScriptCommand, ScriptItem, SeAnchor, VisualKind};

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
    // 台詞IDは組 (シーン, 台詞番号) で一意。同じ台詞番号が別のシーンにあってよい。
    let mut seen_lines: HashMap<(u32, u32), usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    let mut seen_cuts: HashMap<(u32, u32), usize> = HashMap::new();
    let mut seen_notes: HashMap<(u32, u32), usize> = HashMap::new();
    // 現在のシーン番号。最初の #paragraph より前は 0、番号のない #paragraph の中は None
    let mut cur_scene: Option<u32> = Some(0);
    // (参照, #se が書かれたシーン, 行番号)
    let mut se_refs: Vec<(LineRef, Option<u32>, usize)> = Vec::new();
    let mut block: Option<Block> = None;
    let mut warned_s00 = false;
    // 現在のカットの #visual の種類(最初に種類を示したもの)。#paragraph と #cut で空に戻す
    let mut cut_kind: Option<VisualKind> = None;

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
                    let own = id.map(|i| i.scene).or(cur_scene);
                    if let Some(scene) = r.scene.or(own) {
                        if !seen_lines.contains_key(&(scene, r.line)) {
                            let target = LineId {
                                scene,
                                line: r.line,
                            };
                            let msg = if line_scene_known(&seen_lines, &seen_scenes, scene) {
                                format!(
                                    "at= は前にある台詞だけを参照できます（{} は前にありません）",
                                    target.display()
                                )
                            } else {
                                format!(
                                    "at= が参照するシーン {} は、この台詞より前にありません（{}）",
                                    format_scene_id(scene),
                                    target.display()
                                )
                            };
                            d.errors.push(pw(*line_no, msg));
                        }
                    }
                }
                match id {
                    None => d.errors.push(pw(
                        *line_no,
                        "台詞IDがありません。先に `script2video number` を実行してください",
                    )),
                    Some(id) => {
                        if let Some(prev) = seen_lines.insert((id.scene, id.line), *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!(
                                    "台詞ID {} が重複しています（{prev} 行目と同じ）",
                                    id.display()
                                ),
                            ));
                        }
                        if let Some(cur) = cur_scene {
                            if id.scene != cur {
                                d.errors.push(pw(
                                    *line_no,
                                    format!(
                                        "台詞ID {} のシーン {} が、所属するシーン {} と一致しません",
                                        id.display(),
                                        format_scene_id(id.scene),
                                        format_scene_id(cur)
                                    ),
                                ));
                            }
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
                cut_kind = None;
                cur_scene = *id;
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
            ScriptItem::Command(ScriptCommand::Visual {
                kind: Some(k),
                line_no,
                ..
            }) if block.is_some() => match cut_kind {
                None => cut_kind = Some(*k),
                Some(prev) if prev != *k => d.errors.push(pw(
                    *line_no,
                    "同じカットの #visual に image と video が混在しています",
                )),
                _ => {}
            },
            ScriptItem::Command(ScriptCommand::Cut { id, line_no })
            | ScriptItem::Command(ScriptCommand::Note { id, line_no, .. }) => {
                if matches!(item, ScriptItem::Command(ScriptCommand::Cut { .. })) {
                    cut_kind = None;
                }
                let (what, seen) = if matches!(item, ScriptItem::Command(ScriptCommand::Cut { .. }))
                {
                    ("カット", &mut seen_cuts)
                } else {
                    ("ト書き", &mut seen_notes)
                };
                match (id, cur_scene) {
                    (None, _) => d.errors.push(pw(
                        *line_no,
                        format!(
                            "{what}の番号がありません。先に `script2video number` を実行してください"
                        ),
                    )),
                    (Some(n), Some(scene)) => {
                        if let Some(prev) = seen.insert((scene, *n), *line_no) {
                            d.errors.push(pw(
                                *line_no,
                                format!("{what}の番号が重複しています({prev} 行目と同じ)"),
                            ));
                        }
                    }
                    (Some(_), None) => {} // 番号の無い #paragraph の中(そちらがエラーになる)
                }
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
                se_refs.push((*r, cur_scene, *line_no));
            }
            _ => {}
        }
    }
    close_block(&mut block, &mut d);

    for (r, own, line_no) in se_refs {
        // 短縮形 L003 は、#se が書かれたシーンの台詞を指す
        let Some(scene) = r.scene.or(own) else {
            continue;
        };
        if !seen_lines.contains_key(&(scene, r.line)) {
            let target = LineId {
                scene,
                line: r.line,
            };
            let msg = if line_scene_known(&seen_lines, &seen_scenes, scene) {
                format!("#se の基準の台詞 {} が台本にありません", target.display())
            } else {
                format!(
                    "#se の基準のシーン {} が台本にありません（{}）",
                    format_scene_id(scene),
                    target.display()
                )
            };
            d.errors.push(pw(line_no, msg));
        }
    }
    d
}

/// そのシーン番号がここまでの台本にあるか(`#paragraph[Sxx]`、または暗黙の S00 に台詞がある)。
fn line_scene_known(
    seen_lines: &HashMap<(u32, u32), usize>,
    seen_scenes: &HashMap<u32, usize>,
    scene: u32,
) -> bool {
    seen_scenes.contains_key(&scene) || seen_lines.keys().any(|(s, _)| *s == scene)
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
    fn cut_and_note_need_numbers_and_unique_ids() {
        let d = check("#paragraph[S01]\n[S01-L001] A:一\n#cut\n#note 翌朝\n");
        assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
        let d = check(
            "#paragraph[S01]\n[S01-L001] A:一\n#cut[C02]\n#cut[C02]\n#note[N01] a\n#note[N01] b\n",
        );
        assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
        // 別のシーンなら同じ番号でよい
        let d = check(
            "#paragraph[S01]\n[S01-L001] A:一\n#cut[C02]\n#paragraph[S02]\n[S02-L001] A:二\n#cut[C02]\n",
        );
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    }

    #[test]
    fn image_and_video_on_different_lines_of_one_cut_is_an_error() {
        let d = check(
            "#paragraph[S01]
#visual (image) a
#visual (video) b
[S01-L001] A:一
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert_eq!(d.errors[0].line_no, 10);
        assert!(d.errors[0].message.contains("image と video が混在"));
        // 同じ種類・種類の無い行は混在ではない
        let d = check(
            "#paragraph[S01]
#visual (image) a
#visual b
#visual (image) c
[S01-L001] A:一
",
        );
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn image_and_video_in_different_cuts_or_scenes_is_fine() {
        let d = check(
            "#paragraph[S01]
#visual (image) a
[S01-L001] A:一
#cut[C02]
#visual (video) b
[S01-L002] A:二
#paragraph[S02]
#visual (video) c
[S02-L001] A:三
",
        );
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
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

    #[test]
    fn same_line_number_in_different_scenes_is_ok() {
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
#paragraph[S02]
[S02-L001] B:二
",
        );
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn same_scene_and_line_pair_twice_is_an_error() {
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
[S01-L001] B:二
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert!(d.errors[0].message.contains("S01-L001"));
    }

    #[test]
    fn id_scene_must_match_enclosing_paragraph() {
        let d = check(
            "#paragraph[S01]
[S02-L001] A:一
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert!(
            d.errors[0].message.contains("S02"),
            "{}",
            d.errors[0].message
        );
        assert!(
            d.errors[0].message.contains("S01"),
            "{}",
            d.errors[0].message
        );
        // 最初の #paragraph より前の台詞はシーン 0
        let d = check(
            "[S01-L001] A:一
#paragraph[S01]
[S01-L002] B:二
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert!(
            d.errors[0].message.contains("S00"),
            "{}",
            d.errors[0].message
        );
    }

    #[test]
    fn short_at_resolves_in_the_referencing_lines_scene() {
        // S02 に L001 が無ければ、S01 に L001 があっても短縮形は解決できない
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
#paragraph[S02]
[S02-L002] B(at=L001):二
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        // S02 に L001 があれば解決できる
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
#paragraph[S02]
[S02-L001] A:二
[S02-L002] B(at=L001+100):三
",
        );
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn full_form_at_can_reach_another_scene() {
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
#paragraph[S02]
[S02-L001] B(at=S01-L001+100):二
",
        );
        assert!(d.errors.is_empty(), "{:?}", messages(&d.errors));
    }

    #[test]
    fn full_form_at_to_unknown_scene_or_later_scene_is_an_error() {
        let d = check(
            "#paragraph[S01]
[S01-L001] A(at=S09-L001):一
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert!(
            d.errors[0].message.contains("S09"),
            "{}",
            d.errors[0].message
        );
        // 後ろのシーンの台詞は「前にある台詞」ではない
        let d = check(
            "#paragraph[S01]
[S01-L001] A(at=S02-L001):一
#paragraph[S02]
[S02-L001] B:二
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        // 存在しない台詞番号
        let d = check(
            "#paragraph[S01]
[S01-L001] A:一
#paragraph[S02]
[S02-L001] B(at=S01-L009):二
",
        );
        assert_eq!(d.errors.len(), 1, "{:?}", messages(&d.errors));
        assert!(
            d.errors[0].message.contains("S01-L009"),
            "{}",
            d.errors[0].message
        );
    }

    #[test]
    fn se_refs_accept_short_and_full_forms() {
        let ok = check(
            "#paragraph[S01]
[S01-L001] A:一
#se a.wav at=L001
#paragraph[S02]
[S02-L001] B:二
#se b.wav end=L001-100
#se c.wav at=S01-L001+50
#se d.wav end=S02-L001
",
        );
        assert!(ok.errors.is_empty(), "{:?}", messages(&ok.errors));
        // 短縮形は #se が書かれたシーンで解決する(S02 に L005 は無い)
        let bad = check(
            "#paragraph[S01]
[S01-L005] A:一
#paragraph[S02]
[S02-L001] B:二
#se a.wav at=L005
",
        );
        assert_eq!(bad.errors.len(), 1, "{:?}", messages(&bad.errors));
        let unknown = check(
            "#paragraph[S01]
[S01-L001] A:一
#se a.wav end=S07-L001
",
        );
        assert_eq!(unknown.errors.len(), 1, "{:?}", messages(&unknown.errors));
        assert!(unknown.errors[0].message.contains("S07"));
    }

    #[test]
    fn scene_zero_lines_are_referenced_with_short_and_full_forms() {
        // 最初の #paragraph より前の台詞は S00。短縮形はその中で解決する
        let ok = check("[S00-L001] A:一\n[S00-L002] B(at=L001+100):二\n#paragraph[S01]\n[S01-L001] A(at=S00-L001+100):三\n");
        assert!(ok.errors.is_empty(), "{:?}", messages(&ok.errors));
        // S00 の短縮形は S01 の台詞を指さない
        let bad = check("[S00-L001] A(at=L002):一\n#paragraph[S01]\n[S01-L002] B:二\n");
        assert_eq!(bad.errors.len(), 1, "{:?}", messages(&bad.errors));
        // S00 に台詞が無ければ S00 は存在しない
        let none = check("#paragraph[S01]\n[S01-L001] A(at=S00-L001):一\n");
        assert_eq!(none.errors.len(), 1, "{:?}", messages(&none.errors));
    }

    #[test]
    fn se_before_first_paragraph_anchors_to_scene_zero() {
        let ok = check("[S00-L001] A:一\n#se a.wav at=L001\n#paragraph[S01]\n[S01-L001] B:二\n");
        assert!(ok.errors.is_empty(), "{:?}", messages(&ok.errors));
        // S01 にしか無い番号は、S00 で書かれた #se の短縮形では見つからない
        let bad = check("[S00-L001] A:一\n#se a.wav at=L002\n#paragraph[S01]\n[S01-L002] B:二\n");
        assert_eq!(bad.errors.len(), 1, "{:?}", messages(&bad.errors));
    }
}
