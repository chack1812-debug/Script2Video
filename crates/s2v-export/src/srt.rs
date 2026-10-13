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
            l.text
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
}
