//! `script2video render`(映像工程設計書 §4)。
//! 素材・吹き出し・ト書き・音声を合成して `output.mp4` を作る。失敗したときは `output.mp4` を変えない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use s2v_core::config::{Fit as CoreFit, RenderConfig};
use s2v_export::wavio::read_stereo_48k;
use s2v_render::bubbles::Placed;
use s2v_render::draw::Overlay;
use s2v_render::font::FontFace;
use s2v_render::scene::{compose, draw_item, Item, Placement};
use s2v_video::audio_check::{check_audio, check_av_length, decode_args, f32le, is_silent};
use s2v_video::ffmpeg::{
    concat_args, concat_list, cut_args, mux_args, Background, EncodeSettings, Fit,
};
use s2v_video::frames::{overlay_list, total_frames, CutPlan};
use s2v_video::probe::{check_cut, check_cuts_consistent, check_output, probe_args};

use crate::layout::{check_contained, normalize_path, script_folder};
use crate::prepare::{preflight, prepare, probe_file, run_checked, MissingAssets, Prepared};
use crate::proclog::ProcLog;
use crate::Outcome;

pub struct RenderArgs {
    pub script: PathBuf,
    /// 設定ファイル(省略時は同梱の既定設定)。
    pub config: Option<PathBuf>,
}

/// 実行して終了コードを返す。予期しないエラーは終了コード 2。
pub fn run_render(args: &RenderArgs) -> Outcome {
    match render(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

fn render(args: &RenderArgs) -> anyhow::Result<Outcome> {
    // 何かを書く前の検査。失敗は標準エラーにだけ出す(Process.log はまだ触らない)
    let pf = match preflight(&args.script, args.config.as_deref()) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("[ERROR] {msg}");
            eprintln!("[ERROR] 出力していません");
            return Ok(Outcome::Failed);
        }
    };
    let (config, dir) = (&pf.config, &pf.dir);

    // 2. Process.log(追記)
    let mut log = ProcLog::append(&dir.join("Process.log"), "render")?;
    let result = render_main(args, &config.render, dir, &mut log);
    match result {
        // 成功: output.mp4 は置き換え済み。ログの書込み確認は置き換えの前に済ませてあるので、
        // ここで失敗扱いにしない(置き換え後の結果を覆さない)
        Ok(()) if log.warning_count() > 0 => Ok(Outcome::Warnings),
        Ok(()) => Ok(Outcome::Ok),
        Err(msg) => {
            log.error(&msg);
            log.error("output.mp4 は変更していません");
            if let Err(e) = log.check() {
                eprintln!("[ERROR] {e:#}");
            }
            Ok(Outcome::Failed)
        }
    }
}

/// `max(1, 論理コア数 ÷ jobs)`。
fn threads_for(cores: usize, jobs: u32) -> u32 {
    (cores / jobs.max(1) as usize).max(1) as u32
}

/// 区間ごとの項目の組に重ね画像のファイル名を割り当てる。同じ組は 1 つのファイルにまとめる。
/// 戻り値: (組 → ファイル名, カットごとの (r_k, ファイル名))。空の組は `ov_empty.png`。
#[allow(clippy::type_complexity)]
fn assign_overlay_files(
    plans: &[CutPlan],
) -> (BTreeMap<Vec<String>, String>, Vec<Vec<(i64, String)>>) {
    let mut files: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut next = 1;
    let mut per_cut = Vec::new();
    for plan in plans {
        let mut v = Vec::new();
        for iv in &plan.intervals {
            let name = files
                .entry(iv.items.clone())
                .or_insert_with(|| {
                    if iv.items.is_empty() {
                        "ov_empty.png".to_string()
                    } else {
                        let n = format!("ov_{next:04}.png");
                        next += 1;
                        n
                    }
                })
                .clone();
            v.push((iv.rel_start, name));
        }
        per_cut.push(v);
    }
    (files, per_cut)
}

