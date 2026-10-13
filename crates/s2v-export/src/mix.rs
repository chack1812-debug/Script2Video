//! トラック別ミックスとマスター(設計書 5.4)。dialogue / bgm / se は浮動小数のまま、
//! マスター(full_dialogue)にだけソフトリミッタを掛ける。

use std::path::Path;

use s2v_audio::limiter::soft_limit;
use s2v_core::placement::PlacedEvent;
use s2v_core::BgmConfig;

use crate::bgm::compute_bgm_segments;
use crate::wavio::read_stereo_48k;

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

/// 総サンプル数の上限(48kHz で 4 時間)。これを超える配置は切り詰めて警告する。
const MAX_TOTAL: usize = 48_000 * 3600 * 4;

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

pub fn mix(req: MixRequest<'_>) -> Mixed {
    mix_capped(req, MAX_TOTAL)
}

fn mix_capped(req: MixRequest<'_>, max_total: usize) -> Mixed {
    let mut warnings = Vec::new();

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
    let segs = compute_bgm_segments(req.events, base_end as f64 / 48_000.0, req.bgm.crossfade_s);
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
    let mut total = base_end.max(bgm_end).max(se_end);
    if total > max_total {
        warnings.push(format!(
            "音声が長すぎるため {max_total} サンプルで打ち切ります（要求: {total}）"
        ));
        total = max_total;
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
        let src = match read_stereo_48k(&seg.path) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => continue,
            Err(err) => {
                warnings.push(format!(
                    "BGM を読めません（無視します）: {}: {err}",
                    seg.path.display()
                ));
                continue;
            }
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

    Mixed {
        dialogue,
        bgm,
        se,
        master,
        warnings,
    }
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
        let out = mix(MixRequest {
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
        let out = mix(MixRequest {
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
        let out = mix(MixRequest {
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
        let out = mix(MixRequest {
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
        let out = mix(MixRequest {
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

    #[test]
    fn huge_total_is_capped_with_a_warning() {
        let cfg = bgm_cfg();
        let out = mix_capped(
            MixRequest {
                dialogue: vec![(0, vec![[0.1, 0.1]; 10])],
                events: &[],
                timeline_end: i64::MAX,
                bgm: &cfg,
                master_ceiling_db: -1.0,
            },
            1_000,
        );
        assert_eq!(out.master.len(), 1_000);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn empty_bgm_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bgm = tmp.path().join("e.wav");
        write_mono(&bgm, 0.5, 0);
        let events = vec![PlacedEvent::BgmStart {
            path: bgm.to_string_lossy().into_owned(),
            start: 0,
        }];
        let cfg = bgm_cfg();
        let out = mix(MixRequest {
            dialogue: vec![],
            events: &events,
            timeline_end: 1_000,
            bgm: &cfg,
            master_ceiling_db: -1.0,
        });
        assert!(out.bgm.iter().all(|f| *f == [0.0, 0.0]));
    }
}
