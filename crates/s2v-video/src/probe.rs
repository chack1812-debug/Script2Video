//! ffprobe の JSON 出力の解析と、カット・出力の検査。プロセスは起動しない(引数の組み立てと検査だけ)。

use serde::Deserialize;

/// ffprobe の 1 ストリーム(使う項目だけ)。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Stream {
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub codec_tag_string: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i64>,
    pub pix_fmt: Option<String>,
    pub field_order: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_aspect_ratio: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub time_base: Option<String>,
    pub r_frame_rate: Option<String>,
    pub start_pts: Option<i64>,
    pub duration_ts: Option<i64>,
    pub nb_read_frames: Option<String>,
    pub extradata_hash: Option<String>,
    pub sample_rate: Option<String>,
    pub channels: Option<u32>,
}

/// ffprobe の出力全体。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Probe {
    #[serde(default)]
    pub streams: Vec<Stream>,
}

/// ffprobe の JSON を読む。
pub fn parse_probe(json: &str) -> Result<Probe, String> {
    serde_json::from_str(json).map_err(|e| format!("ffprobe の出力を読めません: {e}"))
}

/// ffprobe の引数(カット・出力の検査用)。
pub fn probe_args(path: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-count_frames",
        "-show_streams",
        "-show_data_hash",
        "sha256",
        "-of",
        "json",
        path,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 素材の検査用の ffprobe の引数(フレームは数えない)。
pub fn asset_probe_args(path: &str) -> Vec<String> {
    ["-v", "error", "-show_streams", "-of", "json", path]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 値が明示されているか(なし・unknown・unspecified は明示なし)。
fn explicit(v: &Option<String>) -> Option<&str> {
    match v.as_deref() {
        None | Some("") | Some("unknown") | Some("unspecified") => None,
        Some(s) => Some(s),
    }
}

/// 動画の素材の色。Ok((scale の入力の行列, レンジ))、受け付けない値は Err(エラー 2 の理由)。
pub fn video_color_input(s: &Stream) -> Result<(String, String), String> {
    if let Some(p) = explicit(&s.color_primaries) {
        if p != "bt709" {
            return Err(format!(
                "素材の色の原色 {p} には対応していません(bt709 のみ)"
            ));
        }
    }
    if let Some(t) = explicit(&s.color_transfer) {
        if t != "bt709" {
            return Err(format!(
                "素材の色の伝達関数 {t} には対応していません(HDR などは不可。bt709 のみ)"
            ));
        }
    }
    let matrix = match explicit(&s.color_space) {
        None | Some("bt709") => "bt709",
        Some("bt470bg") | Some("smpte170m") => "bt601",
        Some(other) => {
            return Err(format!("素材の色の行列 {other} には対応していません"));
        }
    };
    let range = match explicit(&s.color_range) {
        Some("pc") => "pc",
        _ => "tv",
    };
    Ok((matrix.to_string(), range.to_string()))
}

fn videos(p: &Probe) -> Vec<&Stream> {
    p.streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("video"))
        .collect()
}

fn audios(p: &Probe) -> Vec<&Stream> {
    p.streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .collect()
}

/// 1 つのカットファイルの検査(映像 1 本だけ、time base 1/(fps*1000)、start_pts 0、duration_ts N*1000、nb_read_frames N)。
pub fn check_cut(p: &Probe, n: i64, fps: u32) -> Vec<String> {
    let mut errs = Vec::new();
    if p.streams.len() != 1 || videos(p).len() != 1 {
        errs.push(format!(
            "カットは映像 1 本だけのはずですが、ストリームが {} 本あります",
            p.streams.len()
        ));
        return errs;
    }
    let s = &p.streams[0];
    let want_tb = format!("1/{}", fps as i64 * 1000);
    if s.time_base.as_deref() != Some(want_tb.as_str()) {
        errs.push(format!(
            "time base が {want_tb} ではありません: {:?}",
            s.time_base
        ));
    }
    if s.start_pts != Some(0) {
        errs.push(format!("start_pts が 0 ではありません: {:?}", s.start_pts));
    }
    if s.duration_ts != Some(n * 1000) {
        errs.push(format!(
            "duration_ts が {} ではありません: {:?}",
            n * 1000,
            s.duration_ts
        ));
    }
    if s.nb_read_frames.as_deref() != Some(n.to_string().as_str()) {
        errs.push(format!(
            "フレーム数が {n} ではありません: {:?}",
            s.nb_read_frames
        ));
    }
    errs
}

/// 全カットの一致(codec・tag・profile・level・pix_fmt・field order・幅・高さ・SAR・色 4 項目・time base・フレームレート・extradata)。
/// 先頭のカットを基準に、ずれた項目をファイル名つきで報告する。
pub fn check_cuts_consistent(probes: &[(String, Probe)]) -> Vec<String> {
    let mut errs = Vec::new();
    let Some((base_name, base_probe)) = probes.first() else {
        return errs;
    };
    let Some(base) = videos(base_probe).first().map(|s| (*s).clone()) else {
        errs.push(format!("{base_name}: 映像ストリームがありません"));
        return errs;
    };
    for (name, probe) in &probes[1..] {
        let Some(s) = videos(probe).first().map(|s| (*s).clone()) else {
            errs.push(format!("{name}: 映像ストリームがありません"));
            continue;
        };
        macro_rules! cmp {
            ($label:expr, $f:ident) => {
                if s.$f != base.$f {
                    errs.push(format!(
                        "{name}: {} が {base_name} と一致しません({:?} と {:?})",
                        $label, s.$f, base.$f
                    ));
                }
            };
        }
        cmp!("codec", codec_name);
        cmp!("codec tag", codec_tag_string);
        cmp!("profile", profile);
        cmp!("level", level);
        cmp!("pix_fmt", pix_fmt);
        cmp!("field order", field_order);
        cmp!("幅", width);
        cmp!("高さ", height);
        cmp!("SAR", sample_aspect_ratio);
        cmp!("color_range", color_range);
        cmp!("color_space", color_space);
        cmp!("color_transfer", color_transfer);
        cmp!("color_primaries", color_primaries);
        cmp!("time base", time_base);
        cmp!("フレームレート", r_frame_rate);
        cmp!("extradata", extradata_hash);
    }
    errs
}

/// 出力の検査(映像 1 + 音声 1、H.264・幅・高さ・fps/1・コマ数、AAC LC・48000・2ch)。
pub fn check_output(
    p: &Probe,
    width: u32,
    height: u32,
    fps: u32,
    total_frames: i64,
) -> Vec<String> {
    let mut errs = Vec::new();
    let (v, a) = (videos(p), audios(p));
    if p.streams.len() != 2 || v.len() != 1 || a.len() != 1 {
        errs.push(format!(
            "出力は映像 1 本と音声 1 本のはずですが、映像 {} 本・音声 {} 本・全 {} 本です",
            v.len(),
            a.len(),
            p.streams.len()
        ));
        return errs;
    }
    let (v, a) = (v[0], a[0]);
    if v.codec_name.as_deref() != Some("h264") {
        errs.push(format!("映像が H.264 ではありません: {:?}", v.codec_name));
    }
    if v.width != Some(width) || v.height != Some(height) {
        errs.push(format!(
            "映像の大きさが {width}x{height} ではありません: {:?}x{:?}",
            v.width, v.height
        ));
    }
    let want_fps = format!("{fps}/1");
    if v.r_frame_rate.as_deref() != Some(want_fps.as_str()) {
        errs.push(format!(
            "フレームレートが {want_fps} ではありません: {:?}",
            v.r_frame_rate
        ));
    }
    if v.nb_read_frames.as_deref() != Some(total_frames.to_string().as_str()) {
        errs.push(format!(
            "コマ数が {total_frames} ではありません: {:?}",
            v.nb_read_frames
        ));
    }
    if a.codec_name.as_deref() != Some("aac") {
        errs.push(format!("音声が AAC ではありません: {:?}", a.codec_name));
    }
    if a.profile.as_deref() != Some("LC") {
        errs.push(format!(
            "音声の profile が LC ではありません: {:?}",
            a.profile
        ));
    }
    if a.sample_rate.as_deref() != Some("48000") {
        errs.push(format!(
            "音声のサンプルレートが 48000 ではありません: {:?}",
            a.sample_rate
        ));
    }
    if a.channels != Some(2) {
        errs.push(format!("音声が 2ch ではありません: {:?}", a.channels));
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut_json(frames: &str, tb: &str, dts: i64, hash: &str) -> String {
        format!(
            r#"{{"streams":[{{"codec_type":"video","codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","field_order":"progressive","width":1920,"height":1080,"sample_aspect_ratio":"1:1","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","time_base":"{tb}","r_frame_rate":"30/1","start_pts":0,"duration_ts":{dts},"nb_read_frames":"{frames}","extradata_hash":"{hash}"}}]}}"#
        )
    }

    #[test]
    fn good_cut_passes() {
        let p = parse_probe(&cut_json("60", "1/30000", 60_000, "SHA256:aa")).unwrap();
        assert!(check_cut(&p, 60, 30).is_empty());
    }

    #[test]
    fn each_cut_mismatch_is_reported() {
        assert!(!check_cut(
            &parse_probe(&cut_json("59", "1/30000", 60_000, "x")).unwrap(),
            60,
            30
        )
        .is_empty());
        assert!(!check_cut(
            &parse_probe(&cut_json("60", "1/15360", 60_000, "x")).unwrap(),
            60,
            30
        )
        .is_empty());
        assert!(!check_cut(
            &parse_probe(&cut_json("60", "1/30000", 59_000, "x")).unwrap(),
            60,
            30
        )
        .is_empty());
        let two =
            cut_json("60", "1/30000", 60_000, "x").replace("}]}", "},{\"codec_type\":\"audio\"}]}");
        assert!(!check_cut(&parse_probe(&two).unwrap(), 60, 30).is_empty());
    }

    #[test]
    fn cuts_must_agree() {
        let a = parse_probe(&cut_json("60", "1/30000", 60_000, "SHA256:aa")).unwrap();
        let b = parse_probe(&cut_json("30", "1/30000", 30_000, "SHA256:bb")).unwrap();
        let errs = check_cuts_consistent(&[
            ("cut_0001.mp4".into(), a.clone()),
            ("cut_0002.mp4".into(), b),
        ]);
        assert!(
            errs.iter()
                .any(|e| e.contains("extradata") && e.contains("cut_0002.mp4")),
            "{errs:?}"
        );
        assert!(check_cuts_consistent(&[("1".into(), a.clone()), ("2".into(), a)]).is_empty());
    }

    #[test]
    fn video_color_rules() {
        let mut s = Stream {
            codec_type: Some("video".into()),
            ..Default::default()
        };
        assert_eq!(video_color_input(&s), Ok(("bt709".into(), "tv".into())));
        s.color_space = Some("smpte170m".into());
        s.color_range = Some("pc".into());
        assert_eq!(video_color_input(&s), Ok(("bt601".into(), "pc".into())));
        s.color_space = Some("bt470bg".into());
        assert_eq!(video_color_input(&s).unwrap().0, "bt601");
        s.color_transfer = Some("smpte2084".into());
        assert!(video_color_input(&s).is_err()); // HDR
        s.color_transfer = Some("unknown".into());
        s.color_primaries = Some("bt2020".into());
        assert!(video_color_input(&s).is_err());
        s.color_primaries = None;
        s.color_space = Some("bt2020nc".into());
        assert!(video_color_input(&s).is_err());
    }

    #[test]
    fn output_checks() {
        let ok = r#"{"streams":[
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"r_frame_rate":"30/1","nb_read_frames":"90"},
            {"codec_type":"audio","codec_name":"aac","profile":"LC","sample_rate":"48000","channels":2}]}"#;
        assert!(check_output(&parse_probe(ok).unwrap(), 1920, 1080, 30, 90).is_empty());
        assert!(!check_output(
            &parse_probe(&ok.replace("\"90\"", "\"89\"")).unwrap(),
            1920,
            1080,
            30,
            90
        )
        .is_empty());
        assert!(!check_output(
            &parse_probe(&ok.replace("\"LC\"", "\"HE-AAC\"")).unwrap(),
            1920,
            1080,
            30,
            90
        )
        .is_empty());
        assert!(!check_output(
            &parse_probe(&ok.replace("\"channels\":2", "\"channels\":1")).unwrap(),
            1920,
            1080,
            30,
            90
        )
        .is_empty());
    }

    #[test]
    fn args() {
        assert_eq!(
            probe_args("cut_0001.mp4"),
            vec![
                "-v",
                "error",
                "-count_frames",
                "-show_streams",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
                "cut_0001.mp4"
            ]
        );
    }
}