/// 重ね画像を描いて `seg` に PNG として書く。
fn write_overlays(
    seg: &Path,
    files: &BTreeMap<Vec<String>, String>,
    items: &[Item],
    placed: &BTreeMap<String, Placed>,
    clipped: &BTreeSet<String>,
    cfg: &RenderConfig,
    face: &FontFace,
) -> Result<(), String> {
    // 使われる項目だけを 1 回ずつ描く
    let used: BTreeSet<&String> = files.keys().flatten().collect();
    let by_id: BTreeMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut drawn: BTreeMap<&str, Overlay> = BTreeMap::new();
    for id in used {
        let ov = draw_item(
            by_id[id.as_str()],
            &placed[id],
            clipped.contains(id.as_str()),
            cfg,
            face,
        )
        .map_err(|e| format!("{id}: {e}"))?;
        drawn.insert(id.as_str(), ov);
    }
    for (set, name) in files {
        let layers: Vec<(&Placed, &Overlay)> = set
            .iter()
            .map(|id| (&placed[id], &drawn[id.as_str()]))
            .collect();
        let img = compose(cfg, &layers);
        let png = img
            .encode_png()
            .map_err(|e| format!("{name} を PNG にできません: {e}"))?;
        std::fs::write(seg.join(name), png).map_err(|e| format!("{name} を書けません: {e}"))?;
    }
    Ok(())
}

