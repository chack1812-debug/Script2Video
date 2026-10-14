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
    let unknown = || VersionCheck::Unknown(format!("ffmpeg の版を読み取れません: {first_line}"));
    let Some(rest) = first_line.trim().strip_prefix("ffmpeg version ") else {
        return unknown();
    };
    let token = rest.split_whitespace().next().unwrap_or("");
    let bytes = token.as_bytes();
    // 日付版: 4桁の年 + '-'
    if bytes.len() > 4 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-' {
        let year: u32 = token[..4].parse().unwrap_or(0);
        return if year >= MIN_GIT_YEAR {
            VersionCheck::Ok
        } else {
            VersionCheck::TooOld(format!(
                "ffmpeg が古すぎます({token})。{MIN_GIT_YEAR} 年以降のビルドが必要です"
            ))
        };
    }
    let num = token.strip_prefix('n').unwrap_or(token);
    let digits: String = num.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<u32>() {
        Ok(major) if major >= MIN_MAJOR => VersionCheck::Ok,
        Ok(_) => VersionCheck::TooOld(format!(
            "ffmpeg が古すぎます({token})。{MIN_MAJOR} 以上が必要です"
        )),
        Err(_) => unknown(),
    }
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
            a.push("-i".into());
            a.push(path.clone());
        }
        Background::Video { path, looped, .. } => {
            if *looped {
                a.extend(strs(&["-stream_loop", "-1"]));
            }
            a.push("-i".into());
            a.push(path.clone());
        }
    }
    a.extend(strs(&["-f", "concat", "-safe", "0", "-i"]));
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
    a.extend(strs(&[
        "-f", "concat", "-safe", "0", "-i", list, "-c:v", "copy",
    ]));
    a.push(out.to_string());
    a
}

/// 映像と音声を多重化する引数。
pub fn mux_args(video: &str, wav: &str, out: &str, audio_bitrate: &str) -> Vec<String> {
    let mut a = base_args();
    a.extend(strs(&["-i", video, "-i", wav]));
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
        let head: Vec<&str> = a.iter().take(10).map(|x| x.as_str()).collect();
        assert_eq!(
            head,
            vec![
                "-nostdin",
                "-hide_banner",
                "-n",
                "-stream_loop",
                "-1",
                "-i",
                "D:/素材/波.mp4",
                "-f",
                "concat",
                "-safe"
            ]
        );
        let tail: Vec<&str> = a.iter().rev().take(4).map(|x| x.as_str()).collect();
        assert_eq!(
            tail,
            vec!["cut_0001.mp4", "30000", "-video_track_timescale", "tv"]
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
            &img[3..9],
            &["-loop", "1", "-framerate", "30", "-i", "a.png"]
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
        assert_eq!(m.join(" "), "-nostdin -hide_banner -n -i video.mp4 -i D:/台本/full_dialogue.wav -map 0:v -map 1:a -c:v copy -c:a aac -profile:a aac_low -b:a 192k -ar 48000 -ac 2 -movflags +faststart output.tmp.mp4");
    }
}
