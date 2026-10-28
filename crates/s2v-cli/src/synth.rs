//! 合成工程の判定・記録・退避の計画(設計書 §3.3〜§3.5)。エンジンは呼ばない純粋部分。
//!
//! `plan` は副作用を持たない(ファイルを動かさず、書かない)。退避の実行・合成・記録の保存は `run` が行う。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use s2v_audio::input::WORK_SAMPLE_RATE;
use s2v_core::voices::match_voice_files;
use s2v_core::{LineId, ScriptItem, SynthSpec};
use s2v_engines::{EngineManager, SynthRequest};
use serde::{Deserialize, Serialize};

use crate::proclog::ProcLog;
use crate::work::{sha256_hex, write_mono_f32};

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
    /// 記録する設定のハッシュ(`params_hash`)。
    pub params_hash: String,
    /// 合成の直前に退避する既存ファイル(`out` と、同じ台詞の別の綴りのファイル)。退避先は `SynthPlan::backups`。
    pub replaces: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct SynthPlan {
    pub jobs: Vec<SynthJob>,
    /// (元, 退避先)。ジョブの `replaces` に入っていないものは、合成と関係なく退避する。
    pub backups: Vec<(PathBuf, PathBuf)>,
    pub warnings: Vec<String>,
    /// 記録と一致したため合成しない台詞の数。
    pub skipped: usize,
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
    sha256_hex(
        serde_json::to_string_pretty(&v)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// 退避ファイル名。元の名前の綴りと拡張子を保ち、拡張子の前に日時を入れる
/// (`S01_L003.wav` → `S01_L003.20261028T153012.wav`、`n > 0` なら `S01_L003.20261028T153012.1.wav`)。
pub fn backup_name(file_name: &str, now: &str, n: usize) -> String {
    let (stem, ext) = file_name.rsplit_once('.').unwrap_or((file_name, "wav"));
    if n == 0 {
        format!("{stem}.{now}.{ext}")
    } else {
        format!("{stem}.{now}.{n}.{ext}")
    }
}

/// `from` の退避先。既にあるファイルと、同じ計画で選んだ退避先(大文字小文字を区別しない)を避ける。
fn backup_path(voice_dir: &Path, from: &Path, now: &str, taken: &mut HashSet<String>) -> PathBuf {
    let dir = voice_dir.join("backup");
    let name = file_label(from);
    let mut n = 0;
    loop {
        let candidate = backup_name(&name, now, n);
        let p = dir.join(&candidate);
        if !p.exists() && taken.insert(candidate.to_lowercase()) {
            return p;
        }
        n += 1;
    }
}

/// 合成の計画を立てる。`allow_flac` は照合(`match_voice_files`)と同じ値を渡す。
/// 照合が同じ台詞とみなすファイル(`S01_L3.wav` などの別の綴り、許可された `.flac`)も、
/// 合成する台詞では退避の対象にする(残すと照合で重複になるため)。
pub fn plan(
    items: &[(LineId, &ScriptItem)],
    voice_dir: &Path,
    record: &SynthRecord,
    now: &str,
    allow_flac: bool,
) -> anyhow::Result<SynthPlan> {
    let mut plan = SynthPlan::default();
    let ids: Vec<LineId> = items.iter().map(|(id, _)| *id).collect();
    let found = if voice_dir.is_dir() {
        match_voice_files(voice_dir, &ids, allow_flac)?.files
    } else {
        BTreeMap::new()
    };
    let mut taken: HashSet<String> = HashSet::new();
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
        // 照合がこの台詞に対応付けるファイル。合成の出力と同じ名前のもの以外は「別の綴り」
        let files = found.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let has_out = files.iter().any(|f| f.file_name() == out.file_name());
        let others: Vec<&PathBuf> = files
            .iter()
            .filter(|f| f.file_name() != out.file_name())
            .collect();
        let existing = if has_out {
            let bytes =
                std::fs::read(&out).with_context(|| format!("{} を読めません", out.display()))?;
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
        let mut replaces: Vec<PathBuf> = Vec::new();
        match (&existing, entry) {
            (None, _) => {}
            (Some(h), Some(e)) if *h == e.file_sha256 => {
                if e.engine == spec.engine
                    && e.text == send_text
                    && e.params_hash == params_hash(spec)
                {
                    // 最新。別の綴りのファイルがあれば、それだけを退避して合成済みのファイルを使う
                    plan.skipped += 1;
                    for o in others {
                        plan.warnings.push(format!(
                            "{}: 同じ台詞の別の名前のファイルを退避します({})。合成済みの {}.wav を使います",
                            id.display(),
                            file_label(o),
                            id.file_stem()
                        ));
                        plan.backups
                            .push((o.clone(), backup_path(voice_dir, o, now, &mut taken)));
                    }
                    continue;
                }
                plan.warnings.push(format!(
                    "{}: 台詞またはパラメータが変わったため、{}.wav を退避して再合成します",
                    id.display(),
                    id.file_stem()
                ));
                replaces.push(out.clone());
            }
            (Some(_), Some(_)) => {
                // 記録はあるがファイルの中身が違う(人が差し替えた肉声など)
                plan.warnings.push(format!(
                    "{}: 合成記録と中身が違うファイルを退避して合成します({}.wav)",
                    id.display(),
                    id.file_stem()
                ));
                replaces.push(out.clone());
            }
            (Some(_), None) => {
                plan.warnings.push(format!(
                    "{}: 合成記録にないファイルを退避して合成します({}.wav)",
                    id.display(),
                    id.file_stem()
                ));
                replaces.push(out.clone());
            }
        }
        for o in others {
            plan.warnings.push(format!(
                "{}: 同じ台詞の別の名前のファイルを退避して合成します({})",
                id.display(),
                file_label(o)
            ));
            replaces.push(o.clone());
        }
        for f in &replaces {
            plan.backups
                .push((f.clone(), backup_path(voice_dir, f, now, &mut taken)));
        }
        plan.jobs.push(SynthJob {
            id: *id,
            out,
            text: send_text,
            engine: spec.engine.clone(),
            params_hash: params_hash(spec),
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
            replaces,
        });
    }
    Ok(plan)
}

/// 合成と関係なく行う退避(合成済みの台詞にある別の綴りのファイル)を実行する。
/// 退避できなければ警告する(そのファイルは残り、照合で重複になる)。
pub fn move_aside(backups: &[(PathBuf, PathBuf)], log: &mut ProcLog) {
    for (from, to) in backups {
        if let Err(e) = move_file(from, to) {
            log.warn(format!("{e:#}"));
        }
    }
}

/// `from` を `to` へ移す(退避先のフォルダが無ければ作る)。
fn move_file(from: &Path, to: &Path) -> anyhow::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("退避先を作れません: {}", dir.display()))?;
    }
    std::fs::rename(from, to)
        .with_context(|| format!("{} を {} へ退避できません", from.display(), to.display()))
}

