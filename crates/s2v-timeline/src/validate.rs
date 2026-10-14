//! 設計書 §3.6 の不変条件。

use std::collections::{HashMap, HashSet};

use crate::{Reason, Timeline};

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_scene_id(s: &str) -> bool {
    s.strip_prefix('S').is_some_and(digits)
}

/// `S01-C02` の形なら、シーンの部分(`S01`)を返す。
fn sub_id_scene(s: &str, letter: char) -> Option<&str> {
    let (scene, rest) = s.split_once('-')?;
    (is_scene_id(scene) && rest.strip_prefix(letter).is_some_and(digits)).then_some(scene)
}

fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

pub(crate) fn check(t: &Timeline) -> Vec<String> {
    let mut e: Vec<String> = Vec::new();
    let dur = t.duration_ms;
    if dur < 0 {
        e.push(format!("duration_ms が負です: {dur}"));
    }
    let in_range = |a: i64, b: i64| 0 <= a && a <= b && b <= dur;

    // シーンとカット・ト書き
    let mut scene_pos: HashMap<&str, usize> = HashMap::new();
    let mut cut_ids = HashSet::new();
    let mut note_ids = HashSet::new();
    let n = t.scenes.len();
    for (i, s) in t.scenes.iter().enumerate() {
        if !is_scene_id(&s.id) {
            e.push(format!("シーン ID の書式が違います: {}", s.id));
        }
        if scene_pos.insert(s.id.as_str(), i).is_some() {
            e.push(format!("シーン ID が重複しています: {}", s.id));
        }
        if s.index != i + 1 {
            e.push(format!(
                "{}: index は {} のはずです({})",
                s.id,
                i + 1,
                s.index
            ));
        }
        if !in_range(s.start_ms, s.end_ms) {
            e.push(format!("{}: 時刻が範囲外です", s.id));
        }
        if s.cuts.is_empty() {
            e.push(format!("{}: カットがありません", s.id));
            continue;
        }
        let first_id = format!("{}-C01", s.id);
        let first_ok = sub_id_scene(&s.cuts[0].id, 'C') == Some(s.id.as_str())
            && s.cuts[0].id[s.id.len() + 2..].parse::<u32>() == Ok(1);
        if !first_ok {
            e.push(format!(
                "{}: 最初のカットは {first_id} のはずです({})",
                s.id, s.cuts[0].id
            ));
        }
        for c in &s.cuts {
            if sub_id_scene(&c.id, 'C') != Some(s.id.as_str()) {
                e.push(format!("{}: カット ID が違います: {}", s.id, c.id));
            }
            if !cut_ids.insert(c.id.as_str()) {
                e.push(format!("カット ID が重複しています: {}", c.id));
            }
            if !in_range(c.start_ms, c.end_ms) {
                e.push(format!("{}: 時刻が範囲外です", c.id));
            }
        }
        for w in s.cuts.windows(2) {
            if w[0].end_ms != w[1].start_ms {
                e.push(format!(
                    "{} と {} の間に隙間か重なりがあります",
                    w[0].id, w[1].id
                ));
            }
        }
        let (vs, ve) = (s.cuts[0].start_ms, s.cuts[s.cuts.len() - 1].end_ms);
        let expect_start = if i == 0 { 0 } else { s.start_ms };
        if vs != expect_start {
            e.push(format!(
                "{}: 映像区間の始め {vs} が {expect_start} と違います",
                s.id
            ));
        }
        let expect_end = if i + 1 == n {
            Some(dur)
        } else {
            Some(t.scenes[i + 1].start_ms)
        };
        if Some(ve) != expect_end {
            e.push(format!(
                "{}: 映像区間の終わり {ve} が {:?} と違います",
                s.id, expect_end
            ));
        }
        for (k, note) in s.notes.iter().enumerate() {
            if sub_id_scene(&note.id, 'N') != Some(s.id.as_str()) {
                e.push(format!("{}: ト書き ID が違います: {}", s.id, note.id));
            }
            if !note_ids.insert(note.id.as_str()) {
                e.push(format!("ト書き ID が重複しています: {}", note.id));
            }
            if !(vs <= note.start_ms && note.start_ms <= note.end_ms && note.end_ms <= ve) {
                e.push(format!("{}: シーンの映像区間の外です", note.id));
            }
            if let Some(next) = s.notes.get(k + 1) {
                if next.start_ms < note.start_ms || note.end_ms > next.start_ms {
                    e.push(format!("{} と {} の時刻の順が違います", note.id, next.id));
                }
            }
        }
    }

    // 役・テイク
    let mut cast_names = HashSet::new();
    for c in &t.cast {
        if !cast_names.insert(c.name.as_str()) {
            e.push(format!("役名が重複しています: {}", c.name));
        }
    }
    let mut takes: HashMap<&str, &crate::TakeEntry> = HashMap::new();
    for tk in &t.takes {
        if takes.insert(tk.id.as_str(), tk).is_some() {
            e.push(format!("テイク ID が重複しています: {}", tk.id));
        }
        let distinct: HashSet<_> = tk.tracks.iter().collect();
        if tk.tracks.is_empty() || distinct.len() != tk.tracks.len() {
            e.push(format!("{}: tracks が空か重複しています", tk.id));
        }
        if !tk.missing {
            if tk.reason.is_some() {
                e.push(format!("{}: 正常なテイクに reason があります", tk.id));
            }
            let trims = tk.trims.as_deref().unwrap_or(&[]);
            if tk.trims.is_none()
                || tk.files.len() != tk.tracks.len()
                || trims.len() != tk.tracks.len()
            {
                e.push(format!(
                    "{}: files・tracks・trims の件数が合いません",
                    tk.id
                ));
            } else {
                for (i, track) in tk.tracks.iter().enumerate() {
                    if &trims[i].track != track {
                        e.push(format!(
                            "{}: trims[{i}].track が tracks[{i}] と違います",
                            tk.id
                        ));
                    }
                    let want = match track {
                        None => tk.id.clone(),
                        Some(r) => format!("{}.{r}", tk.id),
                    };
                    if file_stem(&tk.files[i]) != want {
                        e.push(format!(
                            "{}: files[{i}] が {want} のファイルではありません",
                            tk.id
                        ));
                    }
                }
            }
        } else {
            if tk.trims.is_some() {
                e.push(format!("{}: 不足のテイクに trims があります", tk.id));
            }
            match tk.reason {
                None => e.push(format!("{}: 不足のテイクに reason がありません", tk.id)),
                Some(Reason::NoFile) if !tk.files.is_empty() => {
                    e.push(format!("{}: no_file なのに files があります", tk.id))
                }
                Some(Reason::Duplicate) if tk.files.len() < 2 => {
                    e.push(format!("{}: duplicate なのに files が 2 件未満です", tk.id))
                }
                Some(Reason::NoFile) | Some(Reason::Duplicate) => {}
                Some(_) if tk.files.is_empty() => e.push(format!("{}: files がありません", tk.id)),
                Some(_) => {}
            }
        }
    }

    // 台詞
    let mut line_ids = HashSet::new();
    let mut last_pos = 0usize;
    for l in &t.lines {
        if !line_ids.insert(l.id.as_str()) {
            e.push(format!("台詞 ID が重複しています: {}", l.id));
        }
        match sub_id_scene(&l.id, 'L') {
            Some(sc) if sc == l.scene => {}
            _ => e.push(format!("{}: 台詞 ID の書式かシーンが違います", l.id)),
        }
        match scene_pos.get(l.scene.as_str()) {
            None => e.push(format!("{}: シーン {} がありません", l.id, l.scene)),
            Some(&p) => {
                if p < last_pos {
                    e.push(format!("{}: 台詞の並びとシーンの並びが矛盾します", l.id));
                }
                last_pos = last_pos.max(p);
                if t.scenes[p].silent {
                    e.push(format!(
                        "{}: 無言のシーン {} に台詞があります",
                        l.id, l.scene
                    ));
                }
            }
        }
        if !cast_names.contains(l.cast.as_str()) {
            e.push(format!("{}: 役 {} がありません", l.id, l.cast));
        }
        match takes.get(l.take.as_str()) {
            None => e.push(format!("{}: テイク {} がありません", l.id, l.take)),
            Some(tk) => {
                let ok = match &l.track {
                    None => tk.tracks.iter().all(|x| x.is_none()),
                    Some(r) => tk.tracks.iter().any(|x| x.as_deref() == Some(r)),
                };
                if !ok {
                    e.push(format!("{}: track がテイク {} に属しません", l.id, l.take));
                }
            }
        }
        if !in_range(l.start_ms, l.end_ms) {
            e.push(format!("{}: 時刻が範囲外です", l.id));
        }
        let finite = |v: Option<f64>| v.is_some_and(|x| x.is_finite());
        if !l.missing {
            if l.reason.is_some()
                || l.norm_clamped.is_none()
                || !finite(l.measured_lufs)
                || !finite(l.requested_gain_db)
                || !finite(l.norm_gain_db)
                || l.src_start_ms
                    .zip(l.src_end_ms)
                    .is_none_or(|(a, b)| a > b || a < 0)
            {
                e.push(format!("{}: 正常な台詞の値が欠けているか不正です", l.id));
            }
        } else if l.reason.is_none()
            || l.norm_clamped.is_some()
            || l.measured_lufs.is_some()
            || l.requested_gain_db.is_some()
            || l.norm_gain_db.is_some()
            || l.src_start_ms.is_some()
            || l.src_end_ms.is_some()
        {
            e.push(format!("{}: 不足の台詞の null の規則に合いません", l.id));
        }
    }

    // イベント・数値
    for ev in &t.events {
        if !(0..=dur).contains(&ev.start_ms()) {
            e.push(format!("イベントの時刻が範囲外です: {}", ev.start_ms()));
        }
        if let crate::EventEntry::Se { gain_db, .. } = ev {
            if !gain_db.is_finite() {
                e.push("SE の gain_db が有限ではありません".to_string());
            }
        }
    }
    if !t.normalize.target_lufs.is_finite() {
        e.push("normalize.target_lufs が有限ではありません".to_string());
    }
    if t.schema_version != crate::SCHEMA_VERSION {
        e.push(format!("schema_version が {} です", t.schema_version));
    }
    e
}

