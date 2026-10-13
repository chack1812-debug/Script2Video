//! 収録ファイルの照合(台本仕様 §3、設計書 5.0)。①a は単独ファイル(`S01_L003.wav`)だけを扱う。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;

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
    /// 範囲の書式が不正(`S01_L003-` / 始点が終点より後ろ など)
    InvalidRange,
    NotVoice,
}

fn classify(stem: &str) -> NameKind {
    if let Some(id) = LineId::parse_file_stem(stem) {
        return NameKind::Single(id);
    }
    if let Some(rest) = voice_prefix_rest(stem) {
        if let Some((start, end)) = rest.split_once("-L") {
            if let (Some(a), Some(b)) = (parse_digits(start), parse_digits(end)) {
                if a <= b {
                    return NameKind::Range;
                }
            }
        }
        if rest.contains('-') {
            return NameKind::InvalidRange;
        }
    }
    NameKind::NotVoice
}

/// `S<数字>_L` で始まる stem の、その後ろの部分。
fn voice_prefix_rest(stem: &str) -> Option<&str> {
    let after_s = stem.strip_prefix('S')?;
    let digits = after_s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    after_s[digits..].strip_prefix("_L")
}

/// 空でない ASCII 数字だけの文字列を数値にする。
fn parse_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `dir` 直下の収録ファイルを台詞IDに対応付ける。サブフォルダは見ない。
pub fn match_voice_files(
    dir: &Path,
    script_ids: &[LineId],
    allow_flac: bool,
) -> anyhow::Result<VoiceMatch> {
    let known: HashSet<LineId> = script_ids.iter().copied().collect();
    let mut entries: Vec<PathBuf> = Vec::new();
    let read = std::fs::read_dir(dir)
        .with_context(|| format!("収録フォルダを読めません: {}", dir.display()))?;
    for entry in read {
        let entry =
            entry.with_context(|| format!("収録フォルダの項目を読めません: {}", dir.display()))?;
        let path = entry.path();
        // シンボリックリンクは辿る。壊れたリンクなど取得できない場合は Err にする
        let meta = std::fs::metadata(&path)
            .with_context(|| format!("収録フォルダの項目を調べられません: {}", path.display()))?;
        if meta.is_file() {
            entries.push(path);
        }
    }
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
            NameKind::InvalidRange => m.warnings.push(format!(
                "範囲を示すファイル名の書式が不正です（始点が終点より後ろ、または形式の誤り。無視します）: {name}"
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
        let missing = tmp.path().join("nothing");
        let err = match_voice_files(&missing, &ids(), false).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("nothing"), "ディレクトリ名がない: {msg}");
        let io_reason = std::fs::read_dir(&missing).unwrap_err().to_string();
        assert!(msg.contains(&io_reason), "原因がない: {msg}");
    }

    #[test]
    fn classify_pins_each_class() {
        let l = |s, n| LineId { scene: s, line: n };
        assert_eq!(classify("S01_L003"), NameKind::Single(l(1, 3)));
        assert_eq!(classify("S01_L003-L007"), NameKind::Range);
        assert_eq!(classify("S01_L003-L003"), NameKind::Range);
        for s in ["take-1", "S1-take", "memo", "S01-L003"] {
            assert_eq!(classify(s), NameKind::NotVoice, "{s}");
        }
        for s in [
            "S01_L003-",
            "S01_L-3",
            "S01_L003-L",
            "S01_L004-L003",
            "S01_L003-L007-L009",
        ] {
            assert_eq!(classify(s), NameKind::InvalidRange, "{s}");
        }
    }

    #[test]
    fn invalid_and_reversed_range_names_warn_distinctly() {
        for name in ["S01_L003-.wav", "S01_L-3.wav", "S01_L004-L003.wav"] {
            let tmp = tempfile::tempdir().unwrap();
            touch(tmp.path(), name);
            let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
            assert!(m.files.is_empty(), "{name}");
            assert_eq!(m.warnings.len(), 1, "{name}: {:?}", m.warnings);
            let w = &m.warnings[0];
            assert!(
                w.contains(name) && w.contains("不正") && w.contains("無視"),
                "{w}"
            );
            assert!(!w.contains("未対応"), "{w}");
        }
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L004-L003.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert!(
            m.warnings[0].contains("始点が終点より後ろ"),
            "{:?}",
            m.warnings
        );
    }

    #[test]
    fn non_voice_hyphen_names_are_not_range_warnings() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "take-1.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        assert_eq!(m.warnings.len(), 1);
        assert!(
            m.warnings[0].contains("台詞IDの形式でない"),
            "{:?}",
            m.warnings
        );
    }

    #[test]
    fn duplicates_keep_all_files_sorted_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L001.wav");
        touch(tmp.path(), "S01_L001.flac");
        touch(tmp.path(), "S01_L002.WAV");
        let m = match_voice_files(tmp.path(), &ids(), true).unwrap();
        assert_eq!(
            m.files[&LineId { scene: 1, line: 1 }],
            vec![
                tmp.path().join("S01_L001.flac"),
                tmp.path().join("S01_L001.wav")
            ]
        );
        assert_eq!(
            m.files[&LineId { scene: 1, line: 2 }],
            vec![tmp.path().join("S01_L002.WAV")]
        );
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn files_map_is_ordered_by_id_regardless_of_creation_order() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "S01_L003.wav");
        touch(tmp.path(), "S01_L001.wav");
        touch(tmp.path(), "S01_L002.wav");
        let m = match_voice_files(tmp.path(), &ids(), false).unwrap();
        let keys: Vec<u32> = m.files.keys().map(|k| k.line).collect();
        assert_eq!(keys, vec![1, 2, 3]);
    }
}
