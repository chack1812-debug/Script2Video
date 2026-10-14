//! `script2video number` の採番(台本仕様 §4)。台本を行単位で書き換え、ID 以外のバイトは変えない。

use std::collections::{BTreeMap, HashMap};

use crate::ids::{format_scene_id, parse_scene_id, split_line_id_prefix, LineId};
use crate::parser::ParseWarning;

/// `台本名.ids`: 使用済みの最大番号(削除した番号を再利用しないための記録)。
///
/// 台詞・カット・ト書きの番号は各シーンの中で数えるので、最大番号もシーンごとに持つ。
/// テキスト形式は `max_scene=<n>` と、シーンごとの `max_line.S01=<n>`・`max_cut.S01=<n>`・
/// `max_note.S01=<n>`(シーン番号は `format_scene_id` の書式)。①a の形式にあった全体で1つの
/// `max_line=<n>` は、台詞番号がシーン単位になったため読み捨てる。
/// `max_` で始まらないキーの行は将来の版の記録として `extra` に保存し、書き戻すときに残す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdsRecord {
    pub max_scene: u32,
    /// シーン番号 -> そのシーンで使用済みの最大の台詞番号
    pub scene_max: BTreeMap<u32, u32>,
    /// シーン番号 -> 使用済みの最大のカット番号(C01 は予約なので、記録は 2 以上)
    pub cut_max: BTreeMap<u32, u32>,
    /// シーン番号 -> 使用済みの最大のト書き番号
    pub note_max: BTreeMap<u32, u32>,
    /// 知らないキーの行(将来の版の記録)。書き戻すときにそのまま残す
    pub extra: Vec<String>,
}

impl IdsRecord {
    /// `.ids` の内容を読む。読めない行は無視する。
    pub fn parse(text: &str) -> Self {
        let mut r = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if !k.starts_with("max_") {
                if !k.is_empty() && !k.starts_with('#') {
                    r.extra.push(line.trim_end().to_string());
                }
                continue;
            }
            // 壊れた値の行は無視する(0 で上書きすると欠番が再利用されてしまう)。
            // 同じキーが複数回正しく読めた場合は、安全側として最大値を採る。
            let Ok(v) = v.trim().parse::<u32>() else {
                continue;
            };
            let per_scene = |prefix: &str| k.strip_prefix(prefix).and_then(parse_scene_id);
            let slot = if k == "max_scene" {
                r.max_scene = r.max_scene.max(v);
                continue;
            } else if let Some(s) = per_scene("max_line.") {
                r.scene_max.entry(s).or_insert(0)
            } else if let Some(s) = per_scene("max_cut.") {
                r.cut_max.entry(s).or_insert(0)
            } else if let Some(s) = per_scene("max_note.") {
                r.note_max.entry(s).or_insert(0)
            } else {
                // 旧形式の `max_line=<n>` や、シーン番号の壊れたキーは捨てる
                continue;
            };
            *slot = (*slot).max(v);
        }
        r
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# script2video number が使用した最大番号(削除した番号を再利用しないための記録)\n",
        );
        out.push_str(&format!("max_scene={}\n", self.max_scene));
        for (key, map) in [
            ("max_line", &self.scene_max),
            ("max_cut", &self.cut_max),
            ("max_note", &self.note_max),
        ] {
            for (scene, max) in map {
                out.push_str(&format!("{key}.{}={max}\n", format_scene_id(*scene)));
            }
        }
        for line in &self.extra {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

#[derive(Debug)]
pub struct Numbered {
    pub text: String,
    pub record: IdsRecord,
    pub assigned_lines: usize,
    pub assigned_scenes: usize,
    pub assigned_cuts: usize,
    pub assigned_notes: usize,
    pub warnings: Vec<ParseWarning>,
}

enum Kind<'a> {
    Other,
    /// `#paragraph` の直後の文字列(`[S02] 名称` など)
    Paragraph {
        after: &'a str,
    },
    /// `#cut` の直後の文字列(`[C02]` など)
    Cut {
        after: &'a str,
    },
    /// `#note` の直後の文字列
    Note {
        after: &'a str,
    },
    Speech {
        id: Option<LineId>,
    },
}

