//! `timeline.json` v3(設計書 §6、映像工程設計書 §3.5)。後続工程との契約。
//! 型と検証は s2v-timeline にあり、ここでは組み立てて検証してから書き出す。

use std::collections::HashMap;
use std::path::Path;

use s2v_core::ids::{format_scene_id, LineId};
use s2v_core::placement::{MissingReason, PlacedEvent, Placement};
use s2v_core::time::{ceil_ms, floor_ms};
use s2v_core::visual_timeline::SceneSpans;
use s2v_timeline::{
    CastEntry, CutEntry, EventEntry, LineEntry, Normalize, NoteEntry, SceneEntry, TakeEntry,
    Timeline, Trim, Visual,
};

#[derive(Debug, Clone)]
pub struct CastInfo {
    pub name: String,
    pub speaker: String,
    pub appearance: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TakeInfo {
    /// 単独の台詞は台詞IDの `-` を `_` にした名前(`S01_L003`)
    pub id: String,
    pub files: Vec<String>,
    /// (head_ms, tail_ms)。不足なら None
    pub trims: Option<(i64, i64)>,
    pub missing: Option<MissingReason>,
}

/// 正常な台詞の、収録ファイル上の使用範囲(サンプル)と正規化の記録。
#[derive(Debug, Clone, Copy)]
pub struct LineAudioInfo {
    pub src_start: i64,
    pub src_end: i64,
    pub measured_lufs: f64,
    pub requested_gain_db: f64,
    pub norm_gain_db: f64,
    pub norm_clamped: bool,
}

pub struct TimelineInput<'a> {
    pub script: String,
    /// 台本ファイルのバイト列の SHA-256(小文字16進)
    pub script_sha256: String,
    /// `full_dialogue.wav` のバイト列の SHA-256(小文字16進)
    pub audio_sha256: String,
    pub duration_samples: i64,
    pub target_lufs: f64,
    pub casts: Vec<CastInfo>,
    pub placement: &'a Placement,
    pub takes: Vec<TakeInfo>,
    pub line_audio: &'a HashMap<LineId, LineAudioInfo>,
    /// `layout_visuals` の結果(`placement.scenes` と同じ並び)
    pub visuals: &'a [SceneSpans],
}

fn reason(r: MissingReason) -> s2v_timeline::Reason {
    use s2v_timeline::Reason as R;
    match r {
        MissingReason::NoFile => R::NoFile,
        MissingReason::Duplicate => R::Duplicate,
        MissingReason::Unreadable => R::Unreadable,
        MissingReason::Silent => R::Silent,
        MissingReason::BadFormat => R::BadFormat,
        MissingReason::Unmeasurable => R::Unmeasurable,
    }
}

fn kind(k: s2v_core::VisualKind) -> s2v_timeline::VisualKind {
    match k {
        s2v_core::VisualKind::Image => s2v_timeline::VisualKind::Image,
        s2v_core::VisualKind::Video => s2v_timeline::VisualKind::Video,
    }
}

/// 小数第2位に丸める(LUFS・dB の表示用)。
fn r2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    // -0.0 は "-0.0" と出力されるので正のゼロにそろえる
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// 有限でない値は `timeline.json` に出せない(契約: 数値は有限値のみ)。
fn finite(name: &str, v: f64) -> anyhow::Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        anyhow::bail!("timeline.json に有限でない値が含まれています: {name} = {v}")
    }
}

/// 丸めた値が有限であることを確かめる(巨大な値は丸めで無限大になり得る)。
fn finite_r2(name: &str, v: f64) -> anyhow::Result<f64> {
    finite(name, v)?;
    finite(name, r2(v))
}

