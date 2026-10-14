//! tracks/ の素材をカット ID で照合する(設計書 §4.3)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use s2v_timeline::VisualKind;

/// 素材の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Png,
    Jpeg,
    Video,
}

/// カットに対応づけた素材。
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub cut_id: String,
    pub path: PathBuf,
    pub kind: AssetKind,
}

/// 照合の結果。
#[derive(Debug, Default)]
pub struct AssetMatch {
    pub found: BTreeMap<String, Asset>,
    pub missing: Vec<String>,
    /// エラー 2 の理由(重複・種類違い・リンク・外側)
    pub errors: Vec<String>,
}

/// S01-C01 → "S01"、S01-C02 → "S01_C02"(桁は ID のまま)
pub fn asset_stem(cut_id: &str) -> String {
    if let Some((scene, cut)) = cut_id.split_once("-C") {
        if !cut.is_empty() && cut.bytes().all(|b| b.is_ascii_digit()) {
            let first = cut.trim_start_matches('0') == "1";
            return if first {
                scene.to_string()
            } else {
                format!("{scene}_C{cut}")
            };
        }
    }
    cut_id.replace('-', "_")
}

fn kind_of_ext(ext: &str) -> Option<AssetKind> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(AssetKind::Png),
        "jpg" | "jpeg" => Some(AssetKind::Jpeg),
        "mp4" | "mov" | "webm" => Some(AssetKind::Video),
        _ => None,
    }
}

/// cuts: (カット ID, #visual で明示された種類)。tracks_dir が無ければ全部 missing
pub fn match_assets(tracks_dir: &Path, cuts: &[(String, Option<VisualKind>)]) -> AssetMatch {
    let mut result = AssetMatch::default();
    // (ファイル名, 語幹の小文字, 種類)
    let mut entries: Vec<(String, String, AssetKind)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(tracks_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = Path::new(&name);
            let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) else {
                continue;
            };
            if let Some(kind) = kind_of_ext(&ext.to_string_lossy()) {
                entries.push((name.clone(), stem.to_string_lossy().to_lowercase(), kind));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let root = std::fs::canonicalize(tracks_dir).ok();

    for (cut_id, explicit) in cuts {
        let want = asset_stem(cut_id).to_lowercase();
        let hits: Vec<&(String, String, AssetKind)> =
            entries.iter().filter(|(_, s, _)| *s == want).collect();
        match hits.as_slice() {
            [] => result.missing.push(cut_id.clone()),
            [(name, _, kind)] => {
                let path = tracks_dir.join(name);
                match std::fs::symlink_metadata(&path) {
                    Ok(md) if md.file_type().is_symlink() => {
                        result.errors.push(format!(
                            "{cut_id}: 素材 {name} はリンクです(受け付けません)"
                        ));
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        result
                            .errors
                            .push(format!("{cut_id}: 素材 {name} を調べられません: {e}"));
                        continue;
                    }
                }
                let inside = match (&root, std::fs::canonicalize(&path)) {
                    (Some(r), Ok(c)) => c.starts_with(r),
                    _ => false,
                };
                if !inside {
                    result.errors.push(format!(
                        "{cut_id}: 素材 {name} は tracks フォルダの外にあります"
                    ));
                    continue;
                }
                let is_video = *kind == AssetKind::Video;
                match explicit {
                    Some(VisualKind::Video) if !is_video => {
                        result.errors.push(format!(
                            "{cut_id}: #visual は video ですが、素材 {name} は画像です"
                        ));
                        continue;
                    }
                    Some(VisualKind::Image) if is_video => {
                        result.errors.push(format!(
                            "{cut_id}: #visual は image ですが、素材 {name} は動画です"
                        ));
                        continue;
                    }
                    _ => {}
                }
                result.found.insert(
                    cut_id.clone(),
                    Asset {
                        cut_id: cut_id.clone(),
                        path,
                        kind: *kind,
                    },
                );
            }
            many => {
                let names: Vec<&str> = many.iter().map(|(n, _, _)| n.as_str()).collect();
                result.errors.push(format!(
                    "{cut_id}: 素材が複数あります({})",
                    names.join("、")
                ));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_timeline::VisualKind;

    fn dir(files: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in files {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        d
    }
    fn cuts(ids: &[(&str, Option<VisualKind>)]) -> Vec<(String, Option<VisualKind>)> {
        ids.iter().map(|(i, k)| (i.to_string(), *k)).collect()
    }

    #[test]
    fn stems() {
        assert_eq!(asset_stem("S01-C01"), "S01");
        assert_eq!(asset_stem("S01-C02"), "S01_C02");
        assert_eq!(asset_stem("S12-C0003"), "S12_C0003");
    }

    #[test]
    fn finds_images_and_videos_and_reports_missing() {
        let d = dir(&["S01.png", "S01_C02.mp4", "dialogue.wav", "S02.webp"]);
        let m = match_assets(
            d.path(),
            &cuts(&[
                ("S01-C01", None),
                ("S01-C02", Some(VisualKind::Video)),
                ("S02-C01", None),
            ]),
        );
        assert_eq!(m.found["S01-C01"].kind, AssetKind::Png);
        assert_eq!(m.found["S01-C02"].kind, AssetKind::Video);
        assert_eq!(m.missing, vec!["S02-C01".to_string()]); // WebP は受け付けない
        assert!(m.errors.is_empty(), "{:?}", m.errors);
    }

    #[test]
    fn stem_and_extension_match_case_insensitively() {
        // Review Focus 2
        let d = dir(&["s01.PNG", "S01_c02.JPEG"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None), ("S01-C02", None)]));
        assert_eq!(m.found["S01-C01"].kind, AssetKind::Png);
        assert_eq!(m.found["S01-C02"].kind, AssetKind::Jpeg);
    }

    #[test]
    fn two_assets_for_one_cut_is_an_error_naming_both() {
        // Review Focus 3
        let d = dir(&["S01.png", "S01.jpg"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.errors.len(), 1);
        assert!(
            m.errors[0].contains("S01.png") && m.errors[0].contains("S01.jpg"),
            "{:?}",
            m.errors
        );
        assert!(!m.found.contains_key("S01-C01"));
    }

    #[test]
    fn explicit_kind_mismatch_is_an_error() {
        let d = dir(&["S01.png"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", Some(VisualKind::Video))]));
        assert_eq!(m.errors.len(), 1, "{:?}", m.errors);
    }

    #[test]
    fn missing_tracks_dir_means_all_missing() {
        let d = tempfile::tempdir().unwrap();
        let m = match_assets(&d.path().join("無い"), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.missing, vec!["S01-C01".to_string()]);
    }

    #[test]
    fn symlinked_asset_is_rejected_when_links_can_be_made() {
        let d = dir(&[]);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x.png");
        std::fs::write(&target, b"x").unwrap();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, d.path().join("S01.png")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, d.path().join("S01.png")).is_ok();
        if !made {
            return; // リンクを作る権限が無い環境
        }
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.errors.len(), 1, "{:?}", m.errors);
    }
}
