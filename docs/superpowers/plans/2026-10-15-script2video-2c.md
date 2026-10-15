# Script2Video 計画 2c(プロンプト生成 prompts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `timeline/timeline.json` から、素材(画像・動画)を作るためのプロンプト `prompts/S01.md` … と一覧 `prompts/index.md` を書き出すコマンド `script2video prompts` を作る。

**Architecture:** `s2v-cli` に、ファイルに触れない純粋な組み立て `prompts_md.rs`(Markdown の文字列を作る)と、コマンド本体 `prompts.rs`(ロック・ログ・鮮度の検査・`prompts/` の安全検査・自分の作ったファイルだけの削除・書き込み)を加える。`timeline.json` は `s2v-timeline` の型だけで読み(既存の `freshness::check_fresh`)、素材の有無は `s2v-render::assets::match_assets` で調べる。外部プロセスは使わない。

**Tech Stack:** Rust 1.96、既存のクレート(`s2v-core`・`s2v-timeline`・`s2v-render`)。新しい依存は加えない(`regex` も使わない)。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)の §7(prompts)、§2.2〜2.4(予約名・安全検査・ロック・ログ・鮮度)、§3.4 の表(長さ 0・カットをまたぐ台詞)、§3.5(`timeline.json` v3)、§8 の prompts のテスト。前提として計画 2a・2b が main にある。

## Global Constraints

- 応答・コメント・エラーメッセージ・コミットメッセージは日本語(既存のコードに合わせる)。
- 入力は `timeline/timeline.json`(v3)だけ。`freshness::check_fresh(script, false)` を通す(音声は見ない)。Err は エラー 2、台本の SHA-256 の違いは警告。
- ロック(`ScriptLock::acquire`)を取り、終了まで保持する。`Process.log` は `ProcLog::append(…, "prompts")` で追記する。
- `prompts/` の中で消してよいのは、自分が作る名前 `index.md` と `^S\d+\.md$`(`S` は大文字、数字は1桁以上の ASCII)のファイルだけ。それ以外のファイル・フォルダには触れない。
- `prompts/` がリンク(シンボリックリンク・ジャンクション)・台本フォルダの外を指す → エラー 2(何も消さない)。台本フォルダに `prompts` という名前のファイル(フォルダでないもの)がある → エラー 2。
- シーンごとに `prompts/S01.md`。カットごとに節。長さ 0 のカットは節を作らない。
- 各節: 作る素材のファイル名(`tracks/S01.png` など。`kind` が null なら「png か mp4」)、種類と、動画ならカットの長さ(秒、小数1桁)と `loop` の有無、プロンプト本文。
- プロンプト本文の順: (1) `[prompts]` の `style`、(2) シーン名、(3) カットの `#visual`、(4) 登場人物とその外見(`cast[].appearance`。外見が無い役は名前だけ)、(5) `[prompts]` の `suffix`。
- 登場人物: `line.start_ms < cut.end_ms` かつ `cut.start_ms < line.end_ms` の台詞の話者。台本の並びで初出順、重複なし。
- `prompts/index.md`: 全カットの表(カット ID、ファイル名、種類、長さ、素材の有無)。
- `duration_ms` が 0 → `index.md` の表だけを出す(シーンのファイルは作らない)。
- 終了コード: 0 成功、1 警告(`#visual` の無いカットがある、台本が `build` 後に変わった)、2 失敗(鮮度の検査、書き込み失敗、ロック)。
- 設定 `[prompts]`: `style`(既定 空)、`suffix`(既定「縦横比 16:9。画像の中に文字・吹き出しを入れない。」)。既存の `s2v_core::config::PromptsConfig`。
- 検証コマンド(各タスクの最後に全部通す): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- コミットメッセージの末尾に、空行のあと実装者のモデルの `Co-Authored-By:` 行。

## 計画で決めたこと(設計書が黙っている点)

