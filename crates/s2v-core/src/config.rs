use std::path::Path;

use serde::{Deserialize, Serialize};

/// Script2Video の設定(`config.toml`)。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub audio: AudioConfig,
    pub bgm: BgmConfig,
    /// エクスポート形式の設定(省略可)。
    #[serde(default)]
    pub export: ExportConfig,
    /// 収録音声の取り込みと無音削除(省略可。設計書 5.3、5.6)。
    #[serde(default)]
    pub voice: VoiceConfig,
    /// ラウドネス正規化(省略可。設計書 5.10)。
    #[serde(default)]
    pub normalize: NormalizeConfig,
    /// ミックス(省略可。設計書 5.4)。
    #[serde(default)]
    pub mix: MixConfig,
    /// 動画の描画・合成(省略可。設計書 4.7)。
    #[serde(default)]
    pub render: RenderConfig,
    /// 画像生成プロンプトの共通部分(省略可。計画 2c で使う)。
    #[serde(default)]
    pub prompts: PromptsConfig,
}

/// エクスポート(FCPXML等)に関する設定。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ExportConfig {
    /// FCPXMLを29.97fps(NTSCドロップフレーム相当)で出力するか(既定 false = 30fps)。
    #[serde(default)]
    pub fcpxml_2997fps: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    /// 48000 固定(検証で 48000 以外を拒否する)。
    pub sample_rate: u32,
    pub microphone_spacing: f64,
    pub sound_speed: f64,
    pub air_absorption_coeff: f64,
    pub room_size: f64,
    pub reverb_wet: f64,
    pub reference_dist: f64,
    pub mic_directivity: f64,
    pub mic_angle: f64,
    #[serde(default)]
    pub early_reflections: EarlyConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MaterialConfig {
    pub reflection_coeff: f64,
    pub absorption_cutoff_hz: f64,
}

