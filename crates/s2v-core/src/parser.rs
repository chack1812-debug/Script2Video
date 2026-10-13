use std::collections::HashMap;

use serde_json::Value;

use crate::cast::Cast;
use crate::types::{PauseConfig, Scene, SceneConfig, ScriptCommand, ScriptItem};

/// パース中に検出した非致命的な問題（行は無視されるがパース自体は続行）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseWarning {
    /// 1始まりの行番号
    pub line_no: usize,
    pub message: String,
}

pub struct ScriptParser {
    casts: HashMap<String, Cast>,
    pause_config: PauseConfig,
    asset_config: HashMap<String, String>,
    warnings: Vec<ParseWarning>,
    /// `@cast`セクションで定義行を読んだ直後から、空行または次のセクションが来るまでの間、
    /// 収集中のキャスト名を保持する（自由記述の宛先を追跡するための状態）。
    pending_cast_name: Option<String>,
    /// `pending_cast_name`が`Some`の間に集めた自由記述の行（順序どおり）。
    pending_cast_appearance: Vec<String>,
}

impl ScriptParser {
    pub fn new() -> Self {
        Self {
            casts: HashMap::new(),
            pause_config: PauseConfig::default(),
            asset_config: HashMap::new(),
            warnings: Vec::new(),
            pending_cast_name: None,
            pending_cast_appearance: Vec::new(),
        }
    }

    /// パース中に検出した非致命的な警告（未定義キャストなど）を返す。
    pub fn warnings(&self) -> &[ParseWarning] {
        &self.warnings
    }

    pub fn parse_str(&mut self, text: &str) -> anyhow::Result<Vec<Scene>> {
        // 台本固有の状態を全消去する。casts/pause_config/asset_configは
        // 同一パース内ではシーンをまたいで引き継がれる仕様だが、
        // 別の台本(parse_strの再呼び出し)に持ち越してはいけない。
        self.casts.clear();
        self.pause_config = PauseConfig::default();
        self.asset_config.clear();
        self.warnings.clear();
        self.pending_cast_name = None;
        self.pending_cast_appearance.clear();
        let mut scenes: Vec<Scene> = Vec::new();
        let mut current_scene: Option<Scene> = None;
        let mut section = "";

        for (idx, line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = line.trim();
            if line.is_empty() {
                self.flush_pending_cast();
                continue;
            }

            if line.starts_with('@') {
                self.flush_pending_cast();
                if line.starts_with("@scene") {
                    if let Some(mut s) = current_scene.take() {
                        s.casts = self.casts.clone();
                        Self::fill_items_scene_config(&mut s);
                        scenes.push(s);
                    }
                    let scene_raw = line["@scene".len()..].trim();
                    let scene = self.parse_scene_header(scene_raw, line_no);
                    let mut s = Scene::new(scene);
                    s.pause_config = self.pause_config.clone();
                    current_scene = Some(s);
                    section = "@scene";
                } else if line.starts_with("@pause") {
                    section = "@pause";
                } else if line.starts_with("@asset") {
                    section = "@asset";
                } else if line.starts_with("@cast") {
                    section = "@cast";
                } else if line.starts_with("@script") {
                    section = "@script";
                    if let Some(ref mut s) = current_scene {
                        s.pause_config = self.pause_config.clone();
                    }
                }
                continue;
            }

            match section {
                "@pause" => self.parse_pause_line(line, line_no),
                "@asset" => self.parse_asset_line(line),
                "@cast" => {
                    if self.pending_cast_name.is_some() {
                        if Self::looks_like_cast_definition(line) {
                            self.warnings.push(ParseWarning {
                                line_no,
                                message: format!(
                                    "キャスト定義行に見える行が自由記述として読み込まれました。前のキャスト定義との間に空行がないか確認してください: {}",
                                    line
                                ),
                            });
                        }
                        self.pending_cast_appearance.push(line.to_string());
                    } else {
                        self.parse_cast_line(line, line_no);
                    }
                }
                "@scene" => {
                    if let Some(ref mut s) = current_scene {
                        match s.config.description {
                            Some(ref mut desc) => {
                                desc.push('\n');
                                desc.push_str(line);
                            }
                            None => s.config.description = Some(line.to_string()),
                        }
                    }
                }
                "@script" => {
                    if let Some(ref mut scene) = current_scene {
                        if let Some(item) = self.parse_script_line(line, line_no) {
                            scene.items.push(item);
                        }
                    }
                }
                _ => {}
            }
        }

        self.flush_pending_cast();

        if let Some(mut s) = current_scene {
            s.casts = self.casts.clone();
            Self::fill_items_scene_config(&mut s);
            scenes.push(s);
        }

        Ok(scenes)
    }

    /// シーン確定時に、各 Speech アイテム埋め込みの scene_config へ Scene.config を転写する。
    /// (parse_script_line の時点ではシーン文脈を持たないため、ここで確定値を反映する)
    fn fill_items_scene_config(s: &mut Scene) {
        let cfg = s.config.clone();
        for item in s.items.iter_mut() {
            if let crate::types::ScriptItem::Speech { scene_config, .. } = item {
                *scene_config = cfg.clone();
            }
        }
    }

    /// 収集中のキャストの自由記述をバッファから確定させ、`Cast.appearance`へ書き込む。
    /// 何も収集していない場合は何もしない(空行・セクション境界のたびに無条件で呼んでよい)。
    fn flush_pending_cast(&mut self) {
        if let Some(name) = self.pending_cast_name.take() {
            if !self.pending_cast_appearance.is_empty() {
                if let Some(cast) = self.casts.get_mut(&name) {
                    cast.appearance = Some(self.pending_cast_appearance.join("\n"));
                }
            }
            self.pending_cast_appearance.clear();
        }
    }