/// 台本に台詞IDとシーン番号を書き込む。台詞番号は各シーンで1から数える。
/// ID の重複、または ID のシーンが所属する #paragraph と食い違うときは、何も変えずに Err を返す。
pub fn number_script(text: &str, record: &IdsRecord) -> Result<Numbered, Vec<ParseWarning>> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => ("\u{feff}", b),
        None => ("", text),
    };
    let lines: Vec<&str> = body.split_inclusive('\n').collect();

    // 1回目: 既存の番号を集め、重複を検出する
    let mut errors = Vec::new();
    let mut seen_lines: HashMap<(u32, u32), usize> = HashMap::new();
    let mut seen_scenes: HashMap<u32, usize> = HashMap::new();
    // シーン番号がまだ無い #paragraph もあるので、シーンは「#paragraph の出現順のブロック番号」
    // (最初の #paragraph より前がブロック 0)で数える
    let mut block = 0usize;
    let mut seen_cuts: HashMap<(usize, u32), usize> = HashMap::new();
    let mut seen_notes: HashMap<(usize, u32), usize> = HashMap::new();
    let mut block_cut_max: HashMap<usize, u32> = HashMap::new();
    let mut block_note_max: HashMap<usize, u32> = HashMap::new();
    let mut section = String::new();
    for (i, raw) in lines.iter().enumerate() {
        let line_no = i + 1;
        let (content, _) = split_content(raw);
        update_section(&mut section, content);
        if section != "@script" {
            continue;
        }
        match classify(content) {
            Kind::Cut { after } => {
                if let Some(n) = existing_tag(after, 'C') {
                    if n == 1 {
                        errors.push(ParseWarning {
                            line_no,
                            message: "#cut[C01] は使えません(C01 はシーンの頭の予約です)"
                                .to_string(),
                        });
                    } else if let Some(p) = seen_cuts.insert((block, n), line_no) {
                        errors.push(ParseWarning {
                            line_no,
                            message: format!(
                                "カット番号 C{n:02} が重複しています（{p} 行目と同じ）"
                            ),
                        });
                    }
                    let e = block_cut_max.entry(block).or_insert(1);
                    *e = (*e).max(n);
                }
            }
            Kind::Note { after } => {
                if let Some(n) = existing_tag(after, 'N') {
                    if let Some(p) = seen_notes.insert((block, n), line_no) {
                        errors.push(ParseWarning {
                            line_no,
                            message: format!(
                                "ト書き番号 N{n:02} が重複しています（{p} 行目と同じ）"
                            ),
                        });
                    }
                    let e = block_note_max.entry(block).or_insert(0);
                    *e = (*e).max(n);
                }
            }
            Kind::Paragraph { after } => {
                block += 1;
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
                if let Some(p) = seen_lines.insert((id.scene, id.line), line_no) {
                    errors.push(ParseWarning {
                        line_no,
                        message: format!(
                            "台詞ID {} が重複しています（{p} 行目と同じ）",
                            id.display()
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

    // シーンごとの使用済みの最大台詞番号(既存の番号と .ids の記録の大きい方)。
    // 新しい番号が必要になったときだけ checked_add で進める。
    let mut high: BTreeMap<u32, u32> = record.scene_max.clone();
    for &(scene, line) in seen_lines.keys() {
        let e = high.entry(scene).or_insert(0);
        *e = (*e).max(line);
    }
    let mut last_scene = seen_scenes
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(record.max_scene);

    // シーンごとのカット・ト書きの高水位(.ids の記録と既存の番号の大きい方)。
    // カットは C01 が予約なので下限 1(最初は 2)、ト書きは下限 0(最初は 1)。
    let mut cut_high_by_scene: BTreeMap<u32, u32> = record.cut_max.clone();
    let mut note_high_by_scene: BTreeMap<u32, u32> = record.note_max.clone();
    // ブロックのシーン番号が決まったときに、そのシーンの高水位へブロックの既存の番号を反映する
    let raise =
        |cut: &mut BTreeMap<u32, u32>, note: &mut BTreeMap<u32, u32>, scene: u32, blk: usize| {
            let c = cut.entry(scene).or_insert(1);
            *c = (*c)
                .max(1)
                .max(block_cut_max.get(&blk).copied().unwrap_or(1));
            let n = note.entry(scene).or_insert(0);
            *n = (*n).max(block_note_max.get(&blk).copied().unwrap_or(0));
        };
    let mut block = 0usize;
    raise(&mut cut_high_by_scene, &mut note_high_by_scene, 0, 0);

    // 2回目: 番号のない行に書き込む
    let mut out = String::with_capacity(text.len() + 256);
    out.push_str(bom);
    let mut section = String::new();
    let mut current_scene = 0u32;
    let (mut assigned_lines, mut assigned_scenes) = (0usize, 0usize);
    let (mut assigned_cuts, mut assigned_notes) = (0usize, 0usize);
    let mut warnings = Vec::new();
    let mut warned_s00 = false;
    let mut mismatches = Vec::new();
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
                block += 1;
                if let Some(n) = existing_scene(after) {
                    current_scene = n;
                    raise(&mut cut_high_by_scene, &mut note_high_by_scene, n, block);
                    out.push_str(raw);
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let Some(n) = last_scene.checked_add(1) else {
                        return Err(exhausted("シーン番号"));
                    };
                    last_scene = n;
                    assigned_scenes += 1;
                    current_scene = n;
                    raise(&mut cut_high_by_scene, &mut note_high_by_scene, n, block);
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
            Kind::Cut { after } => {
                if existing_tag(after, 'C').is_some() {
                    out.push_str(raw); // 既存の番号(重複・C01 は1回目で検出済み)
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let high = cut_high_by_scene
                        .get_mut(&current_scene)
                        .expect("ブロックの開始で用意済み");
                    let Some(n) = high.checked_add(1) else {
                        return Err(exhausted("カット番号"));
                    };
                    *high = n;
                    assigned_cuts += 1;
                    let at = indent_len + "#cut".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[C{n:02}]"));
                    out.push_str(&raw[at..]);
                } else {
                    out.push_str(raw); // `[X]` など。パーサがエラーにする
                }
            }
            Kind::Note { after } => {
                if existing_tag(after, 'N').is_some() {
                    out.push_str(raw); // 既存の番号(重複・C01 は1回目で検出済み)
                } else if after.is_empty() || after.starts_with(char::is_whitespace) {
                    let high = note_high_by_scene
                        .get_mut(&current_scene)
                        .expect("ブロックの開始で用意済み");
                    let Some(n) = high.checked_add(1) else {
                        return Err(exhausted("ト書き番号"));
                    };
                    *high = n;
                    assigned_notes += 1;
                    let at = indent_len + "#note".len();
                    out.push_str(&raw[..at]);
                    out.push_str(&format!("[N{n:02}]"));
                    out.push_str(&raw[at..]);
                } else {
                    out.push_str(raw); // `[X]` など。パーサがエラーにする
                }
            }
            Kind::Speech { id: Some(id) } => {
                // ID のシーン部分は、所属する #paragraph のシーンと一致していなければならない
                if id.scene != current_scene {
                    mismatches.push(ParseWarning {
                        line_no,
                        message: format!(
                            "台詞ID {} のシーン {} が、所属するシーン {} と一致しません",
                            id.display(),
                            format_scene_id(id.scene),
                            format_scene_id(current_scene)
                        ),
                    });
                }
                out.push_str(raw);
            }
            Kind::Speech { id: None } => {
                if current_scene == 0 && !warned_s00 {
                    warnings.push(ParseWarning {
                        line_no,
                        message: "最初の #paragraph より前の台詞は、暗黙のシーン S00 に属します"
                            .to_string(),
                    });
                    warned_s00 = true;
                }
                let last_line = high.entry(current_scene).or_insert(0);
                let Some(line) = last_line.checked_add(1) else {
                    return Err(exhausted("台詞番号"));
                };
                *last_line = line;
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

    if !mismatches.is_empty() {
        return Err(mismatches);
    }

    Ok(Numbered {
        text: out,
        record: IdsRecord {
            max_scene: last_scene,
            scene_max: high,
            // 記録の無い・使っていないシーンの行を増やさない
            cut_max: cut_high_by_scene
                .into_iter()
                .filter(|&(_, n)| n >= 2)
                .collect(),
            note_max: note_high_by_scene
                .into_iter()
                .filter(|&(_, n)| n >= 1)
                .collect(),
            extra: record.extra.clone(),
        },
        assigned_lines,
        assigned_scenes,
        assigned_cuts,
        assigned_notes,
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
        let starts_ok = |after: &str| {
            after.is_empty() || after.starts_with('[') || after.starts_with(char::is_whitespace)
        };
        if let Some(after) = rest.strip_prefix("paragraph") {
            if starts_ok(after) {
                return Kind::Paragraph { after };
            }
        }
        if let Some(after) = rest.strip_prefix("cut") {
            if starts_ok(after) {
                return Kind::Cut { after };
            }
        }
        if let Some(after) = rest.strip_prefix("note") {
            if starts_ok(after) {
                return Kind::Note { after };
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

/// `[C02] …` の先頭の番号(`#cut`・`#note` の直後)。
fn existing_tag(after: &str, letter: char) -> Option<u32> {
    let close = after.find(']')?;
    crate::ids::parse_numbered_tag(&after[..=close], letter)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n";

    fn rec(max_scene: u32, scene_max: &[(u32, u32)]) -> IdsRecord {
        IdsRecord {
            max_scene,
            scene_max: scene_max.iter().copied().collect(),
            ..IdsRecord::default()
        }
    }

    fn run(body: &str, record: &IdsRecord) -> Numbered {
        number_script(&format!("{HEAD}{body}"), record).unwrap()
    }

    #[test]
    fn assigns_line_and_scene_numbers_restarting_in_each_scene() {
        let n = run(
            "#paragraph 朝\nA:おはよう\nB:おはよう\n#paragraph\nA:またね\n",
            &IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            format!("{HEAD}#paragraph[S01] 朝\n[S01-L001] A:おはよう\n[S01-L002] B:おはよう\n#paragraph[S02]\n[S02-L001] A:またね\n")
        );
        assert_eq!(n.assigned_lines, 3);
        assert_eq!(n.assigned_scenes, 2);
        assert_eq!(n.record, rec(2, &[(1, 2), (2, 1)]));
    }

    #[test]
    fn three_unnumbered_scenes_each_start_from_one_and_second_run_is_idempotent() {
        let body = "#paragraph\nA:一\nB:二\n#paragraph\nA:三\n#paragraph\nA:四\nB:五\nA:六\n";
        let first = run(body, &IdsRecord::default());
        assert_eq!(
            first.text,
            format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B:二\n#paragraph[S02]\n[S02-L001] A:三\n#paragraph[S03]\n[S03-L001] A:四\n[S03-L002] B:五\n[S03-L003] A:六\n")
        );
        assert_eq!(first.record, rec(3, &[(1, 2), (2, 1), (3, 3)]));
        let second = number_script(&first.text, &first.record).unwrap();
        assert_eq!(second.text, first.text);
        assert_eq!(second.record, first.record);
        assert_eq!(second.assigned_lines, 0);
        assert_eq!(second.assigned_scenes, 0);
        // 記録なしでも、既存の番号から同じ記録を再構成できる
        let third = number_script(&first.text, &IdsRecord::default()).unwrap();
        assert_eq!(third.text, first.text);
        assert_eq!(third.record, first.record);
    }

    #[test]
    fn keeps_existing_ids_and_continues_after_the_scene_maximum() {
        let n = run(
            "#paragraph[S03] 朝\n[S03-L007] A:一\nB:二\n#paragraph\nA:三\n",
            &IdsRecord::default(),
        );
        assert!(
            n.text
                .contains("[S03-L007] A:一\n[S03-L008] B:二\n#paragraph[S04]\n[S04-L001] A:三\n"),
            "{}",
            n.text
        );
    }

    #[test]
    fn record_prevents_reuse_of_deleted_numbers_per_scene() {
        let n = run(
            "#paragraph[S01]\n[S01-L001] A:一\nB:二\n#paragraph[S02]\nA:三\n",
            &rec(4, &[(1, 10), (2, 3)]),
        );
        assert!(n.text.contains("[S01-L011] B:二"), "{}", n.text);
        assert!(n.text.contains("[S02-L004] A:三"), "{}", n.text);
        assert_eq!(n.record, rec(4, &[(1, 11), (2, 4)]));
    }

    #[test]
    fn deleting_the_last_line_of_a_scene_does_not_free_its_number() {
        let first = run(
            "#paragraph\nA:一\nB:二\n#paragraph\nA:三\n",
            &IdsRecord::default(),
        );
        // S01 の最後の行(L002)を削除してから、S01 と S02 に新しい行を足す
        let edited = first
            .text
            .replace("[S01-L002] B:二\n", "")
            .replace("[S01-L001] A:一\n", "[S01-L001] A:一\nB:新1\n")
            .replace("[S02-L001] A:三\n", "[S02-L001] A:三\nB:新2\n");
        let second = number_script(&edited, &first.record).unwrap();
        assert!(second.text.contains("[S01-L003] B:新1"), "{}", second.text);
        assert!(second.text.contains("[S02-L002] B:新2"), "{}", second.text);
        assert_eq!(second.record, rec(2, &[(1, 3), (2, 2)]));
    }

    #[test]
    fn duplicates_are_errors_and_nothing_is_written() {
        let errs = number_script(
            &format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] B:二\n#paragraph[S01]\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn same_line_number_in_two_scenes_is_accepted_and_untouched() {
        let src =
            format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L001] A:二\n");
        let n = number_script(&src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(n.record, rec(2, &[(1, 1), (2, 1)]));
    }

    #[test]
    fn id_scene_disagreeing_with_paragraph_is_an_error() {
        let errs = number_script(
            &format!("{HEAD}#paragraph[S01]\n[S02-L001] A:一\nB:二\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("S02"), "{}", errs[0].message);
        assert!(errs[0].message.contains("S01"), "{}", errs[0].message);
        // 最初の #paragraph より前(シーン 0)に別シーンの ID がある場合
        let errs = number_script(
            &format!("{HEAD}[S01-L001] A:一\n#paragraph[S01]\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        // 番号のない #paragraph に、その番号と合わない ID の行がある場合
        let errs = number_script(
            &format!("{HEAD}#paragraph[S01]\n[S01-L001] A:一\n#paragraph\n[S01-L002] A:二\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn crlf_and_bom_are_preserved() {
        let src = "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph 朝\r\n  A:おはよう\r\n";
        let n = number_script(src, &IdsRecord::default()).unwrap();
        assert_eq!(
            n.text,
            "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01] 朝\r\n  [S01-L001] A:おはよう\r\n"
        );
    }

    #[test]
    fn lines_outside_script_and_commands_are_untouched() {
        let n = run(
            "# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n",
            &IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            format!("{HEAD}# コメント\n#pause 500\n2\n#visual 朝\n#paragraph[S01]\n")
        );
        assert_eq!(n.assigned_lines, 0);
    }

    #[test]
    fn lines_before_first_paragraph_get_s00_and_a_warning() {
        let n = run("A:前置き\n#paragraph\nB:本編\n", &IdsRecord::default());
        assert!(
            n.text
                .contains("[S00-L001] A:前置き\n#paragraph[S01]\n[S01-L001] B:本編\n"),
            "{}",
            n.text
        );
        assert_eq!(n.warnings.len(), 1);
        assert_eq!(n.record, rec(1, &[(0, 1), (1, 1)]));
    }

    #[test]
    fn bracket_text_is_numbered_once() {
        let first = run("#paragraph\nA:[笑]それは:どうかな\n", &IdsRecord::default());
        assert!(
            first.text.contains("[S01-L001] A:[笑]それは:どうかな\n"),
            "{}",
            first.text
        );
        let second = number_script(&first.text, &first.record).unwrap();
        assert_eq!(second.text, first.text);
        assert_eq!(second.assigned_lines, 0);
    }

    #[test]
    fn ids_record_round_trip_and_text_format() {
        let r = rec(3, &[(0, 2), (1, 12), (2, 5)]);
        let text = r.to_text();
        assert!(text.contains("max_scene=3\n"), "{text}");
        assert!(text.contains("max_line.S00=2\n"), "{text}");
        assert!(text.contains("max_line.S01=12\n"), "{text}");
        assert!(text.contains("max_line.S02=5\n"), "{text}");
        assert_eq!(IdsRecord::parse(&text), r);
        assert_eq!(IdsRecord::parse("ゴミ\n"), IdsRecord::default());
    }

    #[test]
    fn old_global_max_line_key_is_ignored() {
        // ①a の形式: max_line=<n>(全体で1つ)
        let r = IdsRecord::parse("max_line=9\nmax_scene=2\n");
        assert_eq!(r, rec(2, &[]));
        let n = run("#paragraph[S01]\nA:一\n", &r);
        assert!(n.text.contains("[S01-L001] A:一"), "{}", n.text);
    }

    fn script(body: &str) -> String {
        format!("{HEAD}{body}")
    }

    #[test]
    fn ids_record_ignores_corrupt_later_line() {
        let r = IdsRecord::parse(
            "max_line.S01=12\nmax_line.S01=壊れた値\nmax_scene=3\nmax_scene=\nmax_line.Sxx=4\nmax_line.=4\n",
        );
        assert_eq!(r, rec(3, &[(1, 12)]));
    }

    #[test]
    fn ids_record_corrupt_only_line_is_zero() {
        let r = IdsRecord::parse("max_line.S01=壊れた値\nmax_scene=-1\n");
        assert_eq!(r, IdsRecord::default());
    }

    #[test]
    fn ids_record_duplicate_valid_lines_keep_largest() {
        let r = IdsRecord::parse(
            "max_line.S01=5\nmax_line.S01=12\nmax_line.S01=7\nmax_scene=9\nmax_scene=2\n",
        );
        assert_eq!(r, rec(9, &[(1, 12)]));
    }

    #[test]
    fn exhausted_line_number_from_record_is_error() {
        let errs = number_script(
            &script("#paragraph[S01]\nA:一\n"),
            &rec(0, &[(1, u32::MAX)]),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("台詞番号"), "{}", errs[0].message);
    }

    #[test]
    fn exhaustion_in_one_scene_does_not_affect_another_scene() {
        let n = number_script(
            &script("#paragraph[S01]\n#paragraph[S02]\nA:一\n"),
            &rec(0, &[(1, u32::MAX)]),
        )
        .unwrap();
        assert!(n.text.contains("[S02-L001] A:一"), "{}", n.text);
    }

    #[test]
    fn exhausted_line_number_from_existing_id_is_error() {
        let errs = number_script(
            &script("#paragraph[S01]\n[S01-L4294967295] A:一\nB:二\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("台詞番号"), "{}", errs[0].message);
    }

    #[test]
    fn max_line_number_with_nothing_to_number_is_ok() {
        let src = script("#paragraph[S01]\n[S01-L4294967295] A:一\n");
        let n = number_script(&src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(n.record, rec(1, &[(1, u32::MAX)]));
        let n = number_script(&script("#paragraph[S01]\n"), &rec(0, &[(1, u32::MAX)])).unwrap();
        assert_eq!(n.record.scene_max[&1], u32::MAX);
    }

    #[test]
    fn exhausted_scene_number_from_record_is_error() {
        let errs = number_script(&script("#paragraph\n"), &rec(u32::MAX, &[])).unwrap_err();
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
            &IdsRecord::default(),
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
        let n = number_script(&src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(n.record, rec(u32::MAX, &[]));
        let n = number_script(&script("[S00-L001] A:一\n"), &rec(u32::MAX, &[])).unwrap();
        assert_eq!(n.record.max_scene, u32::MAX);
    }

    #[test]
    fn paragraph_whitespace_between_tokens_is_preserved() {
        let n = run(
            "#paragraph\n#paragraph\t朝\n#paragraph  \t 名称\n#paragraph[S09] \t名称\n\tA:x\n",
            &IdsRecord::default(),
        );
        assert_eq!(
            n.text,
            script("#paragraph[S10]\n#paragraph[S11]\t朝\n#paragraph[S12]  \t 名称\n#paragraph[S09] \t名称\n\t[S09-L001] A:x\n")
        );
    }

    #[test]
    fn paragraph_like_command_is_untouched() {
        let src = script("#paragraphXYZ\n#paragraph[S01]\n");
        let n = number_script(&src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
        assert_eq!(n.assigned_scenes, 0);
    }

    #[test]
    fn numbering_restarts_across_sections_and_scene_blocks() {
        let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph\nA:一\n@scene 台所\n@cast\nA:田中\n\n@script\n#paragraph\nA:二\n";
        let n = number_script(src, &IdsRecord::default()).unwrap();
        assert_eq!(
            n.text,
            "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n@scene 台所\n@cast\nA:田中\n\n@script\n#paragraph[S02]\n[S02-L001] A:二\n"
        );
        assert_eq!(n.record, rec(2, &[(1, 1), (2, 1)]));
    }

    #[test]
    fn last_line_without_trailing_newline() {
        let n = run("#paragraph\nA:一", &IdsRecord::default());
        assert_eq!(n.text, script("#paragraph[S01]\n[S01-L001] A:一"));
    }

    #[test]
    fn mixed_lf_and_crlf_are_preserved() {
        let n = run("#paragraph\r\nA:一\nB:二\r\n", &IdsRecord::default());
        assert_eq!(
            n.text,
            script("#paragraph[S01]\r\n[S01-L001] A:一\n[S01-L002] B:二\r\n")
        );
    }

    #[test]
    fn empty_and_bom_only_inputs() {
        let n = number_script("", &IdsRecord::default()).unwrap();
        assert_eq!(n.text, "");
        assert_eq!(n.record, IdsRecord::default());
        let n = number_script("\u{feff}", &IdsRecord::default()).unwrap();
        assert_eq!(n.text, "\u{feff}");
        assert_eq!(n.assigned_lines, 0);
    }

    #[test]
    fn cuts_and_notes_are_numbered_per_scene() {
        let n = run(
            "#paragraph\n#note 朝\nA:一\n#cut\n#note 昼\n#cut\n#paragraph\n#cut\n#note 夜\n",
            &IdsRecord::default(),
        );
        assert_eq!(n.text, script("#paragraph[S01]\n#note[N01] 朝\n[S01-L001] A:一\n#cut[C02]\n#note[N02] 昼\n#cut[C03]\n#paragraph[S02]\n#cut[C02]\n#note[N01] 夜\n"));
        assert_eq!(n.assigned_cuts, 3);
        assert_eq!(n.assigned_notes, 3);
        assert_eq!(n.record.cut_max, [(1, 3), (2, 2)].into_iter().collect());
        assert_eq!(n.record.note_max, [(1, 2), (2, 1)].into_iter().collect());
        let again = number_script(&n.text, &n.record).unwrap();
        assert_eq!(again.text, n.text);
        assert_eq!((again.assigned_cuts, again.assigned_notes), (0, 0));
    }

    #[test]
    fn inserted_cut_after_existing_higher_number_continues_from_max() {
        // Review Focus 2: 旧 .ids(max_cut が無い)でも、既存の C05 の後に続ける
        let n = run(
            "#paragraph[S01]\n[S01-L001] A:一\n#cut\n#cut[C05]\n",
            &IdsRecord::parse("max_scene=1\nmax_line.S01=1\n"),
        );
        assert!(n.text.contains("#cut[C06]\n#cut[C05]\n"), "{}", n.text);
        assert_eq!(n.record.cut_max[&1], 6);
    }

    #[test]
    fn record_prevents_reuse_of_deleted_cut_and_note_numbers() {
        let mut r = IdsRecord::default();
        r.cut_max.insert(1, 4);
        r.note_max.insert(1, 7);
        let n = run("#paragraph[S01]\n#cut\n#note x\n", &r);
        assert!(n.text.contains("#cut[C05]\n#note[N08] x\n"), "{}", n.text);
    }

    #[test]
    fn cut_and_note_duplicates_and_c01_are_errors() {
        let errs = number_script(
            &script("#paragraph[S01]\n#cut[C02]\n#cut[C02]\n#note[N01] a\n#note[N01] b\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 2, "{errs:?}");
        let errs = number_script(
            &script("#paragraph[S01]\n#cut[C01]\n"),
            &IdsRecord::default(),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 1, "{errs:?}");
        // 別のシーンなら同じ番号でよい
        assert!(number_script(
            &script("#paragraph[S01]\n#cut[C02]\n#paragraph[S02]\n#cut[C02]\n"),
            &IdsRecord::default()
        )
        .is_ok());
    }

    #[test]
    fn cut_and_note_in_s00() {
        let n = run(
            "#note 前置き\nA:一\n#cut\n#paragraph\n",
            &IdsRecord::default(),
        );
        assert!(
            n.text
                .contains("#note[N01] 前置き\n[S00-L001] A:一\n#cut[C02]\n#paragraph[S01]\n"),
            "{}",
            n.text
        );
        assert!(n.record.to_text().contains("max_cut.S00=2\n"));
        assert!(n.record.to_text().contains("max_note.S00=1\n"));
    }

    #[test]
    fn cut_and_note_numbering_preserves_crlf_and_bom() {
        // Review Focus 5
        let src = "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01]\r\n  #cut\r\n\t#note  翌朝\r\n";
        let n = number_script(src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, "\u{feff}@scene 居間\r\n@cast\r\nA:田中\r\n\r\n@script\r\n#paragraph[S01]\r\n  #cut[C02]\r\n\t#note[N01]  翌朝\r\n");
    }

    #[test]
    fn malformed_cut_and_note_tags_are_untouched() {
        let src = script("#paragraph[S01]\n#cut[X]\n#note[回想] x\n#cutting\n#notes x\n");
        let n = number_script(&src, &IdsRecord::default()).unwrap();
        assert_eq!(n.text, src);
    }

    #[test]
    fn ids_record_keeps_unknown_keys_and_round_trips_cut_and_note() {
        let r = IdsRecord::parse("max_scene=2\nmax_line.S01=3\nmax_cut.S01=4\nmax_note.S02=5\nfuture.key=abc\nmax_line=9\nゴミ\n");
        assert_eq!(r.cut_max, [(1, 4)].into_iter().collect());
        assert_eq!(r.note_max, [(2, 5)].into_iter().collect());
        assert_eq!(r.extra, vec!["future.key=abc".to_string()]);
        let text = r.to_text();
        assert!(text.contains("max_cut.S01=4\n"), "{text}");
        assert!(text.contains("max_note.S02=5\n"), "{text}");
        assert!(text.ends_with("future.key=abc\n"), "{text}");
        assert!(!text.contains("max_line=9"), "{text}");
        assert_eq!(IdsRecord::parse(&text), r);
    }
}
