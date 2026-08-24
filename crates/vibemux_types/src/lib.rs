#![forbid(unsafe_code)]
//! Canonical VibeMux identifiers, domain records, and deterministic state machines.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize, de};
use thiserror::Error;
use uuid::Uuid;

const MAX_TITLE_BYTES: usize = 512;
const MAX_DESCRIPTION_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: usize = 128;
const MAX_BASE_COMMIT_BYTES: usize = 128;

/// Stable failures returned by domain validation and transitions.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DomainError {
    #[error("invalid {entity} identifier")]
    InvalidId { entity: &'static str },
    #[error("{field} must contain between 1 and {max_bytes} UTF-8 bytes")]
    InvalidRequiredField {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("{field} exceeds {max_bytes} UTF-8 bytes")]
    FieldTooLarge {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("{entity} transition {from} -> {to} is not allowed")]
    InvalidTransition {
        entity: &'static str,
        from: &'static str,
        to: &'static str,
    },
    #[error("run success requires structured adapter, verifier, or explicit user authority")]
    CompletionAuthorityRequired,
    #[error("base_commit must be a hexadecimal Git object identifier")]
    InvalidBaseCommit,
}

impl DomainError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidId { .. } => "invalid_id",
            Self::InvalidRequiredField { .. } => "invalid_required_field",
            Self::FieldTooLarge { .. } => "field_too_large",
            Self::InvalidTransition { .. } => "invalid_state_transition",
            Self::CompletionAuthorityRequired => "completion_authority_required",
            Self::InvalidBaseCommit => "invalid_base_commit",
        }
    }
}

macro_rules! define_id {
    ($name:ident, $entity:literal) => {
        #[doc = concat!("Opaque ", $entity, " identifier.")]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_uuid(value: Uuid) -> Result<Self, DomainError> {
                if value.is_nil() {
                    return Err(DomainError::InvalidId { entity: $entity });
                }
                Ok(Self(value))
            }

            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)
                    .ok()
                    .and_then(|parsed| Self::from_uuid(parsed).ok())
                    .ok_or(DomainError::InvalidId { entity: $entity })
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = Uuid::deserialize(deserializer)?;
                Self::from_uuid(value).map_err(de::Error::custom)
            }
        }
    };
}

define_id!(ProjectId, "project");
define_id!(TaskId, "task");
define_id!(RunId, "run");
define_id!(EventId, "event");
define_id!(PluginId, "plugin");
define_id!(ArtifactId, "artifact");

/// Canonical task states retained from the Python behavior reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

impl TaskStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Open, Self::InProgress | Self::Cancelled)
                | (
                    Self::InProgress,
                    Self::Blocked | Self::Done | Self::Cancelled
                )
                | (Self::Blocked, Self::InProgress | Self::Cancelled)
        )
    }
}

/// Canonical run states retained from the Python behavior reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Preparing,
    Running,
    Stopped,
    Failed,
    Succeeded,
    Stale,
}

impl RunStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Succeeded => "succeeded",
            Self::Stale => "stale",
        }
    }

    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Preparing, Self::Running | Self::Failed)
                | (
                    Self::Running,
                    Self::Stopped | Self::Failed | Self::Succeeded | Self::Stale
                )
                | (Self::Stale, Self::Stopped | Self::Failed)
        )
    }
}

/// Authorities allowed to declare a run successful.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCompletionAuthority {
    StructuredAdapter,
    Verifier,
    User,
}

/// Validated input for constructing a task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSpec {
    pub project_id: ProjectId,
    pub title: String,
    pub description: String,
}

/// Canonical task record without persistence or platform details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Task {
    task_id: TaskId,
    project_id: ProjectId,
    title: String,
    description: String,
    status: TaskStatus,
}

impl Task {
    pub fn new(spec: TaskSpec) -> Result<Self, DomainError> {
        validate_required("title", &spec.title, MAX_TITLE_BYTES)?;
        validate_optional("description", &spec.description, MAX_DESCRIPTION_BYTES)?;
        Ok(Self {
            task_id: TaskId::new(),
            project_id: spec.project_id,
            title: spec.title,
            description: spec.description,
            status: TaskStatus::Open,
        })
    }

    pub fn transitioned(&self, target: TaskStatus) -> Result<Self, DomainError> {
        if !self.status.can_transition_to(target) {
            return Err(DomainError::InvalidTransition {
                entity: "task",
                from: self.status.as_str(),
                to: target.as_str(),
            });
        }
        let mut transitioned = self.clone();
        transitioned.status = target;
        Ok(transitioned)
    }

    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub const fn status(&self) -> TaskStatus {
        self.status
    }
}

/// Validated input for constructing a run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunSpec {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub harness: String,
    pub role: String,
    pub protocol: String,
    pub base_commit: String,
}

/// Canonical run record without terminal, Git, database, or network types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Run {
    run_id: RunId,
    project_id: ProjectId,
    task_id: TaskId,
    harness: String,
    role: String,
    protocol: String,
    base_commit: String,
    status: RunStatus,
}

