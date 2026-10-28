pub mod discover;
pub mod engine;
pub mod http_engine;
mod job;
mod lock;
mod process;
pub mod xtts_engine;

pub use engine::{Engine, EngineManager, SynthRequest};
pub use http_engine::HttpEngine;
pub use xtts_engine::XttsEngine;

use std::sync::Arc;
use std::time::Duration;

use s2v_core::config::{EnginesConfig, HttpConfig};

/// 設定の connect/request タイムアウトで、全エンジン共有の `Client` を構築する。
fn build_http_client(http: &HttpConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(http.connect_timeout_s))
        .timeout(Duration::from_secs(http.request_timeout_s))
        .build()
        .expect("reqwest Client の構築に失敗しました")
}

/// 設定から3エンジン(voicevox/aivis/xtts)を登録した `EngineManager` を構築する。
/// voicevox/aivis の実行ファイルは、`exe_path` が無効または未設定ならインストール先から自動で探す。
pub fn build_engine_manager(cfg: &EnginesConfig) -> EngineManager {
    build_engine_manager_with_paths(cfg).0
}

/// [`build_engine_manager`] に加えて、エンジンごとに決まった実行ファイルのパス(見つからなければ `None`)を返す。
/// 返す順は voicevox、aivis、xtts。
pub fn build_engine_manager_with_paths(
    cfg: &EnginesConfig,
) -> (EngineManager, Vec<(&'static str, Option<String>)>) {
    let timeout = |secs: Option<u64>| Duration::from_secs(secs.unwrap_or(60));
    let client = Arc::new(build_http_client(&cfg.http));
    let voicevox_exe =
        discover::resolve_installed_exe_path("voicevox", cfg.voicevox.exe_path.as_deref());
    let aivis_exe = discover::resolve_installed_exe_path("aivis", cfg.aivis.exe_path.as_deref());
    let xtts_exe = cfg.xtts.exe_path.clone();
    let paths = vec![
        ("voicevox", voicevox_exe.clone()),
        ("aivis", aivis_exe.clone()),
        ("xtts", xtts_exe.clone()),
    ];
    let mut em = EngineManager::new();
    em.register(
        "voicevox",
        Arc::new(
            HttpEngine::with_exe_path(
                "voicevox",
                &cfg.voicevox.url,
                Arc::clone(&client),
                voicevox_exe,
            )
            .with_args(cfg.voicevox.args.clone())
            .with_startup_timeout(timeout(cfg.voicevox.startup_timeout_s)),
        ),
    );
    em.register(
        "aivis",
        Arc::new(
            HttpEngine::with_exe_path("aivis", &cfg.aivis.url, Arc::clone(&client), aivis_exe)
                .with_args(cfg.aivis.args.clone())
                .with_startup_timeout(timeout(cfg.aivis.startup_timeout_s)),
        ),
    );
    em.register(
        "xtts",
        Arc::new(
            XttsEngine::with_exe_path("xtts", &cfg.xtts.url, Arc::clone(&client), xtts_exe)
                .with_args(cfg.xtts.args.clone())
                .with_startup_timeout(timeout(cfg.xtts.startup_timeout_s)),
        ),
    );
    (em, paths)
}

#[cfg(test)]
mod build_tests {
    use super::*;

    #[test]
    fn builds_manager_from_default_config() {
        let _em = build_engine_manager(&EnginesConfig::default());
    }

    #[test]
    fn paths_follow_the_configured_exe_path() {
        let mut cfg = EnginesConfig::default();
        cfg.xtts.exe_path = Some("python".to_string());
        let (_em, paths) = build_engine_manager_with_paths(&cfg);
        let names: Vec<&str> = paths.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["voicevox", "aivis", "xtts"]);
        assert_eq!(paths[2].1.as_deref(), Some("python"));
    }
}
