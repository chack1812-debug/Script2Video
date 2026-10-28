use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use std::collections::HashMap;

use serde_json::Value;

/// 1回の合成要求。ロール/キャストの定義ではなく、エンジンへ渡す値だけを持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct SynthRequest {
    pub speaker_name: String,
    /// 既定 "ノーマル" は呼び出し側で補う。
    pub style: String,
    /// 既定 "ja"。
    pub language: String,
    /// エンジン固有パラメータ。
    pub params: HashMap<String, Value>,
}

#[async_trait]
pub trait Engine: Send + Sync {
    async fn activate(&self) -> anyhow::Result<()>;
    async fn synthesize(&self, text: &str, req: &SynthRequest, output: &Path)
        -> anyhow::Result<()>;
    fn is_request_valid(&self, _req: &SynthRequest) -> bool {
        true
    }
    /// activate() でプロセスを起動した場合、それを停止する。既定では何もしない。
    fn terminate(&self) {}
}

pub struct EngineManager {
    engines: std::collections::HashMap<String, Arc<dyn Engine>>,
}

impl EngineManager {
    pub fn new() -> Self {
        Self {
            engines: std::collections::HashMap::new(),
        }
    }

    pub fn register(&mut self, name: impl Into<String>, engine: Arc<dyn Engine>) {
        self.engines.insert(name.into(), engine);
    }

    pub fn get(&self, engine_type: &str) -> Option<Arc<dyn Engine>> {
        self.engines.get(engine_type).cloned()
    }

    pub async fn activate_required(&self, types: &HashSet<String>) -> anyhow::Result<()> {
        let engines: Vec<Arc<dyn Engine>> = types
            .iter()
            .filter_map(|name| self.engines.get(name.as_str()).cloned())
            .collect();
        futures::future::try_join_all(engines.iter().map(|e| e.activate())).await?;
        Ok(())
    }

    pub async fn synthesize(
        &self,
        engine: &str,
        text: &str,
        req: &SynthRequest,
        out: &Path,
    ) -> anyhow::Result<()> {
        let e = self
            .engines
            .get(engine)
            .ok_or_else(|| anyhow::anyhow!("engine '{engine}' not registered"))?;
        e.synthesize(text, req, out).await
    }

    /// 登録済みの全エンジンを停止する（activate() でプロセスを起動した場合のみ実際に停止する）。
    pub fn shutdown_all(&self) {
        for (name, engine) in &self.engines {
            tracing::info!("[{name}] エンジンを停止します。");
            engine.terminate();
        }
    }
}