    pub fn parse_file(&mut self, path: &std::path::Path) -> anyhow::Result<Vec<Scene>> {
        let text = std::fs::read_to_string(path)?;
        self.parse_str(&text)
    }

    /// 数値パラメータとして書かれたが有限値でない（`nan` / `inf` など）キーを警告に積む。
    fn warn_non_finite(&mut self, line_no: usize, rejected: &[String]) {
        for key in rejected {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "パラメータ「{key}」の値が有限の数値ではありません（このパラメータは無視されます）"
                ),
            });
        }
    }

    fn parse_scene_header(&mut self, raw: &str, line_no: usize) -> SceneConfig {
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let mut name_tokens = Vec::new();
        let mut param_tokens = Vec::new();
        for t in &tokens {
            if t.contains('=') {
                param_tokens.push(*t);
            } else {
                name_tokens.push(*t);
            }
        }
        let name = name_tokens.join(" ");
        let (params, rejected) = extract_kv_params(&param_tokens.join(","));
        self.warn_non_finite(line_no, &rejected);
        SceneConfig {
            room_size: params.get("room_size").copied(),
            reverb_wet: params.get("reverb_wet").copied(),
            room_w: params.get("room_w").copied(),
            room_d: params.get("room_d").copied(),
            room_h: params.get("room_h").copied(),
            listener_dx: params.get("listener_dx").copied(),
            listener_dy: params.get("listener_dy").copied(),
            listener_z: params.get("listener_z").copied(),
            ..SceneConfig::new(name)
        }
    }

    fn parse_pause_line(&mut self, line: &str, line_no: usize) {
        let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
        if parts.len() != 2 {
            return;
        }
        let key = parts[0].trim();
        let Ok(val) = parts[1].trim().parse::<f64>() else {
            return;
        };
        if !val.is_finite() {
            self.warn_non_finite(line_no, &[key.to_string()]);
            return;
        }
        match key {
            "sentence" | "sentens" => self.pause_config.sentence_ms = val,
            "cast" => self.pause_config.cast_ms = val,
            "paragraph" => self.pause_config.paragraph_ms = val,
            _ => {}
        }
    }

    fn parse_asset_line(&mut self, line: &str) {
        if let Some((k, v)) = line.split_once('=') {
            self.asset_config
                .insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    /// 収集中のキャストがある状態で読んだ行が、キャスト定義行の書式
    /// (`役名:話者名:style,engine,...`、コロン2つ以上・3番目の要素にカンマを含む)に
    /// 一致するかどうかを判定する。一致する場合、空行区切りの付け忘れによって
    /// 定義行が自由記述に誤って飲み込まれた可能性が高いため、呼び出し側で
    /// 警告を出す判定に使う(この判定自体は自由記述として収集する挙動を変えない)。
    fn looks_like_cast_definition(line: &str) -> bool {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        parts.len() == 3 && parts[2].contains(',')
    }

    fn parse_cast_line(&mut self, line: &str, line_no: usize) {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() < 3 {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "キャスト定義行の書式（役名:話者名:style,エンジン名,...）になっていません（この行は無視されます）: {line}"
                ),
            });
            return;
        }
        let name = parts[0].trim().to_string();
        let speaker_name = parts[1].trim().to_string();
        let remain = parts[2].trim();

        let sub: Vec<&str> = remain.splitn(3, ',').collect();
        let style = sub.first().map(|s| s.trim()).unwrap_or("").to_string();
        let engine_type = sub.get(1).map(|s| s.trim()).unwrap_or("").to_string();
        let params_str = sub.get(2).copied().unwrap_or("");

        let (mut params, rejected) = extract_cast_params(params_str);
        self.warn_non_finite(line_no, &rejected);
        // 音響パラメータは数値でなければ意味をなさない。文字列なら警告して既定値を使う。
        let mut take_number = |key: &str| -> Option<f64> {
            let v = params.remove(key)?;
            match v.as_f64() {
                Some(f) => Some(f),
                None => {
                    self.warnings.push(ParseWarning {
                        line_no,
                        message: format!(
                            "パラメータ「{key}」の値が数値ではありません（既定値を使います）: {v}"
                        ),
                    });
                    None
                }
            }
        };
        let pan = take_number("pan").unwrap_or(0.0);
        let distance = take_number("distance").unwrap_or(1.0);
        let volume = take_number("volume").unwrap_or(1.0);
        let height = take_number("height");
        params.insert("style".to_string(), Value::String(style));

        let cast_key = name.clone();
        self.casts.insert(
            cast_key.clone(),
            Cast {
                name,
                speaker_name,
                engine_type,
                pan,
                distance,
                volume,
                params,
                height,
                height_offset: 0.0,
                appearance: None,
            },
        );
        self.pending_cast_name = Some(cast_key);
    }

    /// `#bgm_start` / `#se` のファイル名を、台本仕様どおり `@asset` の `bgm_dir` / `se_dir` から探す。
    /// 絶対パス、または該当ディレクトリが未指定なら書かれたまま使う。
    /// 解決後のファイルが存在しなければ警告する（存在しない BGM/SE はミックスにも FCPXML にも入らない）。
    fn resolve_asset_path(
        &mut self,
        dir_key: &str,
        label: &str,
        arg: &str,
        line_no: usize,
    ) -> String {
        if arg.is_empty() {
            return String::new();
        }
        let path = std::path::Path::new(arg);
        let resolved = match self.asset_config.get(dir_key) {
            Some(dir) if !path.is_absolute() => std::path::Path::new(dir).join(path),
            _ => path.to_path_buf(),
        };
        if !resolved.is_file() {
            self.warnings.push(ParseWarning {
                line_no,
                message: format!(
                    "{label}ファイルが見つかりません（音声に含まれません）: {}",
                    resolved.display()
                ),
            });
        }
        resolved.to_string_lossy().into_owned()
    }

    fn parse_script_line(&mut self, line: &str, line_no: usize) -> Option<ScriptItem> {
        // 数字のみ → Parallel
        if line.trim().chars().all(|c| c.is_ascii_digit()) && !line.trim().is_empty() {
            let n: usize = line.trim().parse().ok()?;
            return Some(ScriptItem::Command(ScriptCommand::Parallel(n)));
        }

        // # コマンド行
        if line.starts_with('#') {
            let rest = &line[1..];
            // `# ` または `#\t` → コメント
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
                return None;
            }
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            let cmd = parts[0];
            let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");
            return match cmd {
                "pause" => arg
                    .parse::<f64>()
                    .ok()
                    .map(|ms| ScriptItem::Command(ScriptCommand::Pause(ms))),
                "paragraph" => {
                    let name = if arg.is_empty() {
                        None
                    } else {
                        Some(arg.to_string())
                    };
                    let name = match name {
                        Some(n) if n.contains(']') => {
                            self.warnings.push(ParseWarning {
                                line_no,
                                message: format!("#paragraph の名称に「]」は使えません（名称なしとして扱います）: {n}"),
                            });
                            None
                        }
                        Some(n) => {
                            if n.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
                                self.warnings.push(ParseWarning {
                                    line_no,
                                    message: format!("#paragraph の名称にファイル名として使えない文字が含まれます（素材の自動検索が失敗します）: {n}"),
                                });
                            }
                            Some(n)
                        }
                        None => None,
                    };
                    Some(ScriptItem::Command(ScriptCommand::Paragraph(name)))
                }
                "bgm_start" => Some(ScriptItem::Command(ScriptCommand::BgmStart(
                    self.resolve_asset_path("bgm_dir", "BGM", arg, line_no),
                ))),
                "bgm_stop" => Some(ScriptItem::Command(ScriptCommand::BgmStop)),
                "se" => Some(ScriptItem::Command(ScriptCommand::Se(
                    self.resolve_asset_path("se_dir", "SE", arg, line_no),
                ))),
                _ => None,
            };
        }

        // 台詞行: `役名(params):テキスト`（区切りは半角コロンのみ）
        let Some((name_part, raw_text)) = line.split_once(':') else {
            if line.contains('：') {
                self.warnings.push(ParseWarning {
                    line_no,
                    message: "行に全角「：」が使われています。役名の区切りは半角「:」です（この行は無視されます）".to_string(),
                });
            }
            return None;
        };
        let name_part = name_part.trim();
        let raw_text = raw_text.trim();

        let (role, params_str) = if let Some(idx) = name_part.find('(') {
            let role = name_part[..idx].trim();
            let params_inner = name_part[idx + 1..].trim_end_matches(')');
            (role, params_inner)
        } else {
            (name_part, "")
        };

        if !self.casts.contains_key(role) {
            let hint = if role.contains('：') {
                "（役名の区切りが全角「：」になっていないか確認してください）"
            } else {
                ""
            };
            self.warnings.push(ParseWarning {
                line_no,
                message: format!("キャスト「{role}」が未定義です（この行は無視されます）{hint}"),
            });
            return None;
        }

        if raw_text.contains('｜') {
            self.warnings.push(ParseWarning {
                line_no,
                message: "台詞に全角「｜」が含まれます。ルビの区切りは半角「|」です（全角は区切りとして扱われません）".to_string(),
            });
        }

        let (text, display_text) = expand_ruby(raw_text);
        let (offset_params, rejected) = extract_kv_params(params_str);
        self.warn_non_finite(line_no, &rejected);

        Some(ScriptItem::Speech {
            cast_name: role.to_string(),
            text,
            display_text,
            offset_params,
            scene_config: crate::types::SceneConfig::new(String::new()),
        })
    }
}

