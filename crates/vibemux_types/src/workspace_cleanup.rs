//! Private, inert workspace-cleanup metadata and deterministic validation.
//!
//! These serializable audit records grant no process or filesystem authority.
//! Their producer must establish object identity from owned handles and consume
//! fresh sealed witnesses; validation cannot reconstruct either responsibility.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{ChatId, ProjectId, RunId, TaskId, WorkspaceCleanupId, a2a::RunWorkspace};

pub const WORKSPACE_CLEANUP_SCHEMA_VERSION: u32 = 1;
/// The persistence boundary also enforces this limit on serialized UTF-8 bytes.
pub const MAX_WORKSPACE_CLEANUP_METADATA_BYTES: usize = 64 * 1024;
const MAX_PRIVATE_PATH_BYTES: usize = 4096;
const MAX_TERMINAL_ID_BYTES: usize = 128;
const MAX_ERROR_CODE_BYTES: usize = 96;
const MAX_PLAN_LIFETIME_SECONDS: i64 = 300;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CleanupError {
    #[error("workspace cleanup input is invalid")]
    InvalidInput,
    #[error("workspace cleanup transition is invalid")]
    InvalidTransition,
}

impl CleanupError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "workspace_cleanup_invalid_input",
            Self::InvalidTransition => "workspace_cleanup_invalid_transition",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCleanupPhase {
    Planned,
    Claimed,
    Armed,
    Quiescent,
    Removed,
    Refused,
    Unknown,
}

impl WorkspaceCleanupPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Claimed => "claimed",
            Self::Armed => "armed",
            Self::Quiescent => "quiescent",
            Self::Removed => "removed",
            Self::Refused => "refused",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn holds_resource(self) -> bool {
        matches!(
            self,
            Self::Claimed | Self::Armed | Self::Quiescent | Self::Unknown
        )
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Removed | Self::Refused)
    }

    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Planned, Self::Claimed)
                | (
                    Self::Claimed,
                    Self::Claimed | Self::Armed | Self::Quiescent | Self::Unknown
                )
                | (Self::Armed, Self::Quiescent | Self::Unknown)
                | (
                    Self::Quiescent,
                    Self::Removed | Self::Refused | Self::Unknown
                )
                | (Self::Unknown, Self::Unknown | Self::Quiescent)
        )
    }

    /// Checks only the state-machine edge; it issues no execution authority.
    pub fn validate_transition_to(self, target: Self) -> Result<(), CleanupError> {
        if !self.can_transition_to(target) {
            return Err(CleanupError::InvalidTransition);
        }
        Ok(())
    }
}

/// Full-width private object identity, obtained by a platform-owned producer.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum CleanupObjectIdentity {
    WindowsFileId128V1 {
        volume_serial_number: u64,
        file_id: [u8; 16],
    },
}

impl CleanupObjectIdentity {
    pub fn validate(&self) -> Result<(), CleanupError> {
        match self {
            Self::WindowsFileId128V1 { file_id, .. } if file_id.iter().any(|byte| *byte != 0) => {
                Ok(())
            }
            _ => Err(CleanupError::InvalidInput),
        }
    }
}

impl fmt::Debug for CleanupObjectIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupObjectIdentity")
            .finish_non_exhaustive()
    }
}

/// Bound private metadata; paths and Run slots confer no filesystem authority.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupBinding {
    pub project_id: ProjectId,
    pub chat_id: ChatId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub owner_version: u64,
    pub session_generation: u64,
    pub session_version: u64,
    pub workspace: RunWorkspace,
    pub resource_key: String,
    pub repository_root: String,
    pub common_git_dir: String,
    pub repository_identity: CleanupObjectIdentity,
    pub common_directory_identity: CleanupObjectIdentity,
    pub worktree_identity: CleanupObjectIdentity,
    pub head_commit: String,
    pub terminal_plugin_id: String,
    pub session_id: String,
    pub instance_id: String,
}

