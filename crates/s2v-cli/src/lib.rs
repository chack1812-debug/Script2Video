//! Script2Video のコマンド実装。`main.rs` は引数の解析だけを行い、ここを呼ぶ。

pub mod bubbles;
pub mod build;
pub mod freshness;
pub mod fsutil;
pub mod layout;
pub mod lock;
pub mod number;
pub mod prepare;
pub mod proclog;
pub mod prompts;
pub mod prompts_md;
pub mod render;
pub mod verify;
pub mod work;

pub use bubbles::{run_bubbles, BubblesArgs};
pub use build::{run_build, BuildArgs};
pub use number::run_number;
pub use prompts::{run_prompts, PromptsArgs};
pub use render::{run_render, RenderArgs};

/// 終了コード(設計書 5.6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 全件OK
    Ok = 0,
    /// 警告・不足あり(出力は完成)
    Warnings = 1,
    /// 台本の構文エラー等(出力なし)
    Failed = 2,
}

impl Outcome {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 2つの結果のうち重い方を返す。
    pub fn worst(self, other: Outcome) -> Outcome {
        if other.code() > self.code() {
            other
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_picks_the_more_severe_outcome() {
        assert_eq!(Outcome::Ok.worst(Outcome::Warnings), Outcome::Warnings);
        assert_eq!(Outcome::Failed.worst(Outcome::Warnings), Outcome::Failed);
        assert_eq!(Outcome::Warnings.worst(Outcome::Ok), Outcome::Warnings);
    }
}
