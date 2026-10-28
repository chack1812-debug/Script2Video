//! インストール済み VOICEVOX / AivisSpeech のエンジン実行ファイルを自動で探す。
//!
//! VOICEVOX 0.26 以降はエンジンがエディタから分離され、`Program Files\VOICEVOX\vv-engine`
//! から `%APPDATA%\voicevox\vvpp-engines\VOICEVOX_Engine+<UUID>` へ移動した。
//! バージョンアップのたびに `config.toml` の `exe_path` を書き換えずに済むよう、
//! 既知の配置を順に探す。

use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// VOICEVOX 標準エンジンの UUID。エンジンのバージョンに依らず固定。
const VOICEVOX_ENGINE_DIR: &str = "VOICEVOX_Engine+074fc39e-678b-4c13-8916-ffca8d505d1d";

/// 自動探索に対応するエンジン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownEngine {
    Voicevox,
    Aivis,
}

impl KnownEngine {
    /// `config.toml` のセクション名（= エンジン登録名）から対応するエンジンを返す。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "voicevox" => Some(Self::Voicevox),
            "aivis" => Some(Self::Aivis),
            _ => None,
        }
    }
}

/// 探索候補のパスを優先順に返す。`env` は環境変数の読み出し（テストで差し替えるため注入する）。
pub fn candidate_paths(engine: KnownEngine, env: impl Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    // (環境変数, その下の相対パス)
    let locations: &[(&str, &[&str])] = match engine {
        KnownEngine::Voicevox => &[
            // 0.26 以降: エディタから分離されたエンジン
            (
                "APPDATA",
                &["voicevox", "vvpp-engines", VOICEVOX_ENGINE_DIR],
            ),
            (
                "LOCALAPPDATA",
                &["voicevox", "vvpp-engines", VOICEVOX_ENGINE_DIR],
            ),
            // 0.25 以前: エディタ同梱のエンジン（全ユーザー / ユーザー単位インストール）
            ("ProgramFiles", &["VOICEVOX", "vv-engine"]),
            ("LOCALAPPDATA", &["Programs", "VOICEVOX", "vv-engine"]),
        ],
        KnownEngine::Aivis => &[
            ("ProgramFiles", &["AivisSpeech", "AivisSpeech-Engine"]),
            (
                "LOCALAPPDATA",
                &["Programs", "AivisSpeech", "AivisSpeech-Engine"],
            ),
        ],
    };
    locations
        .iter()
        .filter_map(|(var, segments)| {
            let mut path = env(var)?;
            path.extend(segments.iter());
            path.push("run.exe");
            Some(path)
        })
        .collect()
}

/// 実環境（環境変数・ファイルシステム）で [`resolve_exe_path`] を行う。
pub fn resolve_installed_exe_path(name: &str, configured: Option<&str>) -> Option<String> {
    let Some(engine) = KnownEngine::from_name(name) else {
        return configured.map(str::to_string);
    };
    let candidates = candidate_paths(engine, |var| std::env::var_os(var).map(PathBuf::from));
    resolve_exe_path(
        name,
        configured,
        &candidates,
        Path::is_file,
        command_in_path,
    )
}

