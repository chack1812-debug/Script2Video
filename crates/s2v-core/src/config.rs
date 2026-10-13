use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub voicevox: EngineUrl,
    pub aivis: EngineUrl,
    pub xtts: EngineUrl,
    pub audio: AudioConfig,
    pub concurrency: ConcurrencyConfig,
    pub bgm: BgmConfig,
    /// HTTPエンジン呼び出し(version/speakers/音声合成)のタイムアウト設定（省略可、既定値あり）。
    #[serde(default)]
    pub http: HttpConfig,
    /// エクスポート形式の設定（省略可、既定値あり）。
    #[serde(default)]
    pub export: ExportConfig,
}

/// エクスポート（FCPXML等）に関する設定。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ExportConfig {
    /// FCPXMLを29.97fps(NTSCドロップフレーム相当)で出力するか（既定 false = 30fps）。
    /// 字幕ズレ調査(Obsidian記録)により、SRT/WAVとの時間基準を合わせるには30fpsが正しいことが
    /// 確定している。29.97fps前提の案件でのみ true にする。
    #[serde(default)]
    pub fcpxml_2997fps: bool,
}

/// エンジンへのHTTPリクエストのタイムアウト設定。
/// 未設定の場合、エンジンが応答を返さないまま接続だけ握り続けると
/// 1台詞の処理が無期限に待機してしまう（バッチ全体も終了しない）ため、既定値を必ず持つ。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpConfig {
    #[serde(default = "default_connect_timeout_s")]
    pub connect_timeout_s: u64,
    #[serde(default = "default_request_timeout_s")]
    pub request_timeout_s: u64,
}