impl MaterialConfig {
    const fn new(reflection_coeff: f64, absorption_cutoff_hz: f64) -> Self {
        Self {
            reflection_coeff,
            absorption_cutoff_hz,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EarlyConfig {
    #[serde(default = "er_enabled")]
    pub enabled: bool,
    #[serde(default = "er_ear_height")]
    pub ear_height: f64,
    #[serde(default = "er_listener_offset")]
    pub listener_offset: [f64; 2],
    #[serde(default = "er_room_dims_min")]
    pub room_dims_min: [f64; 3],
    #[serde(default = "er_room_dims_max")]
    pub room_dims_max: [f64; 3],
    #[serde(default = "er_floor")]
    pub floor: MaterialConfig,
    #[serde(default = "er_ceiling")]
    pub ceiling: MaterialConfig,
    #[serde(default = "er_front_wall")]
    pub front_wall: MaterialConfig,
    #[serde(default = "er_back_wall")]
    pub back_wall: MaterialConfig,
    #[serde(default = "er_side_walls")]
    pub side_walls: MaterialConfig,
    #[serde(default = "er_early_level")]
    pub early_level: f64,
    #[serde(default = "er_wet_distance_slope")]
    pub wet_distance_slope: f64,
}

fn er_enabled() -> bool {
    true
}
fn er_ear_height() -> f64 {
    1.2
}
fn er_listener_offset() -> [f64; 2] {
    [0.0, 0.0]
}
fn er_room_dims_min() -> [f64; 3] {
    [4.0, 5.0, 3.0]
}
fn er_room_dims_max() -> [f64; 3] {
    [25.0, 45.0, 18.0]
}
fn er_floor() -> MaterialConfig {
    MaterialConfig::new(0.5, 3500.0)
}
fn er_ceiling() -> MaterialConfig {
    MaterialConfig::new(0.6, 6000.0)
}
fn er_front_wall() -> MaterialConfig {
    MaterialConfig::new(0.85, 10000.0)
}
fn er_back_wall() -> MaterialConfig {
    MaterialConfig::new(0.40, 4000.0)
}
fn er_side_walls() -> MaterialConfig {
    MaterialConfig::new(0.70, 8000.0)
}
fn er_early_level() -> f64 {
    1.0
}
fn er_wet_distance_slope() -> f64 {
    0.1
}

impl Default for EarlyConfig {
    fn default() -> Self {
        Self {
            enabled: er_enabled(),
            ear_height: er_ear_height(),
            listener_offset: er_listener_offset(),
            room_dims_min: er_room_dims_min(),
            room_dims_max: er_room_dims_max(),
            floor: er_floor(),
            ceiling: er_ceiling(),
            front_wall: er_front_wall(),
            back_wall: er_back_wall(),
            side_walls: er_side_walls(),
            early_level: er_early_level(),
            wet_distance_slope: er_wet_distance_slope(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BgmConfig {
    pub crossfade_s: f64,
    pub se_fade_out_s: f64,
}

/// 収録音声の取り込みと無音削除の設定。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceConfig {
    /// 無音削除のしきい値[dBFS]。10ms 窓の RMS がこれを超えると「音がある」。
    #[serde(default = "d_trim_threshold_db")]
    pub trim_threshold_db: f64,
    /// 最初の音の手前に残す長さ[ms]。
    #[serde(default = "d_trim_head_margin_ms")]
    pub trim_head_margin_ms: u32,
    /// 最後の音の後に残す長さ[ms]。
    #[serde(default = "d_trim_tail_margin_ms")]
    pub trim_tail_margin_ms: u32,
    /// 不足した台詞の仮の長さ[ms/文字]。
    #[serde(default = "d_placeholder_ms_per_char")]
    pub placeholder_ms_per_char: u32,
    /// `.flac` の収録ファイルを許可するか。
    #[serde(default)]
    pub allow_flac: bool,
}

fn d_trim_threshold_db() -> f64 {
    -50.0
}
fn d_trim_head_margin_ms() -> u32 {
    30
}
fn d_trim_tail_margin_ms() -> u32 {
    80
}
fn d_placeholder_ms_per_char() -> u32 {
    200
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            trim_threshold_db: d_trim_threshold_db(),
            trim_head_margin_ms: d_trim_head_margin_ms(),
            trim_tail_margin_ms: d_trim_tail_margin_ms(),
            placeholder_ms_per_char: d_placeholder_ms_per_char(),
            allow_flac: false,
        }
    }
}

/// ラウドネス正規化の設定(設計書 5.10 E)。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NormalizeConfig {
    /// 台詞の目標ラウドネス[LUFS](-40〜-10)。
    #[serde(default = "d_target_lufs")]
    pub target_lufs: f64,
    /// ゲインの上限[dB](0〜48)。
    #[serde(default = "d_gain_limit_db")]
    pub max_boost_db: f64,
    /// ゲインの下限の絶対値[dB](0〜48)。
    #[serde(default = "d_gain_limit_db")]
    pub max_cut_db: f64,
    /// フェード前の音声を `work/lines_prefade/` にも書く(①a 完了条件の検証用)。
    #[serde(default)]
    pub dump_prefade: bool,
}

fn d_target_lufs() -> f64 {
    -23.0
}
fn d_gain_limit_db() -> f64 {
    24.0
}

impl Default for NormalizeConfig {
    fn default() -> Self {
        Self {
            target_lufs: d_target_lufs(),
            max_boost_db: d_gain_limit_db(),
            max_cut_db: d_gain_limit_db(),
            dump_prefade: false,
        }
    }
}

/// ミックスの設定。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MixConfig {
    /// マスターのソフトリミッタの上限[dBFS](-6〜0)。
    #[serde(default = "d_master_ceiling_db")]
    pub master_ceiling_db: f64,
}

fn d_master_ceiling_db() -> f64 {
    -1.0
}

impl Default for MixConfig {
    fn default() -> Self {
        Self {
            master_ceiling_db: d_master_ceiling_db(),
        }
    }
}

/// 画像・動画素材を画面に収める方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// 全体が収まるように縮める(余白は背景色)。
    Contain,
    /// 画面を覆うように拡大して切り取る。
    Cover,
}

/// 動画の描画・合成の設定(設計書 4.7)。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RenderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub preset: String,
    pub audio_bitrate: String,
    pub fit: Fit,
    pub background: String,
    pub default_image: Option<String>,
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    pub jobs: u32,
    pub font_path: String,
    pub font_index: u32,
    pub font_size: f32,
    pub line_spacing: f32,
    pub bubble_fill: String,
    pub bubble_stroke: String,
    pub bubble_stroke_width: f32,
    pub bubble_padding: u32,
    pub note_fill: String,
    pub note_stroke: String,
    pub note_stroke_width: f32,
    pub note_padding: u32,
    pub text_color: String,
    pub margin: u32,
    pub bubble_gap: u32,
    pub tail_length: u32,
    pub bubble_max_height_ratio: f32,
    pub note_max_height_ratio: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 20,
            preset: "medium".into(),
            audio_bitrate: "192k".into(),
            fit: Fit::Contain,
            background: "#000000".into(),
            default_image: None,
            ffmpeg: None,
            ffprobe: None,
            jobs: 1,
            font_path: "C:\\Windows\\Fonts\\YuGothM.ttc".into(),
            font_index: 0,
            font_size: 44.0,
            line_spacing: 0.4,
            bubble_fill: "#FFFFFF".into(),
            bubble_stroke: "#000000".into(),
            bubble_stroke_width: 4.0,
            bubble_padding: 28,
            note_fill: "#FFFBE6".into(),
            note_stroke: "#000000".into(),
            note_stroke_width: 3.0,
            note_padding: 20,
            text_color: "#000000".into(),
            margin: 40,
            bubble_gap: 24,
            tail_length: 80,
            bubble_max_height_ratio: 0.55,
            note_max_height_ratio: 0.6,
        }
    }
}