/// 組み立てて、§3.6 の不変条件を検証する。違反は内部の誤りとして Err。
pub fn build_timeline(input: &TimelineInput<'_>) -> anyhow::Result<Timeline> {
    finite("normalize.target_lufs", input.target_lufs)?;
    let p = input.placement;
    let mut lines = Vec::with_capacity(p.lines.len());
    for l in &p.lines {
        let a = if l.missing.is_none() {
            input.line_audio.get(&l.id)
        } else {
            None
        };
        let id = l.id.display();
        if l.missing.is_none() && a.is_none() {
            anyhow::bail!("正常な台詞 {id} の音声情報(LineAudioInfo)がありません");
        }
        let (measured_lufs, requested_gain_db, norm_gain_db) = match a {
            Some(a) => (
                Some(finite_r2(&format!("{id}.measured_lufs"), a.measured_lufs)?),
                Some(finite_r2(
                    &format!("{id}.requested_gain_db"),
                    a.requested_gain_db,
                )?),
                Some(finite_r2(&format!("{id}.norm_gain_db"), a.norm_gain_db)?),
            ),
            None => (None, None, None),
        };
        lines.push(LineEntry {
            id,
            scene: format_scene_id(l.scene),
            cast: l.cast.clone(),
            text: l.text.clone(),
            start_ms: floor_ms(l.start),
            end_ms: ceil_ms(l.end),
            take: l.id.file_stem(),
            track: None,
            measured_lufs,
            requested_gain_db,
            norm_gain_db,
            norm_clamped: a.map(|a| a.norm_clamped),
            src_start_ms: a.map(|a| floor_ms(a.src_start)),
            src_end_ms: a.map(|a| ceil_ms(a.src_end)),
            missing: l.missing.is_some(),
            reason: l.missing.map(reason),
        });
    }

    let mut events: Vec<&PlacedEvent> = p.events.iter().collect();
    events.sort_by_key(|e| e.start());
    let mut event_docs = Vec::with_capacity(events.len());
    for e in events {
        event_docs.push(match e {
            PlacedEvent::Se {
                path,
                start,
                gain_db,
            } => EventEntry::Se {
                file: file_name(path),
                start_ms: floor_ms(*start),
                gain_db: finite_r2("events.se.gain_db", *gain_db)?,
            },
            PlacedEvent::BgmStart { path, start } => EventEntry::BgmStart {
                file: file_name(path),
                start_ms: floor_ms(*start),
            },
            PlacedEvent::BgmStop { start } => EventEntry::BgmStop {
                start_ms: floor_ms(*start),
            },
        });
    }

    if p.scenes.len() != input.visuals.len() {
        anyhow::bail!("内部の誤り: シーンとカットの区間の数が違います");
    }
    let scenes = p
        .scenes
        .iter()
        .zip(input.visuals)
        .map(|(s, v)| SceneEntry {
            id: format_scene_id(s.id),
            name: s.name.clone(),
            index: s.index,
            silent: s.silent,
            start_ms: floor_ms(s.start),
            end_ms: ceil_ms(s.end),
            cuts: v
                .cuts
                .iter()
                .map(|c| CutEntry {
                    id: c.id.clone(),
                    start_ms: c.start_ms,
                    end_ms: c.end_ms,
                    visual: c.visual.as_ref().map(|x| Visual {
                        text: x.text.clone(),
                        kind: x.kind.map(kind),
                        looped: x.looped,
                    }),
                })
                .collect(),
            notes: v
                .notes
                .iter()
                .map(|n| NoteEntry {
                    id: n.id.clone(),
                    text: n.text.clone(),
                    start_ms: n.start_ms,
                    end_ms: n.end_ms,
                })
                .collect(),
        })
        .collect();

    let t = Timeline {
        schema_version: s2v_timeline::SCHEMA_VERSION,
        script: input.script.clone(),
        script_sha256: input.script_sha256.clone(),
        audio_sha256: input.audio_sha256.clone(),
        sample_rate: 48_000,
        duration_ms: ceil_ms(input.duration_samples),
        normalize: Normalize {
            target_lufs: input.target_lufs,
        },
        cast: input
            .casts
            .iter()
            .map(|c| CastEntry {
                name: c.name.clone(),
                speaker: c.speaker.clone(),
                appearance: c.appearance.clone(),
            })
            .collect(),
        scenes,
        takes: input
            .takes
            .iter()
            .map(|t| TakeEntry {
                id: t.id.clone(),
                tracks: vec![None],
                files: t.files.clone(),
                trims: t.trims.map(|(h, tl)| {
                    vec![Trim {
                        track: None,
                        head_ms: h,
                        tail_ms: tl,
                    }]
                }),
                missing: t.missing.is_some(),
                reason: t.missing.map(reason),
            })
            .collect(),
        lines,
        events: event_docs,
    };
    if let Err(errs) = t.validate() {
        anyhow::bail!(
            "timeline.json の検証に失敗しました(内部の誤り): {}",
            errs.join(" / ")
        );
    }
    Ok(t)
}

pub fn timeline_value(input: &TimelineInput<'_>) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::to_value(build_timeline(input)?)?)
}