/// `voices/synth.json` を読む。壊れていれば破棄して空から作り直し、警告する。
pub fn load_record(voice_dir: &Path, log: &mut ProcLog) -> SynthRecord {
    match SynthRecord::load(voice_dir) {
        Ok(r) => r,
        Err(e) => {
            log.warn(format!(
                "合成記録を破棄して作り直します(合成役の既存ファイルは退避して再合成されます): {e:#}"
            ));
            SynthRecord::default()
        }
    }
}

/// 合成の結果。
#[derive(Debug, Default)]
pub struct SynthOutcome {
    /// 合成に失敗した台詞(エンジンを起動できなかった台詞を含む)
    pub failed: HashSet<LineId>,
    pub synthesized: usize,
    /// 記録と一致したため合成しなかった台詞の数
    pub skipped: usize,
}

/// 計画を実行する(設計書 §3.3〜§3.5)。エンジンの失敗では止まらず、その台詞を `failed` に入れる。
/// 記録は、ジョブが1つ以上あれば(すべて失敗しても)保存する。ジョブが無ければ保存しない。
pub fn run(
    plan: SynthPlan,
    record: &mut SynthRecord,
    manager: &EngineManager,
    voice_dir: &Path,
    log: &mut ProcLog,
) -> SynthOutcome {
    let mut outcome = SynthOutcome {
        skipped: plan.skipped,
        ..SynthOutcome::default()
    };
    let in_jobs: HashSet<&Path> = plan
        .jobs
        .iter()
        .flat_map(|j| j.replaces.iter().map(PathBuf::as_path))
        .collect();
    let standalone: Vec<(PathBuf, PathBuf)> = plan
        .backups
        .iter()
        .filter(|(from, _)| !in_jobs.contains(from.as_path()))
        .cloned()
        .collect();
    move_aside(&standalone, log);
    if plan.jobs.is_empty() {
        return outcome;
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log.error(format!("音声合成の実行環境を用意できません: {e}"));
            outcome.failed.extend(plan.jobs.iter().map(|j| j.id));
            return outcome;
        }
    };

    // 必要なエンジンだけ起動する。起動できないエンジンの台詞は合成しない
    let needed: BTreeSet<&str> = plan.jobs.iter().map(|j| j.engine.as_str()).collect();
    let mut down: HashSet<&str> = HashSet::new();
    for name in needed {
        let result = match manager.get(name) {
            Some(engine) => rt.block_on(engine.activate()),
            None => Err(anyhow::anyhow!("エンジンが登録されていません")),
        };
        if let Err(e) = result {
            log.error(format!("音声合成エンジン {name} を使えません: {e:#}"));
            down.insert(name);
        }
    }

    let backups: HashMap<&Path, &Path> = plan
        .backups
        .iter()
        .map(|(from, to)| (from.as_path(), to.as_path()))
        .collect();
    for job in &plan.jobs {
        if down.contains(job.engine.as_str()) {
            log.error(format!(
                "{}: エンジン {} を使えないため合成できません",
                job.id.display(),
                job.engine
            ));
            outcome.failed.insert(job.id);
            continue;
        }
        let job_backups: Vec<(&Path, &Path)> = job
            .replaces
            .iter()
            .filter_map(|f| backups.get(f.as_path()).map(|to| (f.as_path(), *to)))
            .collect();
        match synthesize_one(&rt, manager, job, &job_backups) {
            Ok(file_sha256) => {
                record.set(
                    job.id,
                    Entry {
                        engine: job.engine.clone(),
                        text: job.text.clone(),
                        params_hash: job.params_hash.clone(),
                        file_sha256,
                    },
                );
                outcome.synthesized += 1;
            }
            Err(e) => {
                log.error(format!("{}: 合成に失敗しました: {e:#}", job.id.display()));
                outcome.failed.insert(job.id);
            }
        }
    }
    if let Err(e) = record.save(voice_dir) {
        log.warn(format!("合成記録を保存できません: {e:#}"));
    }
    outcome
}

