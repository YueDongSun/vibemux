//! Bounded, sanitized read models for the local frontend control API.

use serde::{Deserialize, Serialize};

use crate::{ArtifactId, ProjectId, RunId, RunStatus, TaskId, TaskStatus};

pub const MAX_FRONTEND_TASKS: u32 = 32;
pub const MAX_FRONTEND_RUNS: usize = 12;
pub const MAX_FRONTEND_EVENTS: usize = 24;
pub const MAX_FRONTEND_ARTIFACTS: usize = 24;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendTasksQuery {
    pub after: Option<TaskId>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendTaskQuery {
    pub task_id: TaskId,
}

impl Default for FrontendTasksQuery {
    fn default() -> Self {
        Self {
            after: None,
            limit: MAX_FRONTEND_TASKS,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendTaskSummary {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    pub title_truncated: bool,
    pub status: TaskStatus,
    pub latest_sequence: u64,
    /// Monotonic revision covering everything `FrontendTaskDetail` renders for
    /// this task: the Task projection, every Run projection that belongs to it,
    /// and every event carrying its `task_id`. A Run-only transition (an
    /// admitted dispatch claiming its Run, for example) does not touch the Task
    /// projection, so `latest_sequence` alone cannot detect it; this field can.
    ///
    /// `0` means "unknown" and always forces a detail fetch, so a client that
    /// meets an older daemon degrades to the unfetched-per-tick behavior
    /// instead of trusting a revision it cannot interpret.
    #[serde(default)]
    pub detail_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendTasksPage {
    pub project_id: Option<ProjectId>,
    pub tasks: Vec<FrontendTaskSummary>,
    pub next_cursor: Option<TaskId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendRunSummary {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub harness: String,
    pub role: String,
    pub status: RunStatus,
    pub worktree_path: Option<String>,
    pub worktree_path_truncated: bool,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendEventSummary {
    pub sequence: u64,
    pub event_type: String,
    pub at: String,
    pub run_id: Option<RunId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendArtifactSummary {
    pub run_id: RunId,
    pub artifact_id: ArtifactId,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontendTaskDetail {
    pub task: FrontendTaskSummary,
    pub runs: Vec<FrontendRunSummary>,
    pub has_more_runs: bool,
    pub events: Vec<FrontendEventSummary>,
    pub has_more_events: bool,
    pub artifacts: Vec<FrontendArtifactSummary>,
    pub has_more_artifacts: bool,
}