impl Run {
    pub fn new(spec: RunSpec) -> Result<Self, DomainError> {
        validate_required("harness", &spec.harness, MAX_NAME_BYTES)?;
        validate_required("role", &spec.role, MAX_NAME_BYTES)?;
        validate_required("protocol", &spec.protocol, MAX_NAME_BYTES)?;
        validate_base_commit(&spec.base_commit)?;
        Ok(Self {
            run_id: RunId::new(),
            project_id: spec.project_id,
            task_id: spec.task_id,
            harness: spec.harness,
            role: spec.role,
            protocol: spec.protocol,
            base_commit: spec.base_commit,
            status: RunStatus::Preparing,
        })
    }

    pub fn transitioned(
        &self,
        target: RunStatus,
        completion_authority: Option<RunCompletionAuthority>,
    ) -> Result<Self, DomainError> {
        if !self.status.can_transition_to(target) {
            return Err(DomainError::InvalidTransition {
                entity: "run",
                from: self.status.as_str(),
                to: target.as_str(),
            });
        }
        if target == RunStatus::Succeeded && completion_authority.is_none() {
            return Err(DomainError::CompletionAuthorityRequired);
        }
        let mut transitioned = self.clone();
        transitioned.status = target;
        Ok(transitioned)
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    #[must_use]
    pub fn harness(&self) -> &str {
        &self.harness
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    #[must_use]
    pub fn base_commit(&self) -> &str {
        &self.base_commit
    }

    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.status
    }
}

fn validate_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), DomainError> {
    let byte_count = value.len();
    if value.trim().is_empty() || byte_count > max_bytes {
        return Err(DomainError::InvalidRequiredField { field, max_bytes });
    }
    Ok(())
}

fn validate_optional(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), DomainError> {
    if value.len() > max_bytes {
        return Err(DomainError::FieldTooLarge { field, max_bytes });
    }
    Ok(())
}

fn validate_base_commit(value: &str) -> Result<(), DomainError> {
    if value.len() < 7
        || value.len() > MAX_BASE_COMMIT_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DomainError::InvalidBaseCommit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    const PYTHON_TRANSITIONS: &str =
        include_str!("../../../tests/fixtures/python_reference/domain_transitions_v1.json");

    #[derive(Deserialize)]
    struct TransitionFixture {
        task_transitions: BTreeMap<String, Vec<String>>,
        run_transitions: BTreeMap<String, Vec<String>>,
    }

    fn task_statuses() -> [TaskStatus; 5] {
        [
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Done,
            TaskStatus::Cancelled,
        ]
    }

    fn run_statuses() -> [RunStatus; 6] {
        [
            RunStatus::Preparing,
            RunStatus::Running,
            RunStatus::Stopped,
            RunStatus::Failed,
            RunStatus::Succeeded,
            RunStatus::Stale,
        ]
    }

    #[test]
    fn state_machine_matches_python_reference_fixture() {
        let fixture: TransitionFixture =
            serde_json::from_str(PYTHON_TRANSITIONS).expect("checked-in fixture must be valid");

        for source in task_statuses() {
            let expected = fixture
                .task_transitions
                .get(source.as_str())
                .expect("fixture includes every task state");
            let actual: Vec<&str> = task_statuses()
                .into_iter()
                .filter(|target| source.can_transition_to(*target))
                .map(TaskStatus::as_str)
                .collect();
            assert_eq!(&actual, expected);
        }

        for source in run_statuses() {
            let expected = fixture
                .run_transitions
                .get(source.as_str())
                .expect("fixture includes every run state");
            let actual: Vec<&str> = run_statuses()
                .into_iter()
                .filter(|target| source.can_transition_to(*target))
                .map(RunStatus::as_str)
                .collect();
            assert_eq!(&actual, expected);
        }
    }

    #[test]
    fn run_success_requires_explicit_non_terminal_authority() {
        let run = Run::new(RunSpec {
            project_id: ProjectId::new(),
            task_id: TaskId::new(),
            harness: "mock".to_owned(),
            role: "worker".to_owned(),
            protocol: "mock".to_owned(),
            base_commit: "0123456789abcdef".to_owned(),
        })
        .expect("valid run");
        let running = run
            .transitioned(RunStatus::Running, None)
            .expect("preparing may become running");

        assert_eq!(
            running.transitioned(RunStatus::Succeeded, None),
            Err(DomainError::CompletionAuthorityRequired)
        );
        assert!(
            running
                .transitioned(
                    RunStatus::Succeeded,
                    Some(RunCompletionAuthority::StructuredAdapter),
                )
                .is_ok()
        );
    }

    #[test]
    fn identifiers_round_trip_as_opaque_strings() {
        let task_id = TaskId::new();
        let encoded = serde_json::to_string(&task_id).expect("serialize identifier");
        let decoded: TaskId = serde_json::from_str(&encoded).expect("deserialize identifier");
        assert_eq!(decoded, task_id);
        assert_eq!(task_id.to_string().parse::<TaskId>(), Ok(task_id));
    }

    proptest! {
        #[test]
        fn terminal_task_states_have_no_outgoing_edges(index in 0_usize..2) {
            let source = [TaskStatus::Done, TaskStatus::Cancelled][index];
            prop_assert!(task_statuses().into_iter().all(|target| !source.can_transition_to(target)));
        }

        #[test]
        fn terminal_run_states_have_no_outgoing_edges(index in 0_usize..3) {
            let source = [RunStatus::Stopped, RunStatus::Failed, RunStatus::Succeeded][index];
            prop_assert!(run_statuses().into_iter().all(|target| !source.can_transition_to(target)));
        }
    }
}