/// 1台詞を合成して `job.out` に書き、そのファイルの SHA-256 を返す。
/// 既存ファイル(`(元, 退避先)`)は合成の直前に退避し、失敗したら元の場所へ戻す。
fn synthesize_one(
    rt: &tokio::runtime::Runtime,
    manager: &EngineManager,
    job: &SynthJob,
    backups: &[(&Path, &Path)],
) -> anyhow::Result<String> {
    for (i, &(from, to)) in backups.iter().enumerate() {
        if let Err(e) = move_file(from, to) {
            return Err(restore(&backups[..i], e));
        }
    }
    let tmp = job.out.with_extension("synth.tmp");
    let result = (|| -> anyhow::Result<String> {
        rt.block_on(manager.synthesize(&job.engine, &job.text, &job.req, &tmp))?;
        ensure_mono_48k(&tmp)?;
        std::fs::rename(&tmp, &job.out)
            .with_context(|| format!("{} に書けません", job.out.display()))?;
        let bytes = std::fs::read(&job.out)
            .with_context(|| format!("{} を読み直せません", job.out.display()))?;
        Ok(sha256_hex(&bytes))
    })();
    let Err(e) = result else { return result };
    let _ = std::fs::remove_file(&tmp);
    Err(restore(backups, e))
}

/// 退避したファイルを元の場所へ戻し、その結果をエラーに書き添える。
fn restore(moved: &[(&Path, &Path)], mut e: anyhow::Error) -> anyhow::Error {
    for &(from, to) in moved {
        e = match std::fs::rename(to, from) {
            Ok(()) => e.context(format!(
                "退避していた {} を元の場所に戻しました",
                file_label(from)
            )),
            Err(re) => e.context(format!(
                "退避した {} を元の場所に戻せません({re})。手で戻してください",
                to.display()
            )),
        };
    }
    e
}

