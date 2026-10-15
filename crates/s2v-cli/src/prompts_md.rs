//! prompts の Markdown の組み立て(映像工程設計書 §7)。ファイルには触れない。

use s2v_core::config::PromptsConfig;
use s2v_render::assets::{asset_stem, AssetMatch};
use s2v_timeline::{CastEntry, CutEntry, SceneEntry, Timeline, VisualKind};

/// ミリ秒を秒(小数1桁、四捨五入)の文字列にする。
pub fn seconds_1(ms: i64) -> String {
    let t = (ms.max(0) + 50) / 100;
    format!("{}.{}", t / 10, t % 10)
}

fn is_zero(cut: &CutEntry) -> bool {
    cut.end_ms <= cut.start_ms
}

/// 作る素材のファイル名(Markdown のインラインコード)。
fn asset_names(cut: &CutEntry) -> String {
    let stem = asset_stem(&cut.id);
    match cut.visual.as_ref().and_then(|v| v.kind) {
        Some(VisualKind::Image) => format!("`tracks/{stem}.png`"),
        Some(VisualKind::Video) => format!("`tracks/{stem}.mp4`"),
        None => format!("`tracks/{stem}.png` か `tracks/{stem}.mp4`"),
    }
}

fn kind_label(cut: &CutEntry) -> &'static str {
    match cut.visual.as_ref() {
        Some(v) => match (v.kind, v.looped) {
            (Some(VisualKind::Image), _) => "画像",
            (Some(VisualKind::Video), true) => "動画(ループする)",
            (Some(VisualKind::Video), false) => "動画(ループしない)",
            (None, true) => "未指定(画像か動画。動画ならループする)",
            (None, false) => "未指定(画像か動画)",
        },
        None => "未指定(画像か動画)",
    }
}

/// 長さを出すのは、動画と、種類が未指定(動画になりうる)のカット。
fn shows_length(cut: &CutEntry) -> bool {
    cut.visual.as_ref().and_then(|v| v.kind) != Some(VisualKind::Image)
}

/// カットの区間と表示区間が交わる台詞の話者(台本の並びの初出順、重複なし)。
pub fn cast_for_cut<'a>(tl: &'a Timeline, cut: &CutEntry) -> Vec<&'a CastEntry> {
    let mut out: Vec<&CastEntry> = Vec::new();
    for l in &tl.lines {
        if !(l.start_ms < cut.end_ms && cut.start_ms < l.end_ms) {
            continue;
        }
        if out.iter().any(|c| c.name == l.cast) {
            continue;
        }
        if let Some(c) = tl.cast.iter().find(|c| c.name == l.cast) {
            out.push(c);
        }
    }
    out
}

fn prompt_body(tl: &Timeline, scene: &SceneEntry, cut: &CutEntry, cfg: &PromptsConfig) -> String {
    let mut parts: Vec<String> = Vec::new();
    let style = cfg.style.trim();
    if !style.is_empty() {
        parts.push(style.to_string());
    }
    if let Some(name) = scene
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        parts.push(name.to_string());
    }
    if let Some(v) = &cut.visual {
        parts.push(v.text.clone());
    }
    let cast = cast_for_cut(tl, cut);
    if !cast.is_empty() {
        let mut block = String::from("登場人物:");
        for c in cast {
            block.push_str("\n- ");
            block.push_str(&c.name);
            if let Some(a) = c.appearance.as_deref() {
                let a = a
                    .split('\n')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !a.is_empty() {
                    block.push_str(": ");
                    block.push_str(&a);
                }
            }
        }
        parts.push(block);
    }
    let suffix = cfg.suffix.trim();
    if !suffix.is_empty() {
        parts.push(suffix.to_string());
    }
    parts.join("\n")
}

