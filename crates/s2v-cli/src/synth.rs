//! 合成工程の判定・記録・退避の計画(設計書 §3.3〜§3.5)。エンジンは呼ばない純粋部分。
//!
//! `plan` は副作用を持たない(ファイルを動かさず、書かない)。退避の実行と記録の保存は呼び出し側が行う。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Context;
use s2v_core::{LineId, ScriptItem, SynthSpec};
use s2v_engines::SynthRequest;
use serde::{Deserialize, Serialize};

use crate::work::sha256_hex;

/// 1行ぶんの合成ジョブ。
#[derive(Debug, Clone)]
pub struct SynthJob {
    pub id: LineId,
    /// 出力先 `<voice_dir>/S01_L003.wav`。
    pub out: PathBuf,
    /// エンジンへ送り、記録する文(`synth_text`、無ければ `text`)。
    pub text: String,
    pub engine: String,
    pub req: SynthRequest,
}

#[derive(Debug, Default)]
pub struct SynthPlan {
    pub jobs: Vec<SynthJob>,
    /// (元, 退避先)
    pub backups: Vec<(PathBuf, PathBuf)>,
    pub warnings: Vec<String>,
}

/// 合成記録の1行ぶん。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub engine: String,
    pub text: String,
    pub params_hash: String,
    pub file_sha256: String,
}

/// `voices/synth.json`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthRecord {
    pub version: u32,
    /// キーは台詞ID表示形 `S01-L003`。
    pub entries: BTreeMap<String, Entry>,
}

