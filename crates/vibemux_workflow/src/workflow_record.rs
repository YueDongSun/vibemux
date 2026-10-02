//! The workflow aggregate: the persisted record the single writer owns and
//! the pure transition rules it applies (ADR 031 §1).
//!
//! A workflow references its admitted contracts by identity and its child
//! attempts by dispatch request ID; it never takes ownership of a
//! dispatch-owned or A2A-owned Task/Run. Its acceptance is derived from
//! verified child receipts, never from a child's optimistic success label.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    gates::WorkflowPhase,
    slots::EvidenceClass,
    supervisor::LoopBudget,
    task_spec::WorkflowMode,
};

pub const WORKFLOW_RECORD_SCHEMA_VERSION: u32 = 1;
pub const WORKFLOW_FINGERPRINT_DOMAIN: &str = "vibemux.workflow.admission.v1";
pub const MAX_WORKFLOW_TASKS: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskProgress {
    Pending,
    Running,
    CandidateCollected,
    Accepted,
    Failed,
    Blocked,
}

impl TaskProgress {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::CandidateCollected => "candidate_collected",
            Self::Accepted => "accepted",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTask {
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub required_suites: Vec<SpecIdentifier>,
    pub progress: TaskProgress,
    pub turns_used: u32,
    pub repairs_used: u32,
    pub max_turns: u32,
    pub max_repairs: u32,
    pub accepted_candidate: Option<Sha256Digest>,
    pub failure_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ContentStoreMode {
    /// Metadata only; replay of prompts and bundles is not possible.
    Disabled,
    Enabled {
        retention_days: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRecord {
    pub schema_version: u32,
    pub workflow_id: Uuid,
    pub request_key: String,
    pub workflow_key: SpecIdentifier,
    pub mode: WorkflowMode,
    pub phase: WorkflowPhase,
    pub version: u64,
    pub evidence_class: EvidenceClass,
    pub base_commit: String,
    pub policy_digest: Sha256Digest,
    pub verifier_digest: Sha256Digest,
    pub integration_suites: Vec<SpecIdentifier>,
    pub tasks: Vec<WorkflowTask>,
    pub budget: LoopBudget,
    pub content_store: ContentStoreMode,
    pub blocked_reason: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRecordError {
    #[error("workflow record is invalid")]
    Invalid,
    #[error("workflow phase transition is not allowed")]
    InvalidTransition,
    #[error("task is not part of this workflow")]
    UnknownTask,
    #[error("task budget is exhausted")]
    TaskBudgetExhausted,
}

impl WorkflowRecordError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "workflow_record_invalid",
            Self::InvalidTransition => "workflow_invalid_transition",
            Self::UnknownTask => "workflow_unknown_task",
            Self::TaskBudgetExhausted => "workflow_task_budget_exhausted",
        }
    }
}

impl WorkflowPhase {
    /// Allowed phase edges. Accepted is reachable only from integrating,
    /// and only after the acceptance gate passes inside the writer.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        use WorkflowPhase::{
            Accepted, Blocked, CancelRequested, Cancelled, Failed, Integrating, Paused, Prepared,
            Running,
        };
        matches!(
            (self, target),
            (Prepared, Running | Cancelled | Blocked)
                | (
                    Running,
                    Paused | Integrating | Failed | CancelRequested | Blocked
                )
                | (Paused, Running | CancelRequested)
                | (Blocked, Running | CancelRequested | Failed)
                | (Integrating, Accepted | Failed | CancelRequested | Running)
                | (CancelRequested, Cancelled)
        )
    }
}

