//! 使い方: cargo run -p s2v-cli --example verify_loudness -- <台本フォルダ>
//! 事前に config.toml で `[normalize] dump_prefade = true` にして build しておく。
//! 終了コード: 0=すべて OK、1=不合格あり、2=確認できない(エラー)。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (code, text) = s2v_cli::verify::run_cli(&args);
    if code == 2 {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
    ExitCode::from(code)
}
