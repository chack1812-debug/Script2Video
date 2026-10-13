//! 使い方: cargo run -p s2v-cli --example verify_loudness -- <台本名の出力フォルダ>
//! 事前に config.toml で `[normalize] dump_prefade = true` にして build しておく。
//! 終了コード: 0=すべて OK、1=不合格あり、2=確認できない(エラー)。

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("使い方: verify_loudness <出力フォルダ>");
        return ExitCode::from(2);
    };
    match s2v_cli::verify::verify_loudness(&dir) {
        Ok(checks) => {
            let mut failed = 0;
            for c in &checks {
                let mark = if c.passed { "OK " } else { "NG " };
                println!("{mark} {:<12} {:?} {}", c.id, c.kind, c.detail);
                if !c.passed {
                    failed += 1;
                }
            }
            println!("{} 件中 {} 件が不合格", checks.len(), failed);
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("[ERROR] {e:#}");
            ExitCode::from(2)
        }
    }
}
