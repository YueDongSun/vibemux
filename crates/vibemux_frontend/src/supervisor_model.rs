#![forbid(unsafe_code)]
//! Backend-neutral supervisor snapshots and UI-to-runtime actions.

use serde::{Deserialize, Serialize};

/// Immutable state presented by the supervisor to the native GUI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SupervisorSnapshot {
    pub project_id: String,
    pub project_name: String,
    pub coordinator: Option<String>,
    pub connection: ConnectionState,
    pub tasks: Vec<TaskView>,
    pub next_cursor: Option<String>,
    pub observed_at: Option<String>,
    pub mode: SnapshotMode,
}

impl SupervisorSnapshot {
    /// Honest initial state before a runtime snapshot has been supplied.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            project_id: String::new(),
            project_name: "No project selected".to_string(),
            coordinator: Some("Coordinator".to_string()),
            connection: ConnectionState {
                status: ConnectionStatus::Unavailable,
                detail: Some("The supervisor runtime is not connected.".to_string()),
            },
            tasks: Vec::new(),
            next_cursor: None,
            observed_at: None,
            mode: SnapshotMode::Live,
        }
    }
}

impl Default for SupervisorSnapshot {
    fn default() -> Self {
        Self::unavailable()
    }
}

/// Current transport and supervisor availability.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionState {
    pub status: ConnectionStatus,
    pub detail: Option<String>,
}

/// Stable status tokens used by the GUI and runtime adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    #[default]
    Unavailable,
}

impl ConnectionStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Connecting => "Connecting",
            Self::Disconnected => "Disconnected",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// Marks synthetic preview data so every window can identify it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    #[default]
    Live,
    Demo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskView {
    pub task_id: String,
    pub title: String,
    pub state: String,
    pub executor: String,
    pub latest_update: String,
    pub latest_sequence: u64,
    pub has_more_runs: bool,
    pub has_more_artifacts: bool,
    pub has_more_events: bool,
    pub runs: Vec<RunView>,
    pub recent_events: Vec<TaskEventView>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunView {
    pub run_id: String,
    pub harness: String,
    pub role: String,
    pub state: String,
    pub worktree: String,
    pub branch: String,
    pub artifacts: Vec<ArtifactView>,
    pub terminal: TerminalView,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactView {
    pub artifact_id: String,
    pub name: String,
    pub kind: String,
    pub state: String,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub media_type: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskEventView {
    pub event_id: String,
    pub sequence: u64,
    pub kind: String,
    pub occurred_at: String,
    pub run_id: Option<String>,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalView {
    pub status: String,
    pub binding_id: Option<String>,
    pub plugin_id: Option<String>,
    pub pane_id: Option<String>,
    pub instance_id: Option<String>,
    pub checked_at: Option<String>,
    pub candidates: Vec<TerminalCandidate>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalCandidate {
    pub plugin_id: String,
    pub pane_id: String,
    pub instance_id: String,
    pub display_name: String,
}

/// Bounded requests emitted by the GUI for the parent runtime adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum SupervisorAction {
    Refresh,
    LoadMoreTasks {
        cursor: String,
    },
    LoadTask {
        task_id: String,
    },
    InspectTerminal {
        task_id: String,
        run_id: String,
    },
    LinkTerminal {
        task_id: String,
        run_id: String,
        plugin_id: String,
        pane_id: String,
        instance_id: String,
    },
    FocusTerminal {
        binding_id: String,
    },
    UnlinkTerminal {
        binding_id: String,
    },
}