impl Default for ScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

/// ルビ記法の正規表現。半角パイプのみを区切りとする（全角｜は対象外）。
fn ruby_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"'([^'|]+?)\|([^'|]+?)'").expect("ルビ正規表現が不正"))
}

/// `'word|reading'` → (reading_text, display_text) に展開
fn expand_ruby(text: &str) -> (String, String) {
    let mut synthesis = text.to_string();
    let mut display = text.to_string();

    for cap in ruby_re().captures_iter(text) {
        let full = &cap[0];
        let word = &cap[1];
        let reading = &cap[2];
        synthesis = synthesis.replace(full, reading);
        display = display.replace(full, word);
    }
    (synthesis, display)
}

/// カンマ区切り `key=value` 文字列から数値パラメータを抽出する。
///
/// 数値として読めない値は捨てる。`nan` / `inf` のように数値としては読めるが有限でない値も捨て、
/// そのキーを2つ目の戻り値で返す（呼び出し側で警告にする）。
fn extract_kv_params(s: &str) -> (HashMap<String, f64>, Vec<String>) {
    let mut map = HashMap::new();
    let mut rejected = Vec::new();
    for part in s.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            if let Ok(val) = v.trim().parse::<f64>() {
                if val.is_finite() {
                    map.insert(k.trim().to_string(), val);
                } else {
                    rejected.push(k.trim().to_string());
                }
            }
        }
    }
    (map, rejected)
}

