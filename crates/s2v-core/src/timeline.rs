use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::PauseConfig;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Audio,
    BgmStart,
    BgmStop,
    Se,
    Paragraph,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimelineEvent {
    pub event_type: EventType,
    pub start_ms: f64,
    pub duration_ms: f64,
    pub path: Option<PathBuf>,
    pub text: Option<String>,
    pub display_text: Option<String>,
    pub cast: Option<String>,
    /// 段落名（`#paragraph 名称`）。段落イベント以外は None。
    #[serde(default)]
    pub name: Option<String>,
}

pub struct TimelineProcessor {
    pub current_ms: f64,
    events: Vec<TimelineEvent>,
    sentence_pause_ms: f64,
    #[allow(dead_code)]
    pub cast_pause_ms: f64,
    paragraph_pause_ms: f64,
}

impl TimelineProcessor {
    pub fn new(pause_config: &PauseConfig) -> Self {
        Self {
            current_ms: 0.0,
            events: Vec::new(),
            sentence_pause_ms: pause_config.sentence_ms,
            cast_pause_ms: pause_config.cast_ms,
            paragraph_pause_ms: pause_config.paragraph_ms,
        }
    }

    pub fn register_audio(
        &mut self,
        path: PathBuf,
        duration_ms: f64,
        start_ms: f64,
        text: String,
        display_text: String,
        cast_name: String,
    ) {
        self.events.push(TimelineEvent {
            event_type: EventType::Audio,
            start_ms,
            duration_ms,
            path: Some(path),
            text: Some(text),
            display_text: Some(display_text),
            cast: Some(cast_name),
            name: None,
        });
    }

    pub fn advance_after_speech(&mut self, duration_ms: f64, pause_ms: Option<f64>) {
        let p = pause_ms.unwrap_or(self.sentence_pause_ms);
        self.current_ms += duration_ms + p;
    }

    pub fn advance_after_parallel(
        &mut self,
        anchor_ms: f64,
        max_occupied_ms: f64,
        pause_ms: Option<f64>,
    ) {
        let p = pause_ms.unwrap_or(self.sentence_pause_ms);
        self.current_ms = anchor_ms + max_occupied_ms + p;
    }

    pub fn advance_pause(&mut self, duration_ms: f64) {
        self.current_ms += duration_ms;
    }

    pub fn advance_paragraph(&mut self) {
        self.current_ms += self.paragraph_pause_ms;
    }

    pub fn register_bgm(&mut self, path: PathBuf) {
        self.events.push(TimelineEvent {
            event_type: EventType::BgmStart,
            start_ms: self.current_ms,
            duration_ms: 0.0,
            path: Some(path),
            text: None,
            display_text: None,
            cast: None,
            name: None,
        });
    }

    pub fn register_bgm_stop(&mut self) {
        self.events.push(TimelineEvent {
            event_type: EventType::BgmStop,
            start_ms: self.current_ms,
            duration_ms: 0.0,
            path: None,
            text: None,
            display_text: None,
            cast: None,
            name: None,
        });
    }

    pub fn register_se(&mut self, path: PathBuf) {
        self.events.push(TimelineEvent {
            event_type: EventType::Se,
            start_ms: self.current_ms,
            duration_ms: 0.0,
            path: Some(path),
            text: None,
            display_text: None,
            cast: None,
            name: None,
        });
    }

    pub fn register_paragraph(&mut self, name: Option<String>) {
        let display = match &name {
            Some(n) => format!("[PARAGRAPH {n}]"),
            None => "[PARAGRAPH]".to_string(),
        };
        self.events.push(TimelineEvent {
            event_type: EventType::Paragraph,
            start_ms: self.current_ms,
            duration_ms: 0.0,
            path: None,
            text: None,
            display_text: Some(display),
            cast: None,
            name,
        });
    }

    pub fn get_events(&self) -> &[TimelineEvent] {
        &self.events
    }

    pub fn into_events(self) -> Vec<TimelineEvent> {
        self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_pause() -> PauseConfig {
        PauseConfig {
            sentence_ms: 200.0,
            cast_ms: 500.0,
            paragraph_ms: 1000.0,
        }
    }

    #[test]
    fn starts_at_zero() {
        let tp = TimelineProcessor::new(&default_pause());
        assert!((tp.current_ms - 0.0).abs() < 1e-10);
        assert!(tp.get_events().is_empty());
    }

    #[test]
    fn advance_after_speech_uses_sentence_pause_by_default() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_after_speech(1000.0, None);
        assert!((tp.current_ms - 1200.0).abs() < 1e-10);
    }