/// 本文の中の最も長い連続したバッククォートより1つ長い囲み(最低3つ)。
fn fence_for(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in body.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// シーンのファイル `prompts/<シーン ID>.md` の中身。長さ 1 以上のカットが無ければ None。
pub fn scene_markdown(tl: &Timeline, scene: &SceneEntry, cfg: &PromptsConfig) -> Option<String> {
    let cuts: Vec<&CutEntry> = scene.cuts.iter().filter(|c| !is_zero(c)).collect();
    if cuts.is_empty() {
        return None;
    }
    let mut md = format!("# {}", scene.id);
    if let Some(name) = scene
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        md.push(' ');
        md.push_str(name);
    }
    md.push('\n');
    for cut in cuts {
        md.push_str(&format!("\n## {}\n\n", cut.id));
        md.push_str(&format!("- 素材: {}\n", asset_names(cut)));
        md.push_str(&format!("- 種類: {}\n", kind_label(cut)));
        if shows_length(cut) {
            md.push_str(&format!(
                "- 長さ: {} 秒\n",
                seconds_1(cut.end_ms - cut.start_ms)
            ));
        }
        if cut.visual.is_none() {
            md.push_str("- #visual: なし(台本に #visual を書いてください)\n");
        }
        let body = prompt_body(tl, scene, cut, cfg);
        let fence = fence_for(&body);
        md.push_str(&format!("\n{fence}text\n"));
        if !body.is_empty() {
            md.push_str(&body);
            md.push('\n');
        }
        md.push_str(&fence);
        md.push('\n');
    }
    Some(md)
}

/// `prompts/index.md` の中身(全カットの表と、tracks/ の確認事項)。
pub fn index_markdown(tl: &Timeline, assets: &AssetMatch) -> String {
    let mut md = String::from(
        "# 素材の一覧\n\n| カット | ファイル | 種類 | 長さ | 素材 |\n|---|---|---|---|---|\n",
    );
    for scene in &tl.scenes {
        for cut in &scene.cuts {
            let have = if is_zero(cut) {
                "不要(長さ 0)".to_string()
            } else if let Some(a) = assets.found.get(&cut.id) {
                let name = a
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("あり(`{name}`)")
            } else {
                "なし".to_string()
            };
            md.push_str(&format!(
                "| {} | {} | {} | {} 秒 | {} |\n",
                cut.id,
                asset_names(cut),
                kind_label(cut),
                seconds_1(cut.end_ms - cut.start_ms),
                have
            ));
        }
    }
    if !assets.errors.is_empty() {
        md.push_str("\n## tracks/ の確認事項\n\n");
        for e in &assets.errors {
            md.push_str(&format!("- {e}\n"));
        }
    }
    md
}

/// 長さ 1 以上で `#visual` の無いカット ID(台本の並び)。
pub fn cuts_without_visual(tl: &Timeline) -> Vec<String> {
    tl.scenes
        .iter()
        .flat_map(|s| &s.cuts)
        .filter(|c| !is_zero(c) && c.visual.is_none())
        .map(|c| c.id.clone())
        .collect()
}