impl Default for SynthRecord {
    fn default() -> Self {
        SynthRecord {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

const RECORD_FILE: &str = "synth.json";

impl SynthRecord {
    /// 記録が無ければ空を返す。壊れていれば Err(作り直しは呼び出し側)。
    pub fn load(voice_dir: &Path) -> anyhow::Result<Self> {
        let path = voice_dir.join(RECORD_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(&path)
            .with_context(|| format!("{} を読めません", path.display()))?;
        serde_json::from_str(&s)
            .with_context(|| format!("{} の形式が正しくありません", path.display()))
    }

    /// 一時ファイルに書いてから rename で確定する。
    pub fn save(&self, voice_dir: &Path) -> anyhow::Result<()> {
        let tmp = voice_dir.join("synth.json.tmp");
        let written = serde_json::to_string_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(std::fs::write(&tmp, s)?))
            .and_then(|()| Ok(std::fs::rename(&tmp, voice_dir.join(RECORD_FILE))?));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    pub fn get(&self, id: LineId) -> Option<&Entry> {
        self.entries.get(&id.display())
    }

    pub fn set(&mut self, id: LineId, entry: Entry) {
        self.entries.insert(id.display(), entry);
    }
}

/// style / language / params / speaker_name のハッシュ(BTreeMap なので順序固定)。
pub fn params_hash(spec: &SynthSpec) -> String {
    let v = serde_json::json!({
        "speaker_name": spec.speaker_name,
        "style": spec.style,
        "language": spec.language,
        "params": spec.params,
    });
    sha256_hex(serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes())
}

/// 退避ファイル名 `S01_L003.20261028T153012.wav`。
pub fn backup_name(id: LineId, now: &str) -> String {
    format!("{}.{}.wav", id.file_stem(), now)
}

fn backup_path(voice_dir: &Path, id: LineId, now: &str) -> PathBuf {
    let dir = voice_dir.join("backup");
    let first = dir.join(backup_name(id, now));
    if !first.exists() {
        return first;
    }
    let mut n = 1;
    loop {
        let p = dir.join(format!("{}.{}.{}.wav", id.file_stem(), now, n));
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

pub fn plan(
    items: &[(LineId, &ScriptItem)],
    voice_dir: &Path,
    record: &SynthRecord,
    now: &str,
) -> anyhow::Result<SynthPlan> {
    let mut plan = SynthPlan::default();
    for (id, item) in items {
        let ScriptItem::Speech {
            id: item_id,
            line_no,
            text,
            synth,
            voice_rec,
            synth_text,
            ..
        } = item
        else {
            continue;
        };
        let Some(spec) = synth else { continue };
        if item_id.is_none() {
            plan.warnings.push(format!(
                "{}行目: 台詞IDがないため合成をスキップしました。先に number を実行してください",
                line_no
            ));
            continue;
        }
        let out = voice_dir.join(format!("{}.wav", id.file_stem()));
        let existing = if out.is_file() {
            let bytes = std::fs::read(&out)
                .with_context(|| format!("{} を読めません", out.display()))?;
            Some(sha256_hex(&bytes))
        } else {
            None
        };
        let entry = record.get(*id);

        if *voice_rec {
            if let (Some(h), Some(e)) = (&existing, entry) {
                if *h == e.file_sha256 {
                    plan.warnings.push(format!(
                        "{}: voice=rec ですが、合成の残りです({}.wav)。収録に差し替えるなら削除してください",
                        id.display(),
                        id.file_stem()
                    ));
                }
            }
            continue;
        }

        let send_text = synth_text.clone().unwrap_or_else(|| text.clone());
        let job = SynthJob {
            id: *id,
            out: out.clone(),
            text: send_text.clone(),
            engine: spec.engine.clone(),
            req: SynthRequest {
                speaker_name: spec.speaker_name.clone(),
                style: spec.style.clone(),
                language: spec.language.clone(),
                params: spec
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<_, _>>(),
            },
        };

        match (&existing, entry) {
            (None, _) => plan.jobs.push(job),
            (Some(h), Some(e)) if *h == e.file_sha256 => {
                if e.engine == spec.engine
                    && e.text == send_text
                    && e.params_hash == params_hash(spec)
                {
                    continue; // 最新
                }
                plan.warnings.push(format!(
                    "{}: 台詞またはパラメータが変わったため、{}.wav を退避して再合成します",
                    id.display(),
                    id.file_stem()
                ));
                plan.backups
                    .push((out.clone(), backup_path(voice_dir, *id, now)));
                plan.jobs.push(job);
            }
            (Some(_), _) => {
                plan.warnings.push(format!(
                    "{}: 合成記録にないファイルを退避して合成します({}.wav)",
                    id.display(),
                    id.file_stem()
                ));
                plan.backups
                    .push((out.clone(), backup_path(voice_dir, *id, now)));
                plan.jobs.push(job);
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::SceneConfig;
    use serde_json::json;
    use tempfile::TempDir;

    const NOW: &str = "20261028T153012";
    const ID: LineId = LineId { scene: 1, line: 3 };

    fn spec() -> SynthSpec {
        SynthSpec {
            engine: "voicevox".into(),
            speaker_name: "四国めたん".into(),
            style: "ノーマル".into(),
            language: "ja".into(),
            params: BTreeMap::from([("speed".to_string(), json!(1.0))]),
        }
    }

    fn speech(text: &str, synth: Option<SynthSpec>, voice_rec: bool, id: Option<LineId>) -> ScriptItem {
        ScriptItem::Speech {
            id,
            line_no: 7,
            cast_name: "A".into(),
            text: text.into(),
            offset_params: HashMap::new(),
            overlap_ms: None,
            at: None,
            synth,
            voice_rec,
            synth_text: None,
            scene_config: SceneConfig::new("室内"),
        }
    }

    fn wav(dir: &Path, bytes: &[u8]) -> PathBuf {
        let p = dir.join("S01_L003.wav");
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn record_for(text: &str, bytes: &[u8]) -> SynthRecord {
        let mut r = SynthRecord::default();
        r.set(
            ID,
            Entry {
                engine: "voicevox".into(),
                text: text.into(),
                params_hash: params_hash(&spec()),
                file_sha256: sha256_hex(bytes),
            },
        );
        r
    }

    fn run(item: &ScriptItem, dir: &Path, rec: &SynthRecord) -> SynthPlan {
        plan(&[(ID, item)], dir, rec, NOW).unwrap()
    }

    #[test]
    fn no_file_means_synthesize() {
        let d = TempDir::new().unwrap();
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.jobs[0].text, "こんにちは");
        assert_eq!(p.jobs[0].out, d.path().join("S01_L003.wav"));
        assert!(p.backups.is_empty());
    }

    #[test]
    fn matching_record_means_skip() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"RIFFaudio");
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert!(p.jobs.is_empty() && p.backups.is_empty());
    }

    #[test]
    fn changed_text_means_resynthesize_with_backup() {
        let d = TempDir::new().unwrap();
        let src = wav(d.path(), b"RIFFaudio");
        let item = speech("変えた", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(
            p.backups,
            vec![(src, d.path().join("backup").join("S01_L003.20261028T153012.wav"))]
        );
    }

    #[test]
    fn foreign_file_is_backed_up() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"human");
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.backups.len(), 1);
        assert!(p.warnings.iter().any(|w| w.contains("合成記録にないファイルを退避")));
    }

    #[test]
    fn file_hash_mismatch_is_backed_up() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"replaced");
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.backups.len(), 1);
        assert!(p.warnings.iter().any(|w| w.contains("合成記録にないファイルを退避")));
    }

    #[test]
    fn voice_rec_is_untouched() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"human");
        let item = speech("こんにちは", Some(spec()), true, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert!(p.jobs.is_empty() && p.backups.is_empty() && p.warnings.is_empty());
        // 記録はあるがハッシュ不一致 → 警告なし
        let p = run(&item, d.path(), &record_for("こんにちは", b"other"));
        assert!(p.warnings.is_empty());
        // ハッシュ一致 → 合成の残り
        let p = run(&item, d.path(), &record_for("こんにちは", b"human"));
        assert!(p.jobs.is_empty() && p.backups.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("合成の残りです")));
    }