impl WorkspaceCleanupBinding {
    pub fn validate(&self) -> Result<(), CleanupError> {
        self.workspace
            .validate()
            .map_err(|_| CleanupError::InvalidInput)?;
        self.repository_identity.validate()?;
        self.common_directory_identity.validate()?;
        self.worktree_identity.validate()?;
        let normalized_slot = self.workspace.path.replace('\\', "/");
        if !valid_version(self.owner_version)
            || !valid_version(self.session_generation)
            || !valid_version(self.session_version)
            || !valid_private_path(&self.workspace.path)
            || self.workspace.branch != format!("codex/run_{}", self.run_id.as_uuid().simple())
            || normalized_slot.rsplit('/').next() != Some(self.run_id.to_string().as_str())
            || normalized_slot
                .split('/')
                .any(|part| matches!(part, "." | ".."))
            || !valid_hex(&self.resource_key, &[64])
            || !valid_private_path(&self.repository_root)
            || !valid_private_path(&self.common_git_dir)
            || !valid_hex(&self.head_commit, &[40, 64])
            || !valid_private_text(&self.terminal_plugin_id, MAX_TERMINAL_ID_BYTES)
            || !valid_private_text(&self.session_id, MAX_TERMINAL_ID_BYTES)
            || !valid_private_text(&self.instance_id, MAX_TERMINAL_ID_BYTES)
        {
            return Err(CleanupError::InvalidInput);
        }
        Ok(())
    }
}

impl fmt::Debug for WorkspaceCleanupBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceCleanupBinding")
            .field("project_id", &self.project_id)
            .field("chat_id", &self.chat_id)
            .field("task_id", &self.task_id)
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupCommandProfile {
    GitWorktreeRemoveV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupContainmentKind {
    WindowsJobV1,
}

/// Executor audit identity. Persistence never makes this a live process handle,
/// restart permission, cancellation permission, or proof of quiescence.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorBinding {
    pub executor_nonce: Uuid,
    pub daemon_incarnation: Uuid,
    pub process_id: u32,
    pub observed_start_seconds: u64,
    pub expected_executable: String,
    pub observed_executable: String,
    pub consumed_executable_identity: CleanupObjectIdentity,
    pub observed_path_identity: CleanupObjectIdentity,
    pub program_executable: String,
    pub program_identity: CleanupObjectIdentity,
    pub command_profile: CleanupCommandProfile,
    pub containment_kind: CleanupContainmentKind,
}

impl ExecutorBinding {
    pub fn validate(&self) -> Result<(), CleanupError> {
        self.consumed_executable_identity.validate()?;
        self.observed_path_identity.validate()?;
        self.program_identity.validate()?;
        if self.executor_nonce.is_nil()
            || self.daemon_incarnation.is_nil()
            || self.process_id == 0
            || self.observed_start_seconds == 0
            || !valid_private_path(&self.expected_executable)
            || !valid_private_path(&self.observed_executable)
            || !valid_private_path(&self.program_executable)
            || self.consumed_executable_identity != self.observed_path_identity
        {
            return Err(CleanupError::InvalidInput);
        }
        Ok(())
    }
}

impl fmt::Debug for ExecutorBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutorBinding")
            .field("executor_nonce", &self.executor_nonce)
            .field("daemon_incarnation", &self.daemon_incarnation)
            .finish_non_exhaustive()
    }
}

/// Audit of consumed local quiescence evidence. `NeverSpawned` requires a fresh,
/// sealed claim-retirement witness at the store boundary. Missing executor rows
/// cannot create that witness; this enum only validates its persisted shape.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CleanupQuiescence {
    NeverSpawned {
        retired_claim_id: Uuid,
    },
    JoinedProcess {
        executor_nonce: Uuid,
        termination_succeeded: bool,
        root_reaped: bool,
        tree_empty: bool,
        stdout_drained: bool,
        stderr_drained: bool,
    },
}

impl CleanupQuiescence {
    pub fn validate(&self) -> Result<(), CleanupError> {
        let id = match self {
            Self::NeverSpawned { retired_claim_id } => retired_claim_id,
            Self::JoinedProcess { executor_nonce, .. } => executor_nonce,
        };
        if id.is_nil() {
            return Err(CleanupError::InvalidInput);
        }
        Ok(())
    }

