use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::SynthRequest;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::engine::Engine;
use crate::process::{
    engine_resource_key, ensure_running, terminate_process, EngineProcess, DEFAULT_STARTUP_TIMEOUT,
};

/// スピーカーキャッシュの型: speaker_name -> style_name -> style_id
type SpeakerCache = HashMap<String, HashMap<String, u32>>;

pub struct HttpEngine {
    name: String,
    url: String,
    client: Arc<Client>,
    cache: Arc<RwLock<SpeakerCache>>,
    exe_path: Option<String>,
    args: Vec<String>,
    startup_timeout: Duration,
    process: Mutex<Option<EngineProcess>>,
    /// 直近の activate() の失敗理由。合成エラーに添えて run.log に残すために保持する。
    activate_error: Mutex<Option<String>>,
}

impl HttpEngine {
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
            cache: Arc::new(RwLock::new(HashMap::new())),
            exe_path,
            args: Vec::new(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            process: Mutex::new(None),
            activate_error: Mutex::new(None),
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
            self.client.get(format!("{}/version", self.url)).send().await,
            Ok(res) if res.status().is_success()
        )
    }

    async fn refresh_cache(&self) -> anyhow::Result<()> {
        let res = self
            .client
            .get(format!("{}/speakers", self.url))
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!("{}: /speakers returned {}", self.name, res.status());
        }
        let speakers: Vec<Value> = res.json().await?;
        let mut cache = self.cache.write().await;
        cache.clear();
        for s in &speakers {
            let speaker_name = s["name"].as_str().unwrap_or("").to_string();
            let styles: HashMap<String, u32> = s["styles"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|st| {
                    let name = st["name"].as_str()?.to_string();
                    let id = st["id"].as_u64()? as u32;
                    Some((name, id))
                })
                .collect();
            cache.insert(speaker_name, styles);
        }
        info!(
            "[{}] スピーカーキャッシュを更新しました ({} 話者)",
            self.name,
            cache.len()
        );
        Ok(())
    }

    async fn resolve_style_id(&self, req: &SynthRequest) -> anyhow::Result<u32> {
        let cache = self.cache.read().await;
        if cache.is_empty() {
            let reason = self.activate_error.lock().unwrap().clone();
            return Err(match reason {
                Some(reason) => anyhow::anyhow!(
                    "{}: エンジンの起動に失敗したため話者 '{}' を解決できません: {reason}",
                    self.name,
                    req.speaker_name
                ),
                None => anyhow::anyhow!(
                    "{}: 話者一覧を取得できていません（エンジンが起動していない可能性があります）。話者 '{}' を解決できません",
                    self.name,
                    req.speaker_name
                ),
            });
        }
        let styles = cache.get(&req.speaker_name).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: 話者 '{}' がエンジンに見つかりません",
                self.name,
                req.speaker_name
            )
        })?;
        let target = req.style.as_str();
        if let Some(&id) = styles.get(target) {
            return Ok(id);
        }
        // fallback to first style
        if let Some((id, name)) = styles.iter().map(|(n, &id)| (id, n)).next() {
            warn!(
                "[{}] スタイル '{}' 不明のため '{}' を使用します",
                self.name, target, name
            );
            return Ok(id);
        }
        anyhow::bail!(
            "{}: 話者 '{}' のスタイル解決失敗（スタイルがありません）",
            self.name,
            req.speaker_name
        )
    }

    async fn activate_inner(&self) -> anyhow::Result<()> {
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
        info!("[{}] 接続確認 OK", self.name);
        self.refresh_cache().await
    }
}

#[async_trait]
impl Engine for HttpEngine {
    async fn activate(&self) -> anyhow::Result<()> {
        let result = self.activate_inner().await;
        *self.activate_error.lock().unwrap() = result.as_ref().err().map(|e| format!("{e:#}"));
        result
    }

    fn terminate(&self) {
        terminate_process(&self.name, &self.process);
    }

