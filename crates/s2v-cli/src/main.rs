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
    /// 素材・吹き出し・ト書き・音声を合成して output.mp4 を作る
    Render {
        script: PathBuf,
        /// 設定ファイル(省略時は同梱の既定設定)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// timeline.json から素材のプロンプト(prompts/S01.md …)と一覧(prompts/index.md)を作る
    Prompts {
        script: PathBuf,
        /// 設定ファイル(省略時は同梱の既定設定)
        #[arg(long)]
        config: Option<PathBuf>,
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
        Command::Render { script, config } => {
            s2v_cli::run_render(&s2v_cli::RenderArgs { script, config })
        }
        Command::Prompts { script, config } => {
            s2v_cli::run_prompts(&s2v_cli::PromptsArgs { script, config })
        }
    };
    ExitCode::from(outcome.code())
}