    #[must_use]
    pub fn is_positive(&self) -> bool {
        match self {
            Self::NeverSpawned { retired_claim_id } => !retired_claim_id.is_nil(),
            Self::JoinedProcess {
                executor_nonce,
                termination_succeeded,
                root_reaped,
                tree_empty,
                stdout_drained,
                stderr_drained,
            } => {
                !executor_nonce.is_nil()
                    && *termination_succeeded
                    && *root_reaped
                    && *tree_empty
                    && *stdout_drained
                    && *stderr_drained
            }
        }
    }
}

impl fmt::Debug for CleanupQuiescence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeverSpawned { retired_claim_id } => formatter
                .debug_struct("NeverSpawned")
                .field("retired_claim_id", retired_claim_id)
                .finish_non_exhaustive(),
            Self::JoinedProcess { executor_nonce, .. } => formatter
                .debug_struct("JoinedProcess")
                .field("executor_nonce", executor_nonce)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupProcessResult {
    pub executor_nonce: Uuid,
    pub exit_code: Option<i32>,
    pub forced_termination: bool,
    pub timed_out: bool,
    pub release_attempted: bool,
}

impl CleanupProcessResult {
    pub fn validate(&self) -> Result<(), CleanupError> {
        if self.executor_nonce.is_nil() {
            return Err(CleanupError::InvalidInput);
        }
        Ok(())
    }

    /// A process result alone never proves quiescence or workspace removal.
    #[must_use]
    pub fn is_successful(&self) -> bool {
        !self.executor_nonce.is_nil()
            && self.exit_code == Some(0)
            && !self.forced_termination
            && !self.timed_out
            && self.release_attempted
    }
}

impl fmt::Debug for CleanupProcessResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupProcessResult")
            .field("executor_nonce", &self.executor_nonce)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCleanupOutcome {
    RemovedExecuted,
    RemovedObserved,
    Refused,
}

/// Versioned private persistence record. Validation is deterministic and inert;
/// the store remains responsible for ownership, compare-and-swap and provenance.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanup {
    pub schema_version: u32,
    pub id: WorkspaceCleanupId,
    pub plan_operation_id: Uuid,
    pub daemon_incarnation: Uuid,
    pub version: u64,
    pub phase: WorkspaceCleanupPhase,
    pub binding: WorkspaceCleanupBinding,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub confirmation_operation_id: Option<Uuid>,
    pub claim_id: Option<Uuid>,
    pub executor: Option<ExecutorBinding>,
    pub arm_operation_id: Option<Uuid>,
    pub process_result: Option<CleanupProcessResult>,
    pub quiescence: Option<CleanupQuiescence>,
    pub outcome: Option<WorkspaceCleanupOutcome>,
    pub error_code: Option<String>,
}

