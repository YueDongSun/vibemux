//! Where the Rust side sends its content-free events.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter};

use crate::ui_contract::UiEvent;

/// Receives the events the webview listens for.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: UiEvent);
}

/// Emits to the Tauri webview.
pub struct TauriEventSink(AppHandle);

impl TauriEventSink {
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self(app)
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: UiEvent) {
        let _ = self.0.emit(event.name(), event.payload());
    }
}

/// Keeps every emitted event in memory. Test support for the services.
#[derive(Debug, Default)]
pub struct RecordingEventSink {
    events: Mutex<Vec<UiEvent>>,
}

impl RecordingEventSink {
    /// The events emitted so far, oldest first.
    #[must_use]
    pub fn events(&self) -> Vec<UiEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl EventSink for RecordingEventSink {
    fn emit(&self, event: UiEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_sink_keeps_every_event() {
        let sink = RecordingEventSink::default();
        sink.emit(UiEvent::ProjectsChanged);
        sink.emit(UiEvent::ConsoleChanged {
            project_key: "abc".to_owned(),
        });
        assert_eq!(sink.events().len(), 2);
        assert_eq!(sink.events()[0], UiEvent::ProjectsChanged);
    }
}