    #[test]
    fn advance_after_speech_uses_provided_pause() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_after_speech(1000.0, Some(500.0));
        assert!((tp.current_ms - 1500.0).abs() < 1e-10);
    }

    #[test]
    fn advance_pause_adds_duration() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_after_speech(500.0, None);
        let before = tp.current_ms;
        tp.advance_pause(300.0);
        assert!((tp.current_ms - before - 300.0).abs() < 1e-10);
    }

    #[test]
    fn advance_paragraph_adds_paragraph_pause() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_paragraph();
        assert!((tp.current_ms - 1000.0).abs() < 1e-10);
    }

    #[test]
    fn advance_after_parallel_sets_anchor_plus_max() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_after_parallel(500.0, 1200.0, None);
        // 500 + 1200 + 200 (sentence_pause) = 1900
        assert!((tp.current_ms - 1900.0).abs() < 1e-10);
    }

    #[test]
    fn register_audio_adds_event() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.register_audio(
            PathBuf::from("a.wav"),
            1000.0,
            0.0,
            "テスト".to_string(),
            "テスト".to_string(),
            "キャラA".to_string(),
        );
        let events = tp.get_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::Audio);
        assert!((events[0].duration_ms - 1000.0).abs() < 1e-10);
        assert_eq!(events[0].cast.as_deref(), Some("キャラA"));
    }

    #[test]
    fn register_bgm_uses_current_time() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_pause(3000.0);
        tp.register_bgm(PathBuf::from("bgm.wav"));
        let e = &tp.get_events()[0];
        assert_eq!(e.event_type, EventType::BgmStart);
        assert!((e.start_ms - 3000.0).abs() < 1e-10);
    }

    #[test]
    fn register_paragraph_uses_current_time_and_zero_duration() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_after_speech(1000.0, None);
        let before = tp.current_ms;
        tp.register_paragraph(None);
        let events = tp.get_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::Paragraph);
        assert!((events[0].start_ms - before).abs() < 1e-10);
        assert!((events[0].duration_ms - 0.0).abs() < 1e-10);
        assert_eq!(events[0].display_text.as_deref(), Some("[PARAGRAPH]"));
        // current_ms は変化しない (advance は呼び出し側が別途行う)
        assert!((tp.current_ms - before).abs() < 1e-10);
    }

    #[test]
    fn register_paragraph_with_name_writes_named_marker() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.current_ms = 1500.0;
        tp.register_paragraph(Some("オープニング".to_string()));
        let events = tp.get_events();
        assert_eq!(
            events[0].display_text.as_deref(),
            Some("[PARAGRAPH オープニング]")
        );
        assert_eq!(events[0].name.as_deref(), Some("オープニング"));
    }

    #[test]
    fn register_bgm_stop_uses_current_time() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_pause(5000.0);
        tp.register_bgm_stop();
        let e = &tp.get_events()[0];
        assert_eq!(e.event_type, EventType::BgmStop);
        assert!((e.start_ms - 5000.0).abs() < 1e-10);
    }

    #[test]
    fn register_se_uses_current_time() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.advance_pause(1500.0);
        tp.register_se(PathBuf::from("se.wav"));
        let e = &tp.get_events()[0];
        assert_eq!(e.event_type, EventType::Se);
        assert!((e.start_ms - 1500.0).abs() < 1e-10);
    }

    #[test]
    fn into_events_transfers_ownership() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.register_se(PathBuf::from("se.wav"));
        let events = tp.into_events();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn sequential_audio_registration_order_preserved() {
        let mut tp = TimelineProcessor::new(&default_pause());
        tp.register_audio(
            PathBuf::from("a.wav"),
            500.0,
            0.0,
            "A".to_string(),
            "A".to_string(),
            "役A".to_string(),
        );
        tp.register_audio(
            PathBuf::from("b.wav"),
            800.0,
            700.0,
            "B".to_string(),
            "B".to_string(),
            "役B".to_string(),
        );
        let events = tp.get_events();
        assert_eq!(events.len(), 2);
        assert!((events[1].start_ms - 700.0).abs() < 1e-10);
    }
}