/// `@cast` 定義行用のパラメータ抽出。`extract_kv_params` と違い、数値として読めない値は
/// 文字列として保持する（`language=en` など。台本仕様の「使用言語」）。
/// 有限でない数値の扱いは `extract_kv_params` と同じ。
fn extract_cast_params(s: &str) -> (HashMap<String, Value>, Vec<String>) {
    let mut map = HashMap::new();
    let mut rejected = Vec::new();
    for part in s.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            match v.parse::<f64>() {
                Ok(val) if val.is_finite() => {
                    map.insert(k.to_string(), Value::from(val));
                }
                Ok(_) => rejected.push(k.to_string()),
                Err(_) if !v.is_empty() => {
                    map.insert(k.to_string(), Value::String(v.to_string()));
                }
                Err(_) => {}
            }
        }
    }
    (map, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ScriptCommand, ScriptItem};

    const SIMPLE_SCRIPT: &str = r#"
@scene 居間 room_size=0.3 reverb_wet=0.5

@pause
sentence 200
cast 500
paragraph 1000

@cast
ずんだもん:ずんだもん:ノーマル,voicevox,pan=-30,distance=1.0

四国めたん:四国めたん:ノーマル,voicevox,pan=30

@script
ずんだもん:こんにちは！
四国めたん:こんにちは！
#pause 500
#paragraph
ずんだもん:またね。
"#;

    #[test]
    fn parses_single_scene() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].config.name, "居間");
    }

    #[test]
    fn parses_scene_room_params() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let sc = &scenes[0].config;
        assert!((sc.room_size.unwrap() - 0.3).abs() < 1e-6);
        assert!((sc.reverb_wet.unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn speech_items_carry_their_scene_config() {
        // produce() は Speech アイテム埋め込みの scene_config を音響処理に渡すため、
        // パーサがシーン確定時に Scene.config をアイテムへ転写していることを保証する。
        let src = "@scene 一 room_size=0.8 reverb_wet=0.3\n\
                   @cast\nA:話者:ノーマル,voicevox,pan=0\n\
                   @script\nA:こんにちは\n\
                   @scene 二 room_w=20 room_d=30 room_h=10 listener_z=1.5\n\
                   @script\nA:こんばんは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        assert_eq!(scenes.len(), 2);

        let ScriptItem::Speech { scene_config, .. } = &scenes[0].items[0] else {
            panic!("scene0 items[0] が Speech ではない");
        };
        assert_eq!(scene_config.name, "一");
        assert_eq!(scene_config.room_size, Some(0.8));
        assert_eq!(scene_config.reverb_wet, Some(0.3));

        let ScriptItem::Speech { scene_config, .. } = &scenes[1].items[0] else {
            panic!("scene1 items[0] が Speech ではない");
        };
        assert_eq!(scene_config.name, "二");
        assert_eq!(scene_config.room_w, Some(20.0));
        assert_eq!(scene_config.room_d, Some(30.0));
        assert_eq!(scene_config.room_h, Some(10.0));
        assert_eq!(scene_config.listener_z, Some(1.5));
    }

    #[test]
    fn parses_cast_entries() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let casts = &scenes[0].casts;
        assert!(casts.contains_key("ずんだもん"));
        let c = &casts["ずんだもん"];
        assert_eq!(c.engine_type, "voicevox");
        assert!((c.pan - (-30.0)).abs() < 1e-6);
        assert!((c.distance - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parses_speech_items() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let speeches: Vec<_> = scenes[0]
            .items
            .iter()
            .filter(|i| matches!(i, ScriptItem::Speech { .. }))
            .collect();
        assert_eq!(speeches.len(), 3);
        if let ScriptItem::Speech {
            cast_name, text, ..
        } = &speeches[0]
        {
            assert_eq!(cast_name, "ずんだもん");
            assert_eq!(text, "こんにちは！");
        } else {
            panic!("expected speech");
        }
    }

    #[test]
    fn parses_pause_command() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let pause_item = scenes[0]
            .items
            .iter()
            .find(|i| matches!(i, ScriptItem::Command(ScriptCommand::Pause(_))));
        assert!(pause_item.is_some());
        if let ScriptItem::Command(ScriptCommand::Pause(ms)) = pause_item.unwrap() {
            assert!((ms - 500.0).abs() < 1e-6);
        }
    }

    #[test]
    fn parses_paragraph_command() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let found = scenes[0]
            .items
            .iter()
            .any(|i| matches!(i, ScriptItem::Command(ScriptCommand::Paragraph(None))));
        assert!(found);
    }

    #[test]
    fn parses_paragraph_with_name() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:こんにちは
#paragraph オープニング 前半
A:さようなら
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        let name = scenes[0].items.iter().find_map(|i| match i {
            ScriptItem::Command(ScriptCommand::Paragraph(n)) => Some(n.clone()),
            _ => None,
        });
        assert_eq!(name, Some(Some("オープニング 前半".to_string())));
    }

    #[test]
    fn paragraph_name_with_bracket_is_dropped_with_warning() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:こんにちは
#paragraph 変な]名前
"#;
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(script).unwrap();
        let name = scenes[0].items.iter().find_map(|i| match i {
            ScriptItem::Command(ScriptCommand::Paragraph(n)) => Some(n.clone()),
            _ => None,
        });
        assert_eq!(name, Some(None), "]を含む名称は無名として扱う");
        assert!(parser.warnings.iter().any(|w| w.message.contains("]")));
    }

    #[test]
    fn paragraph_name_with_invalid_filename_char_warns_but_is_kept() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:こんにちは
