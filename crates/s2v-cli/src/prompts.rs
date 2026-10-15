//! `script2video prompts`(映像工程設計書 §7)。
//! `timeline.json` から素材のプロンプト(`prompts/S01.md` …)と一覧(`prompts/index.md`)を作る。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use s2v_core::config::PromptsConfig;
use s2v_core::Config;
use s2v_render::assets::match_assets;

use crate::layout::{
    check_contained, check_not_symlink, check_reserved_name, script_dir, script_folder,
};
use crate::proclog::ProcLog;
use crate::prompts_md::{asset_query, cuts_without_visual, index_markdown, scene_markdown};
use crate::Outcome;

pub struct PromptsArgs {
    pub script: PathBuf,
    /// 設定ファイル(省略時は同梱の既定設定)。
    pub config: Option<PathBuf>,
}

/// 実行して終了コードを返す。予期しないエラーは終了コード 2。
pub fn run_prompts(args: &PromptsArgs) -> Outcome {
    match prompts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            Outcome::Failed
        }
    }
}

fn prompts(args: &PromptsArgs) -> anyhow::Result<Outcome> {
    // 何かを書く前の検査。失敗は標準エラーにだけ出す(Process.log はまだ触らない)
    let fail = |msg: String| -> anyhow::Result<Outcome> {
        eprintln!("[ERROR] {msg}");
        eprintln!("[ERROR] 出力していません");
        Ok(Outcome::Failed)
    };
    if let Err(e) = check_reserved_name(&args.script) {
        return fail(format!("{e:#}"));
    }
    if let Err(e) = check_not_symlink(&args.script) {
        return fail(format!("{e:#}"));
    }
    if !args.script.is_file() {
        return fail(format!("台本を読めません: {}", args.script.display()));
    }
    let config = match &args.config {
        Some(p) => match Config::from_file(p) {
            Ok(c) => c,
            Err(e) => return fail(format!("設定ファイルを読めません: {}: {e:#}", p.display())),
        },
        None => Config::builtin(),
    };
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        return fail(config_errors.join("\n[ERROR] "));
    }
    let dir = script_dir(&args.script);
    let _lock = match crate::lock::ScriptLock::acquire(&dir) {
        Ok(l) => l,
        Err(e) => return fail(format!("{e:#}")),
    };

    // Process.log(追記)
    let mut log = ProcLog::append(&dir.join("Process.log"), "prompts")?;
    match prompts_main(args, &config.prompts, &dir, &mut log) {
        Ok(()) if log.warning_count() > 0 => Ok(Outcome::Warnings),
        Ok(()) => Ok(Outcome::Ok),
        Err(msg) => {
            log.error(&msg);
            if let Err(e) = log.check() {
                eprintln!("[ERROR] {e:#}");
            }
            Ok(Outcome::Failed)
        }
    }
}

/// 自分が作るファイルの名前か(`index.md`、または `S` + ASCII 数字1桁以上 + `.md`)。
pub(crate) fn is_generated_name(name: &str) -> bool {
    if name == "index.md" {
        return true;
    }
    let Some(stem) = name.strip_prefix('S').and_then(|s| s.strip_suffix(".md")) else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
}

fn prompts_main(
    args: &PromptsArgs,
    cfg: &PromptsConfig,
    dir: &Path,
    log: &mut ProcLog,
) -> Result<(), String> {
    // 鮮度(音声は見ない)。台本の変更は警告
    let fresh = crate::freshness::check_fresh(&args.script, false)?;
    for w in &fresh.warnings {
        log.warn(w);
    }
    let tl = fresh.timeline;

    // prompts の検査: ファイルなら失敗。リンク・台本フォルダの外も失敗(何も消さない)
    let out_dir = dir.join("prompts");
    if let Ok(meta) = std::fs::symlink_metadata(&out_dir) {
        if !meta.file_type().is_symlink() && !meta.is_dir() {
            return Err("台本フォルダに prompts という名前のファイルがあります".to_string());
        }
    }
    let cwd =
        std::env::current_dir().map_err(|e| format!("現在のフォルダを取得できません: {e}"))?;
    let root = script_folder(&args.script, &cwd);
    check_contained(&root, "prompts").map_err(|e| format!("{e:#}"))?;

    // 中身を作る(ファイルにはまだ触らない)
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for sc in &tl.scenes {
        if let Some(md) = scene_markdown(&tl, sc, cfg) {
            files.insert(format!("{}.md", sc.id), md);
        }
    }
    let assets = match_assets(&dir.join("tracks"), &asset_query(&tl));
    for e in &assets.errors {
        log.info(format!("tracks/ の確認事項: {e}"));
    }
    files.insert("index.md".to_string(), index_markdown(&tl, &assets));
    for id in cuts_without_visual(&tl) {
        let scene = id.split('-').next().unwrap_or(&id);
        log.warn(format!(
            "{id}: #visual がありません(prompts/{scene}.md に「なし」と書きました)"
        ));
    }

    // prompts/ を用意し、新しい集合を先に書く(同名は置き換わる。途中で失敗しても既存のファイルは消えない)
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("prompts/ を作れません: {}: {e}", out_dir.display()))?;
    for (name, text) in &files {
        crate::fsutil::write_atomic(&out_dir.join(name), text.as_bytes())
            .map_err(|e| format!("prompts/{name} を書けません: {e}"))?;
    }
    // 全部書けた後で、今回の集合に無い前回の生成ファイルだけを消す
    let entries = std::fs::read_dir(&out_dir)
        .map_err(|e| format!("prompts/ を読めません: {}: {e}", out_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("prompts/ を読めません: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_generated_name(&name) || files.contains_key(&name) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        // フォルダは消さない
        if meta.is_file() || meta.file_type().is_symlink() {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("prompts/{name} を消せません: {e}"))?;
        }
    }
    log.info(format!(
        "prompts/ に {} 個のファイルを書きました",
        files.len()
    ));
    log.check().map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_are_exact() {
        for n in ["index.md", "S01.md", "S00.md", "S123.md", "S1.md"] {
            assert!(is_generated_name(n), "{n}");
        }
        for n in [
            "memo.md",
            "S01_note.md",
            "S1x.md",
            "s01.md",
            "S01.md.bak",
            "S.md",
            "Index.md",
            "S01.txt",
            "S０１.md",
            "xS01.md",
        ] {
            assert!(!is_generated_name(n), "{n}");
        }
    }
}
