//! ①a の完了条件(設計書 §12 項目8)の確認。`dump_prefade = true` で build した出力を調べる。
//! `work/lines_prefade/` の正規化後・フェード前の音声を、`cache.json` に記録した測定範囲で再測定する
//! (測定範囲は再検出しない)。何も書き換えない。

use std::path::Path;

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
    let timeline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("timeline.json"))?)?;
    let target = timeline["normalize"]["target_lufs"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("timeline.json に normalize.target_lufs がありません"))?;
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
        let from = meta
            .measure_start
            .checked_sub(meta.src_start)
            .ok_or_else(|| {
                anyhow::anyhow!("{id}: cache.json の測定範囲が使用範囲より前にあります")
            })? as usize;
        let to = from + measure_len as usize;
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
