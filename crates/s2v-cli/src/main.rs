use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use s2v_cli::Outcome;

#[derive(Parser)]
#[command(
    name = "script2video",
    version,
    about = "台本と収録音声から、動画用の音声とタイムラインを作る"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 台本に台詞IDとシーン番号を書き込み、台本HTMLを出力する
    Number { script: PathBuf },
    /// 収録音声から音声・字幕・タイムラインを作る
    Build {
        script: PathBuf,
        /// 収録ファイルのフォルダ
        #[arg(long)]
        voice_dir: PathBuf,
        /// 設定ファイル(省略時は同梱の既定設定)
        #[arg(long)]
        config: Option<PathBuf>,
        /// 中間ファイルのキャッシュを使わない
        #[arg(long)]
        no_cache: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Number { .. } | Command::Build { .. } => {
            eprintln!("このコマンドはまだ実装されていません");
            Outcome::Failed
        }
    };
    ExitCode::from(outcome.code())
}