#paragraph a/b
"#;
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(script).unwrap();
        let name = scenes[0].items.iter().find_map(|i| match i {
            ScriptItem::Command(ScriptCommand::Paragraph(n)) => Some(n.clone()),
            _ => None,
        });
        assert_eq!(name, Some(Some("a/b".to_string())));
        assert!(parser
            .warnings
            .iter()
            .any(|w| w.message.contains("ファイル名")));
    }

    #[test]
    fn parses_multiple_scenes() {
        let script = r#"
@scene 居間 room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:こんにちは

@scene 屋外 room_size=0.8 reverb_wet=0.2

@script
A:さようなら
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        assert_eq!(scenes.len(), 2);
        assert_eq!(scenes[0].config.name, "居間");
        assert_eq!(scenes[1].config.name, "屋外");
        assert!((scenes[1].config.room_size.unwrap() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn comment_lines_are_skipped() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:セリフ
# これはコメント
A:続き
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        let count = scenes[0]
            .items
            .iter()
            .filter(|i| matches!(i, ScriptItem::Speech { .. }))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn parallel_command_parsed() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox
B:B:スタイル,voicevox

@script
2
A:セリフA
B:セリフB
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        let parallel = scenes[0]
            .items
            .iter()
            .find(|i| matches!(i, ScriptItem::Command(ScriptCommand::Parallel(_))));
        assert!(parallel.is_some());
        if let ScriptItem::Command(ScriptCommand::Parallel(n)) = parallel.unwrap() {
            assert_eq!(*n, 2);
        }
    }

    #[test]
    fn ruby_notation_separates_text_and_display() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:'東京|とうきょう'に行く
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        if let ScriptItem::Speech {
            text, display_text, ..
        } = &scenes[0].items[0]
        {
            assert_eq!(text, "とうきょうに行く");
            assert_eq!(display_text, "東京に行く");
        } else {
            panic!("expected speech");
        }
    }

    #[test]
    fn ruby_word_may_contain_colon() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:'13:00|じゅうさんじ'に始めます
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        if let ScriptItem::Speech {
            text, display_text, ..
        } = &scenes[0].items[0]
        {
            assert_eq!(text, "じゅうさんじに始めます");
            assert_eq!(display_text, "13:00に始めます");
        } else {
            panic!("expected speech");
        }
    }

    #[test]
    fn legacy_colon_ruby_is_not_expanded() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:'東京:とうきょう'に行く
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        if let ScriptItem::Speech {
            text, display_text, ..
        } = &scenes[0].items[0]
        {
            assert_eq!(text, "'東京:とうきょう'に行く");
            assert_eq!(display_text, "'東京:とうきょう'に行く");
        } else {
            panic!("expected speech");
        }
    }

    #[test]
    fn fullwidth_pipe_is_not_a_ruby_separator() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:'東京｜とうきょう'に行く