fn render_main(
    args: &RenderArgs,
    cfg: &RenderConfig,
    dir: &Path,
    log: &mut ProcLog,
) -> Result<(), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("現在のフォルダを取得できません: {e}"))?;
    let abs = |p: &Path| normalize_path(p, &cwd).to_string_lossy().into_owned();

    // 2〜7. 起動時の検査(bubbles と共通)
    let Prepared {
        timeline: tl,
        face,
        ffmpeg,
        ffprobe,
        items,
        placement: Placement { placed, clipped },
        plans,
        assets,
        ..
    } = prepare(&args.script, cfg, dir, MissingAssets::Error, log)?;
    let total = total_frames(tl.duration_ms, cfg.fps);
    let seg = dir.join("work").join("render").join("seg");
    let backgrounds: Vec<Background> = plans
        .iter()
        .map(|p| assets[&p.cut_id].background.clone())
        .collect();

    // 8. 作業フォルダ
    let root = script_folder(&args.script, &cwd);
    check_contained(&root, "work").map_err(|e| format!("{e:#}"))?;
    check_contained(&root, "work/render").map_err(|e| format!("{e:#}"))?;
    let render_dir = dir.join("work").join("render");
    if render_dir.exists() {
        std::fs::remove_dir_all(&render_dir)
            .map_err(|e| format!("{} を消せません: {e}", render_dir.display()))?;
    }
    std::fs::create_dir_all(&seg).map_err(|e| format!("{} を作れません: {e}", seg.display()))?;

    // 9. 重ね画像と一覧
    let (files, per_cut) = assign_overlay_files(&plans);
    write_overlays(&seg, &files, &items, &placed, &clipped, cfg, &face)?;
    let settings = EncodeSettings {
        width: cfg.width,
        height: cfg.height,
        fps: cfg.fps,
        crf: cfg.crf,
        preset: cfg.preset.clone(),
        threads: threads_for(
            std::thread::available_parallelism().map_or(1, |n| n.get()),
            cfg.jobs,
        ),
        fit: match cfg.fit {
            CoreFit::Contain => Fit::Contain,
            CoreFit::Cover => Fit::Cover,
        },
        background: RenderConfig::rgb(&cfg.background).unwrap_or([0, 0, 0]),
    };
    let mut cut_files = Vec::new();
    let mut ffmpeg_jobs: Vec<Vec<String>> = Vec::new();
    for (i, plan) in plans.iter().enumerate() {
        let no = i + 1;
        let (cut_name, list_name) = (format!("cut_{no:04}.mp4"), format!("ovl_{no:04}.txt"));
        std::fs::write(
            seg.join(&list_name),
            overlay_list(&per_cut[i], plan.frames, cfg.fps),
        )
        .map_err(|e| format!("{list_name} を書けません: {e}"))?;
        ffmpeg_jobs.push(cut_args(
            &backgrounds[i],
            &list_name,
            plan.frames,
            &cut_name,
            &settings,
        ));
        cut_files.push(cut_name);
    }

    // 10. カットファイル(jobs 個まで並列)
    let results = run_parallel(&ffmpeg, &seg, &ffmpeg_jobs, cfg.jobs as usize);
    let mut first_error = None;
    for (i, r) in results.iter().enumerate() {
        match r {
            Some(Ok(d)) => log.info(format!(
                "{} を作りました({}、{:.2} 秒)",
                cut_files[i],
                plans[i].cut_id,
                d.as_secs_f64()
            )),
            Some(Err(e)) => {
                first_error.get_or_insert_with(|| format!("{}: {e}", plans[i].cut_id));
            }
            None => {}
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }

    // 11. カットの検査・連結・多重化
    let mut probes = Vec::new();
    let mut internal = Vec::new();
    for (i, name) in cut_files.iter().enumerate() {
        let p = probe_file(&ffprobe, &probe_args(name), &seg)?;
        internal.extend(
            check_cut(&p, plans[i].frames, cfg.fps)
                .into_iter()
                .map(|e| format!("{name}: {e}")),
        );
        probes.push((name.clone(), p));
    }
    internal.extend(check_cuts_consistent(&probes));
    if !internal.is_empty() {
        return Err(format!(
            "内部の誤り: カットファイルが想定と違います\n{}",
            internal.join("\n")
        ));
    }
    std::fs::write(seg.join("concat.txt"), concat_list(&cut_files))
        .map_err(|e| format!("concat.txt を書けません: {e}"))?;
    run_checked(&ffmpeg, &concat_args("concat.txt", "video.mp4"), &seg)?;
    let wav = dir.join("full_dialogue.wav");
    run_checked(
        &ffmpeg,
        &mux_args(
            "video.mp4",
            &abs(&wav),
            "output.tmp.mp4",
            &cfg.audio_bitrate,
        ),
        &seg,
    )?;

    // 12. 出力の検証
    let out_probe = probe_file(&ffprobe, &probe_args("output.tmp.mp4"), &seg)?;
    let mut errs = check_output(&out_probe, cfg.width, cfg.height, cfg.fps, total);
    let decoded = f32le(&run_checked(&ffmpeg, &decode_args("output.tmp.mp4"), &seg)?);
    let reference: Vec<f32> = read_stereo_48k(&wav)
        .map_err(|e| format!("full_dialogue.wav を読めません: {e:#}"))?
        .iter()
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();
    if is_silent(&reference) {
        log.info("full_dialogue.wav 全体が無音なので、音声の時刻の検査を飛ばしました");
    }
    errs.extend(check_audio(&reference, &decoded));
    errs.extend(check_av_length(total, cfg.fps, decoded.len()));
    if !errs.is_empty() {
        return Err(format!("出力の検証に失敗しました\n{}", errs.join("\n")));
    }

    // 13. 置き換え
    commit_output(
        log,
        &seg.join("output.tmp.mp4"),
        &dir.join("output.mp4"),
        &format!(
            "output.mp4 を書き出しました({total} コマ、{:.2} 秒)",
            total as f64 / f64::from(cfg.fps)
        ),
    )
}

/// 成功の記録を先に書き、ログに書けていることを確かめてから output.mp4 へ置き換える。
/// 置き換えの後には失敗しうる処理を何も置かない(置き換え後に失敗扱いにならない)。
fn commit_output(
    log: &mut ProcLog,
    tmp: &Path,
    out: &Path,
    success_msg: &str,
) -> Result<(), String> {
    log.info(success_msg);
    log.check().map_err(|e| format!("{e:#}"))?;
    std::fs::rename(tmp, out)
        .map_err(|e| format!("output.mp4 に置き換えられません: {}: {e}", out.display()))
}

