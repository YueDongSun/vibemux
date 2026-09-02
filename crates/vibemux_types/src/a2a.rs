//! Backend-neutral stateful A2A records. SDK objects never enter this module.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset};

use crate::{ArtifactId, ProjectId, Run, RunId, RunStatus, Task, TaskId};

pub const A2A_RECORD_SCHEMA_VERSION: u32 = 1;
pub const MAX_A2A_ARTIFACTS: usize = 16;
pub const MAX_A2A_RUNS_PER_TASK: usize = 32;
pub const MAX_A2A_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum A2aStateError {
    #[error("A2A state input is invalid")]
    InvalidInput,
    #[error("A2A record identities do not match")]
    IdentityMismatch,
    #[error("A2A record state is invalid")]
    InvalidState,
}

impl A2aStateError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput => "a2a_invalid_input",
            Self::IdentityMismatch => "a2a_identity_mismatch",
            Self::InvalidState => "a2a_invalid_state",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum A2aRemoteState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

impl A2aRemoteState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        self as u8 == next as u8
            || matches!(
                (self, next),
                (
                    Self::Submitted,
                    Self::Working
                        | Self::InputRequired
                        | Self::Completed
                        | Self::Failed
                        | Self::Cancelled
                ) | (
                    Self::Working,
                    Self::InputRequired | Self::Completed | Self::Failed | Self::Cancelled
                ) | (
                    Self::InputRequired,
                    Self::Working | Self::Completed | Self::Failed | Self::Cancelled
                )
            )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum A2aCancellationState {
    None,
    Requested,
    Confirmed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReference {
    pub artifact_id: ArtifactId,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

impl ArtifactReference {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        validate_sha256(&self.sha256)?;
        if self.size_bytes > MAX_A2A_ARTIFACT_BYTES || !valid_label(&self.media_type, 128) {
            return Err(A2aStateError::InvalidInput);
        }
        Ok(())
    }
}

/// Private persistence ownership evidence. It grants no authority to delete by path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunWorkspace {
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub ownership_token: String,
}

impl RunWorkspace {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        let absolute = self.path.starts_with('/')
            || self.path.starts_with("\\\\")
            || (self.path.as_bytes().get(1) == Some(&b':')
                && self
                    .path
                    .as_bytes()
                    .get(2)
                    .is_some_and(|byte| matches!(byte, b'/' | b'\\')));
        if !absolute
            || !valid_label(&self.path, 4096)
            || !valid_identifier(&self.branch, 128)
            || self.branch.starts_with('-')
            || self.branch.contains("..")
            || self.branch.ends_with(".lock")
            || !valid_identifier(&self.ownership_token, 128)
            || !matches!(self.base_commit.len(), 40 | 64)
            || !self
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(A2aStateError::InvalidInput);
        }
        Ok(())
    }

    #[must_use]
    pub fn path_key(&self) -> String {
        self.path
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    }
}

/// Local trusted verifier receipt, never deserialized from a peer wire envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceipt {
    pub reviewer_run_id: RunId,
    pub reviewer_expected_version: u64,
    pub artifact_sha256: String,
    pub review_sha256: String,
    pub verification_sha256: String,
    pub accepted: bool,
}

impl VerificationReceipt {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        if self.reviewer_expected_version == 0 {
            return Err(A2aStateError::InvalidInput);
        }
        validate_sha256(&self.artifact_sha256)?;
        validate_sha256(&self.review_sha256)?;
        validate_sha256(&self.verification_sha256)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct A2aBinding {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub peer_id: String,
    pub external_task_id: String,
    pub transport: String,
    pub protocol_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub idempotency_key: String,
    pub last_remote_state: A2aRemoteState,
    pub cancellation_state: A2aCancellationState,
    pub artifacts: Vec<ArtifactReference>,
    pub verification: Option<VerificationReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "UncheckedA2aRunRecord")]