- **素材の名前**: `s2v_render::assets::asset_stem`(`S01-C01` → `S01`、`S01-C02` → `S01_C02`)に、画像は `.png`、動画は `.mp4` を付ける。`kind` が null なら「`tracks/S01.png` か `tracks/S01.mp4`」。
- **長さの表示**: 動画と、`kind` が null のカット(動画になりうる)に出す。画像には出さない。`index.md` は全カットに出す。四捨五入は `(ms + 50) / 100` を 10 で割った商と余り(`4250` → `4.3`)。
- **プロンプト本文**: Markdown のコードブロック(言語 `text`)に入れ、コピーしやすくする。空の要素(空の `style`・名前の無いシーン・`#visual` の無いカット・登場人物なし・空の `suffix`)は行ごと省く。登場人物は `登場人物:` の行のあとに `- 名前: 外見` を1人1行(外見の改行は空白1つにする)。コードブロックの囲みは、本文の中の最も長い連続したバッククォートより1つ長くする(最低3つ)。
- **`#visual` の無いカット**: 節は作り、`- #visual: なし(台本に #visual を書いてください)` の行を足し、警告する(長さ 0 のカットは警告しない)。
- **全カットが長さ 0 のシーン**: ファイルを作らない(`duration_ms` が 0 のときと同じ扱い)。
- **素材の有無**: 長さ 1 以上のカットだけを `match_assets` に渡す。見つかれば「あり(`S01.jpg`)」、無ければ「なし」、長さ 0 は「不要(長さ 0)」。`match_assets` の `errors`(重複・種類違い・リンク)は `index.md` の末尾の「tracks/ の確認事項」に並べ、`Process.log` に INFO で書く。終了コードは変えない(§7 の警告の条件に無いため。素材の誤りは `render` が エラー 2 にする)。
- **書き込みの順**: 全ファイルの内容をメモリで作ってから、古い生成ファイルを消し、`fsutil::write_atomic` で1つずつ書く。
- **設定**: `render` と同じく `--config <path>`(省略時は同梱の既定)。

## Review Focus

1. **`prompts/` に人が置いたメモ**(`memo.md`、`S01_note.md`、`S1x.md`、`s01.md` でない別名のもの): 消えない・変わらない。前回あって今回は無いシーンの `S09.md` は消える。→ Task 2 のテスト `keeps_user_files_and_removes_stale_generated_ones`。
2. **`build` の後に台本を直した**: 警告(終了コード 1)にして、プロンプトは書く。→ Task 2 のテスト `edited_script_is_a_warning_and_still_writes`。
3. **`#visual` の本文にバッククォート(` ``` `)を含む**: コードブロックが途中で閉じず、本文がそのまま残る。→ Task 1 のテスト `fence_is_longer_than_backticks_in_body`。
4. **前のカット(シーン)から続く台詞**: 話者が両方のカットの登場人物に出る。登場人物は台本の並びの初出順。→ Task 1 のテスト `golden_scene_markdown`(S01-C02 に 佐藤 → 田中 の順)。
5. **`prompts` がリンク・ファイル、`timeline.json` が無い**: エラー 2 で、何も消さず、`prompts/` も作らない。→ Task 2 のテスト `prompts_link_or_file_is_failed_without_deleting`、`missing_timeline_is_failed_without_writing`。

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `crates/s2v-cli/src/prompts_md.rs`(新規) | Markdown の組み立て(秒の表示、素材の名前、種類、登場人物、本文、シーンのファイル、一覧)。ファイルに触れない | 1 |
| `crates/s2v-cli/src/prompts.rs`(新規) | コマンド本体(検査・ロック・ログ・削除・書き込み) | 2 |
| `crates/s2v-cli/src/lib.rs`、`src/main.rs` | モジュールの追加、`run_prompts` の公開、CLI の `prompts` | 1(mod)・2 |
| `crates/s2v-cli/tests/prompts_cli.rs`(新規) | build を通した結合テスト(ffmpeg 不要) | 2 |
| `README.md` | `prompts` の使い方 | 2 |

`s2v-cli` は既に `s2v-core`・`s2v-timeline`・`s2v-render` に依存している(`Cargo.toml` の変更は不要。無ければ足す)。

---

### Task 1: Markdown の組み立て(`prompts_md.rs`)

**Files:**
- Create: `crates/s2v-cli/src/prompts_md.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod prompts_md;` を加える)

**Interfaces:**
- Consumes: `s2v_timeline::{Timeline, SceneEntry, CutEntry, CastEntry, VisualKind}`、`s2v_core::config::PromptsConfig { style: String, suffix: String }`、`s2v_render::assets::{asset_stem, AssetMatch}`(`AssetMatch { found: BTreeMap<String, Asset>, missing: Vec<String>, errors: Vec<String> }`、`Asset { cut_id, path: PathBuf, kind }`)。
- Produces(Task 2 が使う):
  - `pub fn seconds_1(ms: i64) -> String`
  - `pub fn cast_for_cut<'a>(tl: &'a Timeline, cut: &CutEntry) -> Vec<&'a CastEntry>`
  - `pub fn scene_markdown(tl: &Timeline, scene: &SceneEntry, cfg: &PromptsConfig) -> Option<String>`(長さ 1 以上のカットが無ければ None)
  - `pub fn index_markdown(tl: &Timeline, assets: &AssetMatch) -> String`
  - `pub fn cuts_without_visual(tl: &Timeline) -> Vec<String>`(長さ 1 以上で `visual` が null のカット ID。台本の並び)
  - `pub fn asset_query(tl: &Timeline) -> Vec<(String, Option<VisualKind>)>`(長さ 1 以上のカットの (ID, kind)。`match_assets` に渡す)

