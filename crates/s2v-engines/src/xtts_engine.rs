use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::SynthRequest;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::{info, warn};

use crate::engine::Engine;
use crate::process::{
    engine_resource_key, ensure_running, terminate_process, EngineProcess, DEFAULT_STARTUP_TIMEOUT,
};

pub struct XttsEngine {
    name: String,
    url: String,
    client: Arc<Client>,
    speaker_cache: Arc<RwLock<HashSet<String>>>,
    exe_path: Option<String>,
    args: Vec<String>,
    startup_timeout: Duration,
    process: Mutex<Option<EngineProcess>>,
    /// XTTSはグローバルな(リクエスト単位ではない)設定APIしか持たないため、
    /// 「設定更新→合成」を1つの台詞ぶんずつ直列化して、並行合成時に
    /// 別の台詞の設定を使って合成してしまうのを防ぐ。
    synth_lock: AsyncMutex<()>,
}

impl XttsEngine {
    pub fn new(name: impl Into<String>, url: impl Into<String>, client: Arc<Client>) -> Self {
        Self::with_exe_path(name, url, client, None)
    }

    pub fn with_exe_path(
        name: impl Into<String>,
        url: impl Into<String>,
        client: Arc<Client>,
        exe_path: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            client,
            speaker_cache: Arc::new(RwLock::new(HashSet::new())),
            exe_path,
            args: Vec::new(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            process: Mutex::new(None),
            synth_lock: AsyncMutex::new(()),
        }
    }

    /// 自動起動コマンドの引数を設定する（省略時は空、`exe_path` を引数なしで起動）。
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// 自動起動の待機時間を設定する（省略時は [`DEFAULT_STARTUP_TIMEOUT`]）。
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    async fn is_alive(&self) -> bool {
        matches!(
            self.client.get(format!("{}/speakers", self.url)).send().await,
            Ok(res) if res.status().is_success()
        )
    }
}

#[async_trait]
impl Engine for XttsEngine {
    async fn activate(&self) -> anyhow::Result<()> {
        ensure_running(
            &self.name,
            &engine_resource_key(&self.name, &self.url),
            self.exe_path.as_deref(),
            &self.args,
            self.startup_timeout,
            &self.process,
            || self.is_alive(),
        )
        .await?;

        let res = self
            .client
            .get(format!("{}/speakers", self.url))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}: サーバーに接続できません: {}", self.name, e))?;