/// `match_assets` に渡す、長さ 1 以上のカットの (ID, 明示された種類)。
pub fn asset_query(tl: &Timeline) -> Vec<(String, Option<VisualKind>)> {
    tl.scenes
        .iter()
        .flat_map(|s| &s.cuts)
        .filter(|c| !is_zero(c))
        .map(|c| (c.id.clone(), c.visual.as_ref().and_then(|v| v.kind)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use s2v_render::assets::{Asset, AssetKind};
    use s2v_timeline::{LineEntry, Normalize, NoteEntry, Visual};

    fn cut(id: &str, s: i64, e: i64, visual: Option<(&str, Option<VisualKind>, bool)>) -> CutEntry {
        CutEntry {
            id: id.into(),
            start_ms: s,
            end_ms: e,
            visual: visual.map(|(t, k, l)| Visual {
                text: t.into(),
                kind: k,
                looped: l,
            }),
        }
    }

    fn line(id: &str, scene: &str, cast: &str, s: i64, e: i64) -> LineEntry {
        LineEntry {
            id: id.into(),
            scene: scene.into(),
            cast: cast.into(),
            text: "台詞".into(),
            start_ms: s,
            end_ms: e,
            take: id.into(),
            track: None,
            measured_lufs: None,
            requested_gain_db: None,
            norm_gain_db: None,
            norm_clamped: None,
            src_start_ms: None,
            src_end_ms: None,
            missing: false,
            reason: None,
        }
    }

    /// S01(朝の居間): C01 画像 0–3000、C02 動画 loop 3000–7250、C03 長さ 0(#visual なし)。
    /// S02(名前なし): C01 7250–9000(#visual なし)。
    /// 台詞: 田中 500–2000、佐藤 2500–3500(C01 と C02 をまたぐ)、田中 4000–5000。
    fn tl() -> Timeline {
        Timeline {
            schema_version: 3,
            script: "第1話.txt".into(),
            script_sha256: "0".repeat(64),
            audio_sha256: "0".repeat(64),
            sample_rate: 48_000,
            duration_ms: 9_000,
            normalize: Normalize { target_lufs: -16.0 },
            cast: vec![
                CastEntry {
                    name: "田中".into(),
                    speaker: "田中".into(),
                    appearance: Some("30代の男性。\n紺のカーディガン。".into()),
                },
                CastEntry {
                    name: "佐藤".into(),
                    speaker: "佐藤".into(),
                    appearance: None,
                },
            ],
            scenes: vec![
                SceneEntry {
                    id: "S01".into(),
                    name: Some("朝の居間".into()),
                    index: 1,
                    silent: false,
                    start_ms: 0,
                    end_ms: 7_250,
                    cuts: vec![
                        cut(
                            "S01-C01",
                            0,
                            3_000,
                            Some(("朝の光。", Some(VisualKind::Image), false)),
                        ),
                        cut(
                            "S01-C02",
                            3_000,
                            7_250,
                            Some(("波。", Some(VisualKind::Video), true)),
                        ),
                        cut("S01-C03", 7_250, 7_250, None),
                    ],
                    notes: vec![NoteEntry {
                        id: "S01-N01".into(),
                        text: "翌朝。".into(),
                        start_ms: 0,
                        end_ms: 3_000,
                    }],
                },
                SceneEntry {
                    id: "S02".into(),
                    name: None,
                    index: 2,
                    silent: true,
                    start_ms: 7_250,
                    end_ms: 9_000,
                    cuts: vec![cut("S02-C01", 7_250, 9_000, None)],
                    notes: vec![],
                },
            ],
            takes: vec![],
            lines: vec![
                line("S01-L001", "S01", "田中", 500, 2_000),
                line("S01-L002", "S01", "佐藤", 2_500, 3_500),
                line("S01-L003", "S01", "田中", 4_000, 5_000),
            ],
            events: vec![],
        }
    }

    fn cfg() -> PromptsConfig {
        PromptsConfig {
            style: "アニメ調。".into(),
            suffix: "縦横比 16:9。画像の中に文字・吹き出しを入れない。".into(),
        }
    }

    #[test]
    fn seconds_are_rounded_to_one_decimal() {
        assert_eq!(seconds_1(0), "0.0");
        assert_eq!(seconds_1(4_250), "4.3");
        assert_eq!(seconds_1(4_249), "4.2");
        assert_eq!(seconds_1(1_750), "1.8");
        assert_eq!(seconds_1(12_000), "12.0");
    }

    #[test]
    fn cast_is_listed_in_first_appearance_order_without_duplicates() {
        let t = tl();
        let names = |c: &CutEntry| {
            cast_for_cut(&t, c)
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        // 佐藤の台詞(2500–3500)は C01 と C02 の両方に交わる
        assert_eq!(names(&t.scenes[0].cuts[0]), ["田中", "佐藤"]);
        assert_eq!(names(&t.scenes[0].cuts[1]), ["佐藤", "田中"]);
        // 端が接するだけ(line.end_ms == cut.start_ms)は交わらない
        let edge = cut("X", 2_000, 2_500, None);
        assert!(cast_for_cut(&t, &edge).is_empty());
    }

    #[test]
    fn golden_scene_markdown() {
        let t = tl();
        let s01 = scene_markdown(&t, &t.scenes[0], &cfg()).unwrap();
        assert_eq!(
            s01,
            "# S01 朝の居間\n\
\n\
## S01-C01\n\
\n\
- 素材: `tracks/S01.png`\n\
- 種類: 画像\n\
\n\
```text\n\
アニメ調。\n\
朝の居間\n\
朝の光。\n\
登場人物:\n\
- 田中: 30代の男性。 紺のカーディガン。\n\
- 佐藤\n\
縦横比 16:9。画像の中に文字・吹き出しを入れない。\n\
```\n\
\n\
## S01-C02\n\
\n\
- 素材: `tracks/S01_C02.mp4`\n\
- 種類: 動画(ループする)\n\
- 長さ: 4.3 秒\n\
\n\
```text\n\
アニメ調。\n\
朝の居間\n\
波。\n\
登場人物:\n\
- 佐藤\n\
- 田中: 30代の男性。 紺のカーディガン。\n\
縦横比 16:9。画像の中に文字・吹き出しを入れない。\n\
```\n"
        );
        let s02 = scene_markdown(&t, &t.scenes[1], &cfg()).unwrap();
        assert_eq!(
            s02,
            "# S02\n\
\n\
## S02-C01\n\
\n\
- 素材: `tracks/S02.png` か `tracks/S02.mp4`\n\
- 種類: 未指定(画像か動画)\n\
- 長さ: 1.8 秒\n\
- #visual: なし(台本に #visual を書いてください)\n\
\n\
```text\n\
アニメ調。\n\
縦横比 16:9。画像の中に文字・吹き出しを入れない。\n\
```\n"
        );
    }

    #[test]
    fn empty_style_and_suffix_are_omitted() {
        let t = tl();
        let c = PromptsConfig {
            style: "  ".into(),
            suffix: String::new(),
        };
        let s02 = scene_markdown(&t, &t.scenes[1], &c).unwrap();
        // 本文の要素が何も無いときは、空のコードブロックになる
        assert!(s02.ends_with("```text\n```\n"), "{s02}");
    }

    #[test]
    fn scene_with_only_zero_length_cuts_has_no_file() {
        let mut t = tl();
        t.scenes[1].cuts[0].end_ms = 7_250;
        assert!(scene_markdown(&t, &t.scenes[1], &cfg()).is_none());
    }

    #[test]
    fn untyped_visual_with_loop_says_so() {
        let mut t = tl();
        t.scenes[1].cuts[0].visual = Some(Visual {
            text: "海。".into(),
            kind: None,
            looped: true,
        });
        let s02 = scene_markdown(&t, &t.scenes[1], &cfg()).unwrap();
        assert!(
            s02.contains("- 種類: 未指定(画像か動画。動画ならループする)\n"),
            "{s02}"
        );
        assert!(!s02.contains("#visual: なし"), "{s02}");
    }

    #[test]
    fn fence_is_longer_than_backticks_in_body() {
        let mut t = tl();
        t.scenes[0].cuts[0].visual.as_mut().unwrap().text = "看板に ```code```` と書く。".into();
        let s01 = scene_markdown(&t, &t.scenes[0], &cfg()).unwrap();
        assert!(s01.contains("\n`````text\n"), "{s01}");
        assert!(s01.contains("看板に ```code```` と書く。\n"), "{s01}");
        assert!(s01.contains("\n`````\n"), "{s01}");
    }

    #[test]
    fn golden_index_markdown() {
        let t = tl();
        let mut found = BTreeMap::new();
        found.insert(
            "S01-C01".to_string(),
            Asset {
                cut_id: "S01-C01".into(),
                path: PathBuf::from("tracks").join("S01.jpg"),
                kind: AssetKind::Jpeg,
            },
        );
        let assets = AssetMatch {
            found,
            missing: vec!["S01-C02".into(), "S02-C01".into()],
            errors: vec!["tracks/ に S01_C02.mp4 と S01_C02.mov があります".into()],
        };
        assert_eq!(
            index_markdown(&t, &assets),
            "# 素材の一覧\n\
\n\
| カット | ファイル | 種類 | 長さ | 素材 |\n\
|---|---|---|---|---|\n\
| S01-C01 | `tracks/S01.png` | 画像 | 3.0 秒 | あり(`S01.jpg`) |\n\
| S01-C02 | `tracks/S01_C02.mp4` | 動画(ループする) | 4.3 秒 | なし |\n\
| S01-C03 | `tracks/S01_C03.png` か `tracks/S01_C03.mp4` | 未指定(画像か動画) | 0.0 秒 | 不要(長さ 0) |\n\
| S02-C01 | `tracks/S02.png` か `tracks/S02.mp4` | 未指定(画像か動画) | 1.8 秒 | なし |\n\
\n\
## tracks/ の確認事項\n\
\n\
- tracks/ に S01_C02.mp4 と S01_C02.mov があります\n"
        );
        // 確認事項が無ければ節を作らない
        let none = AssetMatch::default();
        assert!(!index_markdown(&t, &none).contains("確認事項"));
    }

    #[test]
    fn cuts_without_visual_skip_zero_length() {
        assert_eq!(cuts_without_visual(&tl()), ["S02-C01"]);
    }

    #[test]
    fn asset_query_has_only_nonzero_cuts() {
        let q = asset_query(&tl());
        let ids: Vec<&str> = q.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["S01-C01", "S01-C02", "S02-C01"]);
        assert_eq!(q[1].1, Some(VisualKind::Video));
    }
}
