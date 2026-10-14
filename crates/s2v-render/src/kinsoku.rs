//! 縦書きの列の分割と禁則(映像工程設計書 §6.1)。フォントに依存しない純粋な手順。

use std::ops::Range;

const EPS: f32 = 1e-3;

const NO_START: &str =
    "、。,.・:;?!ー」』)】〉》〕ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々〜，．：；？！）";
const NO_END: &str = "「『(【〈《〔（";

pub fn is_no_start(c: char) -> bool {
    NO_START.contains(c)
}

pub fn is_no_end(c: char) -> bool {
    NO_END.contains(c)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitClass {
    pub height: f32,
    /// 列の頭に置かない(行頭禁則。単位の最初の文字で判定)
    pub no_start: bool,
    /// 列の末尾に置かない(行末禁則。単位の最後の文字で判定)
    pub no_end: bool,
}

pub fn break_columns(units: &[UnitClass], inner_h: f32) -> Vec<Range<usize>> {
    if units.is_empty() {
        return Vec::new();
    }
    let height = |col: &[usize]| col.iter().map(|&i| units[i].height).sum::<f32>();
    // 1. 詰め込み
    let mut cols: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut h = 0.0f32;
    for (i, unit) in units.iter().enumerate() {
        if !cur.is_empty() && h + unit.height > inner_h + EPS {
            cols.push(std::mem::take(&mut cur));
            h = 0.0;
        }
        cur.push(i);
        h += unit.height;
    }
    cols.push(cur);
    // 2. 修復
    let mut k = 0;
    while k + 1 < cols.len() {
        let saved = (cols[k].clone(), cols[k + 1].clone());
        let mut moves = 0;
        let last = |c: &Vec<usize>| units[*c.last().expect("列は空にならない")];
        if cols[k].len() >= 2 && last(&cols[k]).no_end {
            let x = cols[k].pop().expect("2つ以上");
            cols[k + 1].insert(0, x);
            moves += 1;
        }
        while moves < 2 && cols[k].len() >= 2 && units[cols[k + 1][0]].no_start {
            let x = cols[k].pop().expect("2つ以上");
            cols[k + 1].insert(0, x);
            moves += 1;
        }
        if last(&cols[k]).no_end || units[cols[k + 1][0]].no_start {
            cols[k] = saved.0;
            cols[k + 1] = saved.1;
        }
        while height(&cols[k + 1]) > inner_h + EPS && cols[k + 1].len() >= 2 {
            let x = cols[k + 1].pop().expect("2つ以上");
            if k + 2 == cols.len() {
                cols.push(Vec::new());
            }
            cols[k + 2].insert(0, x);
        }
        k += 1;
    }
    cols.iter().map(|c| c[0]..c[c.len() - 1] + 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(h: f32) -> UnitClass {
        UnitClass {
            height: h,
            no_start: false,
            no_end: false,
        }
    }
    fn s(h: f32) -> UnitClass {
        UnitClass {
            height: h,
            no_start: true,
            no_end: false,
        }
    }
    fn e(h: f32) -> UnitClass {
        UnitClass {
            height: h,
            no_start: false,
            no_end: true,
        }
    }

    #[test]
    fn classifies_characters() {
        for c in "、。，．・：；？！ー」』）】〉》〕ぁっゃァッヵ々〜".chars()
        {
            assert!(is_no_start(c), "{c}");
        }
        for c in "「『（【〈《〔(".chars() {
            assert!(is_no_end(c), "{c}");
        }
        for c in "あア港A1".chars() {
            assert!(!is_no_start(c) && !is_no_end(c), "{c}");
        }
    }

    #[test]
    fn greedy_fill() {
        assert_eq!(break_columns(&[u(1.0); 5], 2.0), vec![0..2, 2..4, 4..5]);
        assert_eq!(
            break_columns(&[], 2.0),
            Vec::<std::ops::Range<usize>>::new()
        );
        // 1つで高さを超える単位は単独の列
        assert_eq!(
            break_columns(&[u(1.0), u(3.0), u(1.0)], 2.0),
            vec![0..1, 1..2, 2..3]
        );
    }

    #[test]
    fn no_start_at_column_head_pushes_previous_unit() {
        // あい|、う → あ|い、う
        assert_eq!(
            break_columns(&[u(1.0), u(1.0), s(1.0), u(1.0)], 2.0),
            vec![0..1, 1..3, 3..4]
        );
    }

    #[test]
    fn no_end_at_column_tail_moves_forward() {
        // あ「|いう → あ|「い|う
        assert_eq!(
            break_columns(&[u(1.0), e(1.0), u(1.0), u(1.0)], 2.0),
            vec![0..1, 1..3, 3..4]
        );
    }

    #[test]
    fn push_out_overflow_and_revert() {
        // あいう|、。え(高さ3): う を送って解消 → 列(う、。え)が高さ4であふれ、え を次の列へ
        let units = [u(1.0), u(1.0), u(1.0), s(1.0), s(1.0), u(1.0)];
        assert_eq!(break_columns(&units, 3.0), vec![0..2, 2..5, 5..6]);
        // あい|、。|」(高さ2): 境目0は い を送って解消し、あふれた 。 が次の列の頭へ。
        // 境目1は、列(い、)の 、 を送っても次の列の頭が 、 のままで、列が1つになって追い出せない → 取り消す
        let units = [u(1.0), u(1.0), s(1.0), s(1.0), s(1.0)];
        assert_eq!(break_columns(&units, 2.0), vec![0..1, 1..3, 3..5]);
    }

    #[test]
    fn single_unit_column_is_not_pushed() {
        // 列 k が1つだけなら追い出せない → そのまま
        assert_eq!(break_columns(&[u(2.0), s(1.0)], 2.0), vec![0..1, 1..2]);
    }

    #[test]
    fn overflow_cascades_to_new_columns() {
        let units = [u(1.0), u(1.0), s(1.0), u(1.0), u(1.0)];
        let cols = break_columns(&units, 2.0);
        // どの列も高さ2以下(1つだけの列を除く)、全ての単位を1回ずつ含む
        let covered: usize = cols.iter().map(|r| r.len()).sum();
        assert_eq!(covered, 5);
        for w in cols.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        for r in &cols {
            let h: f32 = units[r.clone()].iter().map(|x| x.height).sum();
            assert!(h <= 2.0 + 1e-3 || r.len() == 1, "{cols:?}");
        }
        assert_eq!(cols, vec![0..1, 1..3, 3..5]);
    }

    #[test]
    fn deterministic_for_same_input() {
        let units: Vec<UnitClass> = (0..40)
            .map(|i| UnitClass {
                height: 1.0 + (i % 3) as f32 * 0.25,
                no_start: i % 7 == 3,
                no_end: i % 11 == 5,
            })
            .collect();
        assert_eq!(break_columns(&units, 5.0), break_columns(&units, 5.0));
    }
}