/// ffmpeg を `jobs` 個まで並列に起動する。失敗が出たら新しい起動をやめる。
/// 戻り値は引数と同じ並びで、起動しなかったものは None。
fn run_parallel(
    ffmpeg: &str,
    seg: &Path,
    commands: &[Vec<String>],
    jobs: usize,
) -> Vec<Option<Result<Duration, String>>> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let results: Mutex<Vec<Option<Result<Duration, String>>>> =
        Mutex::new(vec![None; commands.len()]);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, commands.len().max(1)) {
            s.spawn(|| loop {
                if failed.load(Ordering::SeqCst) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= commands.len() {
                    break;
                }
                let t = Instant::now();
                let r = run_checked(ffmpeg, &commands[i], seg).map(|_| t.elapsed());
                if r.is_err() {
                    failed.store(true, Ordering::SeqCst);
                }
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_render::scene::place_items;
    use s2v_render::ItemKind;
    use s2v_video::frames::Interval;

    fn plan(cut: &str, sets: &[(i64, &[&str])]) -> CutPlan {
        CutPlan {
            cut_id: cut.into(),
            start_frame: 0,
            frames: 100,
            intervals: sets
                .iter()
                .map(|(r, ids)| Interval {
                    rel_start: *r,
                    items: ids.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn distinct_overlay_sets_are_written_once() {
        let plans = vec![
            plan(
                "S01-C01",
                &[(0, &[]), (10, &["A"]), (20, &[]), (30, &["A"])],
            ),
            plan("S01-C02", &[(0, &["A"]), (5, &["A", "B"])]),
        ];
        let (files, per_cut) = assign_overlay_files(&plans);
        assert_eq!(files.len(), 3, "{files:?}"); // 空・{A}・{A,B}
        assert_eq!(files[&Vec::<String>::new()], "ov_empty.png");
        assert_eq!(files[&vec!["A".to_string()]], "ov_0001.png");
        assert_eq!(
            files[&vec!["A".to_string(), "B".to_string()]],
            "ov_0002.png"
        );
        let names: Vec<&str> = per_cut[0].iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["ov_empty.png", "ov_0001.png", "ov_empty.png", "ov_0001.png"]
        );
        assert_eq!(per_cut[1][1], (5, "ov_0002.png".to_string()));
    }

    #[test]
    fn write_overlays_passes_the_clipped_set_to_drawing() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = RenderConfig {
            width: 320,
            height: 180,
            note_padding: 20,
            ..RenderConfig::default()
        };
        let items = vec![Item {
            id: "S01-N01".into(),
            kind: ItemKind::Note,
            text: "一二三四五".into(),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        }];
        // 文字が収まらない小さな枠
        let placed: BTreeMap<String, Placed> = [(
            "S01-N01".to_string(),
            Placed {
                kind: ItemKind::Note,
                x: 100,
                y: 50,
                w: 60,
                h: 60,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        )]
        .into();
        let files: BTreeMap<Vec<String>, String> =
            [(vec!["S01-N01".to_string()], "ov_0001.png".to_string())].into();

        // 切り詰めた印が無い項目は、収まらなければ失敗
        let d = tempfile::tempdir().unwrap();
        let e = write_overlays(
            d.path(),
            &files,
            &items,
            &placed,
            &BTreeSet::new(),
            &cfg,
            &face,
        )
        .unwrap_err();
        assert!(e.contains("S01-N01") && e.contains("収まりません"), "{e}");
        assert!(!d.path().join("ov_0001.png").exists());

        // 印がある項目は PNG が書かれ、枠(100..160, 50..110)の外は透明のまま
        let clipped: BTreeSet<String> = ["S01-N01".to_string()].into();
        write_overlays(d.path(), &files, &items, &placed, &clipped, &cfg, &face).unwrap();
        let pm = tiny_skia::Pixmap::load_png(d.path().join("ov_0001.png")).unwrap();
        assert_eq!((pm.width(), pm.height()), (320, 180));
        let mut inside = 0;
        for y in 0..180u32 {
            for x in 0..320u32 {
                let a = pm.pixel(x, y).unwrap().alpha();
                if (100..160).contains(&x) && (50..110).contains(&y) {
                    inside += u32::from(a > 0);
                } else {
                    assert_eq!(a, 0, "枠の外に描かれています: ({x}, {y})");
                }
            }
        }
        assert!(inside > 0);
    }

    #[test]
    fn manual_item_too_big_for_the_screen_is_clipped_inside_it() {
        let face = FontFace::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../s2v-render/tests/fonts/test-vrt2.ttf"),
            0,
        )
        .unwrap();
        let cfg = RenderConfig {
            width: 320,
            height: 180,
            ..RenderConfig::default()
        };
        let items = vec![Item {
            id: "S01-L001".into(),
            kind: ItemKind::Bubble,
            text: "あ".repeat(500),
            start_ms: 0,
            end_ms: 1000,
            order: 0,
        }];
        let manual: BTreeMap<String, Placed> = [(
            "S01-L001".to_string(),
            Placed {
                kind: ItemKind::Bubble,
                x: 200,
                y: 100,
                w: 60,
                h: 60,
                tail: None,
                start_offset_ms: 0,
                end_offset_ms: 0,
            },
        )]
        .into();
        let mut w = Vec::new();
        let Placement { placed, clipped } = place_items(&items, &manual, &cfg, &face, &mut w);
        let p = &placed["S01-L001"];
        assert!(p.x >= 0 && p.y >= 0, "{p:?}");
        assert!(p.x as u32 + p.w <= 320 && p.y as u32 + p.h <= 180, "{p:?}");
        assert!(clipped.contains("S01-L001"), "{clipped:?}");
        assert!(w.iter().any(|m| m.contains("画面")), "{w:?}");

        let files: BTreeMap<Vec<String>, String> =
            [(vec!["S01-L001".to_string()], "ov_0001.png".to_string())].into();
        let d = tempfile::tempdir().unwrap();
        write_overlays(d.path(), &files, &items, &placed, &clipped, &cfg, &face).unwrap();
        assert!(d.path().join("ov_0001.png").exists());
    }

    struct FailingWriter;
    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("書き込み失敗"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn commit_output_does_not_replace_when_the_log_cannot_be_written() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, out) = (d.path().join("t.mp4"), d.path().join("output.mp4"));
        std::fs::write(&tmp, "new").unwrap();
        std::fs::write(&out, "old").unwrap();
        let mut log = ProcLog::from_writer(Box::new(FailingWriter));
        let e = commit_output(&mut log, &tmp, &out, "ok").unwrap_err();
        assert!(e.contains("Process.log"), "{e}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "old");
        assert!(tmp.exists());
    }

    #[test]
    fn commit_output_replaces_and_reports_rename_failure() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, out) = (d.path().join("t.mp4"), d.path().join("output.mp4"));
        std::fs::write(&tmp, "new").unwrap();
        std::fs::write(&out, "old").unwrap();
        let mut log = ProcLog::from_writer(Box::new(Vec::<u8>::new()));
        commit_output(&mut log, &tmp, &out, "ok").unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "new");
        // 置き換え元が無ければ Err
        let e = commit_output(&mut log, &tmp, &out, "ok").unwrap_err();
        assert!(e.contains("置き換えられません"), "{e}");
    }

    #[test]
    fn threads_split_cores_by_jobs() {
        assert_eq!(threads_for(8, 1), 8);
        assert_eq!(threads_for(8, 3), 2);
        assert_eq!(threads_for(2, 4), 1);
        assert_eq!(threads_for(1, 64), 1);
    }
}