    fn is_request_valid(&self, req: &SynthRequest) -> bool {
        // キャッシュはブロッキングで読める場合のみ検証（非同期コンテキスト外）
        if let Ok(cache) = self.cache.try_read() {
            if !cache.contains_key(&req.speaker_name) {
                error!(
                    "[{}] 話者 '{}' が見つかりません",
                    self.name, req.speaker_name
                );
                return false;
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
        let style_id = self.resolve_style_id(req).await?;

        // audio_query
        let q_res = self
            .client
            .post(format!("{}/audio_query", self.url))
            .query(&[("text", text), ("speaker", &style_id.to_string())])
            .send()
            .await?;
        if !q_res.status().is_success() {
            anyhow::bail!("{}: audio_query 失敗: {}", self.name, q_res.status());
        }
        let mut query: Value = q_res.json().await?;

        // cast params をクエリにマージ (query にあるキーのみ)
        if let Value::Object(ref mut map) = query {
            for (k, v) in &req.params {
                if map.contains_key(k) {
                    if let Some(f) = v.as_f64() {
                        map.insert(k.clone(), json!(f));
                    }
                }
            }
        }

        // synthesis
        let s_res = self
            .client
            .post(format!("{}/synthesis", self.url))
            .query(&[("speaker", style_id.to_string())])
            .json(&query)
            .send()
            .await?;
        if !s_res.status().is_success() {
            anyhow::bail!("{}: synthesis 失敗: {}", self.name, s_res.status());
        }
        let bytes = s_res.bytes().await?;
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
    use serde_json::Value;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_engine(url: &str) -> HttpEngine {
        HttpEngine::new("test", url, Arc::new(Client::new()))
    }

    fn dummy_request() -> SynthRequest {
        SynthRequest {
            speaker_name: "ずんだもん".to_string(),
            style: "ノーマル".to_string(),
            language: "ja".to_string(),
            params: HashMap::new(),
        }
    }

    fn speakers_response() -> Value {
        serde_json::json!([
            {
                "name": "ずんだもん",
                "styles": [
                    {"name": "ノーマル", "id": 3},
                    {"name": "あまあま", "id": 1}
                ]
            }
        ])
    }

    #[tokio::test]
    async fn activate_succeeds_when_server_up() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
    }

    #[tokio::test]
    async fn activate_fails_when_server_down() {
        let engine = make_engine("http://127.0.0.1:1");
        assert!(engine.activate().await.is_err());
    }

    #[tokio::test]
    async fn activate_populates_speaker_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        let cache = engine.cache.read().await;
        assert!(cache.contains_key("ずんだもん"));
        assert_eq!(cache["ずんだもん"]["ノーマル"], 3);
    }

    /// `SynthRequest.style` で指定したスタイルの ID(あまあま = 1)で audio_query / synthesis が呼ばれること。
    #[tokio::test]
    async fn style_comes_from_request() {
        let server = MockServer::start().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out_path = tmp.path().to_path_buf();

        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio_query"))
            .and(wiremock::matchers::query_param("speaker", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"speedScale": 1.0})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/synthesis"))
            .and(wiremock::matchers::query_param("speaker", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF....".to_vec()))
            .expect(1)
            .mount(&server)
            .await;

        let mut req = dummy_request();
        req.style = "あまあま".to_string();
        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        engine
            .synthesize("こんにちは", &req, &out_path)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn synthesize_calls_audio_query_then_synthesis() {
        let server = MockServer::start().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out_path = tmp.path().to_path_buf();

        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio_query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"speedScale": 1.0})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/synthesis"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF....".to_vec()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        engine
            .synthesize("こんにちは", &dummy_request(), &out_path)
            .await
            .unwrap();

        assert!(out_path.exists());
        assert!(std::fs::metadata(&out_path).unwrap().len() > 0);
    }

    #[tokio::test]
    async fn synthesize_fails_when_audio_query_returns_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio_query"))
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

    /// 起動に失敗したエンジンで合成すると、話者の問題ではなく起動失敗とその理由を報告する
    /// （起動時の警告はコンソールにしか出ないため、合成エラー経由で run.log に残す）。
    #[tokio::test]
    async fn synthesize_after_failed_activate_reports_activation_error() {
        let engine = make_engine("http://127.0.0.1:1");
        assert!(engine.activate().await.is_err());

        let err = engine
            .synthesize("テスト", &dummy_request(), Path::new("/tmp/out.wav"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("起動"), "起動失敗であることを示すべき: {err}");
        assert!(
            err.contains("exe_path も未設定"),
            "起動失敗の理由を含むべき: {err}"
        );
    }

    /// 一度起動に失敗しても、次の activate() が成功したら古い失敗理由を報告しない。
    #[tokio::test]
    async fn successful_reactivate_clears_previous_activation_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        assert!(engine.activate().await.is_err());
        engine.activate().await.unwrap();

        let err = engine
            .synthesize("テスト", &dummy_request(), Path::new("/tmp/out.wav"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("起動に失敗"),
            "成功後に古い起動失敗理由を残してはならない: {err}"
        );
        assert!(err.contains("話者一覧を取得できていません"), "{err}");
    }

    #[tokio::test]
    async fn synthesize_without_speaker_list_mentions_engine_not_running() {
        let engine = make_engine("http://127.0.0.1:1");

        let err = engine
            .synthesize("テスト", &dummy_request(), Path::new("/tmp/out.wav"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("話者一覧を取得できていません"),
            "話者一覧が空であることを示すべき: {err}"
        );
    }

    #[tokio::test]
    async fn synthesize_with_unknown_speaker_reports_speaker_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0.14.0"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/speakers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(speakers_response()))
            .mount(&server)
            .await;

        let engine = make_engine(&server.uri());
        engine.activate().await.unwrap();
        let mut req = dummy_request();
        req.speaker_name = "存在しない話者".to_string();

        let err = engine
            .synthesize("テスト", &req, Path::new("/tmp/out.wav"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("話者 '存在しない話者' がエンジンに見つかりません"),
            "話者が見つからないことを示すべき: {err}"
        );
    }
}