impl WorkflowRecord {
    pub fn validate(&self) -> Result<(), WorkflowRecordError> {
        let mut keys: Vec<&SpecIdentifier> = self.tasks.iter().map(|task| &task.task_key).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        let valid = self.schema_version == WORKFLOW_RECORD_SCHEMA_VERSION
            && !self.workflow_id.is_nil()
            && self.version > 0
            && !self.request_key.is_empty()
            && self.request_key.len() <= 128
            && self
                .request_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            && crate::task_spec::is_full_commit(&self.base_commit)
            && !self.tasks.is_empty()
            && self.tasks.len() <= MAX_WORKFLOW_TASKS
            && keys.len() == count
            && self.tasks.iter().all(|task| {
                task.turns_used <= task.max_turns && task.repairs_used <= task.max_repairs
            });
        if valid {
            Ok(())
        } else {
            Err(WorkflowRecordError::Invalid)
        }
    }

    /// The identity of an admission request, independent of the generated
    /// workflow ID and timestamps, so a retried request is recognized.
    pub fn admission_fingerprint(&self) -> Result<Sha256Digest, CanonicalError> {
        let contracts: Vec<(&str, Sha256Digest)> = self
            .tasks
            .iter()
            .map(|task| (task.task_key.as_str(), task.contract_id))
            .collect();
        canonical_digest(
            WORKFLOW_FINGERPRINT_DOMAIN,
            &serde_json::json!({
                "request_key": self.request_key,
                "workflow_key": self.workflow_key,
                "mode": self.mode,
                "evidence_class": self.evidence_class,
                "base_commit": self.base_commit,
                "policy_digest": self.policy_digest,
                "verifier_digest": self.verifier_digest,
                "integration_suites": self.integration_suites,
                "contracts": contracts,
            }),
        )
    }

    #[must_use]
    pub fn task(&self, task_key: &SpecIdentifier) -> Option<&WorkflowTask> {
        self.tasks.iter().find(|task| &task.task_key == task_key)
    }

    pub fn task_mut(
        &mut self,
        task_key: &SpecIdentifier,
    ) -> Result<&mut WorkflowTask, WorkflowRecordError> {
        self.tasks
            .iter_mut()
            .find(|task| &task.task_key == task_key)
            .ok_or(WorkflowRecordError::UnknownTask)
    }

    /// The next record version with `phase`, or why the edge is refused.
    pub fn with_phase(
        &self,
        phase: WorkflowPhase,
        now_ms: u64,
    ) -> Result<Self, WorkflowRecordError> {
        if !self.phase.can_transition_to(phase) {
            return Err(WorkflowRecordError::InvalidTransition);
        }
        let mut next = self.clone();
        next.phase = phase;
        next.version = self
            .version
            .checked_add(1)
            .ok_or(WorkflowRecordError::Invalid)?;
        next.updated_at_ms = now_ms.max(self.updated_at_ms);
        Ok(next)
    }