impl RenderConfig {
    /// `#RRGGBB`(16進、大文字小文字可)を RGB に読む。それ以外は None。
    pub fn rgb(s: &str) -> Option<[u8; 3]> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }

    fn validate_into(&self, e: &mut Vec<String>) {
        fn even_range(e: &mut Vec<String>, name: &str, v: u32, lo: u32, hi: u32) {
            if !(lo..=hi).contains(&v) || !v.is_multiple_of(2) {
                e.push(format!(
                    "render.{name} は {lo}〜{hi} の偶数にしてください: {v}"
                ));
            }
        }
        fn int_range(e: &mut Vec<String>, name: &str, v: u32, lo: u32, hi: u32) {
            if !(lo..=hi).contains(&v) {
                e.push(format!("render.{name} は {lo}〜{hi} にしてください: {v}"));
            }
        }
        fn f_range(e: &mut Vec<String>, name: &str, v: f32, lo: f32, hi: f32) {
            if !(v.is_finite() && (lo..=hi).contains(&v)) {
                e.push(format!("render.{name} は {lo}〜{hi} にしてください: {v}"));
            }
        }
        even_range(e, "width", self.width, 16, 7680);
        even_range(e, "height", self.height, 16, 4320);
        int_range(e, "fps", self.fps, 1, 120);
        int_range(e, "crf", self.crf, 0, 51);
        const PRESETS: [&str; 9] = [
            "ultrafast",
            "superfast",
            "veryfast",
            "faster",
            "fast",
            "medium",
            "slow",
            "slower",
            "veryslow",
        ];
        if !PRESETS.contains(&self.preset.as_str()) {
            e.push(format!(
                "render.preset は {} のどれかにしてください: {}",
                PRESETS.join(" "),
                self.preset
            ));
        }
        let br_ok = self.audio_bitrate.strip_suffix('k').is_some_and(|d| {
            (2..=4).contains(&d.len())
                && d.bytes().all(|b| b.is_ascii_digit())
                && !d.starts_with('0')
        });
        if !br_ok {
            e.push(format!(
                "render.audio_bitrate は 2〜4 桁の数字に k を付けた形(例 192k)にしてください: {}",
                self.audio_bitrate
            ));
        }
        int_range(e, "jobs", self.jobs, 1, 64);
        for (name, v) in [
            ("background", &self.background),
            ("bubble_fill", &self.bubble_fill),
            ("bubble_stroke", &self.bubble_stroke),
            ("note_fill", &self.note_fill),
            ("note_stroke", &self.note_stroke),
            ("text_color", &self.text_color),
        ] {
            if Self::rgb(v).is_none() {
                e.push(format!("render.{name} は #RRGGBB の形にしてください: {v}"));
            }
        }
        f_range(e, "font_size", self.font_size, 8.0, 400.0);
        f_range(e, "line_spacing", self.line_spacing, 0.0, 3.0);
        f_range(
            e,
            "bubble_stroke_width",
            self.bubble_stroke_width,
            0.0,
            50.0,
        );
        f_range(e, "note_stroke_width", self.note_stroke_width, 0.0, 50.0);
        int_range(e, "bubble_padding", self.bubble_padding, 0, 500);
        int_range(e, "note_padding", self.note_padding, 0, 500);
        int_range(e, "margin", self.margin, 0, 2000);
        int_range(e, "bubble_gap", self.bubble_gap, 0, 2000);
        int_range(e, "tail_length", self.tail_length, 0, 2000);
        for (name, v) in [
            ("bubble_max_height_ratio", self.bubble_max_height_ratio),
            ("note_max_height_ratio", self.note_max_height_ratio),
        ] {
            if !(v.is_finite() && v > 0.0 && v <= 1.0) {
                e.push(format!(
                    "render.{name} は 0 より大きく 1 以下にしてください: {v}"
                ));
            }
        }
        if self.font_path.is_empty() {
            e.push("render.font_path は空にできません".to_string());
        }
    }
}

