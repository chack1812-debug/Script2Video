//! 出力の配置の安全確認(何かを書く・消す前に行う)。パスの正規化は `cwd` を引数に取る純粋な関数にしてある。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::bail;

/// `work/` の中のフォルダ(これらもリンクで台本フォルダの外へ出してはいけない)。
pub const WORK_SUBDIRS: [&str; 3] = ["resampled", "lines", "lines_prefade"];

/// 台本のファイル名にできない、出力側の名前(大文字小文字は区別しない。映像工程設計書 §2.2)。
pub const RESERVED_NAMES: [&str; 8] = [
    "full_dialogue.wav",
    "Process.log",
    "output.mp4",
    "output.stale.mp4",
    "bubbles.json",
    "bubbles.json.bak",
    ".script2video.lock",
    "prompts",
];

/// 台本のファイル名が予約名なら Err。どのコマンドも、何かを書く前に呼ぶ。
pub fn check_reserved_name(script: &Path) -> anyhow::Result<()> {
    let name = script
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if RESERVED_NAMES.iter().any(|r| name.eq_ignore_ascii_case(r)) {
        bail!("台本のファイル名 {name} は出力ファイルと同じ名前です。台本の名前を変えてください");
    }
    Ok(())
}
/// 収録フォルダにできない、出力側のフォルダ名。
pub const OUTPUT_DIRS: [&str; 3] = ["work", "tracks", "timeline"];

/// Windows の `\\?\` 形式(verbatim)の接頭辞を外す。
pub fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// 絶対パスにして `.` `..` を字面で畳み、存在する最も深い祖先を canonicalize して、残りを付け直す。
/// 存在しないパスや相対パスでも比較できる。
pub fn normalize_path(p: &Path, cwd: &Path) -> PathBuf {
    let abs = if p.is_absolute() || p.has_root() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let mut lex = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // ルートや接頭辞より上には行かない
                if matches!(lex.components().next_back(), Some(Component::Normal(_))) {
                    lex.pop();
                }
            }
            other => lex.push(other.as_os_str()),
        }
    }
    // 存在する最も深い祖先
    let mut existing = lex.clone();
    let mut rest: Vec<OsString> = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let base = std::fs::canonicalize(&existing).unwrap_or(existing);
    let mut out = PathBuf::from(strip_verbatim(&base.to_string_lossy()));
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

fn key(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) {
                s.to_lowercase()
            } else {
                s
            }
        })
        .collect()
}

