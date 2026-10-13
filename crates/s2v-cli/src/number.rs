//! `script2video number`(台本仕様 §4、設計書 5.1)。

use std::fs;
use std::path::Path;

use s2v_core::numbering::{number_script, IdsRecord};
use s2v_core::ScriptParser;
use s2v_export::render_script_html;

use crate::Outcome;

/// 行番号が付く問題は「N 行目: 」を前置する。行番号 0 は台本全体の問題(番号の枯渇など)。
fn format_issue(line_no: usize, message: &str) -> String {
    if line_no > 0 {
        format!("{line_no} 行目: {message}")
    } else {
        message.to_string()
    }
}

pub fn run_number(script: &Path) -> Outcome {
    let bytes = match fs::read(script) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[ERROR] 台本を読めません: {}: {e}", script.display());
            return Outcome::Failed;
        }
    };
    let Ok(text) = String::from_utf8(bytes.clone()) else {
        eprintln!("[ERROR] 台本が UTF-8 ではありません: {}", script.display());
        return Outcome::Failed;
    };

    let ids_path = script.with_extension("ids");
    let record = fs::read_to_string(&ids_path)
        .map(|s| IdsRecord::parse(&s))
        .unwrap_or_default();

    let numbered = match number_script(&text, record) {
        Ok(n) => n,
        Err(errors) => {
            for e in errors {
                eprintln!("[ERROR] {}", format_issue(e.line_no, &e.message));
            }
            eprintln!("[ERROR] 台本は書き換えていません");
            return Outcome::Failed;
        }
    };

    let mut outcome = Outcome::Ok;
    for w in &numbered.warnings {
        eprintln!("[WARN] {}", format_issue(w.line_no, &w.message));
        outcome = outcome.worst(Outcome::Warnings);
    }

    if numbered.text != text {
        if let Err(e) = fs::write(script.with_extension("bak"), &bytes) {
            eprintln!("[ERROR] .bak を書けません: {e}");
            return Outcome::Failed;
        }
        if let Err(e) = fs::write(script, numbered.text.as_bytes()) {
            eprintln!("[ERROR] 台本を書けません: {e}");
            return Outcome::Failed;
        }
    }
    if let Err(e) = fs::write(&ids_path, numbered.record.to_text()) {
        eprintln!("[ERROR] .ids を書けません: {e}");
        return Outcome::Failed;
    }

    let mut parser = ScriptParser::new();
    match parser.parse_str(&numbered.text) {
        Ok(scenes) => {
            // number は検査をしないので、パーサの問題は警告として表示する
            for w in parser.warnings().iter().chain(parser.errors()) {
                eprintln!("[WARN] {}", format_issue(w.line_no, &w.message));
                outcome = outcome.worst(Outcome::Warnings);
            }
            let title = script
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Err(e) = fs::write(
                script.with_extension("html"),
                render_script_html(&title, &scenes),
            ) {
                eprintln!("[ERROR] 台本HTMLを書けません: {e}");
                return Outcome::Failed;
            }
        }
        Err(e) => {
            eprintln!("[ERROR] 台本を解析できません: {e}");
            return Outcome::Failed;
        }
    }

    println!(
        "台詞 {} 行、シーン {} 個に番号を付けました",
        numbered.assigned_lines, numbered.assigned_scenes
    );
    outcome
}

#[cfg(test)]
mod tests {
    use super::format_issue;

    #[test]
    fn issue_gets_line_prefix_only_when_line_is_known() {
        assert_eq!(format_issue(3, "重複"), "3 行目: 重複");
        assert_eq!(format_issue(0, "番号が尽きました"), "番号が尽きました");
    }
}
