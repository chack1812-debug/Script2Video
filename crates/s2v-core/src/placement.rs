//! 台詞・シーン・イベントの配置(設計書 5.5)。位置と長さは 48kHz のサンプル数。

use std::collections::HashMap;

use serde::Serialize;

use crate::ids::{format_scene_id, LineId};
use crate::time::ms_to_samples;
use crate::types::{PauseConfig, Scene, ScriptCommand, ScriptItem, SeAnchor, VisualKind};

/// 台詞が不足した理由(`timeline.json` の `reason`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    NoFile,
    Duplicate,
    Unreadable,
    Silent,
    BadFormat,
    Unmeasurable,
}

impl MissingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            MissingReason::NoFile => "no_file",
            MissingReason::Duplicate => "duplicate",
            MissingReason::Unreadable => "unreadable",
            MissingReason::Silent => "silent",
            MissingReason::BadFormat => "bad_format",
            MissingReason::Unmeasurable => "unmeasurable",
        }
    }
}

/// 台詞の音声の有無と長さ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSource {
    /// 使用範囲の長さ(サンプル)
    Present {
        len: i64,
    },
    Missing(MissingReason),
}

/// 台本の並びに平らにした項目。`scene_index` は `@scene`(音響の場面)の番号。
pub struct FlatItem<'a> {
    pub item: &'a ScriptItem,
    pub pause: &'a PauseConfig,
    pub scene_index: usize,
}