"#;
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(script).unwrap();
        if let ScriptItem::Speech { text, .. } = &scenes[0].items[0] {
            assert_eq!(text, "'東京｜とうきょう'に行く");
        } else {
            panic!("expected speech");
        }
        assert!(
            parser.warnings.iter().any(|w| w.message.contains("｜")),
            "全角パイプの警告が出るべき: {:?}",
            parser.warnings
        );
    }

    #[test]
    fn bgm_start_stop_parsed() {
        let script = r#"
@scene テスト room_size=0.1

@cast

@script
#bgm_start bgm01.wav
#bgm_stop
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        let bgm_start = scenes[0]
            .items
            .iter()
            .any(|i| matches!(i, ScriptItem::Command(ScriptCommand::BgmStart(_))));
        let bgm_stop = scenes[0]
            .items
            .iter()
            .any(|i| matches!(i, ScriptItem::Command(ScriptCommand::BgmStop)));
        assert!(bgm_start, "bgm_start not found");
        assert!(bgm_stop, "bgm_stop not found");
    }

    #[test]
    fn pause_config_from_section() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let pc = &scenes[0].pause_config;
        assert!((pc.sentence_ms - 200.0).abs() < 1e-6);
        assert!((pc.cast_ms - 500.0).abs() < 1e-6);
        assert!((pc.paragraph_ms - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn speech_with_inline_params() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox,pan=0

@script
A(pan=15,distance=2):セリフ
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        if let ScriptItem::Speech { offset_params, .. } = &scenes[0].items[0] {
            assert!((offset_params["pan"] - 15.0).abs() < 1e-6);
            assert!((offset_params["distance"] - 2.0).abs() < 1e-6);
        } else {
            panic!("expected speech");
        }
    }

    #[test]
    fn scene_header_parses_room_dims_and_listener() {
        let mut p = ScriptParser::new();
        let sc = p.parse_scene_header(
            "ホール room_w=25 room_d=45 room_h=18 listener_dx=0 listener_dy=-15",
            1,
        );
        assert_eq!(sc.name, "ホール");
        assert_eq!(sc.room_w, Some(25.0));
        assert_eq!(sc.room_d, Some(45.0));
        assert_eq!(sc.room_h, Some(18.0));
        assert_eq!(sc.listener_dx, Some(0.0));
        assert_eq!(sc.listener_dy, Some(-15.0));
    }

    #[test]
    fn scene_header_room_dims_default_none() {
        let mut p = ScriptParser::new();
        let sc = p.parse_scene_header("小部屋 room_size=0.1", 1);
        assert_eq!(sc.room_w, None);
        assert_eq!(sc.room_d, None);
        assert_eq!(sc.room_h, None);
        assert_eq!(sc.listener_dx, None);
        assert_eq!(sc.listener_dy, None);
        assert_eq!(sc.room_size, Some(0.1));
    }

    #[test]
    fn scene_header_parses_listener_z() {
        let mut p = ScriptParser::new();
        let sc = p.parse_scene_header("舞台 room_w=20 room_d=30 room_h=12 listener_z=1.1", 1);
        assert_eq!(sc.listener_z, Some(1.1));
        let sc2 = p.parse_scene_header("小部屋 room_size=0.1", 1);
        assert_eq!(sc2.listener_z, None);
    }

    #[test]
    fn reusing_parser_across_scripts_does_not_leak_casts_or_pause_config() {
        let mut parser = ScriptParser::new();
        let _ = parser.parse_str(SIMPLE_SCRIPT).unwrap(); // ずんだもん/四国めたんを定義, pause 200/500/1000

        let second_script = "@scene 別の台本\n@script\nずんだもん:これは前の台本のキャストのはず\n";
        let scenes = parser.parse_str(second_script).unwrap();

        // 前の台本のキャストが漏れて有効な話者として扱われてはいけない
        let speeches: Vec<_> = scenes[0]
            .items
            .iter()
            .filter(|i| matches!(i, ScriptItem::Speech { .. }))
            .collect();
        assert_eq!(
            speeches.len(),
            0,
            "前の台本のキャストが引き継がれてはいけない"
        );
        assert!(
            parser
                .warnings()
                .iter()
                .any(|w| w.message.contains("ずんだもん")),
            "未定義キャストとして警告が出るべき: {:?}",
            parser.warnings()
        );

        // pause_configも既定値(500/300/1500)に戻っているべき(前の台本は200/500/1000)
        let pc = &scenes[0].pause_config;
        assert!(
            (pc.sentence_ms - 500.0).abs() < 1e-6,
            "既定値に戻っているべき: {}",
            pc.sentence_ms
        );
        assert!(
            (pc.cast_ms - 300.0).abs() < 1e-6,
            "既定値に戻っているべき: {}",
            pc.cast_ms
        );
        assert!(
            (pc.paragraph_ms - 1500.0).abs() < 1e-6,
            "既定値に戻っているべき: {}",
            pc.paragraph_ms
        );
    }

    #[test]
    fn unknown_cast_produces_warning_with_line_number() {
        let mut p = ScriptParser::new();
        let src = "@scene テスト room_size=0.1\n@cast\nA:話者:ノーマル,voicevox,pan=0\n@script\nA:こんにちは\n誰か:こんばんは\n";
        let scenes = p.parse_str(src).unwrap();
        // 未定義キャスト行は従来どおり無視される
        let n = scenes[0]
            .items
            .iter()
            .filter(|i| matches!(i, ScriptItem::Speech { .. }))
            .count();
        assert_eq!(n, 1);
        // 警告が行番号付きで記録される
        let w = p.warnings();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].line_no, 6);
        assert!(w[0].message.contains("誰か"));
    }

    #[test]
    fn warnings_are_reset_per_parse() {
        let mut p = ScriptParser::new();
        let src = "@scene テスト room_size=0.1\n@cast\nA:話者:ノーマル,voicevox,pan=0\n@script\n誰か:こんばんは\n";
        p.parse_str(src).unwrap();
        assert_eq!(p.warnings().len(), 1);
        p.parse_str(src).unwrap();
        assert_eq!(p.warnings().len(), 1, "2回目のparseで累積しない");
    }

    #[test]
    fn cast_line_parses_height_into_field() {
        let scenes = ScriptParser::new()
            .parse_str("@scene テスト room_size=0.1\n@cast\nA:話者:ノーマル,voicevox,pan=0,height=1.7\n@script\nA: こんにちは\n")
            .unwrap();
        let cast = scenes[0].casts.get("A").unwrap();
        assert_eq!(cast.height, Some(1.7));
        assert!((cast.height_offset - 0.0).abs() < 1e-10);
    }

    #[test]
    fn cast_appearance_collected_until_blank_line() {
        let src = "@scene テスト room_size=0.1\n\
                   @cast\n\
                   ずんだもん:ずんだもん:ノーマル,voicevox,pan=0\n\
                   小柄で緑髪の元気なキャラクター。\n\
                   ずんだ餅のイメージカラーの服を着ている。\n\
                   \n\
                   @script\n\
                   ずんだもん:こんにちは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        let cast = scenes[0].casts.get("ずんだもん").unwrap();
        assert_eq!(
            cast.appearance.as_deref(),
            Some("小柄で緑髪の元気なキャラクター。\nずんだ餅のイメージカラーの服を着ている。")
        );
    }

    #[test]
    fn cast_without_free_text_has_no_appearance() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        let cast = scenes[0].casts.get("ずんだもん").unwrap();
        assert_eq!(cast.appearance, None);
    }

    #[test]
    fn cast_entries_without_blank_line_separator_merge_into_appearance() {
        // 空行を挟まないと、次のキャスト定義行が前のキャストの自由記述として飲み込まれる(仕様どおりの制約)
        let src = "@scene テスト room_size=0.1\n\
                   @cast\n\
                   A:話者A:ノーマル,voicevox,pan=0\n\
                   B:話者B:ノーマル,voicevox,pan=10\n\
                   \n\
                   @script\n\
                   A:こんにちは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        assert!(scenes[0].casts.contains_key("A"));
        assert!(!scenes[0].casts.contains_key("B"));
        let cast_a = scenes[0].casts.get("A").unwrap();
        assert_eq!(
            cast_a.appearance.as_deref(),
            Some("B:話者B:ノーマル,voicevox,pan=10")
        );
    }

    #[test]
    fn swallowed_cast_definition_line_produces_warning() {
        let src = "@cast\n\
                   A:話者A:ノーマル,voicevox,pan=0\n\
                   B:話者B:ノーマル,voicevox,pan=10\n\
                   \n\
                   @script\n\
                   A:こんにちは\n";
        let mut parser = ScriptParser::new();
        let _ = parser.parse_str(src).unwrap();
        assert!(
            parser
                .warnings()
                .iter()
                .any(|w| w.message.contains("B:話者B:ノーマル,voicevox,pan=10")),
            "expected a warning about the swallowed B definition, got: {:?}",
            parser.warnings()
        );
    }

    #[test]
    fn genuine_free_text_does_not_produce_swallow_warning() {
        let src = "@cast\n\
                   A:話者A:ノーマル,voicevox,pan=0\n\
                   小柄で緑髪の元気なキャラクター。\n\
                   \n\
                   @script\n\
                   A:こんにちは\n";
        let mut parser = ScriptParser::new();
        let _ = parser.parse_str(src).unwrap();
        assert!(
            parser.warnings().is_empty(),
            "genuine free text should not trigger the swallow warning: {:?}",
            parser.warnings()
        );
    }

    #[test]
    fn cast_appearance_flushes_without_trailing_blank_line_before_next_section() {
        let src = "@scene テスト room_size=0.1\n\
                   @cast\n\
                   A:話者A:ノーマル,voicevox,pan=0\n\
                   眼鏡をかけた青年。\n\
                   @script\n\
                   A:こんにちは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        let cast = scenes[0].casts.get("A").unwrap();
        assert_eq!(cast.appearance.as_deref(), Some("眼鏡をかけた青年。"));
    }

    #[test]
    fn scene_description_collected_until_next_section() {
        let src = "@scene 教室 room_size=0.3\n\
                   放課後の静かな教室。窓から夕日が差し込んでいる。\n\
                   黒板には日直の名前が書かれている。\n\
                   @pause\n\
                   sentence 200\n\
                   @cast\n\
                   A:A:ノーマル,voicevox\n\
                   @script\n\
                   A:こんにちは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        assert_eq!(
            scenes[0].config.description.as_deref(),
            Some("放課後の静かな教室。窓から夕日が差し込んでいる。\n黒板には日直の名前が書かれている。")
        );
    }

    #[test]
    fn scene_without_free_text_has_no_description() {
        let scenes = ScriptParser::new().parse_str(SIMPLE_SCRIPT).unwrap();
        assert_eq!(scenes[0].config.description, None);
    }

    #[test]
    fn scene_description_ignores_blank_lines_within_block() {
        let src = "@scene 教室 room_size=0.3\n\
                   一行目の描写。\n\
                   \n\
                   二行目の描写。\n\
                   @cast\n\
                   A:A:ノーマル,voicevox\n\
                   @script\n\
                   A:こんにちは\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        assert_eq!(
            scenes[0].config.description.as_deref(),
            Some("一行目の描写。\n二行目の描写。")
        );
    }

    #[test]
    fn fullwidth_role_separator_is_rejected_with_warning() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A：こんにちは