fn file_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// エンジンの出力を、取り込みが受け付ける 48kHz モノラル WAV にする
/// (取り込みはモノラル以外を形式エラーにするため。すでにそうなら触らない)。
fn ensure_mono_48k(path: &Path) -> anyhow::Result<()> {
    use hound::SampleFormat::{Float, Int};
    let mut reader =
        hound::WavReader::open(path).context("エンジンの出力を WAV として読めません")?;
    let spec = reader.spec();
    let accepted = matches!(
        (spec.sample_format, spec.bits_per_sample),
        (Int, 16 | 24 | 32) | (Float, 32)
    );
    if spec.channels == 1 && spec.sample_rate == WORK_SAMPLE_RATE && accepted {
        return Ok(());
    }
    if spec.channels == 0 || spec.sample_rate == 0 {
        anyhow::bail!(
            "エンジンの出力の形式が不正です(チャンネル数 {}、{} Hz)",
            spec.channels,
            spec.sample_rate
        );
    }
    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (Int, bits @ 8..=32) => {
            let max = (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()?
        }
        (Float, 32) => reader.samples::<f32>().collect::<Result<_, _>>()?,
        (f, b) => anyhow::bail!("エンジンの出力が対応していない形式です({f:?} {b}bit)"),
    };
    drop(reader);
    let ch = usize::from(spec.channels);
    let mono: Vec<f32> = interleaved
        .chunks_exact(ch)
        .map(|c| c.iter().sum::<f32>() / ch as f32)
        .collect();
    let out = if spec.sample_rate == WORK_SAMPLE_RATE {
        mono
    } else {
        s2v_audio::resample_mono(&mono, spec.sample_rate, WORK_SAMPLE_RATE)
            .context("エンジンの出力を 48kHz に変換できません")?
    };
    if out.is_empty() {
        anyhow::bail!("エンジンの出力に音声がありません");
    }
    write_mono_f32(path, &out).context("変換したエンジンの出力を書けません")
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

    fn speech(
        text: &str,
        synth: Option<SynthSpec>,
        voice_rec: bool,
        id: Option<LineId>,
    ) -> ScriptItem {
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
        plan(&[(ID, item)], dir, rec, NOW, false).unwrap()
    }

    #[test]
    fn other_spelling_file_is_backed_up_too() {
        let d = TempDir::new().unwrap();
        let other = d.path().join("S01_L3.wav");
        std::fs::write(&other, b"human").unwrap();
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(
            p.backups,
            vec![(
                other.clone(),
                d.path().join("backup").join("S01_L3.20261028T153012.wav")
            )]
        );
        assert_eq!(p.jobs[0].replaces, vec![other]);
        assert!(p.warnings.iter().any(|w| w.contains("S01_L3.wav")));
    }

    #[test]
    fn flac_is_backed_up_when_allowed() {
        let d = TempDir::new().unwrap();
        let flac = d.path().join("S01_L003.flac");
        std::fs::write(&flac, b"human").unwrap();
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = plan(&[(ID, &item)], d.path(), &SynthRecord::default(), NOW, true).unwrap();
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(
            p.backups,
            vec![(
                flac,
                d.path()
                    .join("backup")
                    .join("S01_L003.20261028T153012.flac")
            )]
        );
        // 許可されていなければ照合でも無視されるので、退避しない
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs.len(), 1);
        assert!(p.backups.is_empty());
    }

    #[test]
    fn up_to_date_with_other_spelling_backs_up_only_the_other() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"RIFFaudio");
        let other = d.path().join("S01_L0003.wav");
        std::fs::write(&other, b"human").unwrap();
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert!(p.jobs.is_empty());
        assert_eq!(p.skipped, 1);
        assert_eq!(
            p.backups,
            vec![(
                other,
                d.path()
                    .join("backup")
                    .join("S01_L0003.20261028T153012.wav")
            )]
        );
        assert!(p.warnings.iter().any(|w| w.contains("S01_L0003.wav")));
    }

    #[test]
    fn changed_text_backs_up_canonical_and_other_spelling() {
        let d = TempDir::new().unwrap();
        let src = wav(d.path(), b"RIFFaudio");
        let other = d.path().join("S01_L3.wav");
        std::fs::write(&other, b"human").unwrap();
        let item = speech("変えた", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert_eq!(p.jobs.len(), 1);
        let froms: Vec<&PathBuf> = p.backups.iter().map(|(f, _)| f).collect();
        assert_eq!(froms, vec![&src, &other]);
        assert_eq!(p.jobs[0].replaces, vec![src, other]);
    }

    #[test]
    fn backups_in_one_plan_do_not_share_a_name() {
        // 大文字小文字だけ違う名前(Windows では同じ退避先になりうる)
        let d = TempDir::new().unwrap();
        let b = d.path().join("backup");
        let mut taken = HashSet::new();
        let a = backup_path(d.path(), Path::new("S01_L003.wav"), NOW, &mut taken);
        let c = backup_path(d.path(), Path::new("S01_L003.WAV"), NOW, &mut taken);
        assert_eq!(a, b.join("S01_L003.20261028T153012.wav"));
        assert_eq!(c, b.join("S01_L003.20261028T153012.1.WAV"));
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
            vec![(
                src,
                d.path().join("backup").join("S01_L003.20261028T153012.wav")
            )]
        );
    }

    fn assert_changed_setting_resynthesizes(change: impl Fn(&mut SynthSpec)) {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"RIFFaudio");
        let mut sp = spec();
        change(&mut sp);
        let item = speech("こんにちは", Some(sp), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.backups.len(), 1);
        assert!(p.warnings.iter().any(|w| w.contains("退避して再合成")));
    }

    #[test]
    fn changed_engine_means_resynthesize_with_backup() {
        assert_changed_setting_resynthesizes(|s| s.engine = "aivis".into());
    }

    #[test]
    fn changed_param_means_resynthesize_with_backup() {
        assert_changed_setting_resynthesizes(|s| {
            s.params.insert("speed".into(), json!(1.5));
        });
    }

    #[test]
    fn foreign_file_is_backed_up() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"human");
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &SynthRecord::default());
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.backups.len(), 1);
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("合成記録にないファイルを退避")));
    }

    #[test]
    fn file_hash_mismatch_is_backed_up() {
        let d = TempDir::new().unwrap();
        wav(d.path(), b"replaced");
        let item = speech("こんにちは", Some(spec()), false, Some(ID));
        let p = run(&item, d.path(), &record_for("こんにちは", b"RIFFaudio"));
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.backups.len(), 1);
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("合成記録と中身が違うファイルを退避")));
        assert!(!p
            .warnings
            .iter()
            .any(|w| w.contains("合成記録にないファイルを退避")));
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
        assert_eq!(
            backup_name("S01_L003.wav", NOW, 0),
            "S01_L003.20261028T153012.wav"
        );
        assert_eq!(
            backup_name("S01_L3.flac", NOW, 2),
            "S01_L3.20261028T153012.2.flac"
        );
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
        assert_eq!(
            SynthRecord::load(TempDir::new().unwrap().path()).unwrap(),
            SynthRecord::default()
        );
        std::fs::write(d.path().join("synth.json"), "{broken").unwrap();
        assert!(SynthRecord::load(d.path()).is_err());
    }
}

