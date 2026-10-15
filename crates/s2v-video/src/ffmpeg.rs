//! ffmpeg の版の判定と、カット・連結・多重化の引数の組み立て(プロセスは起動しない)。

/// `ffmpeg -version` の判定結果。
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheck {
    Ok,
    TooOld(String),
    Unknown(String),
}

/// 版として受ける最小の主版(リリース版)。
const MIN_MAJOR: u32 = 6;
/// git の日付版として受ける最小の年。
const MIN_GIT_YEAR: u32 = 2023;

/// `ffmpeg -version` の1行目を判定する。
/// "ffmpeg version 6.1.1 …"・"n7.1"・git の日付版 "2026-03-26-git-…"(2023 年以降)を受ける。
pub fn check_version(first_line: &str) -> VersionCheck {
    check_tool_version("ffmpeg", first_line)
}

/// `ffprobe -version` の1行目を判定する(規則は `check_version` と同じ。接頭辞は "ffprobe version ")。
pub fn check_ffprobe_version(first_line: &str) -> VersionCheck {
    check_tool_version("ffprobe", first_line)
}

fn check_tool_version(program: &str, first_line: &str) -> VersionCheck {
    let unknown = || VersionCheck::Unknown(format!("{program} の版を読み取れません: {first_line}"));
    let Some(rest) = first_line
        .trim()
        .strip_prefix(program)
        .and_then(|r| r.strip_prefix(" version "))
    else {
        return unknown();
    };
    let token = rest.split_whitespace().next().unwrap_or("");
    let too_old = |need: String| {
        VersionCheck::TooOld(format!("{program} が古すぎます({token})。{need}が必要です"))
    };
    // (c) 日付の git 版 YYYY-MM-DD-git-…
    if let Some(year) = parse_git_date(token) {
        return if year >= MIN_GIT_YEAR {
            VersionCheck::Ok
        } else {
            too_old(format!("{MIN_GIT_YEAR} 年以降のビルド"))
        };
    }
    // (a) X.Y / X.Y.Z、(b) nX.Y(直後は終わりか '-')
    let num = token.strip_prefix('n').unwrap_or(token);
    if let Some(major) = parse_release(num) {
        return if major >= MIN_MAJOR {
            VersionCheck::Ok
        } else {
            too_old(format!("{MIN_MAJOR} 以上の版"))
        };
    }
    unknown()
}

/// `X.Y` または `X.Y.Z`(直後は終わりか '-')なら主版を返す。
fn parse_release(s: &str) -> Option<u32> {
    let end = s.find('-').unwrap_or(s.len());
    let parts: Vec<&str> = s[..end].split('.').collect();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    parts[0].parse().ok()
}

/// `YYYY-MM-DD-git-…`(`-git-` の後ろに1文字以上、実在する日付)なら年を返す。
fn parse_git_date(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || !s[10..].starts_with("-git-") {
        return None;
    }
    let num = |r: std::ops::Range<usize>| {
        let t = &s[r];
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse::<u32>().ok()
        } else {
            None
        }
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=days).contains(&d).then_some(y)
}

/// 背景の収め方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Contain,
    Cover,
}

/// エンコード設定。
#[derive(Debug, Clone)]
pub struct EncodeSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub preset: String,
    pub threads: u32,
    pub fit: Fit,
    pub background: [u8; 3],
}

/// 背景の素材。
#[derive(Debug, Clone)]
pub enum Background {
    Png {
        path: String,
    },
    Jpeg {
        path: String,
    },
    Video {
        path: String,
        looped: bool,
        matrix: String,
        range: String,
    },
}

