//! Fixed, bounded renderer contracts for task and console reads.

use serde::{Deserialize, Serialize};
use vibemux_types::{ArtifactId, ProjectId, RunId, RunStatus, TaskId, TaskStatus};

pub const TASK_PAGE_LIMIT: u32 = 32;
pub const CONSOLE_ROW_LIMIT: usize = 64;
pub const DECISION_PAGE_LIMIT: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskReadQuery {
    List {
        #[serde(default)]
        after: Option<TaskId>,
        #[serde(default = "default_task_limit")]
        limit: u32,
    },
    Detail {
        task_id: TaskId,
    },
}

const fn default_task_limit() -> u32 {
    TASK_PAGE_LIMIT
}

impl TaskReadQuery {
    pub fn validate(&self) -> Result<(), ReadError> {
        if matches!(self, Self::List { limit, .. } if *limit == 0 || *limit > TASK_PAGE_LIMIT) {
            Err(ReadError::new(
                "read_invalid_query",
                "Request between 1 and 32 tasks per page.",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskCard {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    pub title_truncated: bool,
    pub status: TaskStatus,
    pub latest_sequence: u64,
    pub detail_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunFact {
    pub run_id: RunId,
    pub harness: String,
    pub role: String,
    pub status: RunStatus,
    pub branch: Option<String>,
    pub has_worktree: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventFact {
    pub sequence: u64,
    pub event_type: String,
    pub at: String,
    pub run_id: Option<RunId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactFact {
    pub artifact_id: ArtifactId,
    pub run_id: RunId,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskDetail {
    pub task: TaskCard,
    pub runs: Vec<RunFact>,
    pub events: Vec<EventFact>,
    pub artifacts: Vec<ArtifactFact>,
    pub has_more_runs: bool,
    pub has_more_events: bool,
    pub has_more_artifacts: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskReadResult {
    Page {
        project_id: Option<ProjectId>,
        tasks: Vec<TaskCard>,
        next_cursor: Option<TaskId>,
        observed_at: Option<String>,
    },
    Detail {
        task: TaskDetail,
        observed_at: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSection {
    Overview,
    Harnesses,
    Workspace,
    Tasks,
    Daemon,
    Sessions,
    Decisions,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleQuery {
    pub section: ConsoleSection,
    #[serde(default)]
    pub after_sequence: Option<u64>,
}

impl ConsoleQuery {
    pub fn validate(&self) -> Result<(), ReadError> {
        if self.after_sequence.is_some() && self.section != ConsoleSection::Decisions {
            Err(ReadError::new(
                "read_invalid_query",
                "Only the Decisions section accepts an audit-sequence cursor.",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadAvailability {
    Loaded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleRow {
    pub key: String,
    pub cells: Vec<String>,
    /// Only actual task rows can offer read-only navigation to task details.
    pub task_id: Option<TaskId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleBlock {
    pub key: String,
    pub title: String,
    pub availability: ReadAvailability,
    pub code: Option<String>,
    pub hint: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<ConsoleRow>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleSnapshot {
    pub section: ConsoleSection,
    pub observed_at: Option<String>,
    pub blocks: Vec<ConsoleBlock>,
    pub next_after_sequence: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadError {
    pub code: String,
    pub message: String,
}

impl ReadError {
    pub fn new(code: &str, message: &str) -> Self {
        let safe = !code.is_empty()
            && code.len() <= 96
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        Self {
            code: if safe {
                code.to_owned()
            } else {
                "read_operation_failed".to_owned()
            },
            message: message.to_owned(),
        }
    }

    pub fn unavailable(code: &str) -> Self {
        let mut error = Self::new(code, "The read-only observation is unavailable.");
        error.message = vibemux_client::daemon_model::unavailable(&error.code).hint;
        error
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ReadError {}