- [ ] **Step 1: 失敗するテストを書く**

`prompts_md.rs` の末尾に置く。テスト用の `Timeline` は手で組み立てる(検証は通さない。フィールドは `crates/s2v-timeline/src/lib.rs` の型どおり)。

````rust
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
                        cut("S01-C01", 0, 3_000, Some(("朝の光。", Some(VisualKind::Image), false))),
                        cut("S01-C02", 3_000, 7_250, Some(("波。", Some(VisualKind::Video), true))),
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
````

注意: 上の文字列の `\n\` の続きの行は、行頭の空白が取り除かれる Rust の文字列の継続。Markdown の行頭の空白は無いので、そのまま一致する。

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-cli prompts_md` / Expected: 関数が無くコンパイルエラー。

- [ ] **Step 3: 実装する**

```rust
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
    if let Some(name) = scene.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
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
                let a = a.split('\n').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
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
    if let Some(name) = scene.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        md.push(' ');
        md.push_str(name);
    }
    md.push('\n');
    for cut in cuts {
        md.push_str(&format!("\n## {}\n\n", cut.id));
        md.push_str(&format!("- 素材: {}\n", asset_names(cut)));
        md.push_str(&format!("- 種類: {}\n", kind_label(cut)));
        if shows_length(cut) {
            md.push_str(&format!("- 長さ: {} 秒\n", seconds_1(cut.end_ms - cut.start_ms)));
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
```

`lib.rs` に `pub mod prompts_md;` を加える(アルファベット順で `prompts` の位置。`prompts` 本体は Task 2)。

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-cli prompts_md` / Expected: 10 本すべて PASS。

- [ ] **Step 5: 全体の検証とコミット** — `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。コミット: `feat: prompts の Markdown(シーンごとのプロンプトと素材の一覧)を組み立てる`

---

### Task 2: `prompts` コマンド、結合テスト、README

**Files:**
- Create: `crates/s2v-cli/src/prompts.rs`、`crates/s2v-cli/tests/prompts_cli.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod prompts;`、`pub use prompts::{run_prompts, PromptsArgs};`)、`crates/s2v-cli/src/main.rs`(`Prompts { script, #[arg(long)] config }`)、`README.md`