"#;
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(script).unwrap();
        assert!(scenes[0].items.is_empty(), "全角：の行は台詞として扱わない");
        assert!(
            parser.warnings.iter().any(|w| w.message.contains("全角")),
            "全角：の警告が出るべき: {:?}",
            parser.warnings
        );
    }

    #[test]
    fn halfwidth_colon_in_dialogue_does_not_break_role_split() {
        let script = r#"
@scene テスト room_size=0.1

@cast
A:A:スタイル,voicevox

@script
A:開始は'13:00|じゅうさんじ'です
"#;
        let scenes = ScriptParser::new().parse_str(script).unwrap();
        if let ScriptItem::Speech {
            cast_name, text, ..
        } = &scenes[0].items[0]
        {
            assert_eq!(cast_name, "A");
            assert_eq!(text, "開始はじゅうさんじです");
        } else {
            panic!("expected speech");
        }
    }

    // ---- s2v-18b.2: @cast の文字列パラメータ ----

    #[test]
    fn cast_string_param_is_kept_as_string() {
        let src = "@scene s\n@cast\nE:Ana:default,xtts,language=en,pan=10\n@script\nE:Hello\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        let c = &scenes[0].casts["E"];
        assert_eq!(c.params["language"].as_str(), Some("en"));
        assert!((c.pan - 10.0).abs() < 1e-9);
        assert!(parser.warnings().is_empty(), "{:?}", parser.warnings());
    }

    #[test]
    fn cast_numeric_param_stays_numeric() {
        let src = "@scene s\n@cast\nA:A:ノーマル,voicevox,speedScale=1.2\n@script\nA:x\n";
        let scenes = ScriptParser::new().parse_str(src).unwrap();
        assert_eq!(
            scenes[0].casts["A"].params["speedScale"].as_f64(),
            Some(1.2)
        );
    }

    #[test]
    fn cast_non_numeric_acoustic_param_warns_and_uses_default() {
        let src = "@scene s\n@cast\nA:A:ノーマル,voicevox,pan=left\n@script\nA:x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        let c = &scenes[0].casts["A"];
        assert_eq!(c.pan, 0.0);
        assert!(!c.params.contains_key("pan"));
        let w = parser.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].line_no, 3);
        assert!(w[0].message.contains("pan"), "{}", w[0].message);
    }

    // ---- s2v-18b.11: @cast 定義行の書式不正 ----

    #[test]
    fn malformed_cast_definition_line_warns_with_line_number() {
        let src = "@scene s\n@cast\nA:話者のみ\n@script\nA:x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        assert!(scenes[0].casts.is_empty());
        let w = parser.warnings();
        assert!(
            w.iter()
                .any(|w| w.line_no == 3 && w.message.contains("書式")),
            "{w:?}"
        );
    }

    // ---- s2v-18b.12: 非有限の数値 ----

    #[test]
    fn non_finite_cast_param_is_rejected_with_warning() {
        let src = "@scene s\n@cast\nA:A:ノーマル,voicevox,pan=nan,volume=inf,speedScale=-infinity\n@script\nA:x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        let c = &scenes[0].casts["A"];
        assert_eq!(c.pan, 0.0);
        assert_eq!(c.volume, 1.0);
        assert!(!c.params.contains_key("speedScale"));
        assert_eq!(parser.warnings().len(), 3, "{:?}", parser.warnings());
        assert!(parser.warnings().iter().all(|w| w.line_no == 3));
    }

    #[test]
    fn non_finite_inline_param_is_rejected_with_warning() {
        let src = "@scene s\n@cast\nA:A:ノーマル,voicevox\n@script\nA(pan=inf,volume=0.1):x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        let ScriptItem::Speech { offset_params, .. } = &scenes[0].items[0] else {
            panic!("expected speech");
        };
        assert!(!offset_params.contains_key("pan"));
        assert_eq!(offset_params.get("volume"), Some(&0.1));
        let w = parser.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].line_no, 5);
    }

    #[test]
    fn non_finite_scene_param_is_rejected_with_warning() {
        let src =
            "@scene s room_size=NaN reverb_wet=0.5\n@cast\nA:A:ノーマル,voicevox\n@script\nA:x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        assert_eq!(scenes[0].config.room_size, None);
        assert_eq!(scenes[0].config.reverb_wet, Some(0.5));
        let w = parser.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].line_no, 1);
    }

    #[test]
    fn non_finite_pause_value_is_ignored_with_warning() {
        let src = "@pause\nsentence inf\ncast 300\n@scene s\n@cast\nA:A:ノーマル,voicevox\n@script\nA:x\n";
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(src).unwrap();
        assert_eq!(
            scenes[0].pause_config.sentence_ms,
            PauseConfig::default().sentence_ms
        );
        assert_eq!(scenes[0].pause_config.cast_ms, 300.0);
        let w = parser.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].line_no, 2);
    }

    // ---- s2v-18b.4: @asset の bgm_dir / se_dir ----

    fn asset_commands(scene: &Scene) -> (Vec<String>, Vec<String>) {
        let mut bgm = Vec::new();
        let mut se = Vec::new();
        for item in &scene.items {
            match item {
                ScriptItem::Command(ScriptCommand::BgmStart(p)) => bgm.push(p.clone()),
                ScriptItem::Command(ScriptCommand::Se(p)) => se.push(p.clone()),
                _ => {}
            }
        }
        (bgm, se)
    }

    #[test]
    fn relative_bgm_and_se_are_resolved_against_asset_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let bgm_dir = dir.path().join("BGM 音源");
        let se_dir = dir.path().join("se");
        std::fs::create_dir_all(&bgm_dir).unwrap();
        std::fs::create_dir_all(&se_dir).unwrap();
        std::fs::write(bgm_dir.join("a.wav"), b"").unwrap();
        std::fs::write(se_dir.join("b.wav"), b"").unwrap();

        let src = format!(
            "@scene s\n@asset\nbgm_dir = {}\nse_dir = {}\n@cast\nA:A:ノーマル,voicevox\n@script\n#bgm_start a.wav\n#se b.wav\nA:x\n",
            bgm_dir.display(),
            se_dir.display()
        );
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(&src).unwrap();
        let (bgm, se) = asset_commands(&scenes[0]);
        assert_eq!(
            bgm,
            vec![bgm_dir.join("a.wav").to_string_lossy().into_owned()]
        );
        assert_eq!(
            se,
            vec![se_dir.join("b.wav").to_string_lossy().into_owned()]
        );
        assert!(parser.warnings().is_empty(), "{:?}", parser.warnings());
    }

    #[test]
    fn absolute_bgm_path_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join("abs.wav");
        std::fs::write(&abs, b"").unwrap();
        let src = format!(
            "@scene s\n@asset\nbgm_dir = D:\\no\\such\\dir\n@cast\nA:A:ノーマル,voicevox\n@script\n#bgm_start {}\nA:x\n",
            abs.display()
        );
        let mut parser = ScriptParser::new();
        let scenes = parser.parse_str(&src).unwrap();
        let (bgm, _) = asset_commands(&scenes[0]);
        assert_eq!(bgm, vec![abs.to_string_lossy().into_owned()]);
        assert!(parser.warnings().is_empty(), "{:?}", parser.warnings());
    }

    #[test]
    fn missing_bgm_or_se_file_produces_warning() {
        let dir = tempfile::tempdir().unwrap();
        let src = format!(
            "@scene s\n@asset\nbgm_dir = {0}\nse_dir = {0}\n@cast\nA:A:ノーマル,voicevox\n@script\n#bgm_start none.wav\n#se none2.wav\nA:x\n",
            dir.path().display()
        );
        let mut parser = ScriptParser::new();
        parser.parse_str(&src).unwrap();
        let w = parser.warnings();
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0].line_no, 8);
        assert!(w[0].message.contains("none.wav"), "{}", w[0].message);
        assert_eq!(w[1].line_no, 9);
    }
}