impl Default for EngineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StubEngine {
        activate_count: Arc<AtomicUsize>,
        synthesize_count: Arc<AtomicUsize>,
        terminate_count: Arc<AtomicUsize>,
        should_fail: bool,
        delay_ms: u64,
    }

    impl StubEngine {
        fn new() -> Self {
            Self {
                activate_count: Arc::new(AtomicUsize::new(0)),
                synthesize_count: Arc::new(AtomicUsize::new(0)),
                terminate_count: Arc::new(AtomicUsize::new(0)),
                should_fail: false,
                delay_ms: 0,
            }
        }

        fn failing() -> Self {
            Self {
                should_fail: true,
                ..Self::new()
            }
        }

        fn with_delay(ms: u64) -> Self {
            Self {
                delay_ms: ms,
                ..Self::new()
            }
        }
    }

    #[async_trait]
    impl Engine for StubEngine {
        async fn activate(&self) -> anyhow::Result<()> {
            self.activate_count.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            if self.should_fail {
                anyhow::bail!("engine activation failed");
            }
            Ok(())
        }

        async fn synthesize(
            &self,
            _text: &str,
            _req: &SynthRequest,
            _output: &Path,
        ) -> anyhow::Result<()> {
            self.synthesize_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn terminate(&self) {
            self.terminate_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn dummy_request() -> SynthRequest {
        SynthRequest {
            speaker_name: "ずんだもん".to_string(),
            style: "ノーマル".to_string(),
            language: "ja".to_string(),
            params: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn get_returns_none_for_unregistered_engine() {
        let mgr = EngineManager::new();
        assert!(mgr.get("voicevox").is_none());
    }

    #[tokio::test]
    async fn register_and_get_engine() {
        let mut mgr = EngineManager::new();
        mgr.register("voicevox", Arc::new(StubEngine::new()));
        assert!(mgr.get("voicevox").is_some());
    }

    #[tokio::test]
    async fn activate_required_calls_activate_for_matching_engines() {
        let stub = Arc::new(StubEngine::new());
        let count = Arc::clone(&stub.activate_count);
        let mut mgr = EngineManager::new();
        mgr.register("voicevox", stub);

        let types: HashSet<String> = ["voicevox".to_string()].into();
        mgr.activate_required(&types).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn activate_required_skips_unregistered_engine() {
        let mgr = EngineManager::new();
        let types: HashSet<String> = ["xtts".to_string()].into();
        // should not error even though engine isn't registered
        mgr.activate_required(&types).await.unwrap();
    }

    #[tokio::test]
    async fn activate_required_propagates_error() {
        let mut mgr = EngineManager::new();
        mgr.register("voicevox", Arc::new(StubEngine::failing()));
        let types: HashSet<String> = ["voicevox".to_string()].into();
        assert!(mgr.activate_required(&types).await.is_err());
    }

    #[tokio::test]
    async fn activate_required_runs_engines_concurrently() {
        let mut mgr = EngineManager::new();
        mgr.register("voicevox", Arc::new(StubEngine::with_delay(200)));
        mgr.register("aivis", Arc::new(StubEngine::with_delay(200)));

        let types: HashSet<String> = ["voicevox".to_string(), "aivis".to_string()].into();

        let start = std::time::Instant::now();
        mgr.activate_required(&types).await.unwrap();
        let elapsed = start.elapsed();

        // 逐次なら 400ms 以上かかる。並行なら ~200ms。余裕をみて 350ms 未満を要求する。
        assert!(
            elapsed < std::time::Duration::from_millis(350),
            "並行起動なら 350ms 未満で終わるはず。実測: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn synthesize_dispatches_to_correct_engine() {
        let stub = Arc::new(StubEngine::new());
        let count = Arc::clone(&stub.synthesize_count);
        let mut mgr = EngineManager::new();
        mgr.register("voicevox", stub);

        let req = dummy_request();
        mgr.synthesize("voicevox", "テスト", &req, Path::new("out.wav"))
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn synthesize_errors_for_unregistered_engine() {
        let mgr = EngineManager::new();
        let req = dummy_request();
        let result = mgr
            .synthesize("unknown", "テスト", &req, Path::new("out.wav"))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn default_is_request_valid_returns_true() {
        let stub = StubEngine::new();
        let req = dummy_request();
        assert!(stub.is_request_valid(&req));
    }

    #[test]
    fn default_terminate_is_noop() {
        struct NoopEngine;
        #[async_trait]
        impl Engine for NoopEngine {
            async fn activate(&self) -> anyhow::Result<()> {
                Ok(())
            }
            async fn synthesize(&self, _: &str, _: &SynthRequest, _: &Path) -> anyhow::Result<()> {
                Ok(())
            }
        }
        // デフォルト実装が呼べてパニックしないことを確認する
        NoopEngine.terminate();
    }

    #[test]
    fn shutdown_all_terminates_every_registered_engine() {
        let stub_a = Arc::new(StubEngine::new());
        let stub_b = Arc::new(StubEngine::new());
        let count_a = Arc::clone(&stub_a.terminate_count);
        let count_b = Arc::clone(&stub_b.terminate_count);

        let mut mgr = EngineManager::new();
        mgr.register("voicevox", stub_a);
        mgr.register("aivis", stub_b);

        mgr.shutdown_all();

        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }
}