**Interfaces:**
- Consumes: Task 1 の `prompts_md::{scene_markdown, index_markdown, cuts_without_visual, asset_query}`。既存の `freshness::check_fresh(script: &Path, need_audio: bool) -> Result<Fresh { timeline, warnings }, String>`、`lock::ScriptLock::acquire(&Path)`、`proclog::ProcLog::{append, info, warn, error, warning_count, check}`、`layout::{check_reserved_name, check_not_symlink, check_contained, script_dir}`、`fsutil::write_atomic(&Path, &[u8]) -> io::Result<()>`、`s2v_render::assets::match_assets(&Path, &[(String, Option<VisualKind>)]) -> AssetMatch`、`s2v_core::Config::{from_file, builtin, validate}`。
- Produces: `pub struct PromptsArgs { pub script: PathBuf, pub config: Option<PathBuf> }`、`pub fn run_prompts(args: &PromptsArgs) -> Outcome`。CLI: `script2video prompts <台本> [--config <path>]`。

処理の順:
1. `check_reserved_name`、`check_not_symlink`、台本が無ければ「台本を読めません」で 2。設定を読む(`--config` か同梱)・`validate`(誤りは 2)。`ScriptLock::acquire(script_dir)`(取れなければ 2)。ここまでの失敗は標準エラーにだけ出す(`render.rs` の `fail` と同じ形)。
2. `ProcLog::append(Process.log, "prompts")`。
3. `check_fresh(script, false)`。Err はログに書いて 2。警告はログに `warn`。
4. `prompts` の検査: `symlink_metadata(dir/prompts)` が存在して、リンクでもフォルダでもない(ファイル)なら「台本フォルダに prompts という名前のファイルがあります」で 2。`check_contained(dir, "prompts")` の Err は 2(リンク・外側)。
5. 中身を作る: `files: BTreeMap<String, String>`。各シーンの `scene_markdown` が Some なら `"<シーン ID>.md"`。`duration_ms` が 0 なら シーンのファイルは作らない(`scene_markdown` は全カット長さ 0 なので自然に None になる)。`match_assets(dir/tracks, &asset_query(tl))` の結果で `index.md`。`assets.errors` はそれぞれ `log.info` に「tracks/ の確認事項: …」で書く。`cuts_without_visual` のカットごとに `log.warn("<ID>: #visual がありません(prompts/<シーン>.md に「なし」と書きました)")`。
6. `create_dir_all(dir/prompts)`(失敗は 2)。`read_dir(dir/prompts)` を列挙し、`is_generated_name(name)` のエントリで、`symlink_metadata` がファイルかリンクのものを `remove_file`(失敗は 2)。フォルダは消さない(同名のフォルダがあれば、後の書き込みが失敗して 2)。
7. `files` を名前の順に `write_atomic(dir/prompts/<name>, bytes)`(失敗は「prompts/<name> を書けません: …」で 2)。
8. `log.info("prompts/ に <n> 個のファイルを書きました")`。`log.check()` の Err は 2。終了コード: 警告があれば 1、無ければ 0。

```rust
/// 自分が作るファイルの名前か(`index.md`、または `S` + ASCII 数字1桁以上 + `.md`)。
pub(crate) fn is_generated_name(name: &str) -> bool {
    if name == "index.md" {
        return true;
    }
    let Some(stem) = name.strip_prefix('S').and_then(|s| s.strip_suffix(".md")) else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
}
```

- [ ] **Step 1: 失敗するテストを書く**

`prompts.rs` の単体テスト:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_are_exact() {
        for n in ["index.md", "S01.md", "S00.md", "S123.md", "S1.md"] {
            assert!(is_generated_name(n), "{n}");
        }
        for n in [
            "memo.md", "S01_note.md", "S1x.md", "s01.md", "S01.md.bak", "S.md", "Index.md",
            "S01.txt", "S０１.md", "xS01.md",
        ] {
            assert!(!is_generated_name(n), "{n}");
        }
    }
}
```

`tests/prompts_cli.rs`(ffmpeg は使わない。build だけを通す):

```rust
//! `prompts` コマンドの結合テスト(build を通した台本で確かめる)。

