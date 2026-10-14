//! 台本HTML(印刷・閲覧用。設計書 5.1)。

use std::collections::BTreeMap;

use s2v_core::{Cast, Scene, ScriptCommand, ScriptItem, VisualKind};

/// HTML 本文に埋め込む文字列をエスケープする。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = r#"
body { font-family: "Yu Gothic", "Hiragino Sans", sans-serif; margin: 2em; line-height: 1.6; }
table { border-collapse: collapse; width: 100%; margin-bottom: 1.5em; }
th, td { border: 1px solid #999; padding: 0.3em 0.6em; vertical-align: top; text-align: left; }
th { background: #eee; }
td.id { white-space: nowrap; font-family: monospace; }
p.visual { color: #444; font-style: italic; }
@media print {
  body { margin: 0; }
  h2 { page-break-after: avoid; }
  tr { page-break-inside: avoid; }
}
"#;

/// 台本HTMLを作る: タイトル、登場人物、シーンごとの見出し(番号・名称・情景)、台詞の表。
pub fn render_script_html(title: &str, scenes: &[Scene]) -> String {
    let mut casts: BTreeMap<&str, &Cast> = BTreeMap::new();
    for s in scenes {
        for (name, c) in &s.casts {
            casts.insert(name.as_str(), c);
        }
    }

    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n");
    h.push_str(&format!(
        "<title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n",
        esc(title)
    ));
    h.push_str(&format!("<h1>{}</h1>\n", esc(title)));

    h.push_str(
        "<h2>登場人物</h2>\n<table>\n<tr><th>役名</th><th>話者名</th><th>外見描写</th></tr>\n",
    );
    for (name, c) in &casts {
        h.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            esc(name),
            esc(&c.speaker_name),
            esc(c.appearance.as_deref().unwrap_or("")).replace('\n', "<br>")
        ));
    }
    h.push_str("</table>\n");

    let mut table_open = false;
    let mut heading_done = false;
    let close_table = |h: &mut String, open: &mut bool| {
        if *open {
            h.push_str("</table>\n");
            *open = false;
        }
    };
    for item in scenes.iter().flat_map(|s| s.items.iter()) {
        match item {
            ScriptItem::Command(ScriptCommand::Paragraph { id, name, .. }) => {
                close_table(&mut h, &mut table_open);
                let label = id
                    .map(s2v_core::ids::format_scene_id)
                    .unwrap_or_else(|| "S??".to_string());
                let heading = match name {
                    Some(n) => format!("{label} {n}"),
                    None => label,
                };
                h.push_str(&format!("<h2>{}</h2>\n", esc(&heading)));
                heading_done = true;
            }
            ScriptItem::Command(ScriptCommand::Visual {
                kind, looped, text, ..
            }) => {
                close_table(&mut h, &mut table_open);
                let prefix = match (kind, looped) {
                    (Some(VisualKind::Image), _) => "（画像）",
                    (Some(VisualKind::Video), true) => "（動画・ループ）",
                    (Some(VisualKind::Video), false) => "（動画）",
                    (None, true) => "（ループ）",
                    (None, false) => "",
                };
                h.push_str(&format!(
                    "<p class=\"visual\">{}{}</p>\n",
                    prefix,
                    esc(text)
                ));
            }
            ScriptItem::Command(ScriptCommand::Cut { id, .. }) => {
                close_table(&mut h, &mut table_open);
                let label = id.map_or("C??".to_string(), |n| format!("C{n:02}"));
                h.push_str(&format!("<p class=\"cut\">カット {label}</p>\n"));
            }
            ScriptItem::Command(ScriptCommand::Note { text, .. }) => {
                close_table(&mut h, &mut table_open);
                h.push_str(&format!("<p class=\"note\">ト書き: {}</p>\n", esc(text)));
            }
            ScriptItem::Speech {
                id,
                cast_name,
                text,
                ..
            } => {
                if !heading_done {
                    h.push_str("<h2>S00</h2>\n");
                    heading_done = true;
                }
                if !table_open {
                    h.push_str("<table>\n<tr><th>台詞ID</th><th>役名</th><th>台詞</th></tr>\n");
                    table_open = true;
                }
                let id = id.map(|i| i.display()).unwrap_or_default();
                h.push_str(&format!(
                    "<tr><td class=\"id\">{}</td><td>{}</td><td>{}</td></tr>\n",
                    esc(&id),
                    esc(cast_name),
                    esc(text)
                ));
            }
            _ => {}
        }
    }
    close_table(&mut h, &mut table_open);
    h.push_str("</body>\n</html>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use s2v_core::ScriptParser;

    #[test]
    fn cut_note_and_loop_are_shown() {
        let mut p = s2v_core::ScriptParser::new();
        let scenes = p.parse_str("@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n#visual (video loop) 波\n#cut[C02]\n#note[N01] 翌朝<港>\n").unwrap();
        let h = render_script_html("t", &scenes);
        assert!(h.contains("（動画・ループ）波"), "{h}");
        assert!(h.contains("カット C02"), "{h}");
        assert!(h.contains("ト書き: 翌朝&lt;港&gt;"), "{h}");
    }

    #[test]
    fn html_contains_cast_scenes_and_lines() {
        let text = "@scene 居間\n@cast\nA:田中,pan=-30\n30代の男性。<紺のスーツ>\n\nB:佐藤\n\n@script\n[S00-L001] A:前置き\n#paragraph[S01] 港の朝\n#visual (image) 朝焼けの港。\n[S01-L002] B:おはよう & <こんにちは>\n";
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(text).unwrap();
        let html = render_script_html("第1話", &scenes);
        assert!(html.contains("<title>第1話</title>"));
        assert!(html.contains("@media print"));
        assert!(html.contains("田中"));
        assert!(html.contains("&lt;紺のスーツ&gt;"));
        assert!(html.contains("S00"));
        assert!(html.contains("S01 港の朝"));
        assert!(html.contains("（画像）朝焼けの港。"));
        assert!(html.contains("S01-L002"));
        assert!(html.contains("おはよう &amp; &lt;こんにちは&gt;"));
    }

    #[test]
    fn untrusted_text_is_escaped_everywhere() {
        let text = "@scene 居間
@cast
A:<script>田中\"
外見<script>\"x\"

@script
#paragraph[S01] 見出し<script>
#visual (image) <script>\"v\"
[S01-L001] A:\"台詞\"<script>
";
        let mut p = ScriptParser::new();
        let scenes = p.parse_str(text).unwrap();
        let html = render_script_html("題<script>\"", &scenes);
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("<title>題&lt;script&gt;&quot;</title>"));
    }
}
