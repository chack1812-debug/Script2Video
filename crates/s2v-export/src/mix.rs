//! トラック別ミックスとマスター(設計書 5.4)。dialogue / bgm / se は浮動小数のまま、
//! マスター(full_dialogue)にだけソフトリミッタを掛ける。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use s2v_audio::limiter::soft_limit;
use s2v_core::placement::PlacedEvent;
use s2v_core::BgmConfig;

use crate::bgm::compute_bgm_segments;
use crate::wavio::{duration_48k_samples, read_stereo_48k};

/// BGM の音量(流用元と同じ -10dB 相当)。
const BGM_GAIN: f32 = 0.3;

pub struct MixRequest<'a> {
    /// (開始サンプル, 音響処理後のステレオ)
    pub dialogue: Vec<(i64, Vec<[f32; 2]>)>,
    pub events: &'a [PlacedEvent],
    /// 配置の終わり(無言のシーン・仮の無音を含む)。音声はこれより短くしない
    pub timeline_end: i64,
    pub bgm: &'a BgmConfig,
    pub master_ceiling_db: f64,
}

pub struct Mixed {
    pub dialogue: Vec<[f32; 2]>,
    pub bgm: Vec<[f32; 2]>,
    pub se: Vec<[f32; 2]>,
    pub master: Vec<[f32; 2]>,
    /// 見つからない・読めない BGM/SE(終了コード 1)
    pub warnings: Vec<String>,
}

/// 総サンプル数の上限(48kHz で 1 時間。`render` と同じ)。超える配置は Err にする。
pub const MAX_TOTAL_SAMPLES: usize = 48_000 * 3600;

/// 負の開始位置は 0 に丸める。
fn start_idx(start: i64) -> usize {
    start.max(0) as usize
}

fn add_at(buf: &mut [[f32; 2]], start: usize, src: &[[f32; 2]], gain: f32) {
    for (i, s) in src.iter().enumerate() {
        if let Some(d) = start.checked_add(i).and_then(|k| buf.get_mut(k)) {
            d[0] += s[0] * gain;
            d[1] += s[1] * gain;
        }
    }
}