/// 先頭に付ける共通の引数。
fn base_args() -> Vec<String> {
    ["-nostdin", "-hide_banner", "-n"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 入力ごとに `-i` の直前へ置く入力オプション。ネットワークなどの file 以外のプロトコルで読ませない。
const PROTOCOL_WHITELIST: [&str; 2] = ["-protocol_whitelist", "file"];

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// カットファイルを作る ffmpeg の引数。
pub fn cut_args(
    bg: &Background,
    overlay_list: &str,
    n: i64,
    out: &str,
    s: &EncodeSettings,
) -> Vec<String> {
    let mut a = base_args();
    match bg {
        Background::Png { path } | Background::Jpeg { path } => {
            a.extend(strs(&["-loop", "1", "-framerate"]));
            a.push(s.fps.to_string());
            a.extend(strs(&PROTOCOL_WHITELIST));
            a.push("-i".into());
            a.push(path.clone());
        }
        Background::Video { path, looped, .. } => {
            if *looped {
                a.extend(strs(&["-stream_loop", "-1"]));
            }
            a.extend(strs(&PROTOCOL_WHITELIST));
            a.push("-i".into());
            a.push(path.clone());
        }
    }
    a.extend(strs(&["-f", "concat", "-safe", "0"]));
    a.extend(strs(&PROTOCOL_WHITELIST));
    a.push("-i".into());
    a.push(overlay_list.to_string());
    a.push("-filter_complex".into());
    a.push(cut_filter(bg, n, s));
    a.extend(strs(&["-map", "[v]", "-an", "-frames:v"]));
    a.push(n.to_string());
    a.extend(strs(&["-fps_mode", "cfr", "-threads"]));
    a.push(s.threads.to_string());
    a.extend(strs(&["-c:v", "libx264", "-preset"]));
    a.push(s.preset.clone());
    a.push("-crf".into());
    a.push(s.crf.to_string());
    a.extend(strs(&[
        "-profile:v",
        "high",
        "-pix_fmt",
        "yuv420p",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-color_range",
        "tv",
        // 版によっては libx264 が -color_primaries/-color_trc を無視して unknown になるので、x264 にも直接渡す
        "-x264-params",
        "colorprim=bt709:transfer=bt709",
        "-video_track_timescale",
    ]));
    a.push((s.fps * 1000).to_string());
    a.push(out.to_string());
    a
}

/// カットの `-filter_complex` 文字列(設計書 §4.5)。
pub fn cut_filter(bg: &Background, n: i64, s: &EncodeSettings) -> String {
    let (w, h, fps) = (s.width, s.height, s.fps);
    let input = match bg {
        Background::Png { .. } => String::new(),
        Background::Jpeg { .. } => ":in_color_matrix=bt601:in_range=pc".to_string(),
        Background::Video { matrix, range, .. } => {
            format!(":in_color_matrix={matrix}:in_range={range}")
        }
    };
    let fit = match s.fit {
        Fit::Contain => {
            let [r, g, b] = s.background;
            format!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease{input}:out_color_matrix=bt709:out_range=tv,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=0x{r:02X}{g:02X}{b:02X}"
            )
        }
        Fit::Cover => format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase{input}:out_color_matrix=bt709:out_range=tv,crop={w}:{h}"
        ),
    };
    let tpad = match bg {
        Background::Video { looped: false, .. } => ",tpad=stop_mode=clone:stop=-1",
        _ => "",
    };
    format!(
        "[0:v]setpts=PTS-STARTPTS,fps={fps},{fit},setsar=1,format=yuv420p{tpad},trim=end_frame={n},setpts=PTS-STARTPTS[bg];[1:v]setpts=PTS-STARTPTS,format=rgba,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[o];[bg][o]overlay=x=0:y=0:format=yuv420:alpha=straight:eof_action=repeat:repeatlast=1:shortest=0,format=yuv420p[v]"
    )
}

/// concat demuxer の一覧(各行 `file '<名前>'`、LF、末尾も LF)。
pub fn concat_list(files: &[String]) -> String {
    files
        .iter()
        .map(|f| {
            format!(
                "file '{f}'
"
            )
        })
        .collect()
}

/// カットを連結する引数。
pub fn concat_args(list: &str, out: &str) -> Vec<String> {
    let mut a = base_args();
    a.extend(strs(&["-f", "concat", "-safe", "0"]));
    a.extend(strs(&PROTOCOL_WHITELIST));
    a.extend(strs(&["-i", list, "-c:v", "copy"]));
    a.push(out.to_string());
    a
}