use std::fs;
use std::path::{Path, PathBuf};

use s2v_cli::{run_build, run_prompts, BuildArgs, Outcome, PromptsArgs};

const HEADER: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\n田中:田中,pan=-30\n30代の男性。紺のカーディガン。\n\n佐藤:佐藤,pan=30\n\n@script\n";

/// 2 シーン。S01 は画像のカットと動画(loop)のカット、S02 は #visual なし。
const BODY: &str = "#paragraph[S01] 朝の居間\n#visual (image) 朝の光。\n[S01-L001] 田中:おはよう。\n#cut[C02]\n#visual (video loop) 波。\n[S01-L002] 佐藤:早いね。\n\n#paragraph[S02] 駅前\n[S02-L001] 田中:行こう。\n";

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
}

impl Project {
    fn prompts(&self) -> PathBuf {
        self.dir.join("prompts")
    }
    fn run(&self) -> Outcome {
        run_prompts(&PromptsArgs {
            script: self.script.clone(),
            config: None,
        })
    }
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("Process.log")).unwrap_or_default()
    }
}

/// 440Hz の収録(render_ffmpeg.rs の write_voice と同じ: 200ms 無音 + 1000ms + 300ms 無音)。
fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..72_000usize {
        let v = if (9_600..57_600).contains(&i) {
            0.1 * (2.0 * std::f64::consts::PI * 440.0 * (i - 9_600) as f64 / 48_000.0).sin()
        } else {
            0.0
        };
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

/// 台本を書き、収録を置いて build する。`build` が false なら build しない。
fn project(body: &str, build: bool) -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("voices");
    fs::create_dir_all(&voices).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{body}")).unwrap();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((id, _)) = rest.split_once(']') {
                write_voice(&voices.join(format!("{}.wav", id.replace('-', "_"))));
            }
        }
    }
    if build {
        let o = run_build(&BuildArgs {
            script: script.clone(),
            voice_dir: None,
            config: None,
            no_cache: false,
        });
        assert!(o != Outcome::Failed, "build が失敗しました");
    }
    Project { _tmp: tmp, dir, script }
}

#[test]
fn writes_scene_files_and_index() {
    let p = project(BODY, true);
    // S02 に #visual が無いので警告
    assert_eq!(p.run(), Outcome::Warnings, "{}", p.log());
    let s01 = fs::read_to_string(p.prompts().join("S01.md")).unwrap();
    assert!(s01.starts_with("# S01 朝の居間\n"), "{s01}");
    assert!(s01.contains("## S01-C01\n\n- 素材: `tracks/S01.png`\n- 種類: 画像\n"), "{s01}");
    assert!(s01.contains("## S01-C02\n\n- 素材: `tracks/S01_C02.mp4`\n- 種類: 動画(ループする)\n- 長さ: "), "{s01}");
    assert!(s01.contains("- 田中: 30代の男性。紺のカーディガン。\n"), "{s01}");
    assert!(s01.contains("\n- 佐藤\n"), "{s01}");
    assert!(s01.contains("縦横比 16:9。画像の中に文字・吹き出しを入れない。\n"), "{s01}");
    let s02 = fs::read_to_string(p.prompts().join("S02.md")).unwrap();
    assert!(s02.contains("- #visual: なし(台本に #visual を書いてください)\n"), "{s02}");
    let index = fs::read_to_string(p.prompts().join("index.md")).unwrap();
    assert!(index.contains("| S01-C01 | `tracks/S01.png` | 画像 | "), "{index}");
    assert!(index.contains("| なし |"), "{index}");
    assert!(p.log().contains("S02-C01: #visual がありません"), "{}", p.log());
    assert!(p.log().contains("== prompts "), "{}", p.log());
}