        if !res.status().is_success() {
            anyhow::bail!("{}: /speakers returned {}", self.name, res.status());
        }
        let speakers: Vec<Value> = res.json().await?;
        let mut cache = self.speaker_cache.write().await;
        cache.clear();
        for s in &speakers {
            if let Some(name) = s["name"].as_str() {
                cache.insert(name.to_string());
            }
        }
        info!("[{}] XTTS 起動確認 OK ({} 話者)", self.name, cache.len());
        Ok(())
    }

    fn terminate(&self) {
        terminate_process(&self.name, &self.process);
    }

    fn is_request_valid(&self, req: &SynthRequest) -> bool {
        if let Ok(cache) = self.speaker_cache.try_read() {
            if !cache.is_empty() && !cache.contains(&req.speaker_name) {
                warn!(
                    "[{}] 話者 '{}' がキャッシュに見つかりません",
                    self.name, req.speaker_name
                );
            }
        }
        true
    }

    async fn synthesize(
        &self,
        text: &str,
        req: &SynthRequest,
        output: &Path,
    ) -> anyhow::Result<()> {
        // XTTSのget_tts_settings/set_tts_settingsはリクエスト単位ではなくエンジン全体の
        // グローバル設定を書き換えるAPIのため、「設定更新→合成」を丸ごと直列化する。
        // ここを取らないと、並行合成中に他の台詞の設定を使って合成してしまう。
        let _guard = self.synth_lock.lock().await;

        // get_tts_settings → patch → set_tts_settings
        if let Ok(q_res) = self
            .client
            .post(format!("{}/get_tts_settings", self.url))
            .send()
            .await
        {
            if q_res.status().is_success() {
                if let Ok(mut query) = q_res.json::<Value>().await {
                    if let Value::Object(ref mut map) = query {
                        for (k, v) in &req.params {
                            if map.contains_key(k) {
                                map.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    let _ = self
                        .client
                        .post(format!("{}/set_tts_settings", self.url))
                        .json(&query)
                        .send()
                        .await;
                }
            }
        }

        let lang = req.language.as_str();
        let payload = json!({
            "text": text,
            "speaker_name": req.speaker_name,
            "language": lang,
        });

        let res = self
            .client
            .post(format!("{}/tts_to_audio/", self.url))
            .json(&payload)
            .send()
            .await?;

        if !res.status().is_success() {
            anyhow::bail!("{}: tts_to_audio 失敗: {}", self.name, res.status());
        }
        let bytes = res.bytes().await?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, &bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_engine(url: &str) -> XttsEngine {
        XttsEngine::new("xtts", url, Arc::new(Client::new()))
    }

    fn dummy_request() -> SynthRequest {
        SynthRequest {
            speaker_name: "en-us-1".to_string(),
            style: "ノーマル".to_string(),
            language: "ja".to_string(),
            params: std::collections::HashMap::new(),
        }
    }

    fn speakers_response() -> Value {
        serde_json::json!([{"name": "en-us-1"}, {"name": "ja-jp-1"}])
    }

    #[tokio::test]
    async fn activate_populates_speaker_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        let cache = engine.speaker_cache.read().await;
        assert!(cache.contains("en-us-1"));
        assert!(cache.contains("ja-jp-1"));
    }

    #[tokio::test]
    async fn activate_fails_when_server_down() {
        let engine = make_engine("http://127.0.0.1:1");
        assert!(engine.activate().await.is_err());
    }

    #[tokio::test]
    async fn synthesize_writes_audio_file() {
        let server = MockServer::start().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out_path = tmp.path().to_path_buf();

        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/get_tts_settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/set_tts_settings"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tts_to_audio/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF....".to_vec()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        engine
            .synthesize("Hello", &dummy_request(), &out_path)
            .await
            .unwrap();

        assert!(out_path.exists());
        assert!(std::fs::metadata(&out_path).unwrap().len() > 0);
    }

    /// `SynthRequest.language` が tts_to_audio の payload に入ること。
    #[tokio::test]
    async fn language_comes_from_request() {
        let server = MockServer::start().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out_path = tmp.path().to_path_buf();

        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/get_tts_settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/set_tts_settings"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tts_to_audio/"))
            .and(wiremock::matchers::body_partial_json(
                json!({"language": "en"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF....".to_vec()))
            .mount(&server)
            .await;

        let mut req = dummy_request();
        req.language = "en".to_string();

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        engine
            .synthesize("Hello", &req, &out_path)
            .await
            .expect("language=en が tts_to_audio に渡ること");
    }

    #[tokio::test]
    async fn synthesize_fails_on_tts_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/get_tts_settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/set_tts_settings"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tts_to_audio/"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        let result = engine
            .synthesize("テスト", &dummy_request(), Path::new("/tmp/out.wav"))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn is_request_valid_returns_true_always() {
        let engine = make_engine("http://localhost:8020");
        let req = dummy_request();
        assert!(engine.is_request_valid(&req));
    }

    fn request_with_speed(speed: f64) -> SynthRequest {
        let mut req = dummy_request();
        req.params.insert("speed".to_string(), json!(speed));
        req
    }

    /// `/get_tts_settings` は常にサーバー側の「現在の設定」をそのまま返す。
    struct GetSettingsResponder {
        state: Arc<std::sync::Mutex<Value>>,
    }
    impl wiremock::Respond for GetSettingsResponder {
        fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
            ResponseTemplate::new(200).set_body_json(self.state.lock().unwrap().clone())
        }
    }

    /// `/set_tts_settings` はリクエストボディでサーバー側の「現在の設定」を更新する。
    /// speed=1.0 (台詞A) のリクエストだけ意図的に応答を遅延させ、
    /// その間に台詞Bの get→set→tts_to_audio が丸ごと割り込めるようにする
    /// （実運用でも並行合成中に起こり得る割り込みを、決定的に再現するため）。
    struct SetSettingsResponder {
        state: Arc<std::sync::Mutex<Value>>,
    }
    impl wiremock::Respond for SetSettingsResponder {
        fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let is_speech_a = body.get("speed").and_then(|v| v.as_f64()) == Some(1.0);
            *self.state.lock().unwrap() = body;
            let resp = ResponseTemplate::new(200);
            if is_speech_a {
                resp.set_delay(std::time::Duration::from_millis(300))
            } else {
                resp
            }
        }
    }

    /// `/tts_to_audio/` は、応答時点でサーバーに反映されている `speed` をそのまま
    /// 音声ファイルの中身として返す。これにより、どちらの台詞の設定が実際に
    /// 合成に使われたかをテスト側で検証できる。
    struct TtsToAudioResponder {
        state: Arc<std::sync::Mutex<Value>>,
    }
    impl wiremock::Respond for TtsToAudioResponder {
        fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
            let speed = self
                .state
                .lock()
                .unwrap()
                .get("speed")
                .and_then(|v| v.as_f64())
                .unwrap();
            ResponseTemplate::new(200).set_body_bytes(speed.to_string().into_bytes())
        }
    }

    #[tokio::test]
    async fn concurrent_synthesize_does_not_leak_settings_between_speeches() {
        let server = MockServer::start().await;
        let state = Arc::new(std::sync::Mutex::new(json!({"speed": 0.0})));

        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/get_tts_settings"))
            .respond_with(GetSettingsResponder {
                state: state.clone(),
            })
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/set_tts_settings"))
            .respond_with(SetSettingsResponder {
                state: state.clone(),
            })
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tts_to_audio/"))
            .respond_with(TtsToAudioResponder {
                state: state.clone(),
            })
            .mount(&server)
            .await;

        let engine = Arc::new(make_engine(&server.uri()));
        let tmp = tempfile::tempdir().unwrap();
        let out_a = tmp.path().join("a.wav");
        let out_b = tmp.path().join("b.wav");

        let engine_a = Arc::clone(&engine);
        let out_a2 = out_a.clone();
        let task_a = tokio::spawn(async move {
            engine_a
                .synthesize("台詞A", &request_with_speed(1.0), &out_a2)
                .await
        });
        let engine_b = Arc::clone(&engine);
        let out_b2 = out_b.clone();
        let task_b = tokio::spawn(async move {
            engine_b
                .synthesize("台詞B", &request_with_speed(2.0), &out_b2)
                .await
        });

        task_a.await.unwrap().unwrap();
        task_b.await.unwrap().unwrap();

        let content_a = std::fs::read_to_string(&out_a).unwrap();
        let content_b = std::fs::read_to_string(&out_b).unwrap();
        assert_eq!(
            content_a, "1",
            "台詞Aの合成は台詞A自身のspeed設定を使うべき(Bに上書きされてはいけない)"
        );
        assert_eq!(
            content_b, "2",
            "台詞Bの合成は台詞B自身のspeed設定を使うべき"
        );
    }
}