/// 画像生成プロンプトの共通部分(計画 2c で使う)。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PromptsConfig {
    pub style: String,
    pub suffix: String,
}

impl Default for PromptsConfig {
    fn default() -> Self {
        Self {
            style: String::new(),
            suffix: "縦横比 16:9。画像の中に文字・吹き出しを入れない。".into(),
        }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Self::from_toml(&s)
    }

    /// リポジトリ同梱の既定設定(`config.toml`)。
    pub fn builtin() -> Self {
        Self::from_toml(include_str!("../../../config.toml"))
            .expect("同梱の config.toml が不正です")
    }

    /// 値の範囲を検査する(設計書 5.10 E)。範囲外の項目ごとに1つのメッセージを返す。
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        let n = &self.normalize;
        if !(-40.0..=-10.0).contains(&n.target_lufs) {
            e.push(format!(
                "[normalize] target_lufs は -40〜-10 の範囲で指定してください: {}",
                n.target_lufs
            ));
        }
        if !(0.0..=48.0).contains(&n.max_boost_db) {
            e.push(format!(
                "[normalize] max_boost_db は 0〜48 の範囲で指定してください: {}",
                n.max_boost_db
            ));
        }
        if !(0.0..=48.0).contains(&n.max_cut_db) {
            e.push(format!(
                "[normalize] max_cut_db は 0〜48 の範囲で指定してください: {}",
                n.max_cut_db
            ));
        }
        if !(-6.0..=0.0).contains(&self.mix.master_ceiling_db) {
            e.push(format!(
                "[mix] master_ceiling_db は -6〜0 の範囲で指定してください: {}",
                self.mix.master_ceiling_db
            ));
        }
        if self.audio.sample_rate != 48_000 {
            e.push(format!(
                "[audio] sample_rate は 48000 固定です: {}",
                self.audio.sample_rate
            ));
        }
        let a = &self.audio;
        for (name, v) in [
            ("reference_dist", a.reference_dist),
            ("sound_speed", a.sound_speed),
            ("microphone_spacing", a.microphone_spacing),
        ] {
            if !(v.is_finite() && v > 0.0) {
                e.push(format!(
                    "[audio] {name} は 0 より大きい有限の値で指定してください: {v}"
                ));
            }
        }
        if a.microphone_spacing.is_finite() && a.microphone_spacing > 1.0 {
            e.push(format!(
                "[audio] microphone_spacing は 1.0 m 以下で指定してください: {}",
                a.microphone_spacing
            ));
        }
        self.render.validate_into(&mut e);
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[audio]
sample_rate = 48000
microphone_spacing = 0.2
sound_speed = 340.0
air_absorption_coeff = 0.05
room_size = 0.1
reverb_wet = 1.0
reference_dist = 1.0
mic_directivity = 0.2
mic_angle = 30.0

[bgm]
crossfade_s = 3.0
se_fade_out_s = 0.05
"#;

    #[test]
    fn builtin_config_loads_and_is_valid() {
        let c = Config::builtin();
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        assert_eq!(c.audio.sample_rate, 48_000);
    }

    #[test]
    fn omitted_sections_use_defaults() {
        let c = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(c.voice.trim_threshold_db, -50.0);
        assert_eq!(c.voice.trim_head_margin_ms, 30);
        assert_eq!(c.voice.trim_tail_margin_ms, 80);
        assert_eq!(c.voice.placeholder_ms_per_char, 200);
        assert!(!c.voice.allow_flac);
        assert_eq!(c.normalize.target_lufs, -23.0);
        assert_eq!(c.normalize.max_boost_db, 24.0);
        assert_eq!(c.normalize.max_cut_db, 24.0);
        assert!(!c.normalize.dump_prefade);
        assert_eq!(c.mix.master_ceiling_db, -1.0);
        assert!(c.validate().is_empty());
    }