    #[test]
    fn non_synth_role_is_untouched() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"human");
        let item = speech("こんにちは", None, false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert!(p.jobs.is_empty() && p.backups.is_empty() && p.warnings.is_empty());
    }

    #[test]
    fn id_less_speech_is_skipped_with_warning() {
        let d = TempDir::new().unwrap();
        let item = speech("こんにちは", Some(spec()), false, None);
        let p = run(&item, d.path(), &SynthRecord::default());
        assert!(p.jobs.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("number")));
    }

    #[test]
    fn synth_text_is_sent_and_recorded() {
        let d = TempDir::new().unwrap();
        let mut item = speech("漢字", Some(spec()), false, Some(ID));
        if let ScriptItem::Speech { synth_text, .. } = &mut item {
            *synth_text = Some("かんじ".into());
        }
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs[0].text, "かんじ");
    }

    #[test]
    fn backup_name_format() {
        assert_eq!(backup_name(ID, NOW), "S01_L003.20261028T153012.wav");
    }

    #[test]
    fn same_second_backup_gets_suffix() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"human");
        let b = d.path().join("backup");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("S01_L003.20261028T153012.wav"), b"x").unwrap();
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.backups[0].1, b.join("S01_L003.20261028T153012.1.wav"));
        std::fs::write(b.join("S01_L003.20261028T153012.1.wav"), b"x").unwrap();
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.backups[0].1, b.join("S01_L003.20261028T153012.2.wav"));
    }

    #[test]
    fn record_roundtrip() {
        let d = TempDir::new().unwrap();
        let r = record_for("こんにちは", b"a");
        r.save(d.path()).unwrap();
        assert_eq!(SynthRecord::load(d.path()).unwrap(), r);
        assert_eq!(SynthRecord::load(TempDir::new().unwrap().path()).unwrap(), SynthRecord::default());
        std::fs::write(d.path().join("synth.json"), "{broken").unwrap();
        assert!(SynthRecord::load(d.path()).is_err());
    }
}