/// 検証を通ったものだけを書く(通らなければファイルは作らない)。
pub fn write_timeline_json(path: &Path, input: &TimelineInput<'_>) -> anyhow::Result<()> {
    std::fs::write(path, build_timeline(input)?.to_json_pretty())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::placement::{PlacedCut, PlacedLine, PlacedScene, PlacedVisual};
    use s2v_core::visual_timeline::layout_visuals;
    use s2v_core::VisualKind;

    /// 役 A・B と、台詞ごとのテイクを置いた、既定の組み立て(§3.6 の不変条件を満たす)。
    /// 正常な台詞のテイクは `{ID}.wav`、不足の台詞のテイクは no_file。
    fn input_with<'a>(
        placement: &'a Placement,
        audio: &'a HashMap<LineId, LineAudioInfo>,
        visuals: &'a [SceneSpans],
    ) -> TimelineInput<'a> {
        TimelineInput {
            script: "s.txt".into(),
            script_sha256: "a".repeat(64),
            audio_sha256: "b".repeat(64),
            duration_samples: 480_000,
            target_lufs: -23.0,
            casts: ["A", "B"]
                .into_iter()
                .map(|n| CastInfo {
                    name: n.into(),
                    speaker: format!("話者{n}"),
                    appearance: None,
                })
                .collect(),
            placement,
            takes: placement
                .lines
                .iter()
                .map(|l| {
                    let id = l.id.file_stem();
                    match l.missing {
                        None => TakeInfo {
                            files: vec![format!("{id}.wav")],
                            id,
                            trims: Some((10, 20)),
                            missing: None,
                        },
                        Some(r) => TakeInfo {
                            id,
                            files: vec![],
                            trims: None,
                            missing: Some(r),
                        },
                    }
                })
                .collect(),
            line_audio: audio,
            visuals,
        }
    }

    fn visuals_of(placement: &Placement) -> Vec<SceneSpans> {
        layout_visuals(&placement.scenes, 10_000).0
    }

    fn cut(number: u32) -> PlacedCut {
        PlacedCut {
            number,
            at: None,
            visual: None,
        }
    }

    fn sample_placement() -> Placement {
        Placement {
            lines: vec![
                PlacedLine {
                    id: LineId { scene: 1, line: 1 },
                    scene: 1,
                    cast: "A".into(),
                    text: "一".into(),
                    start: 1,
                    end: 48_001,
                    missing: None,
                    flat_index: 0,
                },
                PlacedLine {
                    id: LineId { scene: 1, line: 2 },
                    scene: 1,
                    cast: "B".into(),
                    text: "二".into(),
                    start: 96_000,
                    end: 120_000,
                    missing: Some(MissingReason::NoFile),
                    flat_index: 1,
                },
            ],
            scenes: vec![
                PlacedScene {
                    id: 1,
                    name: Some("朝".into()),
                    index: 1,
                    silent: false,
                    start: 1,
                    end: 120_000,
                    cuts: vec![PlacedCut {
                        number: 1,
                        at: None,
                        visual: Some(PlacedVisual {
                            text: "港".into(),
                            kind: Some(VisualKind::Image),
                            looped: false,
                        }),
                    }],
                    notes: vec![],
                },
                PlacedScene {
                    id: 2,
                    name: None,
                    index: 2,
                    silent: true,
                    start: 120_000,
                    end: 216_000,
                    cuts: vec![cut(1)],
                    notes: vec![],
                },
            ],
            events: vec![
                PlacedEvent::Se {
                    path: "C:/se/door.wav".into(),
                    start: 4_800,
                    gain_db: -3.0,
                },
                PlacedEvent::BgmStop { start: 0 },
            ],
            warnings: vec![],
            errors: vec![],
            end_cursor: 0,
        }
    }

    fn sample_audio() -> HashMap<LineId, LineAudioInfo> {
        let mut audio = HashMap::new();
        audio.insert(
            LineId { scene: 1, line: 1 },
            LineAudioInfo {
                src_start: 8_160,
                src_end: 61_441,
                measured_lufs: -31.234,
                requested_gain_db: 8.234,
                norm_gain_db: 8.234,
                norm_clamped: false,
            },
        );
        audio
    }

    fn sample_input<'a>(
        placement: &'a Placement,
        audio: &'a HashMap<LineId, LineAudioInfo>,
        visuals: &'a [SceneSpans],
    ) -> TimelineInput<'a> {
        let mut input = input_with(placement, audio, visuals);
        input.script = "第1話.txt".into();
        input.duration_samples = 216_001;
        input.casts = vec![
            CastInfo {
                name: "A".into(),
                speaker: "田中".into(),
                appearance: None,
            },
            CastInfo {
                name: "B".into(),
                speaker: "鈴木".into(),
                appearance: None,
            },
        ];
        input.takes[0].trims = Some((170, 220));
        input
    }

    #[test]
    fn timeline_v3_shape_and_rounding() {
        let placement = sample_placement();
        let audio = sample_audio();
        let (visuals, _) = layout_visuals(&placement.scenes, ceil_ms(216_001));
        let input = sample_input(&placement, &audio, &visuals);
        let v = timeline_value(&input).unwrap();
        assert_eq!(v["schema_version"], 3);
        assert_eq!(v["script_sha256"], "a".repeat(64));
        assert_eq!(v["audio_sha256"], "b".repeat(64));
        assert_eq!(v["sample_rate"], 48_000);
        assert_eq!(v["duration_ms"], 4_501);
        assert_eq!(v["normalize"]["target_lufs"], -23.0);
        assert_eq!(v["cast"][0]["speaker"], "田中");
        assert_eq!(v["cast"][1]["name"], "B");
        assert_eq!(v["scenes"][0]["id"], "S01");
        assert_eq!(v["scenes"][0]["cuts"][0]["id"], "S01-C01");
        assert_eq!(v["scenes"][0]["cuts"][0]["visual"]["text"], "港");
        assert_eq!(v["scenes"][0]["cuts"][0]["visual"]["kind"], "image");
        assert_eq!(v["scenes"][0]["cuts"][0]["visual"]["loop"], false);
        assert!(v["scenes"][0].get("visual").is_none());
        assert_eq!(v["scenes"][1]["silent"], true);
        assert!(v["scenes"][1]["cuts"][0]["visual"].is_null());
        assert_eq!(v["takes"][0]["tracks"][0], serde_json::Value::Null);
        assert_eq!(v["takes"][0]["trims"][0]["head_ms"], 170);
        assert!(v["takes"][1]["trims"].is_null());
        assert_eq!(v["takes"][1]["reason"], "no_file");
        let l0 = &v["lines"][0];
        assert_eq!(
            (l0["start_ms"].as_i64(), l0["end_ms"].as_i64()),
            (Some(0), Some(1_001))
        );
        assert_eq!(
            (l0["src_start_ms"].as_i64(), l0["src_end_ms"].as_i64()),
            (Some(170), Some(1_281))
        );
        assert_eq!(l0["id"], "S01-L001");
        assert_eq!(l0["scene"], "S01");
        assert_eq!(l0["take"], "S01_L001");
        assert!(l0["track"].is_null());
        assert_eq!(l0["measured_lufs"], -31.23);
        assert_eq!(l0["norm_clamped"], false);
        let l1 = &v["lines"][1];
        assert_eq!(l1["missing"], true);
        assert_eq!(l1["reason"], "no_file");
        assert!(
            l1["measured_lufs"].is_null()
                && l1["norm_gain_db"].is_null()
                && l1["src_start_ms"].is_null()
        );
        assert_eq!(v["events"][0]["type"], "bgm_stop");
        assert_eq!(v["events"][1]["type"], "se");
        assert_eq!(v["events"][1]["file"], "door.wav");
        assert_eq!(v["events"][1]["start_ms"], 100);
        // 共通の型で読み直せて、検証を通る
        s2v_timeline::load_str(&serde_json::to_string(&v).unwrap()).unwrap();
    }

    #[test]
    fn invalid_input_is_rejected_before_writing() {
        // 台詞の役が cast に無い → 内部の誤りとして Err、ファイルは作らない
        let placement = sample_placement();
        let audio = sample_audio();
        let (visuals, _) = layout_visuals(&placement.scenes, 4_501);
        let mut input = sample_input(&placement, &audio, &visuals);
        input.casts.clear();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeline.json");
        let e = write_timeline_json(&path, &input).unwrap_err().to_string();
        assert!(e.contains("内部の誤り"), "{e}");
        // 失敗の原因が役の欠落であること(別の理由で Err になっていない)
        assert!(e.contains("役"), "{e}");
        assert!(!path.exists());
    }

    #[test]
    fn scene_and_visual_count_mismatch_is_an_error() {
        let placement = sample_placement();
        let audio = sample_audio();
        let visuals = visuals_of(&placement);
        let input = input_with(&placement, &audio, &visuals[..1]);
        let e = build_timeline(&input).unwrap_err().to_string();
        assert!(e.contains("シーンとカットの区間の数"), "{e}");
    }

    /// 正常な台詞すべてに有効な LineAudioInfo を与える。
    fn audio_for(placement: &Placement) -> HashMap<LineId, LineAudioInfo> {
        placement
            .lines
            .iter()
            .filter(|l| l.missing.is_none())
            .map(|l| {
                (
                    l.id,
                    LineAudioInfo {
                        src_start: 0,
                        src_end: 48,
                        measured_lufs: -20.0,
                        requested_gain_db: 1.0,
                        norm_gain_db: 1.0,
                        norm_clamped: false,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn non_finite_audio_values_are_errors() {
        let placement = sample_placement();
        let visuals = visuals_of(&placement);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for field in 0..3 {
                let mut info = LineAudioInfo {
                    src_start: 0,
                    src_end: 48,
                    measured_lufs: -20.0,
                    requested_gain_db: 1.0,
                    norm_gain_db: 1.0,
                    norm_clamped: false,
                };
                match field {
                    0 => info.measured_lufs = bad,
                    1 => info.requested_gain_db = bad,
                    _ => info.norm_gain_db = bad,
                }
                let mut audio = HashMap::new();
                audio.insert(LineId { scene: 1, line: 1 }, info);
                let input = input_with(&placement, &audio, &visuals);
                assert!(timeline_value(&input).is_err());
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("timeline.json");
                assert!(write_timeline_json(&path, &input).is_err());
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn non_finite_target_or_se_gain_is_an_error() {
        let placement = sample_placement();
        let visuals = visuals_of(&placement);
        let audio = audio_for(&placement);
        let mut input = input_with(&placement, &audio, &visuals);
        input.target_lufs = f64::NAN;
        assert!(timeline_value(&input).is_err());

        let mut p2 = sample_placement();
        p2.events = vec![PlacedEvent::Se {
            path: "a.wav".into(),
            start: 0,
            gain_db: f64::INFINITY,
        }];
        let input = input_with(&p2, &audio, &visuals);
        assert!(timeline_value(&input).is_err());
        // 同じ入力でも SE が有限なら通る(上の Err が行の欠落ではなく SE 由来である確認)
        p2.events = vec![PlacedEvent::Se {
            path: "a.wav".into(),
            start: 0,
            gain_db: -3.0,
        }];
        assert!(timeline_value(&input_with(&p2, &audio, &visuals)).is_ok());
    }

    #[test]
    fn non_finite_values_of_missing_lines_are_ignored() {
        // 不足の台詞は測定値を出さないので、line_audio の中身は見ない
        let placement = sample_placement();
        let visuals = visuals_of(&placement);
        let mut audio = audio_for(&placement);
        audio.insert(
            LineId { scene: 1, line: 2 },
            LineAudioInfo {
                src_start: 0,
                src_end: 48,
                measured_lufs: f64::NAN,
                requested_gain_db: 0.0,
                norm_gain_db: 0.0,
                norm_clamped: false,
            },
        );
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        assert!(v["lines"][1]["measured_lufs"].is_null());
    }

    #[test]
    fn scene_zero_is_s00_and_empty_document_is_valid() {
        let mut placement = Placement::default();
        placement.lines.push(PlacedLine {
            id: LineId { scene: 0, line: 1 },
            scene: 0,
            cast: "A".into(),
            text: "t".into(),
            start: 0,
            end: 48,
            missing: None,
            flat_index: 0,
        });
        placement.scenes.push(PlacedScene {
            id: 0,
            name: None,
            index: 1,
            silent: false,
            start: 0,
            end: 48,
            cuts: vec![cut(1)],
            notes: vec![],
        });
        let audio = audio_for(&placement);
        let visuals = visuals_of(&placement);
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        assert_eq!(v["scenes"][0]["id"], "S00");
        assert_eq!(v["scenes"][0]["cuts"][0]["id"], "S00-C01");
        assert_eq!(v["lines"][0]["scene"], "S00");
        assert_eq!(v["lines"][0]["id"], "S00-L001");

        let empty = Placement::default();
        let audio = HashMap::new();
        let mut input = input_with(&empty, &audio, &[]);
        input.duration_samples = 0;
        input.casts.clear();
        let v = timeline_value(&input).unwrap();
        for k in ["cast", "scenes", "takes", "lines", "events"] {
            assert_eq!(v[k].as_array().unwrap().len(), 0, "{k}");
        }
        assert_eq!(v["duration_ms"], 0);
    }

    #[test]
    fn lines_keep_script_order_and_events_are_sorted() {
        let mut placement = Placement::default();
        // 台詞は S01 のシーンの中に置く(検証が台詞のシーンの存在を見る)
        placement.scenes.push(PlacedScene {
            id: 1,
            name: None,
            index: 1,
            silent: false,
            start: 0,
            end: 96_048,
            cuts: vec![cut(1)],
            notes: vec![],
        });
        for (n, start) in [(1u32, 96_000i64), (2, 0)] {
            placement.lines.push(PlacedLine {
                id: LineId { scene: 1, line: n },
                scene: 1,
                cast: "A".into(),
                text: "t".into(),
                start,
                end: start + 48,
                missing: None,
                flat_index: n as usize,
            });
        }
        placement.events = vec![
            PlacedEvent::BgmStop { start: 96_000 },
            PlacedEvent::BgmStart {
                path: "D:/m/bgm01.wav".into(),
                start: 0,
            },
        ];
        let audio = audio_for(&placement);
        let visuals = visuals_of(&placement);
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        assert_eq!(v["lines"][0]["id"], "S01-L001");
        assert_eq!(v["lines"][1]["id"], "S01-L002");
        assert_eq!(v["events"][0]["type"], "bgm_start");
        assert_eq!(v["events"][0]["file"], "bgm01.wav");
        assert_eq!(v["events"][1]["type"], "bgm_stop");
        assert_eq!(v["events"][1]["start_ms"], 2000);
    }

    #[test]
    fn write_timeline_json_writes_valid_json() {
        let placement = sample_placement();
        let audio = audio_for(&placement);
        let visuals = visuals_of(&placement);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeline.json");
        write_timeline_json(&path, &input_with(&placement, &audio, &visuals)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema_version"], 3);
        assert_eq!(v["lines"][0]["measured_lufs"], -20.0);
        s2v_timeline::load_str(&text).unwrap();
    }

    #[test]
    fn normal_line_without_audio_info_is_an_error() {
        let placement = sample_placement();
        let visuals = visuals_of(&placement);
        let audio = HashMap::new();
        let err = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap_err();
        assert!(err.to_string().contains("S01-L001"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeline.json");
        assert!(write_timeline_json(&path, &input_with(&placement, &audio, &visuals)).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn normal_lines_have_all_audio_fields_non_null() {
        let placement = sample_placement();
        let visuals = visuals_of(&placement);
        let audio = audio_for(&placement);
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        let l = &v["lines"][0];
        for k in [
            "measured_lufs",
            "requested_gain_db",
            "norm_gain_db",
            "norm_clamped",
            "src_start_ms",
            "src_end_ms",
        ] {
            assert!(!l[k].is_null(), "{k}");
        }
        // 不足の台詞は line_audio があっても全部 null
        let mut audio = audio;
        audio.insert(
            LineId { scene: 1, line: 2 },
            audio[&LineId { scene: 1, line: 1 }],
        );
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        for k in [
            "measured_lufs",
            "requested_gain_db",
            "norm_gain_db",
            "norm_clamped",
            "src_start_ms",
            "src_end_ms",
        ] {
            assert!(v["lines"][1][k].is_null(), "{k}");
        }
    }

    #[test]
    fn rounding_has_no_negative_zero_and_rounds_se_gain() {
        let mut placement = sample_placement();
        placement.events = vec![
            PlacedEvent::Se {
                path: "a.wav".into(),
                start: 0,
                gain_db: -3.234,
            },
            PlacedEvent::Se {
                path: "b.wav".into(),
                start: 1,
                gain_db: -0.004,
            },
        ];
        let visuals = visuals_of(&placement);
        let mut audio = HashMap::new();
        audio.insert(
            LineId { scene: 1, line: 1 },
            LineAudioInfo {
                src_start: 0,
                src_end: 48,
                measured_lufs: -31.234,
                requested_gain_db: -0.004,
                norm_gain_db: 8.236,
                norm_clamped: false,
            },
        );
        let v = timeline_value(&input_with(&placement, &audio, &visuals)).unwrap();
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains("-0.0"), "{text}");
        assert!(!text.contains("-0,"), "{text}");
        assert_eq!(v["lines"][0]["measured_lufs"], -31.23);
        assert_eq!(v["lines"][0]["norm_gain_db"], 8.24);
        assert!(text.contains(r#""requested_gain_db":0.0"#), "{text}");
        assert_eq!(v["events"][0]["gain_db"], -3.23);
        assert!(text.contains(r#""gain_db":0.0"#), "{text}");
    }
}
