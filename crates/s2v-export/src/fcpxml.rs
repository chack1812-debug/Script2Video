//! FCPXML(Filmora 向け)。トラック別 wav(dialogue / bgm / se)を3つのレーンに置く。
//! タイムベースは 30000分の1秒(流用元と同じ)。

use std::path::{Path, PathBuf};

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 絶対パスを `file://` URI にする(Windows の verbatim パスの接頭辞を外す。流用元と同じ)。
/// NLE が読めるよう、予約文字・空白・非 ASCII は UTF-8 で `%XX` にする(`/` とドライブの `:` は残す)。
fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    };
    let s = s.replace('\\', "/");
    let encoded = percent_encode_path(&s);
    if let Some(unc) = encoded.strip_prefix("//") {
        format!("file://{unc}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

fn percent_encode_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let mut out = String::with_capacity(s.len() * 3);
    for (i, &b) in bytes.iter().enumerate() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            || (drive && i == 1);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn render_fcpxml(
    project: &str,
    total_samples: i64,
    tracks: &[(&str, PathBuf)],
    fps2997: bool,
) -> String {
    // 48kHz のサンプル → 30000分の1秒(切り上げ)
    let ticks = total_samples.max(0).saturating_mul(5).saturating_add(7) / 8;
    let (format_name, frame) = if fps2997 {
        ("FFVideoFormat1080p2997", "1001/30000s")
    } else {
        ("FFVideoFormat1080p30", "1000/30000s")
    };
    let mut assets = String::new();
    let mut clips = String::new();
    for (i, (name, path)) in tracks.iter().enumerate() {
        let n = i + 1;
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        assets.push_str(&format!(
            "        <asset id=\"a{n}\" name=\"{}\" src=\"{}\" start=\"0s\" duration=\"{ticks}/30000s\" hasAudio=\"1\" audioSources=\"1\" audioChannels=\"2\" audioRate=\"48000\"/>\n",
            xml_escape(&file),
            xml_escape(&file_uri(&abs)),
        ));
        clips.push_str(&format!(
            "                            <asset-clip ref=\"a{n}\" lane=\"{n}\" offset=\"0s\" name=\"{}\" duration=\"{ticks}/30000s\"/>\n",
            xml_escape(name),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.8">
    <resources>
        <format id="r1" name="{format_name}" frameDuration="{frame}"/>
{assets}    </resources>
    <library>
        <event name="Script2Video">
            <project name="{project}">
                <sequence format="r1" duration="{ticks}/30000s">
                    <spine>
                        <gap name="Gap" offset="0s" duration="{ticks}/30000s">
{clips}                        </gap>
                    </spine>
                </sequence>
            </project>
        </event>
    </library>
</fcpxml>
"#,
        project = xml_escape(project),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_lanes_with_escaped_uris_and_total_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a & b");
        std::fs::create_dir(&dir).unwrap();
        let tracks: Vec<(&str, PathBuf)> = ["dialogue", "bgm", "se"]
            .iter()
            .map(|n| {
                let p = dir.join(format!("{n}.wav"));
                std::fs::write(&p, b"x").unwrap();
                (*n, p)
            })
            .collect();
        let xml = render_fcpxml("第1話", 48_000, &tracks, false);
        assert!(xml.contains(r#"duration="30000/30000s""#));
        assert!(xml.contains(r#"lane="3""#));
        // パスは URI として %エンコードされる(空白は %20、& は %26)
        assert!(xml.contains("a%20%26%20b/dialogue.wav"));
        assert!(!xml.contains("a & b"));
        assert!(xml.contains("FFVideoFormat1080p30"));
        assert!(render_fcpxml("x", 1, &tracks, true).contains("1001/30000s"));
    }

    #[test]
    fn names_are_xml_escaped() {
        let tracks: Vec<(&str, PathBuf)> = vec![("a<b>\"c\"&", PathBuf::from("x.wav"))];
        let xml = render_fcpxml("A & B <1>", 48_000, &tracks, false);
        assert!(xml.contains(r#"<project name="A &amp; B &lt;1&gt;">"#));
        assert!(xml.contains("a&lt;b&gt;&quot;c&quot;&amp;"));
    }

    #[test]
    fn file_uri_percent_encodes_special_characters() {
        assert_eq!(
            file_uri(Path::new(r"C:\a b\c.wav")),
            "file:///C:/a%20b/c.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"C:\第1話\音.wav")),
            "file:///C:/%E7%AC%AC1%E8%A9%B1/%E9%9F%B3.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"C:\a#b&c%d?e.wav")),
            "file:///C:/a%23b%26c%25d%3Fe.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"C:\k-e_e.p~1\x.wav")),
            "file:///C:/k-e_e.p~1/x.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\C:\a b\x.wav")),
            "file:///C:/a%20b/x.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"\\server\share\a b.wav")),
            "file://server/share/a%20b.wav"
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\UNC\server\share\a b.wav")),
            "file://server/share/a%20b.wav"
        );
        assert_eq!(file_uri(Path::new("/tmp/a b.wav")), "file:///tmp/a%20b.wav");
    }

    #[test]
    fn zero_duration_does_not_crash() {
        let xml = render_fcpxml("x", 0, &[], false);
        assert!(xml.contains(r#"duration="0/30000s""#));
        let xml = render_fcpxml("x", -5, &[], false);
        assert!(xml.contains(r#"duration="0/30000s""#));
    }

    #[test]
    fn ticks_round_up() {
        // 1 サンプル = 5/8 tick → 切り上げて 1
        assert!(render_fcpxml("x", 1, &[], false).contains(r#"duration="1/30000s""#));
        assert!(render_fcpxml("x", 8, &[], false).contains(r#"duration="5/30000s""#));
        assert!(render_fcpxml("x", 9, &[], false).contains(r#"duration="6/30000s""#));
    }
}
