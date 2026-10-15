//! `script2video bubbles`(映像工程設計書 §5.1・§5.4)。
//! `render` と共通の起動時の検査(`preflight`・`prepare`)を通し、素材と音声を固定して、
//! 127.0.0.1 だけで待ち受ける配置調整のサーバーを動かす。`/api/quit` が来るまで戻らない。
//!
//! Ctrl+C ではプロセスがそのまま終わる(ロックとハンドルは OS が外す。Process.log の終了の行は書かれない)。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use s2v_bubbles::api::{Session, SessionAsset};
use s2v_bubbles::media::{open_locked, LockedFile};
use s2v_bubbles::server::{LogEvent, ServeOptions, Server};
use s2v_render::assets::AssetKind;
use s2v_video::ffmpeg::Background;
use s2v_video::frames::asset_frames;

use crate::prepare::{preflight, prepare, MissingAssets, Prepared};
use crate::proclog::ProcLog;
use crate::Outcome;

pub struct BubblesArgs {
    pub script: PathBuf,
    /// 設定ファイル(省略時は同梱の既定設定)。
    pub config: Option<PathBuf>,
    /// 待ち受けのポート(省略時は OS が決める)。
    pub port: Option<u16>,
    /// ブラウザを自動で開かない。
    pub no_open: bool,
}

/// 実行して終了コードを返す。予期しないエラーは終了コード 2。
pub fn run_bubbles(args: &BubblesArgs) -> Outcome {
    match bubbles(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

fn bubbles(args: &BubblesArgs) -> anyhow::Result<Outcome> {
    // 何かを書く前の検査。失敗は標準エラーにだけ出す(Process.log はまだ触らない)
    let pf = match preflight(&args.script, args.config.as_deref()) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("[ERROR] {msg}");
            eprintln!("[ERROR] 起動していません");
            return Ok(Outcome::Failed);
        }
    };
    // ロックは終了まで持つ(drop で外れる)
    let _lock = pf.lock;
    let mut log = ProcLog::append(&pf.dir.join("Process.log"), "bubbles")?;
    match serve(args, &pf.config.render, &pf.dir, &mut log) {
        Ok(outcome) => {
            if let Err(e) = log.check() {
                eprintln!("[ERROR] {e:#}");
                return Ok(Outcome::Failed);
            }
            Ok(outcome)
        }
        Err(msg) => {
            log.error(&msg);
            if let Err(e) = log.check() {
                eprintln!("[ERROR] {e:#}");
            }
            Ok(Outcome::Failed)
        }
    }
}

/// 起動して終了まで動かす。戻り値は終了コード(起動時の警告があれば 1)。
fn serve(
    args: &BubblesArgs,
    cfg: &s2v_core::config::RenderConfig,
    dir: &std::path::Path,
    log: &mut ProcLog,
) -> Result<Outcome, String> {
    let Prepared {
        timeline,
        face,
        ffmpeg,
        manual,
        items,
        assets,
        warnings,
        ..
    } = prepare(&args.script, cfg, dir, MissingAssets::Allow, log)?;
    let mut warnings = warnings;

    // 素材を固定して開く。開けない素材は「素材なし」として除く
    let mut media: BTreeMap<String, LockedFile> = BTreeMap::new();
    let mut session_assets: BTreeMap<String, SessionAsset> = BTreeMap::new();
    for (cut_id, a) in assets {
        let file = match open_locked(&a.path) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!(
                    "{cut_id}: 素材なし(素材を開けません: {}: {e})",
                    a.path.display()
                );
                log.warn(&msg);
                warnings.push(msg);
                continue;
            }
        };
        let frames = match (&a.video, a.kind) {
            (Some(v), AssetKind::Video) => {
                let n = match (v.duration_ts, v.time_base.as_deref()) {
                    (Some(d), Some(tb)) => asset_frames(d, tb, cfg.fps),
                    _ => None,
                };
                if n.is_none() {
                    let msg = format!(
                        "{cut_id}: 素材の長さを求められません(コマの取り出しは動画の再生に任せます)"
                    );
                    log.warn(&msg);
                    warnings.push(msg);
                }
                n
            }
            _ => None,
        };
        let (matrix, range) = match &a.background {
            Background::Video { matrix, range, .. } => (matrix.clone(), range.clone()),
            _ => (String::new(), String::new()),
        };
        session_assets.insert(
            cut_id.clone(),
            SessionAsset {
                cut_id: cut_id.clone(),
                kind: a.kind,
                looped: a.looped,
                frames,
                matrix,
                range,
                path: a.path,
            },
        );
        media.insert(cut_id, file);
    }
    let wav = dir.join("full_dialogue.wav");
    let audio = open_locked(&wav)
        .map_err(|e| format!("full_dialogue.wav を開けません: {}: {e}", wav.display()))?;

    let session = Session {
        timeline,
        cfg: cfg.clone(),
        face,
        dir: dir.to_path_buf(),
        items,
        assets: session_assets,
        loaded: manual,
        warnings,
    };
    let server = Server::bind(
        session,
        media,
        audio,
        ServeOptions {
            port: args.port,
            ffmpeg,
            frame_worker: None,
        },
    )?;

    // トークンを含む URL は標準出力にだけ出す(Process.log には書かない)
    log.info(format!("待ち受けを始めました(127.0.0.1:{})", server.port()));
    let url = server.url();
    println!("{url}");
    let _ = std::io::stdout().flush();
    if !args.no_open {
        open_browser(&url, log);
    }

    // 起動時の警告の数(ブラウザを開けなかった警告までを含め、実行中の拒否の記録は含めない)
    let startup_warnings = log.warning_count();
    server.run(&mut |ev| match ev {
        LogEvent::Info(m) => log.info(m),
        LogEvent::Warn(m) => log.warn(m),
        LogEvent::Error(m) => log.error(m),
    })?;
    Ok(if startup_warnings > 0 {
        Outcome::Warnings
    } else {
        Outcome::Ok
    })
}

/// 既定のブラウザで開く。失敗は警告(URL は表示済み)。Windows 以外では開かない。
fn open_browser(url: &str, log: &mut ProcLog) {
    // テスト用: 開く処理を失敗させる(製品では設定しない)
    if std::env::var_os("S2V_BUBBLES_TEST_OPEN_FAIL").is_some_and(|v| v == "1") {
        log.warn("ブラウザを開けませんでした(試験用の失敗)。表示した URL を開いてください");
        return;
    }
    #[cfg(windows)]
    {
        // 引数の配列で渡す(シェルの文字列を組み立てない)
        let r = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match r {
            Ok(s) if s.success() => {}
            Ok(s) => log.warn(format!(
                "ブラウザを開けませんでした({s})。表示した URL を開いてください"
            )),
            Err(e) => log.warn(format!(
                "ブラウザを開けませんでした({e})。表示した URL を開いてください"
            )),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (url, log);
    }
}