impl WorkspaceCleanup {
    pub fn validate(&self) -> Result<(), CleanupError> {
        self.binding.validate()?;
        let lifetime = self.expires_at - self.created_at;
        if self.schema_version != WORKSPACE_CLEANUP_SCHEMA_VERSION
            || self.plan_operation_id.is_nil()
            || self.daemon_incarnation.is_nil()
            || !valid_version(self.version)
            || lifetime <= Duration::ZERO
            || lifetime > Duration::seconds(MAX_PLAN_LIFETIME_SECONDS)
            || self.updated_at < self.created_at
            || self.confirmation_operation_id.is_some() != self.claim_id.is_some()
            || [
                self.confirmation_operation_id,
                self.claim_id,
                self.arm_operation_id,
            ]
            .into_iter()
            .flatten()
            .any(|id| id.is_nil())
            || self
                .error_code
                .as_ref()
                .is_some_and(|code| !valid_error_code(code))
        {
            return Err(CleanupError::InvalidInput);
        }
        if let Some(executor) = &self.executor {
            executor.validate()?;
            if executor.daemon_incarnation != self.daemon_incarnation {
                return Err(CleanupError::InvalidInput);
            }
        }
        if self.arm_operation_id.is_some() && self.executor.is_none() {
            return Err(CleanupError::InvalidInput);
        }
        if let Some(result) = &self.process_result {
            result.validate()?;
            if self
                .executor
                .as_ref()
                .map(|executor| executor.executor_nonce)
                != Some(result.executor_nonce)
                || (result.release_attempted && self.arm_operation_id.is_none())
            {
                return Err(CleanupError::InvalidInput);
            }
        }
        if let Some(quiescence) = &self.quiescence {
            quiescence.validate()?;
            let correlated = match quiescence {
                CleanupQuiescence::NeverSpawned { retired_claim_id } => {
                    self.claim_id == Some(*retired_claim_id)
                        && self.executor.is_none()
                        && self.arm_operation_id.is_none()
                }
                CleanupQuiescence::JoinedProcess { executor_nonce, .. } => {
                    self.executor
                        .as_ref()
                        .map(|executor| executor.executor_nonce)
                        == Some(*executor_nonce)
                }
            };
            if !correlated {
                return Err(CleanupError::InvalidInput);
            }
        }
        let positive_quiescence = self
            .quiescence
            .as_ref()
            .is_some_and(CleanupQuiescence::is_positive);
        let valid_phase = match self.phase {
            WorkspaceCleanupPhase::Planned => {
                self.claim_id.is_none()
                    && self.executor.is_none()
                    && self.arm_operation_id.is_none()
                    && self.process_result.is_none()
                    && self.quiescence.is_none()
                    && self.outcome.is_none()
                    && self.error_code.is_none()
            }
            WorkspaceCleanupPhase::Claimed => {
                self.claim_id.is_some()
                    && self.arm_operation_id.is_none()
                    && self.process_result.is_none()
                    && self.quiescence.is_none()
                    && self.outcome.is_none()
                    && self.error_code.is_none()
            }
            WorkspaceCleanupPhase::Armed => {
                self.claim_id.is_some()
                    && self.executor.is_some()
                    && self.arm_operation_id.is_some()
                    && self.quiescence.is_none()
                    && self.outcome.is_none()
                    && self.error_code.is_none()
            }
            WorkspaceCleanupPhase::Quiescent => {
                self.claim_id.is_some()
                    && positive_quiescence
                    && self.outcome.is_none()
                    && self.error_code.is_none()
            }
            WorkspaceCleanupPhase::Unknown => {
                self.claim_id.is_some() && self.outcome.is_none() && self.error_code.is_some()
            }
            WorkspaceCleanupPhase::Removed => {
                self.claim_id.is_some()
                    && positive_quiescence
                    && self.error_code.is_none()
                    && match self.outcome {
                        Some(WorkspaceCleanupOutcome::RemovedExecuted) => {
                            self.arm_operation_id.is_some()
                                && self.executor.is_some()
                                && self
                                    .process_result
                                    .as_ref()
                                    .is_some_and(CleanupProcessResult::is_successful)
                        }
                        Some(WorkspaceCleanupOutcome::RemovedObserved) => true,
                        _ => false,
                    }
            }
            WorkspaceCleanupPhase::Refused => {
                self.claim_id.is_some()
                    && positive_quiescence
                    && self.outcome == Some(WorkspaceCleanupOutcome::Refused)
                    && self.error_code.is_some()
            }
        };
        if !valid_phase {
            return Err(CleanupError::InvalidInput);
        }
        Ok(())
    }
}

impl fmt::Debug for WorkspaceCleanup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceCleanup")
            .field("id", &self.id)
            .field("phase", &self.phase)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

const fn valid_version(version: u64) -> bool {
    version > 0 && version <= i64::MAX as u64
}

fn valid_private_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn valid_private_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let absolute = value.starts_with('/')
        || value.starts_with("\\\\")
        || (bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(1) == Some(&b':')
            && bytes
                .get(2)
                .is_some_and(|byte| matches!(byte, b'/' | b'\\')));
    absolute && valid_private_text(value, MAX_PRIVATE_PATH_BYTES)
}

fn valid_hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ERROR_CODE_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests;