/// 映像と音声を多重化する引数。
pub fn mux_args(video: &str, wav: &str, out: &str, audio_bitrate: &str) -> Vec<String> {
    let mut a = base_args();
    for input in [video, wav] {
        a.extend(strs(&PROTOCOL_WHITELIST));
        a.extend(strs(&["-i", input]));
    }
    a.extend(strs(&[
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-profile:a",
        "aac_low",
        "-b:a",
    ]));
    a.push(audio_bitrate.to_string());
    a.extend(strs(&[
        "-ar",
        "48000",
        "-ac",
        "2",
        "-movflags",
        "+faststart",
    ]));
    a.push(out.to_string());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> EncodeSettings {
        EncodeSettings {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 20,
            preset: "medium".into(),
            threads: 4,
            fit: Fit::Contain,
            background: [0, 0, 0],
        }
    }

    #[test]
    fn version_parsing() {
        // Review Focus 5
        assert_eq!(
            check_version("ffmpeg version 6.1.1-full_build-www.gyan.dev Copyright"),
            VersionCheck::Ok
        );
        assert_eq!(
            check_version("ffmpeg version n7.1 Copyright"),
            VersionCheck::Ok
        );
        assert_eq!(
            check_version(
                "ffmpeg version 2026-03-26-git-fd9f1e9c52-full_build-www.gyan.dev Copyright"
            ),
            VersionCheck::Ok
        );
        assert!(matches!(
            check_version("ffmpeg version 5.1.2 Copyright"),
            VersionCheck::TooOld(_)
        ));
        assert!(matches!(
            check_version("ffmpeg version 2021-01-01-git-x"),
            VersionCheck::TooOld(_)
        ));
        assert!(matches!(
            check_version("something else"),
            VersionCheck::Unknown(_)
        ));
        // "-git-" の後ろが無い・実在しない日付は判定できない
        for v in [
            "2026-03-26-git",
            "2026-03-26-git-",
            "2026-02-31-git-x",
            "2025-02-29-git-x",
            "2026-04-31-git-x",
        ] {
            assert!(
                matches!(
                    check_version(&format!("ffmpeg version {v} Copyright")),
                    VersionCheck::Unknown(_)
                ),
                "{v}"
            );
        }
        // 閏年の 2 月 29 日は受ける
        assert_eq!(
            check_version("ffmpeg version 2024-02-29-git-x Copyright"),
            VersionCheck::Ok
        );
    }

    #[test]
    fn ffprobe_version_parsing() {
        assert_eq!(
            check_ffprobe_version("ffprobe version 6.1.1-full_build-www.gyan.dev Copyright"),
            VersionCheck::Ok
        );
        assert_eq!(
            check_ffprobe_version("ffprobe version 2026-03-26-git-fd9f1e9c52-full_build"),
            VersionCheck::Ok
        );
        match check_ffprobe_version("ffprobe version 5.1.2 Copyright") {
            VersionCheck::TooOld(m) => assert!(m.contains("ffprobe"), "{m}"),
            v => panic!("{v:?}"),
        }
        // 別のプログラム名の行は判定できない
        assert!(matches!(
            check_ffprobe_version("ffmpeg version 6.1.1"),
            VersionCheck::Unknown(_)
        ));
        assert!(matches!(
            check_version("ffprobe version 6.1.1"),
            VersionCheck::Unknown(_)
        ));
    }

    #[test]
    fn png_contain_filter_is_exact() {
        assert_eq!(
            cut_filter(&Background::Png { path: "a.png".into() }, 60, &s()),
            "[0:v]setpts=PTS-STARTPTS,fps=30,scale=1920:1080:force_original_aspect_ratio=decrease:out_color_matrix=bt709:out_range=tv,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x000000,setsar=1,format=yuv420p,trim=end_frame=60,setpts=PTS-STARTPTS[bg];[1:v]setpts=PTS-STARTPTS,format=rgba,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[o];[bg][o]overlay=x=0:y=0:format=yuv420:alpha=straight:eof_action=repeat:repeatlast=1:shortest=0,format=yuv420p[v]"
        );
    }

    #[test]
    fn video_cover_with_tpad_and_input_matrix() {
        let mut st = s();
        st.fit = Fit::Cover;
        let f = cut_filter(
            &Background::Video {
                path: "v.mp4".into(),
                looped: false,
                matrix: "bt601".into(),
                range: "pc".into(),
            },
            90,
            &st,
        );
        assert!(f.contains("scale=1920:1080:force_original_aspect_ratio=increase:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709:out_range=tv,crop=1920:1080"), "{f}");
        assert!(
            f.contains("format=yuv420p,tpad=stop_mode=clone:stop=-1,trim=end_frame=90"),
            "{f}"
        );
        let looped = cut_filter(
            &Background::Video {
                path: "v.mp4".into(),
                looped: true,
                matrix: "bt709".into(),
                range: "tv".into(),
            },
            90,
            &st,
        );
        assert!(!looped.contains("tpad"), "{looped}");
    }

    #[test]
    fn jpeg_declares_bt601_full_range() {
        let f = cut_filter(
            &Background::Jpeg {
                path: "a.jpg".into(),
            },
            30,
            &s(),
        );
        assert!(
            f.contains("decrease:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709"),
            "{f}"
        );
    }

    #[test]
    fn cut_args_are_exact() {
        let a = cut_args(
            &Background::Video {
                path: "D:/素材/波.mp4".into(),
                looped: true,
                matrix: "bt709".into(),
                range: "tv".into(),
            },
            "ovl_0001.txt",
            60,
            "cut_0001.mp4",
            &s(),
        );
        let head: Vec<&str> = a.iter().take(11).map(|x| x.as_str()).collect();
        assert_eq!(
            head,
            vec![
                "-nostdin",
                "-hide_banner",
                "-n",
                "-stream_loop",
                "-1",
                "-protocol_whitelist",
                "file",
                "-i",
                "D:/素材/波.mp4",
                "-f",
                "concat"
            ]
        );
        let tail: Vec<&str> = a.iter().rev().take(4).map(|x| x.as_str()).collect();
        assert_eq!(
            tail,
            vec![
                "cut_0001.mp4",
                "30000",
                "-video_track_timescale",
                "colorprim=bt709:transfer=bt709"
            ]
        );
        for must in [
            "-frames:v",
            "60",
            "-fps_mode",
            "cfr",
            "-threads",
            "4",
            "-profile:v",
            "high",
            "-an",
            "-map",
            "[v]",
        ] {
            assert!(a.iter().any(|x| x == must), "{must}: {a:?}");
        }
        let img = cut_args(
            &Background::Png {
                path: "a.png".into(),
            },
            "ovl_0001.txt",
            60,
            "cut_0001.mp4",
            &s(),
        );
        assert_eq!(
            &img[3..11],
            &[
                "-loop",
                "1",
                "-framerate",
                "30",
                "-protocol_whitelist",
                "file",
                "-i",
                "a.png"
            ]
        );
    }

    #[test]
    fn concat_and_mux_args() {
        assert_eq!(
            concat_list(&["cut_0001.mp4".into(), "cut_0002.mp4".into()]),
            "file 'cut_0001.mp4'\nfile 'cut_0002.mp4'\n"
        );
        assert_eq!(
            concat_args("concat.txt", "video.mp4"),
            vec![
                "-nostdin",
                "-hide_banner",
                "-n",
                "-f",
                "concat",
                "-safe",
                "0",
                "-protocol_whitelist",
                "file",
                "-i",
                "concat.txt",
                "-c:v",
                "copy",
                "video.mp4"
            ]
        );
        let m = mux_args(
            "video.mp4",
            "D:/台本/full_dialogue.wav",
            "output.tmp.mp4",
            "192k",
        );
        assert_eq!(m.join(" "), "-nostdin -hide_banner -n -protocol_whitelist file -i video.mp4 -protocol_whitelist file -i D:/台本/full_dialogue.wav -map 0:v -map 1:a -c:v copy -c:a aac -profile:a aac_low -b:a 192k -ar 48000 -ac 2 -movflags +faststart output.tmp.mp4");
    }

    #[test]
    fn every_input_has_protocol_whitelist() {
        // 全ての "-i" について、直前の "-i"(無ければ先頭)から今の "-i" までの間に
        // 入力オプション "-protocol_whitelist file" がある
        let png = Background::Png {
            path: "a.png".into(),
        };
        let jpg = Background::Jpeg {
            path: "a.jpg".into(),
        };
        let vid = |looped| Background::Video {
            path: "v.mp4".into(),
            looped,
            matrix: "bt709".into(),
            range: "tv".into(),
        };
        let mut all: Vec<Vec<String>> = [png, jpg, vid(true), vid(false)]
            .iter()
            .map(|bg| cut_args(bg, "ovl.txt", 60, "cut.mp4", &s()))
            .collect();
        all.push(concat_args("concat.txt", "video.mp4"));
        all.push(mux_args("video.mp4", "a.wav", "out.mp4", "192k"));
        for args in all {
            let ii: Vec<usize> = args
                .iter()
                .enumerate()
                .filter(|(_, a)| *a == "-i")
                .map(|(i, _)| i)
                .collect();
            assert!(!ii.is_empty());
            for &i in &ii {
                let before = &args[..i];
                let from = before.iter().rposition(|a| a == "-i").map_or(0, |p| p + 2);
                assert!(
                    before[from..]
                        .windows(2)
                        .any(|w| w == ["-protocol_whitelist", "file"]),
                    "{args:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod version_boundary_tests {
    use super::*;

    fn chk(v: &str) -> VersionCheck {
        check_version(&format!("ffmpeg version {v} Copyright"))
    }

    #[test]
    fn broken_versions_are_unknown() {
        for v in [
            "2026-not-a-version",
            "2026-x",
            "n7broken",
            "7",
            "6.x",
            "6.1.1.1",
            "6.1a",
            "2026-13-01-git-x",
            "2026-03-32-git-x",
            "2026-03-26-foo",
            "2026-03-26",
            "n",
            "6..1",
        ] {
            assert!(
                matches!(chk(v), VersionCheck::Unknown(_)),
                "{v}: {:?}",
                chk(v)
            );
        }
    }

    #[test]
    fn valid_boundaries() {
        assert_eq!(chk("6.0"), VersionCheck::Ok);
        assert_eq!(chk("n7.1.2-1-gabc"), VersionCheck::Ok);
        assert_eq!(chk("2023-01-01-git-abc"), VersionCheck::Ok);
        assert!(matches!(chk("2022-12-31-git-abc"), VersionCheck::TooOld(_)));
        assert!(matches!(chk("5.1"), VersionCheck::TooOld(_)));
        assert!(matches!(chk("n4.4.2"), VersionCheck::TooOld(_)));
    }
}

#[cfg(test)]
mod cut_args_golden_tests {
    use super::*;

    fn st(fit: Fit) -> EncodeSettings {
        EncodeSettings {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 20,
            preset: "medium".into(),
            threads: 4,
            fit,
            background: [0, 0, 0],
        }
    }

    fn expect(input: &[&str], bg: &Background, fit: Fit) -> Vec<String> {
        let s = st(fit);
        let mut v: Vec<String> = ["-nostdin", "-hide_banner", "-n"]
            .iter()
            .map(|x| x.to_string())
            .collect();
        v.extend(input.iter().map(|x| x.to_string()));
        v.extend(
            [
                "-f",
                "concat",
                "-safe",
                "0",
                "-protocol_whitelist",
                "file",
                "-i",
                "ovl.txt",
                "-filter_complex",
            ]
            .iter()
            .map(|x| x.to_string()),
        );
        v.push(cut_filter(bg, 60, &s));
        v.extend(
            "-map [v] -an -frames:v 60 -fps_mode cfr -threads 4 -c:v libx264 -preset medium -crf 20 -profile:v high -pix_fmt yuv420p -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv -x264-params colorprim=bt709:transfer=bt709 -video_track_timescale 30000 cut.mp4"
                .split(' ')
                .map(|x| x.to_string()),
        );
        v
    }

    fn vid(looped: bool) -> Background {
        Background::Video {
            path: "v.mp4".into(),
            looped,
            matrix: "bt601".into(),
            range: "pc".into(),
        }
    }

    #[test]
    fn full_vectors() {
        let cases: Vec<(Background, Fit, Vec<&str>, &str)> = vec![
            (vid(true), Fit::Contain, vec!["-stream_loop", "-1", "-protocol_whitelist", "file", "-i", "v.mp4"], "scale=1920:1080:force_original_aspect_ratio=decrease:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709:out_range=tv,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x000000,setsar=1,format=yuv420p,trim=end_frame=60"),
            (vid(false), Fit::Cover, vec!["-protocol_whitelist", "file", "-i", "v.mp4"], "scale=1920:1080:force_original_aspect_ratio=increase:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709:out_range=tv,crop=1920:1080,setsar=1,format=yuv420p,tpad=stop_mode=clone:stop=-1,trim=end_frame=60"),
            (Background::Png { path: "a.png".into() }, Fit::Contain, vec!["-loop", "1", "-framerate", "30", "-protocol_whitelist", "file", "-i", "a.png"], "scale=1920:1080:force_original_aspect_ratio=decrease:out_color_matrix=bt709:out_range=tv,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x000000,setsar=1,format=yuv420p,trim=end_frame=60"),
            (Background::Jpeg { path: "a.jpg".into() }, Fit::Cover, vec!["-loop", "1", "-framerate", "30", "-protocol_whitelist", "file", "-i", "a.jpg"], "scale=1920:1080:force_original_aspect_ratio=increase:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709:out_range=tv,crop=1920:1080,setsar=1,format=yuv420p,trim=end_frame=60"),
        ];
        for (bg, fit, input, scale_part) in cases {
            let got = cut_args(&bg, "ovl.txt", 60, "cut.mp4", &st(fit));
            assert_eq!(got, expect(&input, &bg, fit));
            // フィルタ全体も固定(収め方の部分は独立に書いた文字列で確認)
            let f = &got[got.iter().position(|x| x == "-filter_complex").unwrap() + 1];
            let want = format!("[0:v]setpts=PTS-STARTPTS,fps=30,{scale_part},setpts=PTS-STARTPTS[bg];[1:v]setpts=PTS-STARTPTS,format=rgba,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[o];[bg][o]overlay=x=0:y=0:format=yuv420:alpha=straight:eof_action=repeat:repeatlast=1:shortest=0,format=yuv420p[v]");
            assert_eq!(f, &want);
        }
    }
}