#[test]
fn existing_asset_is_shown_in_index() {
    let p = project(BODY, true);
    let tracks = p.dir.join("tracks");
    fs::create_dir_all(&tracks).unwrap();
    fs::write(tracks.join("s01.JPG"), b"x").unwrap();
    p.run();
    let index = fs::read_to_string(p.prompts().join("index.md")).unwrap();
    assert!(index.contains("| あり(`s01.JPG`) |"), "{index}");
}

#[test]
fn all_visuals_present_is_ok() {
    let body = BODY.replace("#paragraph[S02] 駅前\n", "#paragraph[S02] 駅前\n#visual 駅前の広場。\n");
    let p = project(&body, true);
    assert_eq!(p.run(), Outcome::Ok, "{}", p.log());
}

#[test]
fn keeps_user_files_and_removes_stale_generated_ones() {
    // Review Focus 1
    let p = project(BODY, true);
    let dir = p.prompts();
    fs::create_dir_all(dir.join("下書き")).unwrap();
    for (name, body) in [
        ("memo.md", "メモ"),
        ("S01_note.md", "注"),
        ("S1x.md", "x"),
        ("S01.md.bak", "bak"),
        ("S09.md", "古いシーン"),
        ("index.md", "古い一覧"),
    ] {
        fs::write(dir.join(name), body).unwrap();
    }
    fs::write(dir.join("下書き").join("S01.md"), "下書き").unwrap();
    p.run();
    assert_eq!(fs::read_to_string(dir.join("memo.md")).unwrap(), "メモ");
    assert_eq!(fs::read_to_string(dir.join("S01_note.md")).unwrap(), "注");
    assert_eq!(fs::read_to_string(dir.join("S1x.md")).unwrap(), "x");
    assert_eq!(fs::read_to_string(dir.join("S01.md.bak")).unwrap(), "bak");
    assert_eq!(fs::read_to_string(dir.join("下書き").join("S01.md")).unwrap(), "下書き");
    assert!(!dir.join("S09.md").exists(), "前回の生成ファイルは消える");
    assert!(fs::read_to_string(dir.join("index.md")).unwrap().starts_with("# 素材の一覧"));
}

#[test]
fn edited_script_is_a_warning_and_still_writes() {
    // Review Focus 2
    let body = BODY.replace("#paragraph[S02] 駅前\n", "#paragraph[S02] 駅前\n#visual 駅前の広場。\n");
    let p = project(&body, true);
    let mut text = fs::read_to_string(&p.script).unwrap();
    text.push_str("\n");
    fs::write(&p.script, text).unwrap();
    assert_eq!(p.run(), Outcome::Warnings, "{}", p.log());
    assert!(p.log().contains("build をやり直してください"), "{}", p.log());
    assert!(p.prompts().join("S01.md").exists());
}

#[test]
fn missing_timeline_is_failed_without_writing() {
    // Review Focus 5
    let p = project(BODY, false);
    assert_eq!(p.run(), Outcome::Failed);
    assert!(!p.prompts().exists(), "prompts/ を作らない");
    assert!(p.log().contains("build を実行してください"), "{}", p.log());
}

#[test]
fn prompts_link_or_file_is_failed_without_deleting() {
    // Review Focus 5: prompts という名前のファイル
    let p = project(BODY, true);
    fs::write(p.prompts(), "ファイル").unwrap();
    assert_eq!(p.run(), Outcome::Failed);
    assert_eq!(fs::read_to_string(p.prompts()).unwrap(), "ファイル");

    // 台本フォルダの外を指すリンク: 中の生成名のファイルも消さない
    let p = project(BODY, true);
    let foreign = p._tmp.path().join("foreign");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("S01.md"), "外").unwrap();
    if make_dir_link(&p.prompts(), &foreign) {
        assert_eq!(p.run(), Outcome::Failed);
        assert_eq!(fs::read_to_string(foreign.join("S01.md")).unwrap(), "外");
    }
}

