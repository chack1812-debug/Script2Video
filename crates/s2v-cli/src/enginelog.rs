//! `s2v-engines` が `tracing` で出す記録(エンジンの自動検出・起動・停止など)を集めて `Process.log` へ渡す。
//!
//! 購読者はこのスレッドだけに設定する(`with_default`)。エンジンの呼び出しは現在のスレッドの
//! `block_on` の中で動くので、その間の記録を取りこぼさない。グローバルには何も設定しないので、
//! 並行して動く別の `build`(テスト)の記録と混ざらない。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// 集める記録の出どころ。他のクレート(hyper など)の記録は入れない。
const TARGET_PREFIX: &str = "s2v_engines";

/// 集めた記録 1 件(レベルと本文)。
pub type Entry = (Level, String);

struct Collector {
    entries: Arc<Mutex<Vec<Entry>>>,
    next_span: AtomicU64,
}

#[derive(Default)]
struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for Collector {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // INFO 以上(WARN・ERROR を含む)だけを集める
        *metadata.level() <= Level::INFO && metadata.target().starts_with(TARGET_PREFIX)
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut v = MessageVisitor::default();
        event.record(&mut v);
        if let Ok(mut entries) = self.entries.lock() {
            entries.push((*event.metadata().level(), v.0));
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// `f` の実行中に `s2v-engines` が出した記録を集めて、`f` の結果と一緒に返す。
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<Entry>) {
    let entries = Arc::new(Mutex::new(Vec::new()));
    let collector = Collector {
        entries: Arc::clone(&entries),
        next_span: AtomicU64::new(0),
    };
    let result = tracing::subscriber::with_default(collector, f);
    let collected = entries.lock().map(|e| e.clone()).unwrap_or_default();
    (result, collected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_info_and_above_from_engines_only() {
        let (value, entries) = capture(|| {
            tracing::info!(target: "s2v_engines::discover", "見つけた: {}", "C:/x/run.exe");
            tracing::warn!(target: "s2v_engines::lock", "ロックできません");
            tracing::error!(target: "s2v_engines::http_engine", "接続できません");
            tracing::debug!(target: "s2v_engines::discover", "細かい記録");
            tracing::info!(target: "hyper::client", "他のクレートの記録");
            7
        });
        assert_eq!(value, 7);
        assert_eq!(
            entries,
            vec![
                (Level::INFO, "見つけた: C:/x/run.exe".to_string()),
                (Level::WARN, "ロックできません".to_string()),
                (Level::ERROR, "接続できません".to_string()),
            ]
        );
    }

    #[test]
    fn nothing_outside_capture_is_collected() {
        let (_, first) = capture(|| tracing::info!(target: "s2v_engines::x", "中"));
        tracing::info!(target: "s2v_engines::x", "外");
        let (_, second) = capture(|| ());
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
    }
}
