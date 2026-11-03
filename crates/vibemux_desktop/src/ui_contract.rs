//! The DTOs and events the Rust side sends to the webview.
//!
//! Events carry no vendor text: they name what changed, and the webview
//! fetches what it shows. `ask_reply` and explicit private `chat_request`
//! read variants return vendor content.

use serde::{Deserialize, Serialize};
use vibemux_harness::{
    AgentKind,
    dispatch::{DispatchPhase, NativeProtocol},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskHarness {
    Claude,
    Codex,
}

impl AskHarness {
    pub const fn agent(self) -> AgentKind {
        match self {
            Self::Claude => AgentKind::Claude,
            Self::Codex => AgentKind::Codex,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code: tool-free, one turn.",
            Self::Codex => "Codex: configured read-only, one turn.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Running,
    Stopped,
    Unreachable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectRow {
    pub project_key: String,
    pub name: String,
    pub daemon: DaemonState,
    pub code: Option<String>,
    pub missing: bool,
    pub active: bool,
    pub ask_confirmed: bool,
    pub chat_confirmed: bool,
    pub ask_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AskTarget {
    pub harness: AskHarness,
    pub label: String,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AskTargets {
    pub targets: Vec<AskTarget>,
    pub setup_offered: bool,
    pub code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AskReceipt {
    pub ask_id: String,
    pub request_id: String,
    pub submission_key: String,
    pub duplicate: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAskPhase {
    Submitting,
    Running,
    Cancelling,
    Finished,
    Uncertain,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverState {
    Observing,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputState {
    Pending,
    Partial,
    Complete,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalAsk {
    pub request_id: String,
    pub task_id: String,
    pub run_id: String,
    pub protocol: NativeProtocol,
    pub phase: DispatchPhase,
    pub sequence: Option<u64>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AskStatus {
    pub ask_id: String,
    pub project_key: String,
    pub harness: AskHarness,
    pub revision: u64,
    pub phase: LocalAskPhase,
    pub canonical: Option<CanonicalAsk>,
    pub terminal_observed: bool,
    pub observer_state: ObserverState,
    pub observer_error: Option<String>,
    pub output_state: OutputState,
    pub reply_available: bool,
    pub delivery_complete: bool,
    pub cancel_requested: bool,
}

#[derive(Clone, Serialize)]
pub struct AskReply {
    pub ask_id: String,
    pub revision: u64,
    pub prompt: String,
    pub reply: String,
    pub truncated: bool,
    pub delivery_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UiEvent {
    ProjectsChanged,
    SnapshotChanged {
        project_key: String,
        revision: u64,
    },
    AskChanged {
        ask_id: String,
        phase: LocalAskPhase,
        revision: u64,
    },
    ConsoleChanged {
        project_key: String,
    },
}

impl UiEvent {
    /// The event name the webview listens for.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ProjectsChanged => "projects_changed",
            Self::SnapshotChanged { .. } => "snapshot_changed",
            Self::AskChanged { .. } => "ask_changed",
            Self::ConsoleChanged { .. } => "console_changed",
        }
    }

    /// The event payload. It carries the `event` tag, so a payload names
    /// the event it belongs to.
    #[must_use]
    pub fn payload(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AboutInfo {
    pub app_version: String,
    pub webview_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_name_matches_the_tagged_payload() {
        let event = UiEvent::SnapshotChanged {
            project_key: "abc123".to_owned(),
            revision: 7,
        };
        assert_eq!(event.name(), "snapshot_changed");
        let payload = event.payload();
        assert_eq!(payload["event"], "snapshot_changed");
        assert_eq!(payload["project_key"], "abc123");
        assert_eq!(payload["revision"], 7);
    }

    #[test]
    fn an_event_carries_no_content_field() {
        let event = UiEvent::AskChanged {
            ask_id: "ask-1".to_owned(),
            phase: LocalAskPhase::Running,
            revision: 1,
        };
        let payload = event.payload();
        assert_eq!(payload["event"], "ask_changed");
        assert_eq!(payload["ask_id"], "ask-1");
        assert_eq!(payload["phase"], "running");
        assert_eq!(payload.as_object().map(|object| object.len()), Some(4));
    }
}