#[test]
fn zero_length_script_writes_only_index() {
    // 台詞も #pause も無い台本(build は通る。duration_ms = 0)
    let p = project("#paragraph[S01] 無言\n#visual 空。\n", true);
    p.run();
    assert!(p.prompts().join("index.md").exists());
    assert!(!p.prompts().join("S01.md").exists());
}

/// ディレクトリのリンクを作る(build_golden.rs の make_dir_link と同じ)。
/// Windows では mklink /J に権限は要らないので、作れなければテストの準備の不具合として止める。
fn make_dir_link(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "ディレクトリのリンクを作れません: {}", link.display());
        true
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
```

注意:
- `zero_length_script_writes_only_index` は、`#paragraph` と `#visual` だけで台詞の無い台本を build が通すこと(設計書 §3.4 の表「`duration_ms` が 0 … `build` は従来どおり」)を前提にする。build が通らない(または `S01` の存在の検査で落ちる)なら、この台本の形をコードに合わせて直し、何に合わせたかを報告に書く。
- `HEADER` の `@cast` の書式(定義行の次の行が外見、空行で区切る)は `sample/sample_s2video.txt` と同じ。外見の複数行は `timeline.json` で `\n` 連結になる。
- `s2v-cli` の dev-dependencies に `hound`・`tempfile` が無ければ足す(既存の結合テストが使っているので、あるはず)。

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-cli --test prompts_cli` / Expected: `run_prompts`・`PromptsArgs` が無くコンパイルエラー。

- [ ] **Step 3: 実装する** — 上の「処理の順」で `prompts.rs` を書く。形は `render.rs` の `run_render`・`render`(何かを書く前の `fail`、`ProcLog::append` の後は `log.error` してから `Outcome::Failed`)に合わせる。`main.rs` に次を加える:

```rust
    /// timeline.json から素材のプロンプト(prompts/S01.md …)と一覧(prompts/index.md)を作る
    Prompts {
        script: PathBuf,
        /// 設定ファイル(省略時は同梱の既定設定)
        #[arg(long)]
        config: Option<PathBuf>,
    },
```

```rust
        Command::Prompts { script, config } => {
            s2v_cli::run_prompts(&s2v_cli::PromptsArgs { script, config })
        }
```

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-cli prompts`、`cargo test -p s2v-cli --test prompts_cli` / Expected: すべて PASS。

- [ ] **Step 5: README を更新する**

`README.md` の「使い方」に `script2video prompts 台本.txt [--config config.toml]` を加え、`render` の節の前に「## prompts(素材のプロンプトを作る)」の節を加える。書くこと: `build` の後に実行する、出力(`prompts/S01.md` … と `prompts/index.md`)、各節の中身(素材のファイル名・種類・長さ・プロンプト本文の5つの要素の順)、`[prompts]` の `style`・`suffix`、`prompts/` の中で上書き・削除するのは `index.md` と `S<数字>.md` だけ(自分のメモは別の名前で置ける)、終了コード(1: `#visual` の無いカット・台本の変更、2: `build` がまだ・`prompts` がリンクやファイル・書き込み失敗・ロック)、`index.md` の「素材」の列と「tracks/ の確認事項」(終了コードは変えない。誤りは `render` で エラー 2)。README の既存の文体・見出しの深さに合わせる。

- [ ] **Step 6: 全体の検証とコミット** — `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。コミット: `feat: prompts コマンドで素材のプロンプトと一覧を prompts/ に書き出す`

- [ ] **Step 7: 手動の確認(Human)**

`sample/sample_s2video.txt` で `script2video build` → `script2video prompts` を実行し、`prompts/S01.md` などのプロンプトが画像生成に使える内容か(登場人物と外見、シーン名、`#visual`、末尾の注意書き)、`index.md` の表が読みやすいかを目で確認してもらう。