    #[test]
    fn validate_reports_each_out_of_range_value() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.normalize.target_lufs = -5.0;
        c.normalize.max_boost_db = 60.0;
        c.normalize.max_cut_db = -1.0;
        c.mix.master_ceiling_db = 0.5;
        let errors = c.validate();
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors[0].contains("target_lufs"));
    }

    #[test]
    fn validate_rejects_non_48k_sample_rate() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.audio.sample_rate = 44_100;
        let errors = c.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("48000"));
    }

    #[test]
    fn boundary_values_are_accepted() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.normalize.target_lufs = -40.0;
        c.normalize.max_boost_db = 48.0;
        c.normalize.max_cut_db = 0.0;
        c.mix.master_ceiling_db = -6.0;
        assert!(c.validate().is_empty());
    }

    #[test]
    fn validate_rejects_non_positive_or_non_finite_geometry_values() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut c = Config::from_toml(MINIMAL).unwrap();
            c.audio.reference_dist = bad;
            let e = c.validate();
            assert_eq!(e.len(), 1, "{bad}: {e:?}");
            assert!(e[0].contains("reference_dist"));

            let mut c = Config::from_toml(MINIMAL).unwrap();
            c.audio.sound_speed = bad;
            let e = c.validate();
            assert_eq!(e.len(), 1, "{bad}: {e:?}");
            assert!(e[0].contains("sound_speed"));

            let mut c = Config::from_toml(MINIMAL).unwrap();
            c.audio.microphone_spacing = bad;
            let e = c.validate();
            assert_eq!(e.len(), 1, "{bad}: {e:?}");
            assert!(e[0].contains("microphone_spacing"));
        }
    }

    #[test]
    fn validate_rejects_microphone_spacing_above_one_metre() {
        let mut c = Config::from_toml(MINIMAL).unwrap();
        c.audio.microphone_spacing = 1.5;
        let e = c.validate();
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("microphone_spacing"));
        c.audio.microphone_spacing = 1.0;
        assert!(c.validate().is_empty());
    }

    #[test]
    fn render_defaults_match_the_spec() {
        let c = Config::builtin();
        let r = &c.render;
        assert_eq!((r.width, r.height, r.fps, r.crf), (1920, 1080, 30, 20));
        assert_eq!(r.preset, "medium");
        assert_eq!(r.fit, Fit::Contain);
        assert_eq!(r.jobs, 1);
        assert_eq!(r.font_path, "C:\\Windows\\Fonts\\YuGothM.ttc");
        assert_eq!(r.font_size, 44.0);
        assert_eq!((r.margin, r.bubble_gap, r.tail_length), (40, 24, 80));
        assert_eq!(
            c.prompts.suffix,
            "縦横比 16:9。画像の中に文字・吹き出しを入れない。"
        );
        assert!(c.validate().is_empty(), "{:?}", c.validate());
    }

    #[test]
    fn render_section_may_be_omitted() {
        let mut text = include_str!("../../../config.toml").to_string();
        let cut = text.find("[render]").unwrap();
        text.truncate(cut);
        let c = Config::from_toml(&text).unwrap();
        assert_eq!(c.render.width, 1920);
    }

    #[test]
    fn render_values_are_validated() {
        let mut c = Config::builtin();
        c.render.width = 1921;
        c.render.fps = 0;
        c.render.preset = "turbo".into();
        c.render.background = "black".into();
        c.render.jobs = 0;
        c.render.bubble_max_height_ratio = 0.0;
        c.render.audio_bitrate = "192".into();
        let errs = c.validate();
        for key in [
            "render.width",
            "render.fps",
            "render.preset",
            "render.background",
            "render.jobs",
            "render.bubble_max_height_ratio",
            "render.audio_bitrate",
        ] {
            assert!(errs.iter().any(|e| e.contains(key)), "{key}: {errs:?}");
        }
    }

    #[test]
    fn rgb_parses_hex_colors() {
        assert_eq!(RenderConfig::rgb("#FFfbE6"), Some([0xFF, 0xFB, 0xE6]));
        assert_eq!(RenderConfig::rgb("FFFBE6"), None);
        assert_eq!(RenderConfig::rgb("#FFFBE"), None);
        assert_eq!(RenderConfig::rgb("#GGGGGG"), None);
    }
}