#[cfg(test)]
pub(crate) mod run_tests {
    use super::*;
    use crate::proclog::ProcLog;
    use s2v_core::SceneConfig;
    use s2v_engines::{EngineManager, HttpEngine};
    use std::sync::Arc;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const NOW: &str = "20261028T153012";
    const L1: LineId = LineId { scene: 1, line: 1 };
    const L3: LineId = LineId { scene: 1, line: 3 };

    /// 1 秒の 440Hz(振幅 0.1)、16bit。
    pub(crate) fn tone_wav(rate: u32, channels: u16) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cur = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
            for i in 0..rate {
                let v = 0.1 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin();
                for _ in 0..channels {
                    w.write_sample((v * 32_767.0).round() as i16).unwrap();
                }
            }
            w.finalize().unwrap();
        }
        cur.into_inner()
    }

    /// VOICEVOX 風の偽サーバ。テストのスレッドとは別の実行環境で動かす
    /// (`run` は自分の実行環境で `block_on` するため、非同期の文脈の外から呼ぶ必要がある)。
    pub(crate) struct FakeVoicevox {
        rt: tokio::runtime::Runtime,
        server: Option<MockServer>,
    }

    impl FakeVoicevox {
        pub(crate) fn start(synthesis: ResponseTemplate) -> Self {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let server = rt.block_on(async {
                let s = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/version"))
                    .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
                    .mount(&s)
                    .await;
                Mock::given(method("GET"))
                    .and(path("/speakers"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                        {"name": "ずんだもん", "styles": [{"name": "ノーマル", "id": 3}]}
                    ])))
                    .mount(&s)
                    .await;
                Mock::given(method("POST"))
                    .and(path("/audio_query"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(serde_json::json!({"speedScale": 1.0})),
                    )
                    .mount(&s)
                    .await;
                Mock::given(method("POST"))
                    .and(path("/synthesis"))
                    .respond_with(synthesis)
                    .mount(&s)
                    .await;
                s
            });
            FakeVoicevox {
                rt,
                server: Some(server),
            }
        }

        /// VOICEVOX と同じ 24kHz モノラルを返す。
        pub(crate) fn ok() -> Self {
            Self::start(ResponseTemplate::new(200).set_body_bytes(tone_wav(24_000, 1)))
        }

        pub(crate) fn uri(&self) -> String {
            self.server.as_ref().unwrap().uri()
        }

        pub(crate) fn requests(&self) -> usize {
            let s = self.server.as_ref().unwrap();
            self.rt.block_on(s.received_requests()).unwrap().len()
        }
    }

    impl Drop for FakeVoicevox {
        fn drop(&mut self) {
            let _g = self.rt.enter();
            drop(self.server.take());
        }
    }

    fn manager(engines: &[(&str, String)]) -> EngineManager {
        let client = Arc::new(reqwest::Client::new());
        let mut m = EngineManager::new();
        for (name, url) in engines {
            m.register(
                *name,
                Arc::new(HttpEngine::new(*name, url.clone(), Arc::clone(&client))),
            );
        }
        m
    }

    fn speech(id: LineId, engine: &str) -> ScriptItem {
        ScriptItem::Speech {
            id: Some(id),
            line_no: 7,
            cast_name: "A".into(),
            text: "こんにちは".into(),
            offset_params: HashMap::new(),
            overlap_ms: None,
            at: None,
            synth: Some(SynthSpec {
                engine: engine.into(),
                speaker_name: "ずんだもん".into(),
                style: "ノーマル".into(),
                language: "ja".into(),
                params: BTreeMap::new(),
            }),
            voice_rec: false,
            synth_text: None,
            scene_config: SceneConfig::new("室内"),
        }
    }

    fn log() -> ProcLog {
        ProcLog::from_writer(Box::new(std::io::sink()))
    }

    /// 記録を読み、計画して実行するまでを1回ぶん行う。
    fn run_once(items: &[(LineId, ScriptItem)], dir: &Path, m: &EngineManager) -> SynthOutcome {
        let refs: Vec<(LineId, &ScriptItem)> = items.iter().map(|(id, i)| (*id, i)).collect();
        let mut record = SynthRecord::load(dir).unwrap();
        let p = plan(&refs, dir, &record, NOW, false).unwrap();
        run(p, &mut record, m, dir, &mut log())
    }

    fn wav_spec(p: &Path) -> hound::WavSpec {
        hound::WavReader::open(p).unwrap().spec()
    }

    #[test]
    fn synthesizes_and_records() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert!(o.failed.is_empty(), "{:?}", o.failed);
        assert_eq!((o.synthesized, o.skipped), (1, 0));
        let out = d.path().join("S01_L003.wav");
        assert!(out.is_file());
        // エンジンの 24kHz は 48kHz モノラルにして書く
        let spec = wav_spec(&out);
        assert_eq!((spec.channels, spec.sample_rate), (1, 48_000));
        assert!(s2v_audio::input::read_mono_48k(&out).is_ok());
        let rec = SynthRecord::load(d.path()).unwrap();
        let e = rec.get(L3).expect("記録がある");
        assert_eq!(e.engine, "voicevox");
        assert_eq!(e.text, "こんにちは");
        assert_eq!(e.file_sha256, sha256_hex(&std::fs::read(&out).unwrap()));
        // 一時ファイルを残さない(S01_L003.wav と synth.json だけ)
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn second_run_does_not_call_engine() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let items = [(L3, speech(L3, "voicevox"))];
        run_once(&items, d.path(), &m);
        let first = fake.requests();
        assert!(first > 0);
        let o = run_once(&items, d.path(), &m);
        assert_eq!(fake.requests(), first, "2回目はエンジンを呼ばない");
        assert_eq!((o.synthesized, o.skipped), (0, 1));
        assert!(o.failed.is_empty());
    }

    #[test]
    fn no_jobs_starts_no_engine() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run(
            SynthPlan::default(),
            &mut SynthRecord::default(),
            &m,
            d.path(),
            &mut log(),
        );
        assert_eq!(fake.requests(), 0);
        assert_eq!((o.synthesized, o.skipped, o.failed.len()), (0, 0, 0));
        assert!(
            !d.path().join("synth.json").exists(),
            "合成しなければ記録も書かない"
        );
    }

    #[test]
    fn engine_down_marks_failed_and_continues() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let m = manager(&[
            ("voicevox", fake.uri()),
            ("aivis", "http://127.0.0.1:1".to_string()),
        ]);
        let items = [(L1, speech(L1, "aivis")), (L3, speech(L3, "voicevox"))];
        let o = run_once(&items, d.path(), &m);
        assert_eq!(o.failed, HashSet::from([L1]));
        assert_eq!(o.synthesized, 1);
        assert!(!d.path().join("S01_L001.wav").exists());
        assert!(d.path().join("S01_L003.wav").is_file());
        let rec = SynthRecord::load(d.path()).unwrap();
        assert!(rec.get(L1).is_none() && rec.get(L3).is_some());
    }

    #[test]
    fn failure_restores_backup() {
        let fake = FakeVoicevox::start(ResponseTemplate::new(500));
        let d = TempDir::new().unwrap();
        let human = tone_wav(48_000, 1);
        let out = d.path().join("S01_L003.wav");
        std::fs::write(&out, &human).unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert_eq!(o.failed, HashSet::from([L3]));
        assert_eq!(std::fs::read(&out).unwrap(), human, "肉声が元の場所に戻る");
        let left: Vec<PathBuf> = std::fs::read_dir(d.path().join("backup"))
            .map(|r| r.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "退避先に残さない: {left:?}");
        assert!(SynthRecord::load(d.path()).unwrap().get(L3).is_none());
    }

    #[test]
    fn success_moves_old_file_to_backup() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let human = b"human".to_vec();
        std::fs::write(d.path().join("S01_L003.wav"), &human).unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert!(o.failed.is_empty(), "{:?}", o.failed);
        let b = d.path().join("backup").join("S01_L003.20261028T153012.wav");
        assert_eq!(std::fs::read(b).unwrap(), human);
    }

    #[test]
    fn success_moves_other_spelling_to_backup() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("S01_L3.wav"), b"human").unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert!(o.failed.is_empty(), "{:?}", o.failed);
        assert!(!d.path().join("S01_L3.wav").exists());
        assert!(d.path().join("S01_L003.wav").is_file());
        let b = d.path().join("backup").join("S01_L3.20261028T153012.wav");
        assert_eq!(std::fs::read(b).unwrap(), b"human");
    }

    #[test]
    fn failure_restores_every_backup() {
        let fake = FakeVoicevox::start(ResponseTemplate::new(500));
        let d = TempDir::new().unwrap();
        let canonical = d.path().join("S01_L003.wav");
        let other = d.path().join("S01_L3.wav");
        std::fs::write(&canonical, b"one").unwrap();
        std::fs::write(&other, b"two").unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert_eq!(o.failed, HashSet::from([L3]));
        assert_eq!(std::fs::read(&canonical).unwrap(), b"one");
        assert_eq!(std::fs::read(&other).unwrap(), b"two");
        let left: Vec<PathBuf> = std::fs::read_dir(d.path().join("backup"))
            .map(|r| r.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "退避先に残さない: {left:?}");
    }

    #[test]
    fn up_to_date_line_moves_other_spelling_without_engine() {
        let fake = FakeVoicevox::ok();
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let items = [(L3, speech(L3, "voicevox"))];
        run_once(&items, d.path(), &m);
        let n = fake.requests();
        std::fs::write(d.path().join("S01_L0003.wav"), b"human").unwrap();
        let o = run_once(&items, d.path(), &m);
        assert_eq!(fake.requests(), n, "合成済みならエンジンを呼ばない");
        assert_eq!((o.synthesized, o.skipped), (0, 1));
        assert!(!d.path().join("S01_L0003.wav").exists());
        assert!(d
            .path()
            .join("backup")
            .join("S01_L0003.20261028T153012.wav")
            .is_file());
        assert!(d.path().join("S01_L003.wav").is_file());
    }

    #[test]
    fn stereo_output_is_converted_to_mono() {
        let fake =
            FakeVoicevox::start(ResponseTemplate::new(200).set_body_bytes(tone_wav(44_100, 2)));
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert!(o.failed.is_empty(), "{:?}", o.failed);
        let out = d.path().join("S01_L003.wav");
        let spec = wav_spec(&out);
        assert_eq!((spec.channels, spec.sample_rate), (1, 48_000));
        assert!(s2v_audio::input::read_mono_48k(&out).is_ok());
    }

    #[test]
    fn broken_output_is_failure() {
        let fake =
            FakeVoicevox::start(ResponseTemplate::new(200).set_body_bytes(b"not a wav".to_vec()));
        let d = TempDir::new().unwrap();
        let m = manager(&[("voicevox", fake.uri())]);
        let o = run_once(&[(L3, speech(L3, "voicevox"))], d.path(), &m);
        assert_eq!(o.failed, HashSet::from([L3]));
        assert!(!d.path().join("S01_L003.wav").exists());
        assert!(!d.path().join("S01_L003.synth.tmp").exists());
    }

    #[test]
    fn corrupt_record_is_discarded_with_warning() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("synth.json"), "{broken").unwrap();
        let mut l = log();
        assert_eq!(load_record(d.path(), &mut l), SynthRecord::default());
        assert_eq!(l.warning_count(), 1);
        let mut l = log();
        assert_eq!(
            load_record(TempDir::new().unwrap().path(), &mut l),
            SynthRecord::default()
        );
        assert_eq!(l.warning_count(), 0);
    }
}
