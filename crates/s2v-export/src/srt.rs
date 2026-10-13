//! 字幕(SRT)。台詞を開始時刻の順に並べる。不足の台詞も出力する。

use s2v_core::placement::PlacedLine;
use s2v_core::time::{ceil_ms, floor_ms};

fn srt_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms % 3_600_000) / 60_000,
        (ms % 60_000) / 1_000,
        ms % 1_000
    )
}

/// 字幕本文を整える。改行を LF にそろえ、空行・空白だけの行は空白1つにする
/// (空行はキューの終わりと解釈されるため)。全体が空なら空白1つの行にする。
fn cue_text(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let unified = unified.trim_end_matches('\n');
    unified
        .split('\n')
        .map(|l| if l.trim().is_empty() { " " } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_srt(lines: &[PlacedLine]) -> String {
    let mut sorted: Vec<&PlacedLine> = lines.iter().collect();
    sorted.sort_by_key(|l| (l.start, l.flat_index));
    let mut out = String::new();
    for (i, l) in sorted.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            srt_time(floor_ms(l.start)),
            srt_time(ceil_ms(l.end)),
            cue_text(&l.text)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::ids::LineId;

    #[test]
    fn lines_are_sorted_and_times_use_floor_and_ceil() {
        let line = |n: u32, start: i64, end: i64, text: &str| PlacedLine {
            id: LineId { scene: 1, line: n },
            scene: 1,
            cast: "A".into(),
            text: text.into(),
            start,
            end,
            missing: None,
            flat_index: n as usize,
        };
        let srt = render_srt(&[line(2, 72_000, 120_001, "二"), line(1, 0, 48_000, "一")]);
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:01,000\n一\n\n2\n00:00:01,500 --> 00:00:02,501\n二\n\n"
        );
    }

    #[test]
    fn empty_is_empty_string_and_ties_keep_script_order() {
        assert_eq!(render_srt(&[]), "");
        let line = |n: u32, idx: usize, text: &str| PlacedLine {
            id: LineId { scene: 1, line: n },
            scene: 1,
            cast: "A".into(),
            text: text.into(),
            start: 0,
            end: 48,
            missing: None,
            flat_index: idx,
        };
        let srt = render_srt(&[line(2, 5, "後"), line(1, 3, "先")]);
        assert!(srt.find("先").unwrap() < srt.find("後").unwrap());
    }

    #[test]
    fn hours_and_missing_lines_are_rendered() {
        let l = PlacedLine {
            id: LineId { scene: 1, line: 1 },
            scene: 1,
            cast: "A".into(),
            text: "不足".into(),
            start: 3_600_000 * 48,
            end: 3_600_000 * 48 + 48,
            missing: Some(s2v_core::placement::MissingReason::NoFile),
            flat_index: 0,
        };
        assert_eq!(
            render_srt(&[l]),
            "1\n01:00:00,000 --> 01:00:00,001\n不足\n\n"
        );
    }

    fn one(text: &str) -> String {
        render_srt(&[PlacedLine {
            id: LineId { scene: 1, line: 1 },
            scene: 1,
            cast: "A".into(),
            text: text.into(),
            start: 0,
            end: 48,
            missing: None,
            flat_index: 0,
        }])
    }

    #[test]
    fn crlf_and_multiline_text_is_normalized() {
        assert_eq!(
            one("一\r\n二\r三"),
            "1\n00:00:00,000 --> 00:00:00,001\n一\n二\n三\n\n"
        );
    }

    #[test]
    fn blank_lines_inside_text_do_not_end_the_cue() {
        let srt = one("一行目\n\n二行目\n");
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:00,001\n一行目\n \n二行目\n\n"
        );
        // 空行はキューの区切りの1つだけ
        assert_eq!(srt.matches("\n\n").count(), 1);
    }

    #[test]
    fn empty_or_whitespace_text_becomes_a_single_space_line() {
        let expect = "1\n00:00:00,000 --> 00:00:00,001\n \n\n";
        assert_eq!(one(""), expect);
        assert_eq!(one("  \t"), expect);
        assert_eq!(one("\n\n"), expect);
    }
}
