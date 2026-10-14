//! ①a の完了条件(設計書 §12 項目8)の確認。`dump_prefade = true` で build した出力を調べる。
//! 引数は台本フォルダ(`timeline/timeline.json` と `work/` がある場所)。
//! `work/lines_prefade/` の正規化後・フェード前の音声を、`cache.json` に記録した測定範囲で再測定する
//! (測定範囲は再検出しない)。何も書き換えない。

use std::path::Path;

use anyhow::Context;

use s2v_audio::loudness::{measure_mono, Loudness};

use crate::work::load_cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// 測定範囲が 400ms 以上: 目標の ±1 LU に入ること
    Measured,
    /// 測定範囲が 400ms 未満: 適用ゲインが式どおりであること
    ShortLine,
    /// ゲインが制限に掛かった: 判定の対象外(合格扱い)
    Clamped,
}

#[derive(Debug, Clone)]
pub struct LineCheck {
    pub id: String,
    pub kind: CheckKind,
    pub passed: bool,
    pub detail: String,
}

pub fn verify_loudness(out_dir: &Path) -> anyhow::Result<Vec<LineCheck>> {
    let tl_path = out_dir.join("timeline").join("timeline.json");
    let timeline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&tl_path).with_context(|| {
            format!(
                "{} を読めません(台本フォルダを指定してください)",
                tl_path.display()
            )
        })?)?;
    let target = timeline["normalize"]["target_lufs"]
        .as_f64()
        .ok_or_else(|| {
            anyhow::anyhow!("timeline/timeline.json に normalize.target_lufs がありません")
        })?;
    let work = out_dir.join("work");
    let entries = load_cache(&work)?;

    let mut checks = Vec::new();
    for (rel, entry) in &entries {
        let Some(id) = rel
            .strip_prefix("lines/")
            .and_then(|n| n.strip_suffix(".wav"))
        else {
            continue;
        };
        let Some(meta) = &entry.norm else { continue };
        if meta.norm_clamped {
            checks.push(LineCheck {
                id: id.to_string(),
                kind: CheckKind::Clamped,
                passed: true,
                detail: "ゲインが制限に掛かったため対象外".into(),
            });
            continue;
        }
        let measure_len = meta
            .measure_end
            .checked_sub(meta.measure_start)
            .ok_or_else(|| {
                anyhow::anyhow!("{id}: cache.json の測定範囲が不正です(終了が開始より前)")
            })?;
        if measure_len < 19_200 {
            let expected = target - meta.measured_lufs;
            let passed = (meta.norm_gain_db - expected).abs() < 1e-6;
            checks.push(LineCheck {
                id: id.to_string(),
                kind: CheckKind::ShortLine,
                passed,
                detail: format!("適用 {:+.3} dB / 式 {:+.3} dB", meta.norm_gain_db, expected),
            });
            continue;
        }
        let prefade = work.join("lines_prefade").join(format!("{id}.wav"));
        if !prefade.exists() {
            anyhow::bail!(
                "{} がありません。[normalize] dump_prefade = true で build し直してください",
                prefade.display()
            );
        }
        let mut r = hound::WavReader::open(&prefade)?;
        let samples: Vec<f32> = r.samples::<f32>().collect::<Result<_, _>>()?;
        let from_u64 = meta
            .measure_start
            .checked_sub(meta.src_start)
            .ok_or_else(|| {
                anyhow::anyhow!("{id}: cache.json の measure_start が src_start より前にあります")
            })?;
        let to_u64 = meta
            .measure_end
            .checked_sub(meta.src_start)
            .ok_or_else(|| {
                anyhow::anyhow!("{id}: cache.json の measure_end が src_start より前にあります")
            })?;
        let from = usize::try_from(from_u64)
            .map_err(|_| anyhow::anyhow!("{id}: cache.json の measure_start が大きすぎます"))?;
        let to = usize::try_from(to_u64)
            .map_err(|_| anyhow::anyhow!("{id}: cache.json の measure_end が大きすぎます"))?;
        let Some(range) = samples.get(from..to) else {
            anyhow::bail!(
                "{id}: 測定範囲 {from}..{to} が lines_prefade の長さ {} を超えています",
                samples.len()
            );
        };
        let (passed, detail) = match measure_mono(range) {
            Loudness::Lufs(l) => (
                (l - target).abs() <= 1.0,
                format!("{l:.2} LUFS(目標 {target:.1})"),
            ),
            Loudness::Unmeasurable => (false, "測定できません".to_string()),
        };
        checks.push(LineCheck {
            id: id.to_string(),
            kind: CheckKind::Measured,
            passed,
            detail,
        });
    }
    if checks.is_empty() {
        anyhow::bail!("確認できる台詞がありません(cache.json に lines/ の正規化記録がありません)");
    }
    Ok(checks)
}

/// 例 `verify_loudness` の本体。(終了コード, 表示する文面)を返す。
/// 0=すべて OK、1=不合格あり、2=使い方の誤り・確認できない。
pub fn run_cli(args: &[String]) -> (u8, String) {
    let Some(dir) = args.first() else {
        return (
            2,
            "使い方: verify_loudness <台本フォルダ>
"
            .to_string(),
        );
    };
    match verify_loudness(Path::new(dir)) {
        Ok(checks) => {
            let mut text = String::new();
            let mut failed = 0;
            for c in &checks {
                let mark = if c.passed { "OK " } else { "NG " };
                text.push_str(&format!(
                    "{mark} {:<12} {:?} {}
",
                    c.id, c.kind, c.detail
                ));
                if !c.passed {
                    failed += 1;
                }
            }
            text.push_str(&format!(
                "{} 件中 {} 件が不合格
",
                checks.len(),
                failed
            ));
            (u8::from(failed != 0), text)
        }
        Err(e) => (
            2,
            format!(
                "[ERROR] {e:#}
"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_names_the_script_folder() {
        let (code, text) = run_cli(&[]);
        assert_eq!(code, 2);
        assert!(text.contains("台本フォルダ"), "{text}");
    }

    #[test]
    fn reads_timeline_under_timeline_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (code, text) = run_cli(&[tmp.path().to_string_lossy().into_owned()]);
        assert_eq!(code, 2);
        assert!(text.contains("timeline"), "{text}");
    }
}
