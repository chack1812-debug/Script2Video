//! 収録ファイルの照合(台本仕様 §3、設計書 5.0)。①a は単独ファイル(`S01_L003.wav`)だけを扱う。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ids::LineId;

#[derive(Debug, Default)]
pub struct VoiceMatch {
    /// 台詞ごとの収録ファイル(名前順)。2つ以上なら重複。
    pub files: BTreeMap<LineId, Vec<PathBuf>>,
    /// 照合から外したファイルの警告(終了コード 1)。
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum NameKind {
    Single(LineId),
    /// 範囲を示すファイル名(`S01_L003-L007` など)。①a では未対応
    Range,
    NotVoice,
}

fn classify(stem: &str) -> NameKind {
    if let Some(id) = LineId::parse_file_stem(stem) {
        return NameKind::Single(id);
    }
    if stem.starts_with('S') && stem.contains("_L") && stem.contains('-') {
        return NameKind::Range;
    }
    NameKind::NotVoice
}

/// `dir` 直下の収録ファイルを台詞IDに対応付ける。サブフォルダは見ない。
pub fn match_voice_files(
    dir: &Path,
    script_ids: &[LineId],
    allow_flac: bool,
) -> anyhow::Result<VoiceMatch> {
    let known: HashSet<LineId> = script_ids.iter().copied().collect();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    let mut m = VoiceMatch::default();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        match ext.to_ascii_lowercase().as_str() {
            "wav" => {}
            "flac" if allow_flac => {}
            "flac" => {
                m.warnings.push(format!(
                    "FLAC は設定で許可されていません（[voice] allow_flac = true で許可。無視します）: {name}"
                ));
                continue;
            }
            _ => continue,
        }
        match classify(stem) {
            NameKind::Single(id) if known.contains(&id) => {
                m.files.entry(id).or_default().push(path);
            }
            NameKind::Single(_) => {
                m.warnings.push(format!(
                    "台本にない台詞IDのファイルです（無視します）: {name}"
                ));
            }
            NameKind::Range => m.warnings.push(format!(
                "範囲を示すファイル名は、この版(①a)では未対応です（照合の対象から外します）: {name}"
            )),
            NameKind::NotVoice => {
                m.warnings.push(format!(
                    "台詞IDの形式でないファイル名です（無視します）: {name}"
                ));
            }
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    fn ids() -> Vec<LineId> {
        (1..=4).map(|n| LineId { scene: 1, line: n }).collect()
    }

    #[test]
    fn matches_single_files_and_reports_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("収録 フォルダ");
        std::fs::create_dir(&dir).unwrap();
        touch(&dir, "S01_L001.wav");
        touch(&dir, "S01_L002.wav");
        touch(&dir, "S01_L002.flac");
        touch(&dir, "S01_L003-L004.wav");
        touch(&dir, "S09_L999.wav");
        touch(&dir, "memo.txt");
        touch(&dir, "take1.wav");

        let m = match_voice_files(&dir, &ids(), true).unwrap();
        assert_eq!(m.files[&LineId { scene: 1, line: 1 }].len(), 1);
        assert_eq!(
            m.files[&LineId { scene: 1, line: 2 }].len(),
            2,
            "wav と flac は重複"
        );
        assert!(!m.files.contains_key(&LineId { scene: 1, line: 3 }));
        assert_eq!(m.warnings.len(), 3, "{:?}", m.warnings);
        assert!(m
            .warnings
            .iter()
            .any(|w| w.contains("S01_L003-L004.wav") && w.contains("未対応")));
        assert!(m.warnings.iter().any(|w| w.contains("S09_L999.wav")));
        assert!(m.warnings.iter().any(|w| w.contains("take1.wav")));
    }

    #[test]
    fn flac_is_ignored_with_warning_unless_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L001.FLAC");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(m.files.is_empty());
        assert_eq!(m.warnings.len(), 1);
        assert!(m.warnings[0].contains("allow_flac"));
    }

    #[test]
    fn scene_part_must_match_the_script_id() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S02_L001.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(m.files.is_empty());
        assert_eq!(m.warnings.len(), 1);
    }

    #[test]
    fn uppercase_wav_extension_matches() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L001.WAV");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert_eq!(m.files[&LineId { scene: 1, line: 1 }].len(), 1);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn directories_and_subfolders_are_not_matched() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("S01_L001.wav")).unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        touch(&tmp.path().join("sub"), "S01_L002.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(m.files.is_empty());
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(match_voice_files(&tmp.path().join("nothing"), &ids(), false).is_err());
    }
}
