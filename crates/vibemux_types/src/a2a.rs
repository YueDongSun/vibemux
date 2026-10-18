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
    use crate::{RunCompletionAuthority, RunSpec, TaskSpec};

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

    fn at(unix_seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(unix_seconds).expect("timestamp")
    }

    fn task() -> Task {
        Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "a2a fixture".to_string(),
            description: String::new(),
        })
        .expect("task")
    }

    fn run_for(task: &Task, role: &str) -> Run {
        Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: role.to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run")
    }

    fn start() -> A2aRunStart {
        let task = task();
        A2aRunStart {
            run: run_for(&task, "worker"),
            task,
            peer_id: "peer_1".to_string(),
            external_task_id: "external_1".to_string(),
            transport: "http_json".to_string(),
            protocol_version: "1.0".to_string(),
            workspace: workspace(),
            timestamp: at(1_800_000_000),
            idempotency_key: "start_1".to_string(),
        }
    }

    fn start_with(change: impl FnOnce(&mut A2aRunStart)) -> A2aRunStart {
        let mut start = start();
        change(&mut start);
        start
    }

    fn receipt() -> VerificationReceipt {
        VerificationReceipt {
            reviewer_run_id: RunId::new(),
            reviewer_expected_version: 1,
            artifact_sha256: "a".repeat(64),
            review_sha256: "b".repeat(64),
            verification_sha256: "c".repeat(64),
            accepted: false,
        }
    }

    fn update(action: A2aRunAction) -> A2aRunUpdate {
        A2aRunUpdate {
            run_id: RunId::new(),
            expected_version: 1,
            timestamp: at(1_800_000_000),
            idempotency_key: "update_1".to_string(),
            action,
        }
    }

    fn record() -> A2aRunRecord {
        let start = start();
        A2aRunRecord {
            schema_version: A2A_RECORD_SCHEMA_VERSION,
            binding: A2aBinding {
                project_id: start.run.project_id(),
                task_id: start.run.task_id(),
                run_id: start.run.run_id(),
                peer_id: start.peer_id,
                external_task_id: start.external_task_id,
                transport: start.transport,
                protocol_version: start.protocol_version,
                created_at: start.timestamp,
                updated_at: start.timestamp,
                idempotency_key: start.idempotency_key,
                last_remote_state: A2aRemoteState::Submitted,
                cancellation_state: A2aCancellationState::None,
                artifacts: Vec::new(),
                verification: None,
            },
            task: start.task,
            run: start.run,
            workspace: start.workspace,
            version: 1,
        }
    }

    fn record_with(change: impl FnOnce(&mut A2aRunRecord)) -> A2aRunRecord {
        let mut record = record();
        change(&mut record);
        record
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

    #[test]
    fn a_run_start_is_a_preparing_worker_reviewer_or_verifier_run_of_its_task() {
        for role in ["worker", "reviewer", "verifier"] {
            let valid = start_with(|s| s.run = run_for(&s.task, role));
            assert_eq!(valid.validate(), Ok(()), "{role}");
        }
        let other_task = task();
        for invalid in [
            start_with(|s| s.run = run_for(&s.task, "planner")),
            start_with(|s| {
                s.task = Task::new(TaskSpec {
                    project_id: s.task.project_id(),
                    title: "a2a fixture".to_string(),
                    description: "instructions travel in the message, not the task".to_string(),
                })
                .expect("task");
                s.run = run_for(&s.task, "worker");
            }),
            start_with(|s| {
                s.run = s
                    .run
                    .transitioned(RunStatus::Running, None)
                    .expect("running");
            }),
            start_with(|s| s.run = run_for(&other_task, "worker")),
            start_with(|s| s.workspace.base_commit = "b".repeat(40)),
            start_with(|s| s.workspace.path = "repo".to_string()),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn binding_fields_name_a_known_transport_and_bounded_identifiers() {
        for valid in [
            start_with(|s| s.transport = "json_rpc".to_string()),
            start_with(|s| s.transport = "grpc".to_string()),
            start_with(|s| s.peer_id = "p".repeat(128)),
            start_with(|s| s.external_task_id = "e".repeat(256)),
            start_with(|s| s.protocol_version = "1".repeat(32)),
            start_with(|s| s.idempotency_key = "k".repeat(256)),
        ] {
            assert_eq!(valid.validate(), Ok(()), "{valid:?}");
        }
        for invalid in [
            start_with(|s| s.transport = "http".to_string()),
            start_with(|s| s.transport = "HTTP_JSON".to_string()),
            start_with(|s| s.transport = "grpc ".to_string()),
            start_with(|s| s.peer_id = "p".repeat(129)),
            start_with(|s| s.peer_id = "peer 1".to_string()),
            start_with(|s| s.peer_id = String::new()),
            start_with(|s| s.external_task_id = "e".repeat(257)),
            start_with(|s| s.protocol_version = "1".repeat(33)),
            start_with(|s| s.idempotency_key = "k".repeat(257)),
            start_with(|s| s.idempotency_key = "start?1".to_string()),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn timestamps_must_be_utc_and_not_before_1970() {
        let east = UtcOffset::from_hms(8, 0, 0).expect("offset");
        let latest = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(9999, time::Month::December, 31).expect("date"),
            time::Time::from_hms(23, 59, 59).expect("time"),
        )
        .assume_utc();
        for valid in [at(0), latest] {
            assert_eq!(start_with(|s| s.timestamp = valid).validate(), Ok(()));
            let mut later = update(A2aRunAction::Start);
            later.timestamp = valid;
            assert_eq!(later.validate(), Ok(()));
        }
        // The same instant written with a non-UTC offset is still refused.
        for invalid in [at(-1), at(1_800_000_000).to_offset(east)] {
            assert_eq!(
                start_with(|s| s.timestamp = invalid).validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid}"
            );
            let mut later = update(A2aRunAction::Start);
            later.timestamp = invalid;
            assert_eq!(later.validate(), Err(A2aStateError::InvalidInput));
        }
    }

    #[test]
    fn a_verification_receipt_needs_a_reviewer_version_and_three_digests() {
        assert_eq!(receipt().validate(), Ok(()));
        let with = |change: fn(&mut VerificationReceipt)| {
            let mut receipt = receipt();
            change(&mut receipt);
            receipt
        };
        assert_eq!(with(|r| r.accepted = true).validate(), Ok(()));
        for invalid in [
            with(|r| r.reviewer_expected_version = 0),
            with(|r| r.artifact_sha256 = "A".repeat(64)),
            with(|r| r.review_sha256 = "b".repeat(63)),
            with(|r| r.verification_sha256 = "g".repeat(64)),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn a_run_update_checks_its_version_key_and_action() {
        for valid in [
            A2aRunAction::Start,
            A2aRunAction::Observe {
                state: A2aRemoteState::Working,
                artifacts: vec![artifact(1), artifact(2)],
            },
            A2aRunAction::RequestCancellation,
            A2aRunAction::ConfirmCancellation,
            A2aRunAction::FinalizeCancellation,
            A2aRunAction::Fail {
                code: "remote_failed".to_string(),
            },
            A2aRunAction::Verify { receipt: receipt() },
        ] {
            assert_eq!(update(valid.clone()).validate(), Ok(()), "{valid:?}");
        }
        let repeated = artifact(3);
        let mut unversioned = update(A2aRunAction::Start);
        unversioned.expected_version = 0;
        let mut unkeyed = update(A2aRunAction::Start);
        unkeyed.idempotency_key = String::new();
        for invalid in [
            unversioned,
            unkeyed,
            update(A2aRunAction::Observe {
                state: A2aRemoteState::Working,
                artifacts: vec![repeated.clone(), repeated],
            }),
            update(A2aRunAction::Fail {
                code: "remote failed".to_string(),
            }),
            update(A2aRunAction::Fail {
                code: String::new(),
            }),
            update(A2aRunAction::Verify {
                receipt: VerificationReceipt {
                    reviewer_expected_version: 0,
                    ..receipt()
                },
            }),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn a_run_record_binds_one_task_run_and_workspace() {
        assert_eq!(record().validate(), Ok(()));
        let other_task = task();
        for mismatched in [
            record_with(|r| r.schema_version = A2A_RECORD_SCHEMA_VERSION + 1),
            record_with(|r| r.version = 0),
            record_with(|r| r.task = other_task.clone()),
            record_with(|r| r.binding.project_id = ProjectId::new()),
            record_with(|r| r.binding.task_id = TaskId::new()),
            record_with(|r| r.binding.run_id = RunId::new()),
            record_with(|r| r.workspace.base_commit = "b".repeat(40)),
        ] {
            assert_eq!(
                mismatched.validate(),
                Err(A2aStateError::IdentityMismatch),
                "{mismatched:?}"
            );
        }
        let east = UtcOffset::from_hms(8, 0, 0).expect("offset");
        for invalid in [
            record_with(|r| r.binding.transport = "http".to_string()),
            record_with(|r| r.binding.idempotency_key = String::new()),
            record_with(|r| r.binding.created_at = r.binding.created_at.to_offset(east)),
            record_with(|r| r.binding.updated_at = at(1_799_999_999)),
            record_with(|r| r.workspace.branch = "-run".to_string()),
            record_with(|r| {
                r.binding.artifacts = vec![ArtifactReference {
                    sha256: "F".repeat(64),
                    ..artifact(4)
                }];
            }),
            record_with(|r| {
                r.binding.verification = Some(VerificationReceipt {
                    reviewer_expected_version: 0,
                    ..receipt()
                });
            }),
        ] {
            assert_eq!(
                invalid.validate(),
                Err(A2aStateError::InvalidInput),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn a_succeeded_run_record_carries_a_verification_receipt() {
        let succeeded = |run: &Run| {
            run.transitioned(RunStatus::Running, None)
                .and_then(|running| {
                    running
                        .transitioned(RunStatus::Succeeded, Some(RunCompletionAuthority::Verifier))
                })
                .expect("succeeded")
        };
        let unverified = record_with(|r| r.run = succeeded(&r.run));
        assert_eq!(unverified.validate(), Err(A2aStateError::InvalidState));
        let verified = record_with(|r| {
            r.run = succeeded(&r.run);
            r.binding.verification = Some(receipt());
        });
        assert_eq!(verified.validate(), Ok(()));
    }

    #[test]
    fn a_record_that_fails_validation_does_not_decode() {
        let valid = record();
        let encoded = serde_json::to_value(&valid).expect("encode");
        let decoded: A2aRunRecord = serde_json::from_value(encoded.clone()).expect("decode");
        assert_eq!(decoded, valid);
        for (pointer, value) in [
            ("/version", serde_json::json!(0)),
            ("/binding/transport", serde_json::json!("http")),
            (
                "/binding/created_at",
                serde_json::json!("2027-01-15T08:00:00+08:00"),
            ),
        ] {
            let mut tampered = encoded.clone();
            *tampered.pointer_mut(pointer).expect(pointer) = value;
            assert!(
                serde_json::from_value::<A2aRunRecord>(tampered).is_err(),
                "{pointer}"
            );
        }
    }
}
