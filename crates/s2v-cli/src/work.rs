//! 出力フォルダ `work/` の中間ファイルとキャッシュ(設計書 5.10 D)。
//! - `resampled/<台詞ID語幹>.wav`: 48kHz モノラル・レベルを変えない変換結果
//! - `lines/<台詞ID>.wav`: 正規化・フェード後の台詞
//! - `lines_prefade/<台詞ID>.wav`: フェード前(`dump_prefade = true` のとき)
//! - `cache.json`: キー・出力のバイト数と SHA-256・正規化の記録

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 変換処理のバージョン(処理を変えたら上げる)。
pub const CONVERT_VERSION: u32 = 1;
/// 正規化処理のバージョン(測定方式・400ms・フェードを含む。変えたら上げる)。
pub const NORMALIZE_VERSION: u32 = 1;

const SUBDIRS: [&str; 3] = ["resampled", "lines", "lines_prefade"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormMeta {
    pub measured_lufs: f64,
    pub requested_gain_db: f64,
    pub norm_gain_db: f64,
    pub norm_clamped: bool,
    /// 測定範囲(収録ファイル上のサンプル位置)
    pub measure_start: u64,
    pub measure_end: u64,
    /// 使用範囲(収録ファイル上のサンプル位置)
    pub src_start: u64,
    pub src_end: u64,
    /// キャッシュの命中時にも同じ警告を出すための記録
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub norm: Option<NormMeta>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: BTreeMap<String, CacheEntry>,
}

pub struct WorkDir {
    root: PathBuf,
    cache: CacheFile,
    no_cache: bool,
    used: HashSet<String>,
}

impl WorkDir {
    /// `root`(出力フォルダの `work/`)を開く。書き込めなければ Err(→ 終了コード 2)。
    pub fn open(root: &Path, no_cache: bool) -> anyhow::Result<(WorkDir, Option<String>)> {
        for sub in SUBDIRS {
            std::fs::create_dir_all(root.join(sub))?;
        }
        let (entries, note) = match std::fs::read_to_string(root.join("cache.json")) {
            Ok(s) => match serde_json::from_str::<CacheFile>(&s) {
                Ok(c) if c.version == 1 => (c.entries, None),
                _ => (
                    BTreeMap::new(),
                    Some(
                        "cache.json が壊れているため破棄し、中間ファイルを作り直します".to_string(),
                    ),
                ),
            },
            Err(_) => (BTreeMap::new(), None),
        };
        Ok((
            WorkDir {
                root: root.to_path_buf(),
                cache: CacheFile {
                    version: 1,
                    entries,
                },
                no_cache,
                used: HashSet::new(),
            },
            note,
        ))
    }

    /// キーが一致し、出力ファイルのバイト数と SHA-256 が一致すれば記録を返す(命中したファイルは使用中にする)。
    pub fn lookup(&mut self, rel: &str, key: &str) -> Option<CacheEntry> {
        if self.no_cache {
            return None;
        }
        check_rel(rel).ok()?;
        let e = self.cache.entries.get(rel)?;
        if e.key != key {
            return None;
        }
        let bytes = std::fs::read(self.root.join(rel)).ok()?;
        if bytes.len() as u64 != e.bytes || sha256_hex(&bytes) != e.sha256 {
            return None;
        }
        self.used.insert(rel.to_string());
        Some(e.clone())
    }

    /// モノラル 32bit 浮動小数で書き、記録を更新する(一時ファイルから rename で確定)。
    pub fn store(
        &mut self,
        rel: &str,
        key: &str,
        samples: &[f32],
        norm: Option<NormMeta>,
    ) -> anyhow::Result<()> {
        check_rel(rel)?;
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("wav.tmp");
        // 同名の既存ファイルは rename が置き換える(Windows の std::fs::rename も置換する)。
        let written =
            write_mono_f32(&tmp, samples).and_then(|()| Ok(std::fs::rename(&tmp, &path)?));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        let bytes = std::fs::read(&path)?;
        self.cache.entries.insert(
            rel.to_string(),
            CacheEntry {
                key: key.to_string(),
                bytes: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
                norm,
            },
        );
        self.used.insert(rel.to_string());
        self.save()
    }

    pub fn read_mono(&self, rel: &str) -> anyhow::Result<Vec<f32>> {
        check_rel(rel)?;
        let mut r = hound::WavReader::open(self.root.join(rel))?;
        Ok(r.samples::<f32>().collect::<Result<_, _>>()?)
    }

    /// 不足になった台詞のファイルを、使用中から外す(`finish` で削除される)。
    pub fn release(&mut self, rel: &str) {
        self.used.remove(rel);
    }

    /// 今回使わなかった中間ファイルと記録を削除する。
    pub fn finish(mut self) -> anyhow::Result<()> {
        for sub in SUBDIRS {
            let dir = match std::fs::read_dir(self.root.join(sub)) {
                Ok(d) => d,
                // 誰かがサブフォルダを消していても、消すものが無いだけで失敗にしない
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in dir {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let rel = format!("{sub}/{}", entry.file_name().to_string_lossy());
                if !self.used.contains(&rel) {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        let used = &self.used;
        self.cache.entries.retain(|k, _| used.contains(k));
        self.save()
    }

    fn save(&self) -> anyhow::Result<()> {
        let tmp = self.root.join("cache.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.cache)?)?;
        std::fs::rename(&tmp, self.root.join("cache.json"))?;
        Ok(())
    }
}

/// `rel` が `work/` の中に収まる相対パス(通常の名前の連なりだけ)であることを確かめる。
fn check_rel(rel: &str) -> anyhow::Result<()> {
    let p = Path::new(rel);
    let ok = p.components().next().is_some()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        && !rel.starts_with(['/', '\\']);
    if ok {
        Ok(())
    } else {
        anyhow::bail!("work/ の外を指す相対パスは使えません: {rel}")
    }
}

/// `cache.json` の記録を読む(完了条件の確認ツール用)。
pub fn load_cache(root: &Path) -> anyhow::Result<BTreeMap<String, CacheEntry>> {
    let s = std::fs::read_to_string(root.join("cache.json"))?;
    Ok(serde_json::from_str::<CacheFile>(&s)?.entries)
}

fn write_mono_f32(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// `resampled/` のキー: 収録ファイルの内容 + 変換処理のバージョン + 48kHz。
pub fn resampled_key(src_sha: &str) -> String {
    sha256_hex(format!("resampled|v{CONVERT_VERSION}|48000|{src_sha}").as_bytes())
}

#[derive(Debug, Clone, Copy)]
pub struct LineKeyParams {
    pub src_start: u64,
    pub src_end: u64,
    pub trim_threshold_db: f64,
    pub target_lufs: f64,
    pub max_boost_db: f64,
    pub max_cut_db: f64,
    pub prefade: bool,
}

/// `lines/` のキー: `resampled/` のキー + 使用範囲 + 測定範囲を決める設定 + 正規化の設定 + 処理のバージョン。
pub fn line_key(resampled_key: &str, p: &LineKeyParams) -> String {
    sha256_hex(
        format!(
            "lines|v{NORMALIZE_VERSION}|{resampled_key}|{}|{}|{}|{}|{}|{}|{}",
            p.src_start,
            p.src_end,
            p.trim_threshold_db,
            p.target_lufs,
            p.max_boost_db,
            p.max_cut_db,
            p.prefade
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> NormMeta {
        NormMeta {
            measured_lufs: -30.0,
            requested_gain_db: 7.0,
            norm_gain_db: 7.0,
            norm_clamped: false,
            measure_start: 10,
            measure_end: 20,
            src_start: 5,
            src_end: 25,
            warnings: vec!["注意".into()],
        }
    }

    #[test]
    fn store_then_lookup_hits_and_keeps_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, note) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(note.is_none());
        w.store("lines/S01-L001.wav", "k1", &[0.1, 0.2], Some(meta()))
            .unwrap();
        let (mut w2, _) = WorkDir::open(tmp.path(), false).unwrap();
        let e = w2.lookup("lines/S01-L001.wav", "k1").unwrap();
        assert_eq!(e.norm, Some(meta()));
        assert_eq!(w2.read_mono("lines/S01-L001.wav").unwrap(), vec![0.1, 0.2]);
    }

    #[test]
    fn key_mismatch_changed_file_and_no_cache_all_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        w.store("resampled/S01_L001.wav", "k1", &[0.5], None)
            .unwrap();
        assert!(w.lookup("resampled/S01_L001.wav", "k2").is_none());
        std::fs::write(tmp.path().join("resampled/S01_L001.wav"), b"broken").unwrap();
        assert!(w.lookup("resampled/S01_L001.wav", "k1").is_none());
        let (mut nc, _) = WorkDir::open(tmp.path(), true).unwrap();
        nc.store("resampled/S01_L002.wav", "k", &[0.5], None)
            .unwrap();
        assert!(nc.lookup("resampled/S01_L002.wav", "k").is_none());
    }

    #[test]
    fn corrupt_cache_json_is_discarded_with_a_note() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cache.json"), "{ 壊れた").unwrap();
        let (mut w, note) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(note.unwrap().contains("cache.json"));
        assert!(w.lookup("lines/x.wav", "k").is_none());
    }

    #[test]
    fn finish_deletes_unused_and_released_files() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
            w.store("lines/S01-L001.wav", "a", &[0.1], None).unwrap();
            w.store("lines/S01-L002.wav", "b", &[0.1], None).unwrap();
            w.store("resampled/S01_L002.wav", "c", &[0.1], None)
                .unwrap();
            w.finish().unwrap();
        }
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(w.lookup("lines/S01-L001.wav", "a").is_some());
        w.store("resampled/S01_L002.wav", "c", &[0.1], None)
            .unwrap();
        w.release("resampled/S01_L002.wav");
        w.finish().unwrap();
        assert!(tmp.path().join("lines/S01-L001.wav").exists());
        assert!(!tmp.path().join("lines/S01-L002.wav").exists());
        assert!(!tmp.path().join("resampled/S01_L002.wav").exists());
        let entries = load_cache(tmp.path()).unwrap();
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            vec!["lines/S01-L001.wav"]
        );
    }

    #[test]
    fn keys_change_with_every_parameter() {
        let base = LineKeyParams {
            src_start: 1,
            src_end: 2,
            trim_threshold_db: -50.0,
            target_lufs: -23.0,
            max_boost_db: 24.0,
            max_cut_db: 24.0,
            prefade: false,
        };
        let k = line_key("r", &base);
        assert_ne!(k, line_key("r2", &base));
        assert_ne!(k, line_key("r", &LineKeyParams { src_end: 3, ..base }));
        assert_ne!(
            k,
            line_key(
                "r",
                &LineKeyParams {
                    target_lufs: -20.0,
                    ..base
                }
            )
        );
        assert_ne!(
            k,
            line_key(
                "r",
                &LineKeyParams {
                    prefade: true,
                    ..base
                }
            )
        );
        assert_ne!(resampled_key("a"), resampled_key("b"));
    }

    #[test]
    fn store_twice_to_same_path_replaces_content() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        w.store("lines/a.wav", "k1", &[0.1, 0.2], None).unwrap();
        w.store("lines/a.wav", "k2", &[0.3], None).unwrap();
        assert_eq!(w.read_mono("lines/a.wav").unwrap(), vec![0.3]);
        assert!(w.lookup("lines/a.wav", "k1").is_none());
        assert!(w.lookup("lines/a.wav", "k2").is_some());
        assert!(!tmp.path().join("lines/a.wav.tmp").exists());
    }

    #[test]
    fn leftover_tmp_file_is_removed_by_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        w.store("lines/a.wav", "k", &[0.1], None).unwrap();
        std::fs::write(tmp.path().join("lines/b.wav.tmp"), b"half").unwrap();
        std::fs::write(tmp.path().join("resampled/c.wav.tmp"), b"half").unwrap();
        assert!(w.lookup("lines/b.wav.tmp", "k").is_none());
        w.finish().unwrap();
        assert!(tmp.path().join("lines/a.wav").exists());
        assert!(!tmp.path().join("lines/b.wav.tmp").exists());
        assert!(!tmp.path().join("resampled/c.wav.tmp").exists());
    }

    #[test]
    fn stale_cache_entries_miss_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let json = r#"{"version":1,"entries":{
            "lines/gone.wav":{"key":"k","bytes":10,"sha256":"00"},
            "lines/bad.wav":{"key":"k","bytes":6,"sha256":"deadbeef"}}}"#;
        std::fs::create_dir_all(tmp.path().join("lines")).unwrap();
        std::fs::write(tmp.path().join("lines/bad.wav"), b"broken").unwrap();
        std::fs::write(tmp.path().join("cache.json"), json).unwrap();
        let (mut w, note) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(note.is_none());
        assert!(w.lookup("lines/gone.wav", "k").is_none());
        assert!(w.lookup("lines/bad.wav", "k").is_none());
        w.release("lines/gone.wav");
        w.release("never/seen.wav");
        w.finish().unwrap();
        assert!(!tmp.path().join("lines/bad.wav").exists());
        assert!(load_cache(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn miss_does_not_mark_file_as_used() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
            w.store("lines/a.wav", "k", &[0.1], None).unwrap();
        }
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        assert!(w.lookup("lines/a.wav", "other").is_none());
        w.finish().unwrap();
        assert!(!tmp.path().join("lines/a.wav").exists());
    }

    #[test]
    fn finish_tolerates_missing_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        w.store("resampled/a.wav", "k", &[0.1], None).unwrap();
        std::fs::remove_dir_all(tmp.path().join("lines")).unwrap();
        w.finish().unwrap();
        assert!(tmp.path().join("resampled/a.wav").exists());
    }

    #[test]
    fn relative_paths_outside_work_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = WorkDir::open(tmp.path(), false).unwrap();
        let abs = tmp.path().join("lines/x.wav").to_string_lossy().to_string();
        for bad in [
            "../x.wav",
            "lines/../../x.wav",
            "",
            "C:/x.wav",
            "C:x.wav",
            "//srv/share/x.wav",
            "/x.wav",
            "\\\\x.wav",
            "\\x.wav",
            abs.as_str(),
        ] {
            assert!(w.store(bad, "k", &[0.1], None).is_err(), "store {bad}");
            assert!(w.read_mono(bad).is_err(), "read_mono {bad}");
            assert!(w.lookup(bad, "k").is_none(), "lookup {bad}");
        }
        assert!(!tmp.path().parent().unwrap().join("x.wav").exists());
    }
}