    /// Charges one turn (and one repair when `repair`) to a task.
    pub fn charge_turn(
        &mut self,
        task_key: &SpecIdentifier,
        repair: bool,
    ) -> Result<(), WorkflowRecordError> {
        let task = self.task_mut(task_key)?;
        if task.turns_used >= task.max_turns || (repair && task.repairs_used >= task.max_repairs) {
            return Err(WorkflowRecordError::TaskBudgetExhausted);
        }
        task.turns_used += 1;
        if repair {
            task.repairs_used += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::supervisor::LoopBudget;

    pub fn task(key: &str) -> WorkflowTask {
        WorkflowTask {
            task_key: SpecIdentifier::new(key).expect("task key"),
            contract_id: Sha256Digest::of(key.as_bytes()),
            contract_version: 1,
            required_suites: vec![SpecIdentifier::new("api").expect("suite")],
            progress: TaskProgress::Pending,
            turns_used: 0,
            repairs_used: 0,
            max_turns: 4,
            max_repairs: 1,
            accepted_candidate: None,
            failure_code: None,
        }
    }

    pub fn record() -> WorkflowRecord {
        WorkflowRecord {
            schema_version: WORKFLOW_RECORD_SCHEMA_VERSION,
            workflow_id: Uuid::from_u128(9),
            request_key: "taskboard_cooperate_1".into(),
            workflow_key: SpecIdentifier::new("taskboard_lite").expect("key"),
            mode: WorkflowMode::Cooperate,
            phase: WorkflowPhase::Prepared,
            version: 1,
            evidence_class: EvidenceClass::Fixture,
            base_commit: "a".repeat(40),
            policy_digest: Sha256Digest::of(b"policy"),
            verifier_digest: Sha256Digest::of(b"verifier"),
            integration_suites: vec![SpecIdentifier::new("api").expect("suite")],
            tasks: vec![task("track_a"), task("track_b")],
            budget: LoopBudget {
                decisions_remaining: 16,
                malformed_repairs_remaining: 2,
                model_requests_remaining: 40,
                deadline_unix_ms: u64::MAX,
            },
            content_store: ContentStoreMode::Enabled { retention_days: 7 },
            blocked_reason: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn acceptance_is_only_reachable_from_integration_and_terminal_phases_stay_terminal() {
        use WorkflowPhase::*;
        assert!(Integrating.can_transition_to(Accepted));
        for phase in [
            Prepared,
            Running,
            Paused,
            Blocked,
            CancelRequested,
            Cancelled,
            Failed,
            Accepted,
        ] {
            assert!(!phase.can_transition_to(Accepted), "{phase:?}");
        }
        for terminal in [Accepted, Failed, Cancelled] {
            for target in [
                Prepared,
                Running,
                Paused,
                Integrating,
                Accepted,
                Failed,
                CancelRequested,
                Cancelled,
                Blocked,
            ] {
                assert!(!terminal.can_transition_to(target));
            }
        }
        assert!(CancelRequested.can_transition_to(Cancelled));
        assert!(!CancelRequested.can_transition_to(Running));
    }

    #[test]
    fn transitions_bump_versions_and_never_move_time_backwards() {
        let record = record();
        let running = record.with_phase(WorkflowPhase::Running, 0).expect("start");
        assert_eq!(running.version, 2);
        assert_eq!(running.updated_at_ms, 1);
        assert_eq!(
            running.with_phase(WorkflowPhase::Accepted, 5),
            Err(WorkflowRecordError::InvalidTransition)
        );
    }

    #[test]
    fn the_fingerprint_ignores_generated_identity_and_time() {
        let first = record();
        let mut retried = record();
        retried.workflow_id = Uuid::from_u128(10);
        retried.created_at_ms = 99;
        assert_eq!(
            first.admission_fingerprint().expect("digest"),
            retried.admission_fingerprint().expect("digest")
        );
        let mut changed = record();
        changed.tasks[0].contract_id = Sha256Digest::of(b"other contract");
        assert_ne!(
            first.admission_fingerprint().expect("digest"),
            changed.admission_fingerprint().expect("digest")
        );
    }

    #[test]
    fn turn_and_repair_budgets_are_enforced() {
        let mut record = record();
        let key = SpecIdentifier::new("track_a").expect("key");
        record.charge_turn(&key, true).expect("first repair");
        assert_eq!(
            record.charge_turn(&key, true),
            Err(WorkflowRecordError::TaskBudgetExhausted)
        );
        for _ in 0..3 {
            record.charge_turn(&key, false).expect("turn");
        }
        assert_eq!(
            record.charge_turn(&key, false),
            Err(WorkflowRecordError::TaskBudgetExhausted)
        );
        assert_eq!(
            record.charge_turn(&SpecIdentifier::new("track_z").expect("key"), false),
            Err(WorkflowRecordError::UnknownTask)
        );
        record.validate().expect("still valid");
    }

    #[test]
    fn invalid_records_are_refused() {
        let mut duplicate = record();
        duplicate.tasks.push(task("track_a"));
        assert_eq!(duplicate.validate(), Err(WorkflowRecordError::Invalid));
        let mut bad_key = record();
        bad_key.request_key = "has space".into();
        assert_eq!(bad_key.validate(), Err(WorkflowRecordError::Invalid));
    }
}
