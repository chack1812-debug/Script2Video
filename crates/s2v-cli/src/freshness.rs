//! 後続コマンド(prompts・bubbles・render)の共通の鮮度の検査(映像工程設計書 §2.4)。

use std::path::{Path, PathBuf};

use s2v_timeline::Timeline;

use crate::fsutil::sha256_file;

/// 24 時間(ミリ秒)。これを超える台本は bubbles・render で扱わない(設計書 §4.2)。
pub const MAX_DURATION_MS: i64 = 86_400_000;

pub struct Fresh {
    pub timeline: Timeline,
    pub warnings: Vec<String>,
}

fn folder(script: &Path) -> PathBuf {
    match script.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Err は終了コード 2 の理由。`need_audio` は bubbles・render(音声と長さも確かめる)。
pub fn check_fresh(script: &Path, need_audio: bool) -> Result<Fresh, String> {
    let dir = folder(script);
    let tl_path = dir.join("timeline").join("timeline.json");
    let text = match std::fs::read_to_string(&tl_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "{} がありません。先に build を実行してください",
                tl_path.display()
            ))
        }
        Err(e) => return Err(format!("{} を読めません: {e}", tl_path.display())),
    };
    let timeline = s2v_timeline::load_str(&text).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    match sha256_file(script) {
        Ok(h) if h == timeline.script_sha256 => {}
        Ok(_) => warnings
            .push("台本が build の後に変更されています。build をやり直してください".to_string()),
        Err(e) => return Err(format!("台本を読めません: {}: {e}", script.display())),
    }
    if need_audio {
        let wav = dir.join("full_dialogue.wav");
        match sha256_file(&wav) {
            Ok(h) if h == timeline.audio_sha256 => {}
            Ok(_) => {
                return Err(
                    "full_dialogue.wav が timeline.json と別の build のものです。build をやり直してください"
                        .to_string(),
                )
            }
            Err(e) => return Err(format!("{} を読めません: {e}", wav.display())),
        }
        if timeline.duration_ms == 0 {
            return Err("台本の長さが 0 です(台詞も #pause もありません)".to_string());
        }
        if timeline.duration_ms > MAX_DURATION_MS {
            return Err("台本の長さが 24 時間を超えています".to_string());
        }
    }
    Ok(Fresh { timeline, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_timeline::{CutEntry, Normalize, SceneEntry};

    fn minimal_timeline(script_sha: &str, audio_sha: &str, duration_ms: i64) -> Timeline {
        Timeline {
            schema_version: 3,
            script: "s.txt".to_string(),
            script_sha256: script_sha.to_string(),
            audio_sha256: audio_sha.to_string(),
            sample_rate: 48000,
            duration_ms,
            normalize: Normalize { target_lufs: -16.0 },
            cast: vec![],
            scenes: vec![SceneEntry {
                id: "S01".to_string(),
                name: None,
                index: 1,
                silent: true,
                start_ms: 0,
                end_ms: duration_ms,
                cuts: vec![CutEntry {
                    id: "S01-C01".to_string(),
                    start_ms: 0,
                    end_ms: duration_ms,
                    visual: None,
                }],
                notes: vec![],
            }],
            takes: vec![],
            lines: vec![],
            events: vec![],
        }
    }

    /// 台本・音声・timeline.json(v3)を置いたフォルダを作る。
    fn setup(dur: i64) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("s.txt");
        std::fs::write(&script, "台本").unwrap();
        std::fs::write(d.path().join("full_dialogue.wav"), "音声").unwrap();
        let t = minimal_timeline(
            &crate::work::sha256_hex("台本".as_bytes()),
            &crate::work::sha256_hex("音声".as_bytes()),
            dur,
        );
        std::fs::create_dir_all(d.path().join("timeline")).unwrap();
        std::fs::write(
            d.path().join("timeline").join("timeline.json"),
            t.to_json_pretty(),
        )
        .unwrap();
        (d, script)
    }

    #[test]
    fn fresh_build_passes_without_warnings() {
        let (_d, script) = setup(1000);
        let f = check_fresh(&script, true).unwrap();
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    }

    #[test]
    fn edited_script_is_a_warning() {
        let (_d, script) = setup(1000);
        std::fs::write(&script, "台本を直した").unwrap();
        let f = check_fresh(&script, true).unwrap();
        assert_eq!(f.warnings.len(), 1);
    }

    #[test]
    fn audio_mismatch_missing_timeline_and_zero_duration_are_errors() {
        let (d, script) = setup(1000);
        std::fs::write(d.path().join("full_dialogue.wav"), "別の音声").unwrap();
        assert!(check_fresh(&script, true).is_err());
        assert!(check_fresh(&script, false).is_ok()); // prompts は音声を見ない
        let (_d2, script2) = setup(0);
        assert!(check_fresh(&script2, true).is_err());
        assert!(check_fresh(&script2, false).is_ok());
        let d3 = tempfile::tempdir().unwrap();
        let e = check_fresh(&d3.path().join("s.txt"), false).err().unwrap();
        assert!(e.contains("build"), "{e}");
    }

    #[test]
    fn v2_timeline_is_rejected() {
        let (d, script) = setup(1000);
        std::fs::write(
            d.path().join("timeline").join("timeline.json"),
            "{\"schema_version\":2}",
        )
        .unwrap();
        assert!(check_fresh(&script, false).is_err());
    }
}