#[cfg(test)]
mod tests {
    use crate::*;

    /// 2シーン(S01 は C01・C02 とト書き1つ、S02 は無言)の正しい timeline。
    pub(crate) fn sample() -> Timeline {
        Timeline {
            schema_version: 3,
            script: "第1話.txt".into(),
            script_sha256: "a".repeat(64),
            audio_sha256: "b".repeat(64),
            sample_rate: 48_000,
            duration_ms: 9_000,
            normalize: Normalize { target_lufs: -23.0 },
            cast: vec![CastEntry {
                name: "A".into(),
                speaker: "田中".into(),
                appearance: None,
            }],
            scenes: vec![
                SceneEntry {
                    id: "S01".into(),
                    name: Some("朝".into()),
                    index: 1,
                    silent: false,
                    start_ms: 0,
                    end_ms: 3_000,
                    cuts: vec![
                        CutEntry {
                            id: "S01-C01".into(),
                            start_ms: 0,
                            end_ms: 2_000,
                            visual: Some(Visual {
                                text: "港".into(),
                                kind: Some(VisualKind::Image),
                                looped: false,
                            }),
                        },
                        CutEntry {
                            id: "S01-C02".into(),
                            start_ms: 2_000,
                            end_ms: 5_000,
                            visual: None,
                        },
                    ],
                    notes: vec![NoteEntry {
                        id: "S01-N01".into(),
                        text: "翌朝".into(),
                        start_ms: 0,
                        end_ms: 2_000,
                    }],
                },
                SceneEntry {
                    id: "S02".into(),
                    name: None,
                    index: 2,
                    silent: true,
                    start_ms: 5_000,
                    end_ms: 7_000,
                    cuts: vec![CutEntry {
                        id: "S02-C01".into(),
                        start_ms: 5_000,
                        end_ms: 9_000,
                        visual: None,
                    }],
                    notes: vec![],
                },
            ],
            takes: vec![
                TakeEntry {
                    id: "S01_L001".into(),
                    tracks: vec![None],
                    files: vec!["S01_L001.wav".into()],
                    trims: Some(vec![Trim {
                        track: None,
                        head_ms: 10,
                        tail_ms: 20,
                    }]),
                    missing: false,
                    reason: None,
                },
                TakeEntry {
                    id: "S01_L002".into(),
                    tracks: vec![None],
                    files: vec![],
                    trims: None,
                    missing: true,
                    reason: Some(Reason::NoFile),
                },
            ],
            lines: vec![
                LineEntry {
                    id: "S01-L001".into(),
                    scene: "S01".into(),
                    cast: "A".into(),
                    text: "一".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                    take: "S01_L001".into(),
                    track: None,
                    measured_lufs: Some(-30.0),
                    requested_gain_db: Some(7.0),
                    norm_gain_db: Some(7.0),
                    norm_clamped: Some(false),
                    src_start_ms: Some(10),
                    src_end_ms: Some(1_010),
                    missing: false,
                    reason: None,
                },
                LineEntry {
                    id: "S01-L002".into(),
                    scene: "S01".into(),
                    cast: "A".into(),
                    text: "二".into(),
                    start_ms: 2_000,
                    end_ms: 3_000,
                    take: "S01_L002".into(),
                    track: None,
                    measured_lufs: None,
                    requested_gain_db: None,
                    norm_gain_db: None,
                    norm_clamped: None,
                    src_start_ms: None,
                    src_end_ms: None,
                    missing: true,
                    reason: Some(Reason::NoFile),
                },
            ],
            events: vec![EventEntry::BgmStop { start_ms: 0 }],
        }
    }

