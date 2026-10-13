//! Script2Video のコマンド実装。`main.rs` は引数の解析だけを行い、ここを呼ぶ。

pub mod build;
pub mod number;
pub mod proclog;
pub mod work;

pub use build::{run_build, BuildArgs};
pub use number::run_number;

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
