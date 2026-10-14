use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
        /// 収録ファイルのフォルダ(省略時は台本と同じフォルダの voices)
        #[arg(long)]
        voice_dir: Option<PathBuf>,
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
        Command::Number { script } => s2v_cli::run_number(&script),
        Command::Build {
            script,
            voice_dir,
            config,
            no_cache,
        } => s2v_cli::run_build(&s2v_cli::BuildArgs {
            script,
            voice_dir,
            config,
            no_cache,
        }),
    };
    ExitCode::from(outcome.code())
}