    fn errs(t: &Timeline) -> Vec<String> {
        t.validate().err().unwrap_or_default()
    }

    #[test]
    fn sample_is_valid_and_round_trips() {
        let t = sample();
        assert_eq!(errs(&t), Vec::<String>::new());
        let back = load_str(&t.to_json_pretty()).unwrap();
        assert_eq!(back, t);
        // JSON のキーは loop
        assert!(t.to_json_pretty().contains("\"loop\": false"));
    }

    #[test]
    fn version_other_than_3_is_rejected_before_parsing() {
        assert_eq!(
            load_str("{\"schema_version\": 2}"),
            Err(LoadError::Version(Some(2)))
        );
        assert_eq!(load_str("{}"), Err(LoadError::Version(None)));
        assert!(matches!(load_str("not json"), Err(LoadError::Json(_))));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json_pretty()).unwrap();
        v["future_field"] = serde_json::json!(1);
        v["scenes"][0]["cuts"][0]["future"] = serde_json::json!("x");
        assert!(load_str(&v.to_string()).is_ok());
    }

    /// 1つ壊して、検証が失敗することを確かめる。
    fn broken(f: impl FnOnce(&mut Timeline)) -> Vec<String> {
        let mut t = sample();
        f(&mut t);
        errs(&t)
    }

    #[test]
    fn id_formats_and_scene_parts() {
        assert!(!broken(|t| t.scenes[0].id = "X01".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].cuts[1].id = "S01-X02".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].cuts[1].id = "S02-C02".into()).is_empty());
        assert!(!broken(|t| t.scenes[0].notes[0].id = "S01-N".into()).is_empty());
        assert!(!broken(|t| t.lines[0].id = "S02-L001".into()).is_empty());
        // 桁数は問わない
        assert!(broken(|t| t.scenes[0].cuts[1].id = "S01-C0002".into()).is_empty());
    }

    #[test]
    fn duplicates_are_rejected() {
        assert!(!broken(|t| t.scenes[1].id = "S01".into()).is_empty());
        assert!(!broken(|t| t.lines[1].id = "S01-L001".into()).is_empty());
        assert!(!broken(|t| t.takes[1].id = "S01_L001".into()).is_empty());
        assert!(!broken(|t| t.cast.push(t.cast[0].clone())).is_empty());
    }

    #[test]
    fn cuts_must_tile_the_video() {
        // 最初のカットは C01
        assert!(!broken(|t| t.scenes[0].cuts[0].id = "S01-C03".into()).is_empty());
        // カットが無いシーン
        assert!(!broken(|t| t.scenes[1].cuts.clear()).is_empty());
        // 隙間
        assert!(!broken(|t| t.scenes[0].cuts[1].start_ms = 2_100).is_empty());
        // 最初のシーンが 0 から始まらない
        assert!(!broken(|t| t.scenes[0].cuts[0].start_ms = 1).is_empty());
        // 2番目のシーンの先頭カットが start_ms と違う
        assert!(!broken(|t| {
            t.scenes[1].cuts[0].start_ms = 4_000;
            t.scenes[0].cuts[1].end_ms = 4_000;
        })
        .is_empty());
        // 最後が duration_ms で終わらない
        assert!(!broken(|t| t.scenes[1].cuts[0].end_ms = 8_000).is_empty());
        // 長さ 0 は許す
        assert!(broken(|t| {
            t.scenes[0].cuts[0].end_ms = 0;
            t.scenes[0].cuts[1].start_ms = 0;
            t.scenes[0].notes[0].end_ms = 0;
        })
        .is_empty());
    }

    #[test]
    fn notes_must_be_ordered_and_inside_the_scene_video() {
        assert!(!broken(|t| t.scenes[0].notes[0].end_ms = 6_000).is_empty());
        assert!(!broken(|t| t.scenes[0].notes.push(NoteEntry {
            id: "S01-N02".into(),
            text: "x".into(),
            start_ms: 1_000,
            end_ms: 1_500
        }))
        .is_empty()); // N01 の終わり 2000 > N02 の始め 1000
    }

    #[test]
    fn times_must_be_in_range() {
        assert!(!broken(|t| t.lines[0].end_ms = 9_001).is_empty());
        assert!(!broken(|t| t.lines[0].start_ms = 1_001).is_empty());
        assert!(!broken(|t| t.events.push(EventEntry::BgmStop { start_ms: 9_001 })).is_empty());
        assert!(!broken(|t| t.duration_ms = -1).is_empty());
    }

    #[test]
    fn references_must_resolve() {
        assert!(!broken(|t| t.lines[0].scene = "S09".into()).is_empty());
        assert!(!broken(|t| t.lines[0].cast = "Z".into()).is_empty());
        assert!(!broken(|t| t.lines[0].take = "S09_L001".into()).is_empty());
        assert!(!broken(|t| t.lines[0].track = Some("A".into())).is_empty()); // 単独のテイクは null
                                                                              // 所属シーンの位置が減る並び
        assert!(!broken(|t| {
            t.lines.swap(0, 1);
            t.lines[0].scene = "S02".into();
            t.lines[0].id = "S02-L001".into();
        })
        .is_empty());
    }

    #[test]
    fn line_and_take_null_rules() {
        assert!(!broken(|t| t.lines[0].reason = Some(Reason::Silent)).is_empty());
        assert!(!broken(|t| t.lines[0].norm_clamped = None).is_empty());
        assert!(!broken(|t| t.lines[0].measured_lufs = Some(f64::NAN)).is_empty());
        assert!(!broken(|t| t.lines[1].norm_clamped = Some(false)).is_empty());
        assert!(!broken(|t| t.lines[1].reason = None).is_empty());
        assert!(!broken(|t| t.takes[0].files.clear()).is_empty());
        assert!(!broken(|t| t.takes[0].files = vec!["S01_L001.A.wav".into()]).is_empty()); // 単独は id と同じ名前
        assert!(!broken(|t| t.takes[0].trims = None).is_empty());
        assert!(!broken(|t| t.takes[1].files = vec!["S01_L002.wav".into()]).is_empty()); // no_file は 0 件
        assert!(!broken(|t| t.takes[1].reason = Some(Reason::Duplicate)).is_empty()); // duplicate は 2 件以上
        assert!(!broken(|t| t.takes[0].tracks = vec![]).is_empty());
    }

    #[test]
    fn role_take_files_must_match_tracks() {
        let ok = broken(|t| {
            t.takes[0] = TakeEntry {
                id: "S01_L001".into(),
                tracks: vec![Some("A".into()), Some("B".into())],
                files: vec!["S01_L001.A.wav".into(), "S01_L001.B.flac".into()],
                trims: Some(vec![
                    Trim {
                        track: Some("A".into()),
                        head_ms: 0,
                        tail_ms: 0,
                    },
                    Trim {
                        track: Some("B".into()),
                        head_ms: 0,
                        tail_ms: 0,
                    },
                ]),
                missing: false,
                reason: None,
            };
            t.lines[0].track = Some("A".into());
        });
        assert!(ok.is_empty(), "{ok:?}");
        assert!(!broken(|t| {
            t.takes[0] = TakeEntry {
                id: "S01_L001".into(),
                tracks: vec![Some("A".into()), Some("B".into())],
                files: vec!["S01_L001.B.wav".into(), "S01_L001.A.wav".into()], // 逆順
                trims: Some(vec![
                    Trim {
                        track: Some("A".into()),
                        head_ms: 0,
                        tail_ms: 0,
                    },
                    Trim {
                        track: Some("B".into()),
                        head_ms: 0,
                        tail_ms: 0,
                    },
                ]),
                missing: false,
                reason: None,
            };
            t.lines[0].track = Some("A".into());
        })
        .is_empty());
    }

    #[test]
    fn scene_index_and_silent() {
        assert!(!broken(|t| t.scenes[1].index = 3).is_empty());
        assert!(!broken(|t| {
            t.scenes[1].silent = true;
            t.lines[1].scene = "S02".into();
            t.lines[1].id = "S02-L002".into();
        })
        .is_empty());
    }
}