/// PATH 上のコマンド名（例: "run"）が実在するか。拡張子なしの指定は PATHEXT で補って探す。
fn command_in_path(command: &str) -> bool {
    let Some(dirs) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: Vec<String> = std::env::var("PATHEXT")
        .map(|v| {
            v.split(';')
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|_| vec![".EXE".to_string()]);
    std::env::split_paths(&dirs).any(|dir| {
        let base = dir.join(command);
        base.is_file()
            || exts.iter().any(|ext| {
                let mut with_ext = base.clone().into_os_string();
                with_ext.push(ext);
                Path::new(&with_ext).is_file()
            })
    })
}

/// 使用する実行ファイルを決める。
///
/// - `configured` が存在するファイル、または PATH 上に実在するコマンド名（区切り文字なし）ならそのまま使う。
/// - `configured` が存在しないパス・PATH 上に無いコマンド名なら警告し、`candidates` から探す。
/// - `configured` が未設定なら `candidates` から探す。
///
/// どこにも見つからなければ `configured`（未設定なら `None`）を返し、
/// 起動時のエラーで利用者に知らせる。
pub fn resolve_exe_path(
    name: &str,
    configured: Option<&str>,
    candidates: &[PathBuf],
    is_file: impl Fn(&Path) -> bool,
    in_path: impl Fn(&str) -> bool,
) -> Option<String> {
    if let Some(path) = configured {
        let is_bare_command = !path.contains(['/', '\\']);
        let exists = if is_bare_command {
            in_path(path)
        } else {
            is_file(Path::new(path))
        };
        if exists {
            return Some(path.to_string());
        }
        warn!("[{name}] exe_path が見つかりません。インストール済みのエンジンを探します: {path}");
    }
    match candidates.iter().find(|c| is_file(c)) {
        Some(found) => {
            info!(
                "[{name}] エンジンの実行ファイルを自動検出しました: {}",
                found.display()
            );
            Some(found.to_string_lossy().into_owned())
        }
        None => configured.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    const VOICEVOX_UUID_DIR: &str = "VOICEVOX_Engine+074fc39e-678b-4c13-8916-ffca8d505d1d";

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let map: HashMap<String, PathBuf> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), PathBuf::from(v)))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn full_env() -> impl Fn(&str) -> Option<PathBuf> {
        env_of(&[
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("ProgramFiles", r"C:\Program Files"),
        ])
    }

    fn commands(names: &[&str]) -> impl Fn(&str) -> bool {
        let set: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
        move |c| set.contains(c)
    }

    fn files(paths: &[PathBuf]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.iter().cloned().collect();
        move |p| set.contains(p)
    }

    #[test]
    fn from_name_maps_voicevox_and_aivis_only() {
        assert_eq!(
            KnownEngine::from_name("voicevox"),
            Some(KnownEngine::Voicevox)
        );
        assert_eq!(KnownEngine::from_name("aivis"), Some(KnownEngine::Aivis));
        assert_eq!(KnownEngine::from_name("xtts"), None);
    }

    #[test]
    fn voicevox_candidates_prefer_vvpp_engine_dir_then_legacy_vv_engine() {
        let c = candidate_paths(KnownEngine::Voicevox, full_env());
        assert_eq!(
            c,
            vec![
                PathBuf::from(r"C:\Users\u\AppData\Roaming")
                    .join("voicevox")
                    .join("vvpp-engines")
                    .join(VOICEVOX_UUID_DIR)
                    .join("run.exe"),
                PathBuf::from(r"C:\Users\u\AppData\Local")
                    .join("voicevox")
                    .join("vvpp-engines")
                    .join(VOICEVOX_UUID_DIR)
                    .join("run.exe"),
                PathBuf::from(r"C:\Program Files")
                    .join("VOICEVOX")
                    .join("vv-engine")
                    .join("run.exe"),
                PathBuf::from(r"C:\Users\u\AppData\Local")
                    .join("Programs")
                    .join("VOICEVOX")
                    .join("vv-engine")
                    .join("run.exe"),
            ]
        );
    }

    #[test]
    fn aivis_candidates_cover_machine_and_per_user_install() {
        let c = candidate_paths(KnownEngine::Aivis, full_env());
        assert_eq!(
            c,
            vec![
                PathBuf::from(r"C:\Program Files")
                    .join("AivisSpeech")
                    .join("AivisSpeech-Engine")
                    .join("run.exe"),
                PathBuf::from(r"C:\Users\u\AppData\Local")
                    .join("Programs")
                    .join("AivisSpeech")
                    .join("AivisSpeech-Engine")
                    .join("run.exe"),
            ]
        );
    }

    #[test]
    fn candidates_skip_locations_whose_env_var_is_missing() {
        let c = candidate_paths(
            KnownEngine::Voicevox,
            env_of(&[("ProgramFiles", r"C:\Program Files")]),
        );
        assert_eq!(
            c,
            vec![PathBuf::from(r"C:\Program Files")
                .join("VOICEVOX")
                .join("vv-engine")
                .join("run.exe")]
        );
    }

    #[test]
    fn existing_configured_path_is_used_as_is() {
        let configured = PathBuf::from(r"D:\tools\VOICEVOX\run.exe");
        let found = PathBuf::from(r"C:\auto\run.exe");
        let got = resolve_exe_path(
            "voicevox",
            configured.to_str(),
            std::slice::from_ref(&found),
            files(&[configured.clone(), found.clone()]),
            commands(&[]),
        );
        assert_eq!(got.as_deref(), configured.to_str());
    }

    #[test]
    fn missing_configured_path_falls_back_to_first_existing_candidate() {
        let stale = r"C:\Program Files\VOICEVOX\vv-engine\run.exe";
        let absent = PathBuf::from(r"C:\absent\run.exe");
        let found = PathBuf::from(r"C:\found\run.exe");
        let later = PathBuf::from(r"C:\later\run.exe");
        let got = resolve_exe_path(
            "voicevox",
            Some(stale),
            &[absent, found.clone(), later.clone()],
            files(&[found.clone(), later]),
            commands(&[]),
        );
        assert_eq!(got.as_deref(), found.to_str());
    }

    #[test]
    fn unset_configured_path_uses_discovered_candidate() {
        let found = PathBuf::from(r"C:\found\run.exe");
        let got = resolve_exe_path(
            "aivis",
            None,
            std::slice::from_ref(&found),
            files(std::slice::from_ref(&found)),
            commands(&[]),
        );
        assert_eq!(got.as_deref(), found.to_str());
    }

    #[test]
    fn nothing_found_keeps_configured_value_so_startup_reports_it() {
        let stale = r"C:\Program Files\VOICEVOX\vv-engine\run.exe";
        let got = resolve_exe_path(
            "voicevox",
            Some(stale),
            &[PathBuf::from(r"C:\absent\run.exe")],
            files(&[]),
            commands(&[]),
        );
        assert_eq!(got.as_deref(), Some(stale));
    }

    #[test]
    fn nothing_found_and_unset_stays_none() {
        let got = resolve_exe_path("voicevox", None, &[], files(&[]), commands(&[]));
        assert_eq!(got, None);
    }

    #[test]
    fn bare_command_found_in_path_is_kept_without_searching() {
        let found = PathBuf::from(r"C:\found\run.exe");
        let got = resolve_exe_path(
            "voicevox",
            Some("run"),
            std::slice::from_ref(&found),
            files(std::slice::from_ref(&found)),
            commands(&["run"]),
        );
        assert_eq!(got.as_deref(), Some("run"));
    }

    #[test]
    fn command_in_path_finds_system_command_with_pathext() {
        assert!(command_in_path("cmd"), "cmd.exe は PATH 上にあるはず");
        assert!(!command_in_path("s2v-no-such-command-zz"));
    }

    #[test]
    fn bare_command_missing_from_path_falls_back_to_candidate() {
        let found = PathBuf::from(r"C:\found\run.exe");
        let got = resolve_exe_path(
            "voicevox",
            Some("missing-engine"),
            std::slice::from_ref(&found),
            files(std::slice::from_ref(&found)),
            commands(&[]),
        );
        assert_eq!(got.as_deref(), found.to_str());
    }
}
