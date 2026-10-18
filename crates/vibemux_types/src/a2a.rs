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

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [A2aRemoteState; 6] = [
        A2aRemoteState::Submitted,
        A2aRemoteState::Working,
        A2aRemoteState::InputRequired,
        A2aRemoteState::Completed,
        A2aRemoteState::Failed,
        A2aRemoteState::Cancelled,
    ];

    fn workspace() -> RunWorkspace {
        RunWorkspace {
            path: r"C:\work\repo".to_string(),
            branch: "vibemux/run_1".to_string(),
            base_commit: "a".repeat(40),
            ownership_token: "owner_token_1".to_string(),
        }
    }

    fn workspace_with(change: impl FnOnce(&mut RunWorkspace)) -> RunWorkspace {
        let mut workspace = workspace();
        change(&mut workspace);
        workspace
    }

    fn artifact(byte: u8) -> ArtifactReference {
        ArtifactReference {
            artifact_id: ArtifactId::new(),
            sha256: format!("{byte:02x}").repeat(32),
            size_bytes: 1,
            media_type: "text/plain".to_string(),
        }
    }

    #[test]
    fn remote_states_never_return_to_submitted_or_leave_a_terminal_state() {
        use A2aRemoteState::{Cancelled, Completed, Failed, InputRequired, Submitted, Working};
        for from in STATES {
            for to in STATES {
                let expected = from == to
                    || match from {
                        Submitted => true,
                        Working | InputRequired => to != Submitted,
                        Completed | Failed | Cancelled => false,
                    };
                assert_eq!(from.can_transition_to(to), expected, "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn a_run_workspace_needs_an_absolute_path_and_a_safe_branch() {
        assert_eq!(workspace().validate(), Ok(()));
        for valid in [
            workspace_with(|w| w.path = "/srv/repo".to_string()),
            workspace_with(|w| w.path = r"\\server\share\repo".to_string()),
            workspace_with(|w| w.path = "C:/work/repo".to_string()),
            workspace_with(|w| w.base_commit = "0123456789abcdef".repeat(4)),
        ] {
            assert_eq!(valid.validate(), Ok(()), "{valid:?}");
        }
        for invalid in [
            workspace_with(|w| w.path = "repo".to_string()),
            workspace_with(|w| w.path = "C:repo".to_string()),
            workspace_with(|w| w.path = String::new()),
            workspace_with(|w| w.path = "/srv/repo\n".to_string()),
            workspace_with(|w| w.path = format!("/{}", "a".repeat(4096))),
            workspace_with(|w| w.branch = "-run".to_string()),
            workspace_with(|w| w.branch = "run..1".to_string()),
            workspace_with(|w| w.branch = "run.lock".to_string()),
            workspace_with(|w| w.branch = "run 1".to_string()),
            workspace_with(|w| w.branch = String::new()),
            workspace_with(|w| w.base_commit = "a".repeat(39)),
            workspace_with(|w| w.base_commit = "g".repeat(40)),
            workspace_with(|w| w.ownership_token = String::new()),
            workspace_with(|w| w.ownership_token = "owner token".to_string()),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn workspace_path_keys_ignore_case_separators_and_trailing_slashes() {
        let key = |path: &str| workspace_with(|w| w.path = path.to_string()).path_key();
        assert_eq!(key(r"C:\Work\Repo\"), "c:/work/repo");
        assert_eq!(key(r"C:\Work\Repo\"), key("c:/work/repo"));
        assert_eq!(key("/srv/Repo//"), "/srv/repo");
        assert_ne!(key(r"C:\Work\Repo"), key(r"C:\Work\Repo2"));
    }

    #[test]
    fn artifacts_are_bounded_unique_and_lowercase_hex() {
        let full: Vec<_> = (0_u8..).take(MAX_A2A_ARTIFACTS).map(artifact).collect();
        assert_eq!(validate_artifacts(&full), Ok(()));
        let mut over = full.clone();
        over.push(artifact(0xff));
        assert_eq!(validate_artifacts(&over), Err(A2aStateError::InvalidInput));
        let first = artifact(1);
        assert_eq!(
            validate_artifacts(&[first.clone(), first]),
            Err(A2aStateError::InvalidInput)
        );
        let mut largest = artifact(2);
        largest.size_bytes = MAX_A2A_ARTIFACT_BYTES;
        assert_eq!(largest.validate(), Ok(()));
        for invalid in [
            ArtifactReference {
                size_bytes: MAX_A2A_ARTIFACT_BYTES + 1,
                ..artifact(3)
            },
            ArtifactReference {
                sha256: "AB".repeat(32),
                ..artifact(4)
            },
            ArtifactReference {
                sha256: "ab".repeat(31),
                ..artifact(5)
            },
            ArtifactReference {
                media_type: "text/plain\u{7}".to_string(),
                ..artifact(6)
            },
            ArtifactReference {
                media_type: " ".to_string(),
                ..artifact(7)
            },
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }
}