pub fn flatten(scenes: &[Scene]) -> Vec<FlatItem<'_>> {
    scenes
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            s.items.iter().map(move |item| FlatItem {
                item,
                pause: &s.pause_config,
                scene_index: i,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLine {
    pub id: LineId,
    /// 属するシーン(`#paragraph`)の番号。最初の `#paragraph` より前は 0。
    pub scene: u32,
    pub cast: String,
    pub text: String,
    pub start: i64,
    pub end: i64,
    pub missing: Option<MissingReason>,
    /// `flatten` の結果での位置
    pub flat_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedScene {
    pub id: u32,
    pub name: Option<String>,
    /// 1始まりの並び順
    pub index: usize,
    /// 無言のシーン(台詞なし、#pause のみ)
    pub silent: bool,
    pub start: i64,
    pub end: i64,
    pub visual_text: Option<String>,
    pub visual_kind: Option<VisualKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacedEvent {
    Se {
        path: String,
        start: i64,
        gain_db: f64,
    },
    BgmStart {
        path: String,
        start: i64,
    },
    BgmStop {
        start: i64,
    },
}

impl PlacedEvent {
    pub fn start(&self) -> i64 {
        match self {
            PlacedEvent::Se { start, .. }
            | PlacedEvent::BgmStart { start, .. }
            | PlacedEvent::BgmStop { start } => *start,
        }
    }
}

#[derive(Debug, Default)]
pub struct Placement {
    pub lines: Vec<PlacedLine>,
    pub scenes: Vec<PlacedScene>,
    pub events: Vec<PlacedEvent>,
    /// 継続できる問題(終了コード 1)
    pub warnings: Vec<String>,
    /// 出力を止める問題(終了コード 2)
    pub errors: Vec<String>,
}

impl Placement {
    /// 台詞・シーン・イベントの最も遅い時刻(サンプル)。
    pub fn end_sample(&self) -> i64 {
        let l = self.lines.iter().map(|l| l.end).max().unwrap_or(0);
        let s = self.scenes.iter().map(|s| s.end).max().unwrap_or(0);
        let e = self.events.iter().map(|e| e.start()).max().unwrap_or(0);
        l.max(s).max(e)
    }
}

struct OpenScene {
    id: u32,
    name: Option<String>,
    open_cursor: i64,
    pause_total: i64,
    span: Option<(i64, i64)>,
    visual_text: Option<String>,
    visual_kind: Option<VisualKind>,
}

impl OpenScene {
    fn new(id: u32, name: Option<String>, open_cursor: i64) -> Self {
        Self {
            id,
            name,
            open_cursor,
            pause_total: 0,
            span: None,
            visual_text: None,
            visual_kind: None,
        }
    }

    fn add_line(&mut self, start: i64, end: i64) {
        self.span = Some(match self.span {
            Some((a, b)) => (a.min(start), b.max(end)),
            None => (start, end),
        });
    }
}

fn placeholder_len(text: &str, ms_per_char: u32) -> i64 {
    (text.chars().count() as i64).saturating_mul(ms_to_samples(ms_per_char as f64))
}

/// 台本の並びに台詞を配置する。`sources` に無い台詞は `no_file` として仮の無音を置く。
pub fn place(
    items: &[FlatItem<'_>],
    sources: &HashMap<LineId, LineSource>,
    placeholder_ms_per_char: u32,
) -> Placement {
    let mut p = Placement::default();
    // cursor: ここまでに置いた全台詞の終了時刻の最大値(後退しない)
    let mut cursor: i64 = 0;
    // last_end: 台本の並びで直前の台詞の終了時刻(overlap の基準)
    let mut last_end: i64 = 0;
    let mut prev_cast: Option<&str> = None;
    let mut any_line = false;
    let mut first_in_scene = true;
    let mut open: Option<OpenScene> = None;
    let mut closed: Vec<OpenScene> = Vec::new();
    let mut spans: HashMap<u32, (i64, i64)> = HashMap::new();
    let mut deferred_se: Vec<(String, SeAnchor, f64)> = Vec::new();
    let mut parallel_left = 0usize;
    let mut parallel_anchor: Option<i64> = None;

    for (flat_index, fi) in items.iter().enumerate() {
        match fi.item {
            ScriptItem::Command(cmd) => match cmd {
                ScriptCommand::Paragraph { id, name, .. } => {
                    if let Some(s) = open.take() {
                        closed.push(s);
                    }
                    open = Some(OpenScene::new(id.unwrap_or(0), name.clone(), cursor));
                    first_in_scene = true;
                }
                ScriptCommand::Visual { kind, text } => match open.as_mut() {
                    Some(s) => {
                        s.visual_text = Some(match s.visual_text.take() {
                            Some(t) => format!("{t}\n{text}"),
                            None => text.clone(),
                        });
                        if s.visual_kind.is_none() {
                            s.visual_kind = *kind;
                        }
                    }
                    None => p.warnings.push(
                        "#visual が最初の #paragraph より前にあります（無視します）".to_string(),
                    ),
                },
                ScriptCommand::Pause(ms) => {
                    // 負の値や極端な値でも cursor が後退・あふれないようにする
                    let n = ms_to_samples(*ms).max(0);
                    cursor = cursor.saturating_add(n);
                    if let Some(s) = open.as_mut() {
                        s.pause_total = s.pause_total.saturating_add(n);
                    }
                }
                ScriptCommand::Parallel(n) => {
                    parallel_left = *n;
                    parallel_anchor = None;
                }
                ScriptCommand::BgmStart(path) => p.events.push(PlacedEvent::BgmStart {
                    path: path.clone(),
                    start: cursor,
                }),
                ScriptCommand::BgmStop => p.events.push(PlacedEvent::BgmStop { start: cursor }),
                ScriptCommand::Se {
                    path,
                    anchor,
                    gain_db,
                    ..
                } => match anchor {
                    None => p.events.push(PlacedEvent::Se {
                        path: path.clone(),
                        start: cursor,
                        gain_db: *gain_db,
                    }),
                    Some(a) => deferred_se.push((path.clone(), *a, *gain_db)),
                },
            },
            ScriptItem::Speech {
                id,
                cast_name,
                text,
                overlap_ms,
                at,
                ..
            } => {
                // ID の無い台詞は検証(validate)で弾かれる
                let Some(id) = *id else { continue };
                if open.is_none() {
                    open = Some(OpenScene::new(0, None, cursor));
                    first_in_scene = true;
                }
                let (len, missing) = match sources.get(&id) {
                    Some(LineSource::Present { len }) => (*len, None),
                    Some(LineSource::Missing(r)) => {
                        (placeholder_len(text, placeholder_ms_per_char), Some(*r))
                    }
                    None => (
                        placeholder_len(text, placeholder_ms_per_char),
                        Some(MissingReason::NoFile),
                    ),
                };
                let gap = if !any_line {
                    0
                } else if first_in_scene {
                    ms_to_samples(fi.pause.paragraph_ms)
                } else if prev_cast == Some(cast_name.as_str()) {
                    ms_to_samples(fi.pause.sentence_ms)
                } else {
                    ms_to_samples(fi.pause.cast_ms)
                };
                let mut start = if let Some(n) = overlap_ms {
                    let s = last_end.saturating_sub(ms_to_samples(*n));
                    if s < 0 {
                        p.warnings.push(format!(
                            "{}: overlap が大きすぎて開始が 0 より前になります（0 にします）",
                            id.display()
                        ));
                        0
                    } else {
                        s
                    }
                } else if let Some(r) = at {
                    match spans.get(&r.line) {
                        Some(&(s, _)) => {
                            let t = s.saturating_add(ms_to_samples(r.offset_ms as f64));
                            if t < 0 {
                                p.warnings.push(format!(
                                    "{}: at が大きすぎて開始が 0 より前になります（0 にします）",
                                    id.display()
                                ));
                                0
                            } else {
                                t
                            }
                        }
                        None => cursor.saturating_add(gap),
                    }
                } else {
                    cursor.saturating_add(gap)
                };
                if parallel_left > 0 {
                    match parallel_anchor {
                        Some(a) => start = a,
                        None => parallel_anchor = Some(start),
                    }
                    parallel_left -= 1;
                }
                let end = start.saturating_add(len);
                spans.insert(id.line, (start, end));
                cursor = cursor.max(end);
                last_end = end;
                prev_cast = Some(cast_name.as_str());
                any_line = true;
                first_in_scene = false;
                let s = open.as_mut().expect("直前で開いている");
                s.add_line(start, end);
                let scene = s.id;
                p.lines.push(PlacedLine {
                    id,
                    scene,
                    cast: cast_name.clone(),
                    text: text.clone(),
                    start,
                    end,
                    missing,
                    flat_index,
                });
            }
        }
    }
    if let Some(s) = open.take() {
        closed.push(s);
    }

    // 台詞を基準にした SE(書かれた位置とは無関係に、指定した台詞を基準に置く)
    for (path, anchor, gain_db) in deferred_se {
        let (r, base) = match anchor {
            SeAnchor::Start(r) => (r, spans.get(&r.line).map(|v| v.0)),
            SeAnchor::End(r) => (r, spans.get(&r.line).map(|v| v.1)),
        };
        let Some(base) = base else {
            p.errors.push(format!(
                "#se の基準の台詞 L{:03} が見つかりません: {path}",
                r.line
            ));
            continue;
        };
        let mut start = base.saturating_add(ms_to_samples(r.offset_ms as f64));
        if start < 0 {
            p.warnings.push(format!(
                "#se の位置が 0 より前になります（0 にします）: {path}"
            ));
            start = 0;
        }
        p.events.push(PlacedEvent::Se {
            path,
            start,
            gain_db,
        });
    }

    // シーンの時間: 所属する台詞の最小開始〜最大終了。無言のシーンは #pause の合計
    for (i, s) in closed.into_iter().enumerate() {
        let (start, end, silent) = match s.span {
            Some((a, b)) => (a, b, false),
            None if s.pause_total > 0 => (
                s.open_cursor,
                s.open_cursor.saturating_add(s.pause_total),
                true,
            ),
            None => {
                p.errors.push(format!(
                    "{}: 台詞も #pause もないシーンです",
                    format_scene_id(s.id)
                ));
                (s.open_cursor, s.open_cursor, true)
            }
        };
        p.scenes.push(PlacedScene {
            id: s.id,
            name: s.name,
            index: i + 1,
            silent,
            start,
            end,
            visual_text: s.visual_text,
            visual_kind: s.visual_kind,
        });
    }
    // シーンの開始時刻は台本の並びで単調非減少(同時刻は許す)
    for w in p.scenes.windows(2) {
        if w[1].start < w[0].start {
            p.errors.push(format!(
                "シーン {} の開始({}ms)が、前のシーン {} の開始({}ms)より早くなっています",
                format_scene_id(w[1].id),
                w[1].start / 48,
                format_scene_id(w[0].id),
                w[0].start / 48
            ));
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ScriptParser;

    fn scenes(body: &str) -> Vec<Scene> {
        let text = format!(
            "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中\n\nB:佐藤\n\n@script\n{body}"
        );
        ScriptParser::new().parse_str(&text).unwrap()
    }

    fn present(list: &[(u32, u32, f64)]) -> HashMap<LineId, LineSource> {
        list.iter()
            .map(|&(scene, line, ms)| {
                (
                    LineId { scene, line },
                    LineSource::Present {
                        len: ms_to_samples(ms),
                    },
                )
            })
            .collect()
    }

    fn ms(samples: i64) -> f64 {
        samples as f64 / 48.0
    }

    fn line_ms(p: &Placement, line: u32) -> (f64, f64) {
        let l = p.lines.iter().find(|l| l.id.line == line).unwrap();
        (ms(l.start), ms(l.end))
    }

    #[test]
    fn first_line_at_zero_and_gaps_follow_pause_rules() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] A:二\n[S01-L003] B:三\n#paragraph[S02]\n[S02-L004] A:四\n");
        let src = present(&[
            (1, 1, 1000.0),
            (1, 2, 1000.0),
            (1, 3, 1000.0),
            (2, 4, 1000.0),
        ]);
        let p = place(&flatten(&s), &src, 200);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(line_ms(&p, 1), (0.0, 1000.0));
        assert_eq!(line_ms(&p, 2), (1500.0, 2500.0));
        assert_eq!(line_ms(&p, 3), (2800.0, 3800.0));
        assert_eq!(line_ms(&p, 4), (4800.0, 5800.0));
        assert_eq!(p.scenes.len(), 2);
        assert_eq!((ms(p.scenes[0].start), ms(p.scenes[0].end)), (0.0, 3800.0));
        assert_eq!(p.scenes[1].index, 2);
    }

    #[test]
    fn overlap_uses_last_line_end_and_normal_line_uses_cursor() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n[S01-L002] B(at=L001+1000):二\n[S01-L003] A(overlap=100):三\n[S01-L004] B:四\n");
        let src = present(&[
            (1, 1, 5000.0),
            (1, 2, 1000.0),
            (1, 3, 1000.0),
            (1, 4, 1000.0),
        ]);
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 2), (1000.0, 2000.0));
        assert_eq!(line_ms(&p, 3), (1900.0, 2900.0));
        assert_eq!(line_ms(&p, 4), (5300.0, 6300.0));
    }

    #[test]
    fn silent_scene_takes_pause_time_without_paragraph_pause() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n#paragraph[S02] 間\n#pause 2000\n#paragraph[S03]\n[S03-L002] A:二\n");
        let src = present(&[(1, 1, 1000.0), (3, 2, 1000.0)]);
        let p = place(&flatten(&s), &src, 200);
        let s2 = &p.scenes[1];
        assert!(s2.silent);
        assert_eq!((ms(s2.start), ms(s2.end)), (1000.0, 3000.0));
        assert_eq!(s2.name.as_deref(), Some("間"));
        assert_eq!(line_ms(&p, 2), (4000.0, 5000.0));
        assert!(!p.scenes[2].silent);
    }

    #[test]
    fn missing_lines_use_placeholder_length_and_keep_reason() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:あいうえお\n[S01-L002] B:かき\n");
        let mut src = HashMap::new();
        src.insert(
            LineId { scene: 1, line: 2 },
            LineSource::Missing(MissingReason::Silent),
        );
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 1), (0.0, 1000.0));
        assert_eq!(p.lines[0].missing, Some(MissingReason::NoFile));
        assert_eq!(line_ms(&p, 2), (1300.0, 1700.0));
        assert_eq!(p.lines[1].missing, Some(MissingReason::Silent));
    }

    #[test]
    fn se_positions_follow_anchors_and_are_clamped() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n#se now.wav\n#se door.wav at=L001+200\n#se bell.wav end=L001-200 gain=-3\n#se early.wav at=L001-500\n");
        let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
        let at = |name: &str| {
            p.events
                .iter()
                .find_map(|e| match e {
                    PlacedEvent::Se {
                        path,
                        start,
                        gain_db,
                    } if path.ends_with(name) => Some((ms(*start), *gain_db)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(at("now.wav"), (1000.0, 0.0));
        assert_eq!(at("door.wav"), (200.0, 0.0));
        assert_eq!(at("bell.wav"), (800.0, -3.0));
        assert_eq!(at("early.wav"), (0.0, 0.0));
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn parallel_lines_start_together() {
        let s = scenes("#paragraph[S01]\n[S01-L001] A:一\n2\n[S01-L002] A:二\n[S01-L003] B:三\n[S01-L004] A:四\n");
        let src = present(&[
            (1, 1, 1000.0),
            (1, 2, 1000.0),
            (1, 3, 2000.0),
            (1, 4, 1000.0),
        ]);
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(line_ms(&p, 2), (1500.0, 2500.0));
        assert_eq!(line_ms(&p, 3), (1500.0, 3500.0));
        assert_eq!(line_ms(&p, 4), (3800.0, 4800.0));
    }

    #[test]
    fn scene_start_regression_is_an_error_but_same_time_is_allowed() {
        let bad = scenes("#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L002] B(overlap=3500):二\n");
        let src = present(&[(1, 1, 1000.0), (2, 2, 1000.0)]);
        let p = place(&flatten(&bad), &src, 200);
        assert_eq!(line_ms(&p, 1), (3000.0, 4000.0));
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);

        let same = scenes("#paragraph[S01]\n#pause 3000\n[S01-L001] A:一\n#paragraph[S02]\n[S02-L002] B(at=L001):二\n");
        let p = place(&flatten(&same), &src, 200);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
    }

    #[test]
    fn bgm_events_and_visuals_are_recorded() {
        let s = scenes("#bgm_start a.wav\n#paragraph[S01] 朝\n#visual (image) 港。\n#visual 船。\n[S01-L001] A:一\n#bgm_stop\n");
        let p = place(&flatten(&s), &present(&[(1, 1, 1000.0)]), 200);
        assert!(matches!(
            p.events[0],
            PlacedEvent::BgmStart { start: 0, .. }
        ));
        assert!(
            matches!(p.events[1], PlacedEvent::BgmStop { start } if start == ms_to_samples(1000.0))
        );
        assert_eq!(p.scenes[0].visual_text.as_deref(), Some("港。\n船。"));
        assert_eq!(p.scenes[0].visual_kind, Some(VisualKind::Image));
        assert_eq!(p.end_sample(), ms_to_samples(1000.0));
    }

    #[test]
    fn huge_pause_overlap_and_at_values_saturate_without_panic() {
        let src = present(&[
            (1, 1, 1000.0),
            (1, 2, 1000.0),
            (1, 3, 1000.0),
            (1, 4, 1000.0),
        ]);
        let s = scenes(
            "#paragraph[S01]
#pause 1e300
[S01-L001] A:一
[S01-L002] A:二
",
        );
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.lines[0].start, i64::MAX);
        assert_eq!(p.lines[1].start, i64::MAX);
        let s = scenes(
            "#paragraph[S01]
[S01-L001] A:一
[S01-L002] B(overlap=1e300):二
[S01-L003] B(at=L001+9223372036854775807):三
[S01-L004] B(at=L001-9223372036854775807):四
#se x.wav at=L001+9223372036854775807
#se y.wav end=L001-9223372036854775807
",
        );
        let p = place(&flatten(&s), &src, 200);
        assert_eq!(p.lines.len(), 4);
        assert_eq!(p.lines[1].start, 0);
        assert!(p.lines.iter().all(|l| l.start >= 0 && l.end >= l.start));
    }
}