fn default_connect_timeout_s() -> u64 {
    10
}
fn default_request_timeout_s() -> u64 {
    180
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_s: default_connect_timeout_s(),
            request_timeout_s: default_request_timeout_s(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EngineUrl {
    pub url: String,
    /// 未起動時に自動起動する実行ファイルのパス（省略可）。
    /// 絶対パス（例: "C:/VOICEVOX/run.exe"）と、PATH上のコマンド名（例: "python"）の両方を指定できる。
    #[serde(default)]
    pub exe_path: Option<String>,
    /// `exe_path` に渡す引数（省略可、既定は空）。
    /// 例: XTTS を `python -m xtts_api_server --port 8020` で起動する場合、
    /// `exe_path = "python"`、`args = ["-m", "xtts_api_server", "--port", "8020"]` のように分離する。
    #[serde(default)]
    pub args: Vec<String>,
    /// 自動起動の待機秒数（省略時は既定 60 秒）。モデルロードが遅い場合に増やす。
    #[serde(default)]
    pub startup_timeout_s: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub microphone_spacing: f64,
    pub sound_speed: f64,
    pub air_absorption_coeff: f64,
    pub room_size: f64,
    pub reverb_wet: f64,
    pub reference_dist: f64,
    pub reference_gain_db: f64,
    pub max_gain_db: f64,
    pub mic_directivity: f64,
    pub mic_angle: f64,
    pub engine_volume_offsets: HashMap<String, f64>,
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
pub struct ConcurrencyConfig {
    pub voicevox: usize,
    pub aivis: usize,
    pub xtts: usize,
    pub audio_process: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BgmConfig {
    pub crossfade_s: f64,
    pub se_fade_out_s: f64,
}

impl Config {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Self::from_toml(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[voicevox]
url = "http://127.0.0.1:50021"

[aivis]
url = "http://127.0.0.1:10101"

[xtts]
url = "http://localhost:8020"

[audio]
sample_rate = 48000
microphone_spacing = 0.2
sound_speed = 340.0
air_absorption_coeff = 0.05
room_size = 0.1
reverb_wet = 0.7
reference_dist = 1.0
reference_gain_db = -5.0
max_gain_db = -1.0
mic_directivity = 0.5
mic_angle = 45.0

[audio.engine_volume_offsets]
voicevox = 1.2
aivis = 0.9
xtts = 1.0

[concurrency]
voicevox = 3
aivis = 3
xtts = 2
audio_process = 0

[bgm]
crossfade_s = 3.0
se_fade_out_s = 0.05
"#;

    #[test]
    fn parses_engine_urls() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg.voicevox.url, "http://127.0.0.1:50021");
        assert_eq!(cfg.aivis.url, "http://127.0.0.1:10101");
        assert_eq!(cfg.xtts.url, "http://localhost:8020");
    }

    #[test]
    fn exe_path_is_optional_and_defaults_to_none() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg.voicevox.exe_path, None);
    }

    #[test]
    fn parses_exe_path_when_present() {
        let toml_with_exe = SAMPLE_TOML.replacen(
            r#"url = "http://127.0.0.1:50021""#,
            "url = \"http://127.0.0.1:50021\"\nexe_path = \"C:\\\\VOICEVOX\\\\run.exe\"",
            1,
        );
        let cfg = Config::from_toml(&toml_with_exe).unwrap();
        assert_eq!(
            cfg.voicevox.exe_path.as_deref(),
            Some("C:\\VOICEVOX\\run.exe")
        );
    }

    #[test]
    fn args_is_optional_and_defaults_to_empty() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert!(cfg.voicevox.args.is_empty());
    }

    #[test]
    fn parses_args_when_present() {
        // XTTS のように exe_path を PATH 上のコマンド名にし、実引数を args に分離する形式。
        let toml_with_args = SAMPLE_TOML.replacen(
            r#"url = "http://localhost:8020""#,
            "url = \"http://localhost:8020\"\nexe_path = \"python\"\nargs = [\"-m\", \"xtts_api_server\", \"--port\", \"8020\"]",
            1,
        );
        let cfg = Config::from_toml(&toml_with_args).unwrap();
        assert_eq!(cfg.xtts.exe_path.as_deref(), Some("python"));
        assert_eq!(
            cfg.xtts.args,
            vec!["-m", "xtts_api_server", "--port", "8020"]
        );
    }

    #[test]
    fn http_config_is_optional_and_has_sane_defaults() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg.http.connect_timeout_s, 10);
        assert_eq!(cfg.http.request_timeout_s, 180);
    }

    #[test]
    fn http_config_can_be_overridden() {
        let toml_with_http =
            format!("{SAMPLE_TOML}\n[http]\nconnect_timeout_s = 3\nrequest_timeout_s = 20\n");
        let cfg = Config::from_toml(&toml_with_http).unwrap();
        assert_eq!(cfg.http.connect_timeout_s, 3);
        assert_eq!(cfg.http.request_timeout_s, 20);
    }

    #[test]
    fn export_config_defaults_to_30fps() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert!(!cfg.export.fcpxml_2997fps);
    }

    #[test]
    fn export_config_can_opt_into_2997fps() {
        let toml_with_export = format!("{SAMPLE_TOML}\n[export]\nfcpxml_2997fps = true\n");
        let cfg = Config::from_toml(&toml_with_export).unwrap();
        assert!(cfg.export.fcpxml_2997fps);
    }

    #[test]
    fn engine_startup_timeout_parses_and_defaults() {
        let toml_with_timeout = SAMPLE_TOML.replacen(
            r#"url = "http://127.0.0.1:50021""#,
            "url = \"http://127.0.0.1:50021\"\nstartup_timeout_s = 90",
            1,
        );
        let cfg = Config::from_toml(&toml_with_timeout).unwrap();
        assert_eq!(cfg.voicevox.startup_timeout_s, Some(90));

        let cfg2 = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg2.voicevox.startup_timeout_s, None);
    }

    #[test]
    fn parses_audio_config() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg.audio.sample_rate, 48000);
        assert!((cfg.audio.microphone_spacing - 0.2).abs() < 1e-10);
        assert!((cfg.audio.sound_speed - 340.0).abs() < 1e-10);
        assert!((cfg.audio.reverb_wet - 0.7).abs() < 1e-10);
        assert!((cfg.audio.reference_gain_db - (-5.0)).abs() < 1e-10);
        assert!((cfg.audio.max_gain_db - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn parses_engine_volume_offsets() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert!((cfg.audio.engine_volume_offsets["voicevox"] - 1.2).abs() < 1e-10);
        assert!((cfg.audio.engine_volume_offsets["aivis"] - 0.9).abs() < 1e-10);
        assert!((cfg.audio.engine_volume_offsets["xtts"] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn parses_concurrency_config() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(cfg.concurrency.voicevox, 3);
        assert_eq!(cfg.concurrency.aivis, 3);
        assert_eq!(cfg.concurrency.xtts, 2);
        assert_eq!(cfg.concurrency.audio_process, 0);
    }

    #[test]
    fn parses_bgm_config() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        assert!((cfg.bgm.crossfade_s - 3.0).abs() < 1e-10);
        assert!((cfg.bgm.se_fade_out_s - 0.05).abs() < 1e-10);
    }

    #[test]
    fn rejects_invalid_toml() {
        let result = Config::from_toml("this is not valid toml [[[");
        assert!(result.is_err());
    }

    #[test]
    fn early_reflections_defaults_when_section_absent() {
        let cfg = Config::from_toml(SAMPLE_TOML).unwrap();
        let er = &cfg.audio.early_reflections;
        assert!(er.enabled);
        assert!((er.ear_height - 1.2).abs() < 1e-10);
        assert_eq!(er.room_dims_min, [4.0, 5.0, 3.0]);
        assert_eq!(er.room_dims_max, [25.0, 45.0, 18.0]);
        assert!((er.front_wall.reflection_coeff - 0.85).abs() < 1e-10);
        assert!((er.back_wall.absorption_cutoff_hz - 4000.0).abs() < 1e-10);
        assert!((er.wet_distance_slope - 0.1).abs() < 1e-10);
    }

    #[test]
    fn early_reflections_partial_section_fills_missing_fields() {
        let toml = format!(
            "{SAMPLE_TOML}\n[audio.early_reflections]\nenabled = false\near_height = 1.7\n"
        );
        let cfg = Config::from_toml(&toml).unwrap();
        let er = &cfg.audio.early_reflections;
        assert!(!er.enabled);
        assert!((er.ear_height - 1.7).abs() < 1e-10);
        assert_eq!(er.room_dims_max, [25.0, 45.0, 18.0]);
        assert!((er.floor.reflection_coeff - 0.5).abs() < 1e-10);
    }

    #[test]
    fn parses_real_config_toml_with_early_reflections() {
        let s = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml"))
            .unwrap();
        let cfg = Config::from_toml(&s).unwrap();
        assert!(cfg.audio.early_reflections.enabled);
        assert!((cfg.audio.early_reflections.front_wall.reflection_coeff - 0.85).abs() < 1e-10);
    }

    #[test]
    fn real_config_reverb_wet_is_scaler_default() {
        let s = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml"))
            .unwrap();
        let cfg = Config::from_toml(&s).unwrap();
        assert!(
            (cfg.audio.reverb_wet - 1.0).abs() < 1e-10,
            "reverb_wet 既定はスケーラ 1.0"
        );
    }
}