/// ミックスする。総長が `MAX_TOTAL_SAMPLES` を超える、または dialogue に非有限値が
/// あるときは Err(大きな確保の前に判定する)。見つからない BGM/SE は警告して無視する。
pub fn mix(req: MixRequest<'_>) -> anyhow::Result<Mixed> {
    let mut warnings = Vec::new();

    if !req.master_ceiling_db.is_finite() || req.master_ceiling_db > 0.0 {
        anyhow::bail!(
            "マスターの天井が不正です（有限で 0dB 以下が必要）: {}",
            req.master_ceiling_db
        );
    }

    // 音声を読む前に、安く判定できる上限チェックをすべて済ませる
    let cap = MAX_TOTAL_SAMPLES as i64;
    if req.timeline_end > cap {
        anyhow::bail!("配置の終わりが上限を超えています: {}", req.timeline_end);
    }
    for (start, v) in &req.dialogue {
        if *start > cap || start.max(&0).saturating_add(v.len() as i64) > cap {
            anyhow::bail!("台詞の配置が上限（{MAX_TOTAL_SAMPLES} サンプル）を超えています");
        }
    }
    for e in req.events {
        if e.start() > cap {
            anyhow::bail!(
                "イベントの開始が上限（{MAX_TOTAL_SAMPLES} サンプル）を超えています: {}",
                e.start()
            );
        }
        if let PlacedEvent::Se { path, .. } | PlacedEvent::BgmStart { path, .. } = e {
            // ヘッダーだけで長さを調べる(読めないファイルはここでは無視し、後の読み込みで警告する)
            if let Ok(n) = duration_48k_samples(Path::new(path)) {
                if n > MAX_TOTAL_SAMPLES as u64 {
                    anyhow::bail!("音声ファイルが長すぎます（上限 1 時間）: {path}");
                }
                // SE は終わりが「開始 + ファイル長」で確定する。BGM は区間の終わりで
                // 切られる/ループするので、ここでは単独の長さだけを見る
                if let PlacedEvent::Se { start, .. } = e {
                    if (*start).max(0) as u64 + n > MAX_TOTAL_SAMPLES as u64 {
                        anyhow::bail!("SE の終わりが上限（1 時間）を超えます: {path}");
                    }
                }
            }
        }
    }

    for (start, v) in &req.dialogue {
        if v.iter().any(|f| !f[0].is_finite() || !f[1].is_finite()) {
            anyhow::bail!("台詞の音声に NaN/無限大が含まれています（開始サンプル {start}）");
        }
    }

    // SE を読む
    let mut ses: Vec<(usize, Vec<[f32; 2]>, f32)> = Vec::new();
    for e in req.events {
        if let PlacedEvent::Se {
            path,
            start,
            gain_db,
        } = e
        {
            match read_stereo_48k(Path::new(path)) {
                Ok(mut se) => {
                    let len = se.len();
                    let fo = ((req.bgm.se_fade_out_s * 48_000.0) as usize).min(len);
                    for (i, s) in se[len - fo..].iter_mut().enumerate() {
                        let g = 1.0 - i as f32 / fo as f32;
                        s[0] *= g;
                        s[1] *= g;
                    }
                    ses.push((start_idx(*start), se, 10f32.powf(*gain_db as f32 / 20.0)));
                }
                Err(err) => warnings.push(format!("SE を読めません（無視します）: {path}: {err}")),
            }
        }
    }

    let dialogue_end = req
        .dialogue
        .iter()
        .map(|(s, v)| start_idx(*s).saturating_add(v.len()))
        .max()
        .unwrap_or(0);
    let base_end = dialogue_end.max(start_idx(req.timeline_end));

    // 読めない・空の BGM は、その #bgm_start ごと無視する(区間も長さも作らない)
    let mut bgm_src: HashMap<PathBuf, Vec<[f32; 2]>> = HashMap::new();
    let mut bgm_failed: HashSet<PathBuf> = HashSet::new();
    let mut events: Vec<PlacedEvent> = Vec::with_capacity(req.events.len());
    for e in req.events {
        if let PlacedEvent::BgmStart { path, .. } = e {
            let p = PathBuf::from(path);
            if bgm_failed.contains(&p) {
                continue;
            }
            if !bgm_src.contains_key(&p) {
                match read_stereo_48k(&p) {
                    Ok(v) => {
                        bgm_src.insert(p, v);
                    }
                    Err(err) => {
                        warnings.push(format!("BGM を読めません（無視します）: {path}: {err}"));
                        bgm_failed.insert(p);
                        continue;
                    }
                }
            }
        }
        events.push(e.clone());
    }
    let segs = compute_bgm_segments(&events, base_end as f64 / 48_000.0, req.bgm.crossfade_s);
    let bgm_end = segs
        .iter()
        .map(|s| ((s.mix_start_s + s.mix_duration_s) * 48_000.0) as usize)
        .max()
        .unwrap_or(0);
    let se_end = ses
        .iter()
        .map(|(s, v, _)| s.saturating_add(v.len()))
        .max()
        .unwrap_or(0);
    let total = base_end.max(bgm_end).max(se_end);
    if total > MAX_TOTAL_SAMPLES {
        anyhow::bail!("音声が長すぎます: {total} サンプル（上限 {MAX_TOTAL_SAMPLES}）");
    }

    let mut dialogue = vec![[0.0f32; 2]; total];
    for (start, v) in &req.dialogue {
        add_at(&mut dialogue, start_idx(*start), v, 1.0);
    }

    let mut se = vec![[0.0f32; 2]; total];
    for (start, v, g) in &ses {
        add_at(&mut se, *start, v, *g);
    }

    let mut bgm = vec![[0.0f32; 2]; total];
    for seg in &segs {
        let Some(src) = bgm_src.get(&seg.path) else {
            continue;
        };
        let seg_start = (seg.mix_start_s * 48_000.0) as usize;
        if seg_start >= total {
            continue;
        }
        let need = ((seg.mix_duration_s * 48_000.0) as usize).min(total - seg_start);
        if need == 0 {
            continue;
        }
        let mut looped: Vec<[f32; 2]> = src.iter().copied().cycle().take(need).collect();
        let fi = ((seg.fade_in_s * 48_000.0) as usize).min(looped.len());
        for (i, s) in looped[..fi].iter_mut().enumerate() {
            let g = i as f32 / fi.max(1) as f32;
            s[0] *= g;
            s[1] *= g;
        }
        let fo = ((seg.fade_out_s * 48_000.0) as usize).min(looped.len());
        let fo_start = looped.len() - fo;
        for (i, s) in looped[fo_start..].iter_mut().enumerate() {
            let g = 1.0 - i as f32 / fo.max(1) as f32;
            s[0] *= g;
            s[1] *= g;
        }
        add_at(&mut bgm, seg_start, &looped, BGM_GAIN);
    }

    let mut master: Vec<[f32; 2]> = (0..total)
        .map(|i| {
            [
                dialogue[i][0] + bgm[i][0] + se[i][0],
                dialogue[i][1] + bgm[i][1] + se[i][1],
            ]
        })
        .collect();
    soft_limit(&mut master, req.master_ceiling_db);

    Ok(Mixed {
        dialogue,
        bgm,
        se,
        master,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wavio::{write_f32_stereo, write_i24_stereo};

    fn bgm_cfg() -> BgmConfig {
        BgmConfig {
            crossfade_s: 3.0,
            se_fade_out_s: 0.05,
        }
    }

    fn mix_ok(req: MixRequest<'_>) -> Mixed {
        mix(req).unwrap()
    }

    fn write_mono(path: &Path, value: f32, n: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..n {
            w.write_sample(value).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn dialogue_is_placed_and_tracks_are_separate() {
        let cfg = bgm_cfg();
        let out = mix_ok(MixRequest {
            dialogue: vec![(480, vec![[0.5, 0.25]; 100])],
            events: &[],
            timeline_end: 1_000,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.dialogue.len(), 1_000);
        assert_eq!(out.dialogue[480], [0.5, 0.25]);
        assert_eq!(out.dialogue[479], [0.0, 0.0]);
        assert_eq!(out.master[500], [0.5, 0.25]);
        assert!(out.bgm.iter().all(|f| *f == [0.0, 0.0]));
        assert!(out.se.iter().all(|f| *f == [0.0, 0.0]));
    }

    #[test]
    fn se_uses_gain_and_missing_files_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let se = tmp.path().join("効果 音.wav");
        write_mono(&se, 0.5, 4_800);
        let events = vec![
            PlacedEvent::Se {
                path: se.to_string_lossy().into_owned(),
                start: 0,
                gain_db: -6.0206,
            },
            PlacedEvent::Se {
                path: tmp.path().join("none.wav").to_string_lossy().into_owned(),
                start: 0,
                gain_db: 0.0,
            },
        ];
        let cfg = bgm_cfg();
        let out = mix_ok(MixRequest {
            dialogue: vec![],
            events: &events,
            timeline_end: 0,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.se.len(), 4_800);
        assert!((out.se[100][0] - 0.25).abs() < 1e-4);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn bgm_loops_until_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let bgm = tmp.path().join("bgm.wav");
        write_mono(&bgm, 0.5, 4_800);
        let events = vec![
            PlacedEvent::BgmStart {
                path: bgm.to_string_lossy().into_owned(),
                start: 0,
            },
            PlacedEvent::BgmStop { start: 48_000 },
        ];
        let cfg = bgm_cfg();
        let out = mix_ok(MixRequest {
            dialogue: vec![],
            events: &events,
            timeline_end: 96_000,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert!((out.bgm[24_000][0] - 0.15).abs() < 1e-4);
        assert_eq!(out.bgm[60_000], [0.0, 0.0]);
    }

    #[test]
    fn master_is_limited_but_dialogue_track_is_not() {
        let cfg = bgm_cfg();
        let out = mix_ok(MixRequest {
            dialogue: vec![(0, vec![[3.0, -3.0]; 4_800])],
            events: &[],
            timeline_end: 0,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.dialogue[2_400], [3.0, -3.0]);
        let c = 10f32.powf(-1.0 / 20.0);
        assert!(out
            .master
            .iter()
            .all(|f| f[0].abs() <= c + 1e-6 && f[1].abs() <= c + 1e-6));
    }

    #[test]
    fn writers_produce_expected_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = vec![[0.5f32, -0.5]; 10];
        let f = tmp.path().join("f.wav");
        let i = tmp.path().join("i.wav");
        write_f32_stereo(&f, &[[2.0, -2.0]]).unwrap();
        write_i24_stereo(&i, &buf).unwrap();
        let rf = hound::WavReader::open(&f).unwrap();
        assert_eq!(
            (
                rf.spec().bits_per_sample,
                rf.spec().sample_format,
                rf.spec().channels
            ),
            (32, hound::SampleFormat::Float, 2)
        );
        let mut ri = hound::WavReader::open(&i).unwrap();
        assert_eq!(
            (ri.spec().bits_per_sample, ri.spec().sample_rate),
            (24, 48_000)
        );
        let first: i32 = ri.samples::<i32>().next().unwrap().unwrap();
        assert_eq!(first, 4_194_304);
    }

    #[test]
    fn negative_start_is_clamped_to_zero_and_zero_fade_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let se = tmp.path().join("se.wav");
        write_mono(&se, 0.5, 100);
        let events = vec![PlacedEvent::Se {
            path: se.to_string_lossy().into_owned(),
            start: -500,
            gain_db: 0.0,
        }];
        let cfg = BgmConfig {
            crossfade_s: 3.0,
            se_fade_out_s: 0.0,
        };
        let out = mix_ok(MixRequest {
            dialogue: vec![(-10, vec![[0.1, 0.1]; 5])],
            events: &events,
            timeline_end: -1,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert_eq!(out.se.len(), 100);
        assert_eq!(out.se[0], [0.5, 0.5]);
        assert_eq!(out.se[99], [0.5, 0.5]);
        assert_eq!(out.dialogue[0], [0.1, 0.1]);
    }

    fn req<'a>(
        dialogue: Vec<(i64, Vec<[f32; 2]>)>,
        events: &'a [PlacedEvent],
        timeline_end: i64,
        cfg: &'a BgmConfig,
    ) -> MixRequest<'a> {
        MixRequest {
            dialogue,
            events,
            timeline_end,
            bgm: cfg,
            master_ceiling_db: -1.0,
        }
    }

    #[test]
    fn total_beyond_cap_is_an_error_without_allocating() {
        let cfg = bgm_cfg();
        let far = MAX_TOTAL_SAMPLES as i64 + 1;
        assert!(mix(req(vec![(far, vec![[0.0; 2]])], &[], 0, &cfg)).is_err());
        assert!(mix(req(vec![], &[], i64::MAX, &cfg)).is_err());
        let se = PlacedEvent::Se {
            path: "none.wav".into(),
            start: far,
            gain_db: 0.0,
        };
        // 開始位置が上限超なら、読めないファイルでも Err(音声を読む前に判定する)
        assert!(mix(req(vec![], &[se], 0, &cfg)).is_err());
        assert!(mix(req(vec![], &[PlacedEvent::BgmStop { start: far }], 0, &cfg)).is_err());
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("s.wav");
        write_mono(&wav, 0.1, 10);
        let se = PlacedEvent::Se {
            path: wav.to_string_lossy().into_owned(),
            start: far,
            gain_db: 0.0,
        };
        assert!(mix(req(vec![], &[se], 0, &cfg)).is_err());
        let bgm = PlacedEvent::BgmStart {
            path: wav.to_string_lossy().into_owned(),
            start: far,
        };
        assert!(mix(req(vec![], &[bgm], 0, &cfg)).is_err());
    }

    #[test]
    fn non_finite_dialogue_is_an_error() {
        let cfg = bgm_cfg();
        assert!(mix(req(vec![(0, vec![[0.0, f32::NAN]])], &[], 0, &cfg)).is_err());
        assert!(mix(req(vec![(0, vec![[f32::INFINITY, 0.0]])], &[], 0, &cfg)).is_err());
    }

    #[test]
    fn empty_bgm_file_is_ignored_with_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let bgm = tmp.path().join("e.wav");
        write_mono(&bgm, 0.5, 0);
        let events = vec![PlacedEvent::BgmStart {
            path: bgm.to_string_lossy().into_owned(),
            start: 0,
        }];
        let cfg = bgm_cfg();
        let out = mix_ok(req(vec![], &events, 1_000, &cfg));
        assert_eq!(out.bgm.len(), 1_000);
        assert!(out.bgm.iter().all(|f| *f == [0.0, 0.0]));
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("BGM を読めません"),
            "{:?}",
            out.warnings
        );
        assert!(out.warnings[0].contains("e.wav"));
    }

    #[test]
    fn missing_bgm_past_the_end_does_not_extend_the_timeline() {
        let tmp = tempfile::tempdir().unwrap();
        let none = tmp.path().join("none.wav").to_string_lossy().into_owned();
        let events = vec![
            PlacedEvent::BgmStart {
                path: none.clone(),
                start: 5_000,
            },
            PlacedEvent::BgmStart {
                path: none,
                start: 9_000,
            },
            PlacedEvent::BgmStop { start: 9_500 },
        ];
        let cfg = bgm_cfg();
        let out = mix_ok(req(vec![], &events, 1_000, &cfg));
        assert_eq!(out.master.len(), 1_000);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    }

    #[test]
    fn invalid_ceiling_is_an_error() {
        let cfg = bgm_cfg();
        for c in [f64::NAN, 0.5, f64::INFINITY, f64::NEG_INFINITY] {
            let mut r = req(vec![(0, vec![[0.1, 0.1]])], &[], 0, &cfg);
            r.master_ceiling_db = c;
            assert!(mix(r).is_err(), "{c}");
        }
        let mut r = req(vec![(0, vec![[0.1, 0.1]])], &[], 0, &cfg);
        r.master_ceiling_db = 0.0;
        assert!(mix(r).is_ok());
    }

    /// ヘッダーだけが巨大な長さを宣言する WAV(本体はほぼ空)。デコードしたら Err ではなく
    /// 警告で終わるので、Err になることは「デコード前にヘッダーで弾いた」ことの証拠になる。
    fn write_fake_huge_wav(path: &Path, secs: u32) {
        let data_len: u32 = 48_000 * 2 * secs; // 16bit モノラル 48kHz で secs 秒
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&96_000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.extend_from_slice(&[0u8; 10]);
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn file_longer_than_cap_is_rejected_from_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("huge.wav");
        write_fake_huge_wav(&wav, 4000);
        let p = wav.to_string_lossy().into_owned();
        let cfg = bgm_cfg();
        let bgm = [PlacedEvent::BgmStart {
            path: p.clone(),
            start: 0,
        }];
        let err = mix(req(vec![], &bgm, 0, &cfg)).err().unwrap().to_string();
        assert!(err.contains("huge.wav"), "{err}");
        let se = [PlacedEvent::Se {
            path: p,
            start: 0,
            gain_db: 0.0,
        }];
        assert!(mix(req(vec![], &se, 0, &cfg)).is_err());
    }

    #[test]
    fn se_whose_start_plus_length_exceeds_cap_is_rejected_before_decoding() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("long_se.wav");
        write_fake_huge_wav(&wav, 1000);
        let p = wav.to_string_lossy().into_owned();
        let cfg = bgm_cfg();
        let se = |start: i64| {
            [PlacedEvent::Se {
                path: p.clone(),
                start,
                gain_db: 0.0,
            }]
        };
        // 3000s + 1000s > 3600s(開始も長さも単独では上限内)
        let err = mix(req(vec![], &se(3000 * 48_000), 0, &cfg))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("long_se.wav"), "{err}");
        // 2000s + 1000s は収まる(本体が空なのでデコードは警告で無視される)
        let out = mix(req(vec![], &se(2000 * 48_000), 0, &cfg)).unwrap();
        assert_eq!(out.warnings.len(), 1);
    }
}