pub struct A2aRunRecord {
    pub schema_version: u32,
    pub task: Task,
    pub run: Run,
    pub binding: A2aBinding,
    pub workspace: RunWorkspace,
    pub version: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedA2aRunRecord {
    schema_version: u32,
    task: Task,
    run: Run,
    binding: A2aBinding,
    workspace: RunWorkspace,
    version: u64,
}

impl TryFrom<UncheckedA2aRunRecord> for A2aRunRecord {
    type Error = A2aStateError;
    fn try_from(value: UncheckedA2aRunRecord) -> Result<Self, Self::Error> {
        let record = Self {
            schema_version: value.schema_version,
            task: value.task,
            run: value.run,
            binding: value.binding,
            workspace: value.workspace,
            version: value.version,
        };
        record.validate()?;
        Ok(record)
    }
}

impl A2aRunRecord {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        if self.schema_version != A2A_RECORD_SCHEMA_VERSION
            || self.version == 0
            || self.task.project_id() != self.run.project_id()
            || self.task.task_id() != self.run.task_id()
            || self.binding.project_id != self.run.project_id()
            || self.binding.task_id != self.run.task_id()
            || self.binding.run_id != self.run.run_id()
            || self.workspace.base_commit != self.run.base_commit()
        {
            return Err(A2aStateError::IdentityMismatch);
        }
        validate_binding_fields(
            &self.binding.peer_id,
            &self.binding.external_task_id,
            &self.binding.transport,
            &self.binding.protocol_version,
        )?;
        validate_idempotency_key(&self.binding.idempotency_key)?;
        validate_timestamp(self.binding.created_at)?;
        validate_timestamp(self.binding.updated_at)?;
        if self.binding.updated_at < self.binding.created_at {
            return Err(A2aStateError::InvalidInput);
        }
        self.workspace.validate()?;
        validate_artifacts(&self.binding.artifacts)?;
        if let Some(receipt) = &self.binding.verification {
            receipt.validate()?;
        }
        if self.run.status() == RunStatus::Succeeded && self.binding.verification.is_none() {
            return Err(A2aStateError::InvalidState);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct A2aRunStart {
    pub task: Task,
    pub run: Run,
    pub peer_id: String,
    pub external_task_id: String,
    pub transport: String,
    pub protocol_version: String,
    pub workspace: RunWorkspace,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub idempotency_key: String,
}

impl A2aRunStart {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        if self.run.status() != RunStatus::Preparing
            || !self.task.description().is_empty()
            || !matches!(self.run.role(), "worker" | "reviewer" | "verifier")
            || self.task.project_id() != self.run.project_id()
            || self.task.task_id() != self.run.task_id()
            || self.workspace.base_commit != self.run.base_commit()
        {
            return Err(A2aStateError::InvalidInput);
        }
        validate_binding_fields(
            &self.peer_id,
            &self.external_task_id,
            &self.transport,
            &self.protocol_version,
        )?;
        validate_idempotency_key(&self.idempotency_key)?;
        validate_timestamp(self.timestamp)?;
        self.workspace.validate()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct A2aRunUpdate {
    pub run_id: RunId,
    pub expected_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub idempotency_key: String,
    pub action: A2aRunAction,
}

impl A2aRunUpdate {
    pub fn validate(&self) -> Result<(), A2aStateError> {
        if self.expected_version == 0 {
            return Err(A2aStateError::InvalidInput);
        }
        validate_idempotency_key(&self.idempotency_key)?;
        validate_timestamp(self.timestamp)?;
        match &self.action {
            A2aRunAction::Observe { artifacts, .. } => validate_artifacts(artifacts),
            A2aRunAction::Fail { code } if !valid_identifier(code, 128) => {
                Err(A2aStateError::InvalidInput)
            }
            A2aRunAction::Verify { receipt } => receipt.validate(),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum A2aRunAction {
    Start,
    Observe {
        state: A2aRemoteState,
        artifacts: Vec<ArtifactReference>,
    },
    RequestCancellation,
    ConfirmCancellation,
    /// Trusted owner finalization after every related Run has stopped; it does not forge a remote state.
    FinalizeCancellation,
    Fail {
        code: String,
    },
    Verify {
        receipt: VerificationReceipt,
    },
}

pub fn validate_artifacts(artifacts: &[ArtifactReference]) -> Result<(), A2aStateError> {
    if artifacts.len() > MAX_A2A_ARTIFACTS {
        return Err(A2aStateError::InvalidInput);
    }
    let mut ids = std::collections::BTreeSet::new();
    for artifact in artifacts {
        artifact.validate()?;
        if !ids.insert(artifact.artifact_id) {
            return Err(A2aStateError::InvalidInput);
        }
    }
    Ok(())
}

fn validate_binding_fields(
    peer: &str,
    external: &str,
    transport: &str,
    protocol: &str,
) -> Result<(), A2aStateError> {
    if !valid_identifier(peer, 128)
        || !valid_identifier(external, 256)
        || !matches!(transport, "http_json" | "json_rpc" | "grpc")
        || !valid_identifier(protocol, 32)
    {
        return Err(A2aStateError::InvalidInput);
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> Result<(), A2aStateError> {
    if !valid_identifier(key, 256) {
        return Err(A2aStateError::InvalidInput);
    }
    Ok(())
}

fn validate_timestamp(timestamp: OffsetDateTime) -> Result<(), A2aStateError> {
    if timestamp.offset() != UtcOffset::UTC || !(1970..=9999).contains(&timestamp.year()) {
        return Err(A2aStateError::InvalidInput);
    }
    Ok(())
}

fn validate_sha256(digest: &str) -> Result<(), A2aStateError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(A2aStateError::InvalidInput);
    }
    Ok(())
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./:-".contains(&byte))
}

fn valid_label(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