/// 正規化済みのパスが同じ場所か(Windows では大文字小文字を区別しない)。
pub fn same_path(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// `inner` が `root` の中(同じ場所を含む)か。両方とも正規化済みであること。
pub fn is_inside(inner: &Path, root: &Path) -> bool {
    let (i, r) = (key(inner), key(root));
    i.len() >= r.len() && i[..r.len()] == r[..]
}

/// 台本フォルダ(相対の台本名だけなら `cwd`)。正規化済み。
pub fn script_folder(script: &Path, cwd: &Path) -> PathBuf {
    let parent = script.parent().unwrap_or(Path::new(""));
    normalize_path(parent, cwd)
}

/// 出力用フォルダが台本フォルダの外へのリンクでないことを確かめる(リンク越しに削除しないため)。
pub fn check_contained(root: &Path, name: &str) -> anyhow::Result<()> {
    let p = name
        .split('/')
        .fold(root.to_path_buf(), |acc, c| acc.join(c));
    let Ok(meta) = std::fs::symlink_metadata(&p) else {
        return Ok(()); // まだ無い
    };
    if meta.file_type().is_symlink() {
        bail!(
            "{} はリンク(シンボリックリンク・ジャンクション)です。台本フォルダの中の実際のフォルダにしてください",
            p.display()
        );
    }
    let real = normalize_path(&p, root);
    let real_root = normalize_path(root, root);
    if !is_inside(&real, &real_root) {
        bail!(
            "{} が台本フォルダの外を指しています。台本フォルダの中の実際のフォルダにしてください",
            p.display()
        );
    }
    Ok(())
}

/// 配置の確認。何かを書く前に呼ぶ。
pub fn check_layout(script: &Path, voice_dir: Option<&Path>, cwd: &Path) -> anyhow::Result<()> {
    check_reserved_name(script)?;
    let root = script_folder(script, cwd);
    if let Some(v) = voice_dir {
        let v_norm = normalize_path(v, cwd);
        for sub in OUTPUT_DIRS {
            if same_path(&v_norm, &root.join(sub)) {
                bail!(
                    "収録フォルダ {} が出力用の {sub}/ と同じ場所です。収録フォルダを別にしてください",
                    v.display()
                );
            }
        }
    }
    for sub in OUTPUT_DIRS {
        check_contained(&root, sub)?;
    }
    for sub in WORK_SUBDIRS {
        check_contained(&root, &format!("work/{sub}"))?;
    }
    if voice_dir.is_none() {
        check_contained(&root, "voices")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn verbatim_prefix_is_just_a_prefix() {
        assert_eq!(strip_verbatim("\\\\?\\C:\\a\\b"), "C:\\a\\b");
        assert_eq!(
            strip_verbatim("\\\\?\\UNC\\srv\\share\\a"),
            "\\\\srv\\share\\a"
        );
        assert_eq!(strip_verbatim("C:\\a"), "C:\\a");
        assert_eq!(strip_verbatim("/a/b"), "/a/b");
    }

    #[test]
    fn relative_and_missing_paths_normalize_against_cwd() {
        let t = tmp();
        let cwd = t.path();
        let n = normalize_path(Path::new("work"), cwd);
        assert!(same_path(&n, &normalize_path(&cwd.join("work"), cwd)));
        let dotted = normalize_path(&PathBuf::from(".").join("a").join("..").join("work"), cwd);
        assert!(same_path(&dotted, &n));
        // ルートより上には行かない
        let mut up = PathBuf::new();
        for _ in 0..40 {
            up.push("..");
        }
        assert!(normalize_path(&up.join("x"), cwd).is_absolute());
    }

    #[test]
    fn case_variants_are_the_same_only_on_windows() {
        let t = tmp();
        let a = normalize_path(Path::new("WORK"), t.path());
        let b = normalize_path(Path::new("work"), t.path());
        assert_eq!(same_path(&a, &b), cfg!(windows));
    }

    #[test]
    fn relative_voice_dir_is_checked_against_script_folder_with_explicit_cwd() {
        let t = tmp();
        let script = t.path().join("s.txt");
        // cwd = 台本フォルダ、work/ はまだ無い
        for bad in ["work", "./work", "tracks", "timeline", "sub/../timeline"] {
            let e = check_layout(&script, Some(Path::new(bad)), t.path());
            assert!(e.is_err(), "{bad}");
        }
        // ..\<台本フォルダ名>\tracks
        let folder = t.path().file_name().unwrap().to_owned();
        let elsewhere = t.path().join("sub");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let via_parent = PathBuf::from("..").join(&folder).join("tracks");
        let sibling = t.path().parent().unwrap().join("other-cwd");
        assert!(check_layout(&script, Some(&via_parent), &sibling).is_err());
        // cwd が別の場所なら、相対の work は台本フォルダの work ではない
        assert!(check_layout(&script, Some(Path::new("work")), &elsewhere).is_ok());
        // 通常の voices は問題ない
        assert!(check_layout(&script, Some(Path::new("voices")), t.path()).is_ok());
        assert!(check_layout(&script, None, t.path()).is_ok());
    }

    #[test]
    fn bare_script_name_uses_cwd_as_script_folder() {
        let t = tmp();
        let r = check_layout(Path::new("s.txt"), Some(Path::new("work")), t.path());
        assert!(r.is_err());
        assert!(check_layout(Path::new("s.txt"), None, t.path()).is_ok());
        assert!(same_path(
            &script_folder(Path::new("s.txt"), t.path()),
            &normalize_path(t.path(), t.path())
        ));
    }

    #[test]
    fn reserved_names_ignore_case() {
        let t = tmp();
        for n in [
            "PROCESS.LOG",
            "Full_Dialogue.WAV",
            "OUTPUT.MP4",
            "output.mp4",
            "Output.Stale.MP4",
            "BUBBLES.JSON",
            "bubbles.json.bak",
            ".script2video.lock",
            "Prompts",
        ] {
            assert!(
                check_layout(&t.path().join(n), None, t.path()).is_err(),
                "{n}"
            );
        }
        assert!(check_layout(&t.path().join("voices.txt"), None, t.path()).is_ok());
    }

    #[test]
    fn containment_is_checked_with_explicit_paths() {
        let root = PathBuf::from("a").join("script");
        assert!(is_inside(&root.join("tracks"), &root));
        assert!(is_inside(&root, &root));
        assert!(!is_inside(&PathBuf::from("a").join("foreign"), &root));
        assert!(!is_inside(
            &PathBuf::from("a").join("script2").join("x"),
            &root
        ));
    }

    #[test]
    fn work_subfolders_are_checked_too() {
        let t = tmp();
        std::fs::create_dir_all(t.path().join("work").join("lines")).unwrap();
        assert!(check_contained(t.path(), "work/lines").is_ok());
        assert!(check_contained(t.path(), "work/resampled").is_ok());
        let script = t.path().join("s.txt");
        assert!(check_layout(&script, None, t.path()).is_ok());
    }

    #[test]
    fn real_subfolders_pass() {
        let t = tmp();
        std::fs::create_dir_all(t.path().join("tracks")).unwrap();
        assert!(check_contained(t.path(), "tracks").is_ok());
        assert!(check_contained(t.path(), "timeline").is_ok());
    }
}
