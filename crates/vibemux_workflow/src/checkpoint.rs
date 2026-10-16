//! Worker checkpoint and reviewer report parsing (ADR 031 §5).
//!
//! A worker ends its turn with one fenced `vibemux_checkpoint` JSON block;
//! a reviewer with one `vibemux_review` block. Both are claims: they route
//! messages and record what the session said, but they never mark anything
//! accepted. Only the last block with the exact info string counts, the
//! JSON is strict, and every list and string is bounded.
//!
//! `RecoveryCheckpoint` is a separate authoritative daemon record. It refers to
//! committed state and reconciled resources; a worker report cannot create one.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    gates::{ReviewVerdict, WorkflowPhase},
    leases::LeaseState,
    messages::MessageKind,
    native_controller::{ApprovalChallenge, ControllerOwner, ControllerPhase},
    renderer::{CHECKPOINT_INFO_STRING, REVIEW_INFO_STRING},
    supervisor::LoopBudget,
};

pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub const MAX_REPORT_BYTES: usize = 32 * 1024;
pub const MAX_REPORT_ITEMS: usize = 32;
pub const MAX_REPORT_TEXT_BYTES: usize = 4096;
pub const RECOVERY_CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub const MAX_RECOVERY_CHECKPOINT_BYTES: usize = 256 * 1024;
pub const MAX_RECOVERY_REFERENCES: usize = 512;
const RECOVERY_CHECKPOINT_DOMAIN: &str = "vibemux.workflow.recovery_checkpoint.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryContractRef {
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub template_digest: Sha256Digest,
    pub depends_on: Vec<SpecIdentifier>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryLeaseRef {
    pub lease_id: Uuid,
    pub task_key: SpecIdentifier,
    pub generation: u64,
    pub state: LeaseState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryControllerRef {
    pub controller_id: Uuid,
    pub session_id: Uuid,
    pub task_key: SpecIdentifier,
    pub generation: u64,
    pub version: u64,
    pub owner: ControllerOwner,
    pub phase: ControllerPhase,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAttemptState {
    Reserved,
    Submitted,
    Running,
    Unknown,
    Settled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAttemptRef {
    pub request_id: Uuid,
    pub submission_id: Uuid,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub controller_id: Uuid,
    pub controller_generation: u64,
    pub state: RecoveryAttemptState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryApprovalRef {
    pub task_key: SpecIdentifier,
    pub challenge: ApprovalChallenge,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryMessageCursor {
    pub participant_id: Uuid,
    /// None denotes a supervisor cursor affecting the whole workflow.
    pub task_key: Option<SpecIdentifier>,
    pub inbox_sequence: u64,
    pub outbox_sequence: u64,
    pub acknowledged_outbox_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryEffectRef {
    pub effect_id: Uuid,
    /// Unknown/global effects block every dependent admission.
    pub task_key: Option<SpecIdentifier>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTaskBudget {
    pub task_key: SpecIdentifier,
    pub turns_used: u32,
    pub max_turns: u32,
    pub repairs_used: u32,
    pub max_repairs: u32,
    pub model_requests_used: u32,
    pub max_model_requests: u32,
}

/// Configuration observations only. Retrieval/output budgets never modify these
/// fields. Unknown configuration cannot establish preservation on resume.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSettingsIdentity {
    pub configured_context_tokens: Option<u64>,
    pub compaction_settings_digest: Option<Sha256Digest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum RecoveryAction {
    Inspect,
    Reconcile,
    ObserveAttempt { request_id: Uuid },
    ContinueTask { task_key: SpecIdentifier },
}

/// The writer builds this complete projection in one transaction. Reference
/// changes require a fresh checkpoint; observations never rewrite its identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryState {
    pub workflow_id: Uuid,
    pub workflow_version: u64,
    pub workflow_phase: WorkflowPhase,
    pub plan_generation: u64,
    pub plan_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub committed_event_cursor: u64,
    pub contracts: Vec<RecoveryContractRef>,
    pub attempts: Vec<RecoveryAttemptRef>,
    pub leases: Vec<RecoveryLeaseRef>,
    pub controllers: Vec<RecoveryControllerRef>,
    pub pending_approvals: Vec<RecoveryApprovalRef>,
    pub message_cursors: Vec<RecoveryMessageCursor>,
    pub artifact_digests: Vec<Sha256Digest>,
    pub verifier_receipt_ids: Vec<Uuid>,
    pub unresolved_effects: Vec<RecoveryEffectRef>,
    pub loop_budget: LoopBudget,
    pub task_budgets: Vec<RecoveryTaskBudget>,
    pub context_settings: ContextSettingsIdentity,
    /// None is the default: automatic continuation has no authority.
    pub preapproved_resume_policy: Option<Sha256Digest>,
    pub next_action: RecoveryAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCheckpoint {
    pub schema_version: u32,
    pub checkpoint_id: Uuid,
    pub state: RecoveryState,
    pub state_digest: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RecoveryError {
    #[error("recovery checkpoint is malformed or exceeds its bounds")]
    Invalid,
    #[error("recovery checkpoint digest does not match its contents")]
    DigestMismatch,
    #[error("recovery checkpoint has a missing or conflicting reference")]
    ReferenceMismatch,
    #[error("committed state changed; refresh the checkpoint after reconciliation")]
    StateChanged,
    #[error("recovery would reset consumed budget or extend a deadline")]
    BudgetReset,
    #[error("configured model context or compaction settings changed")]
    ContextSettingsChanged,
    #[error("reconciliation observations are duplicated, foreign, or stale")]
    InvalidObservation,
}

impl RecoveryError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "recovery_checkpoint_invalid",
            Self::DigestMismatch => "recovery_checkpoint_digest_mismatch",
            Self::ReferenceMismatch => "recovery_checkpoint_reference_mismatch",
            Self::StateChanged => "recovery_checkpoint_state_changed",
            Self::BudgetReset => "recovery_checkpoint_budget_reset",
            Self::ContextSettingsChanged => "recovery_context_settings_changed",
            Self::InvalidObservation => "recovery_observation_invalid",
        }
    }
}

impl RecoveryState {
    pub fn validate(&self) -> Result<(), RecoveryError> {
        let counts = [
            self.contracts.len(),
            self.attempts.len(),
            self.leases.len(),
            self.controllers.len(),
            self.pending_approvals.len(),
            self.message_cursors.len(),
            self.artifact_digests.len(),
            self.verifier_receipt_ids.len(),
            self.unresolved_effects.len(),
            self.task_budgets.len(),
        ];
        if self.workflow_id.is_nil()
            || self.workflow_version == 0
            || self.plan_generation == 0
            || self.committed_event_cursor == 0
            || self.contracts.is_empty()
            || self.loop_budget.deadline_unix_ms == 0
            || self.context_settings.configured_context_tokens == Some(0)
            || counts
                .into_iter()
                .any(|count| count > MAX_RECOVERY_REFERENCES)
        {
            return Err(RecoveryError::Invalid);
        }
        let tasks: BTreeMap<_, _> = self
            .contracts
            .iter()
            .map(|value| (&value.task_key, value))
            .collect();
        // Bound total graph work as well as the number of individual references.
        if self
            .contracts
            .iter()
            .try_fold(0_usize, |count, contract| {
                count.checked_add(contract.depends_on.len())
            })
            .is_none_or(|count| count > MAX_RECOVERY_REFERENCES)
        {
            return Err(RecoveryError::Invalid);
        }
        if self.contracts.iter().any(|value| {
            value.depends_on.len() > MAX_RECOVERY_REFERENCES
                || !unique(value.depends_on.iter())
                || value
                    .depends_on
                    .iter()
                    .any(|key| key == &value.task_key || !tasks.contains_key(key))
        }) {
            return Err(RecoveryError::ReferenceMismatch);
        }
        for contract in &self.contracts {
            if contract
                .depends_on
                .iter()
                .any(|key| dependency_scope(&self.contracts, key).contains(&contract.task_key))
            {
                return Err(RecoveryError::ReferenceMismatch);
            }
        }
        if tasks.len() != self.contracts.len()
            || self
                .contracts
                .iter()
                .any(|value| value.contract_version == 0)
            || !unique(self.contracts.iter().map(|value| value.contract_id))
            || !unique(self.artifact_digests.iter().copied())
            || !valid_ids(self.verifier_receipt_ids.iter().copied())
            || !valid_ids(self.leases.iter().map(|value| value.lease_id))
            || !valid_ids(self.controllers.iter().map(|value| value.controller_id))
            || !valid_ids(self.controllers.iter().map(|value| value.session_id))
            || !valid_ids(self.attempts.iter().map(|value| value.request_id))
            || !valid_ids(self.attempts.iter().map(|value| value.submission_id))
            || !valid_ids(
                self.pending_approvals
                    .iter()
                    .map(|value| value.challenge.challenge_id),
            )
            || !valid_ids(
                self.message_cursors
                    .iter()
                    .map(|value| value.participant_id),
            )
            || !valid_ids(self.unresolved_effects.iter().map(|value| value.effect_id))
        {
            return Err(RecoveryError::ReferenceMismatch);
        }
        let leases: BTreeMap<_, _> = self
            .leases
            .iter()
            .map(|value| (value.lease_id, value))
            .collect();
        let controllers: BTreeMap<_, _> = self
            .controllers
            .iter()
            .map(|value| (value.controller_id, value))
            .collect();
        if self.leases.iter().any(|value| value.generation == 0 || !tasks.contains_key(&value.task_key))
            || self.controllers.iter().any(|value| value.generation == 0 || value.version == 0 || !tasks.contains_key(&value.task_key) || matches!(value.owner, ControllerOwner::Human { operator_id } if operator_id.is_nil()))
        { return Err(RecoveryError::ReferenceMismatch); }
        for attempt in &self.attempts {
            let Some(contract) = tasks.get(&attempt.task_key) else {
                return Err(RecoveryError::ReferenceMismatch);
            };
            let Some(lease) = leases.get(&attempt.lease_id) else {
                return Err(RecoveryError::ReferenceMismatch);
            };
            let Some(controller) = controllers.get(&attempt.controller_id) else {
                return Err(RecoveryError::ReferenceMismatch);
            };
            if contract.contract_id != attempt.contract_id
                || lease.task_key != attempt.task_key
                || lease.generation != attempt.lease_generation
                || controller.task_key != attempt.task_key
                || controller.generation != attempt.controller_generation
                || (attempt.state != RecoveryAttemptState::Settled
                    && !lease.state.holds_reservation())
            {
                return Err(RecoveryError::ReferenceMismatch);
            }
        }
        for approval in &self.pending_approvals {
            let challenge = approval.challenge;
            let controller = controllers
                .get(&challenge.controller.controller_id)
                .ok_or(RecoveryError::ReferenceMismatch)?;
            if challenge.requester_id.is_nil()
                || controller.task_key != approval.task_key
                || controller.generation != challenge.controller.generation
                || controller.session_id != challenge.session_id
                || !self.attempts.iter().any(|attempt| {
                    attempt.submission_id == challenge.submission_id
                        && attempt.controller_id == controller.controller_id
                        && attempt.task_key == approval.task_key
                        && matches!(
                            attempt.state,
                            RecoveryAttemptState::Submitted
                                | RecoveryAttemptState::Running
                                | RecoveryAttemptState::Unknown
                        )
                })
            {
                return Err(RecoveryError::ReferenceMismatch);
            }
        }
        if self.message_cursors.iter().any(|cursor| {
            cursor.acknowledged_outbox_sequence > cursor.outbox_sequence
                || cursor
                    .task_key
                    .as_ref()
                    .is_some_and(|key| !tasks.contains_key(key))
        }) || self.unresolved_effects.iter().any(|effect| {
            effect
                .task_key
                .as_ref()
                .is_some_and(|key| !tasks.contains_key(key))
        }) || self.task_budgets.len() != self.contracts.len()
            || !unique(
                self.task_budgets
                    .iter()
                    .map(|budget| budget.task_key.clone()),
            )
            || self.task_budgets.iter().any(|budget| {
                !tasks.contains_key(&budget.task_key)
                    || budget.turns_used > budget.max_turns
                    || budget.repairs_used > budget.max_repairs
                    || budget.model_requests_used > budget.max_model_requests
            })
        {
            return Err(RecoveryError::ReferenceMismatch);
        }
        match &self.next_action {
            RecoveryAction::ContinueTask { task_key } if !tasks.contains_key(task_key) => {
                Err(RecoveryError::ReferenceMismatch)
            }
            RecoveryAction::ObserveAttempt { request_id }
                if !self
                    .attempts
                    .iter()
                    .any(|attempt| attempt.request_id == *request_id) =>
            {
                Err(RecoveryError::ReferenceMismatch)
            }
            _ => Ok(()),
        }
    }

    fn normalize(&mut self) {
        for contract in &mut self.contracts {
            contract.depends_on.sort();
        }
        self.contracts
            .sort_by(|left, right| left.task_key.cmp(&right.task_key));
        self.attempts.sort_by_key(|value| value.request_id);
        self.leases.sort_by_key(|value| value.lease_id);
        self.controllers.sort_by_key(|value| value.controller_id);
        self.pending_approvals
            .sort_by_key(|value| value.challenge.challenge_id);
        self.message_cursors
            .sort_by_key(|value| value.participant_id);
        self.artifact_digests.sort();
        self.verifier_receipt_ids.sort();
        self.unresolved_effects.sort_by_key(|value| value.effect_id);
        self.task_budgets
            .sort_by(|left, right| left.task_key.cmp(&right.task_key));
    }
}

fn unique<T: Ord>(values: impl IntoIterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

fn valid_ids(values: impl IntoIterator<Item = Uuid>) -> bool {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .all(|value| !value.is_nil() && seen.insert(value))
}

impl RecoveryCheckpoint {
    pub fn new(checkpoint_id: Uuid, mut state: RecoveryState) -> Result<Self, RecoveryError> {
        state.validate()?;
        state.normalize();
        let state_digest = canonical_digest(RECOVERY_CHECKPOINT_DOMAIN, &state)
            .map_err(|_| RecoveryError::Invalid)?;
        let checkpoint = Self {
            schema_version: RECOVERY_CHECKPOINT_SCHEMA_VERSION,
            checkpoint_id,
            state,
            state_digest,
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RecoveryError> {
        if bytes.len() > MAX_RECOVERY_CHECKPOINT_BYTES {
            return Err(RecoveryError::Invalid);
        }
        let checkpoint: Self = serde_json::from_slice(bytes).map_err(|_| RecoveryError::Invalid)?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn validate(&self) -> Result<(), RecoveryError> {
        if self.schema_version != RECOVERY_CHECKPOINT_SCHEMA_VERSION || self.checkpoint_id.is_nil()
        {
            return Err(RecoveryError::Invalid);
        }
        self.state.validate()?;
        if serde_json::to_vec(self)
            .map_err(|_| RecoveryError::Invalid)?
            .len()
            > MAX_RECOVERY_CHECKPOINT_BYTES
        {
            return Err(RecoveryError::Invalid);
        }
        let mut normalized = self.state.clone();
        normalized.normalize();
        if normalized != self.state {
            return Err(RecoveryError::Invalid);
        }
        if canonical_digest(RECOVERY_CHECKPOINT_DOMAIN, &self.state)
            .map_err(|_| RecoveryError::Invalid)?
            != self.state_digest
        {
            return Err(RecoveryError::DigestMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryPresence {
    Present,
    Terminal,
    Absent,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryObservation {
    pub resource_id: Uuid,
    pub generation: u64,
    pub presence: RecoveryPresence,
}

/// Observations must come from owned adapter/process identities and be made
/// against this exact committed workflow version. Missing observations are unknown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryObservations {
    pub workflow_version: u64,
    pub controllers: Vec<RecoveryObservation>,
    pub leases: Vec<RecoveryObservation>,
}

/// Constructed only after the control boundary authenticates the operator or
/// matches a previously admitted resume policy. Model/worker text is not input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeAuthorization {
    None,
    OperatorStart {
        checkpoint_id: Uuid,
        workflow_version: u64,
    },
    Preapproved {
        policy_digest: Sha256Digest,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeDecision {
    ReadOnly(RecoveryAction),
    RequiresOperatorStart,
    WorkflowStopped,
    ContextSettingsUnknown,
    BudgetExhausted,
    AwaitingApproval {
        challenge_ids: Vec<Uuid>,
    },
    HumanControlled {
        controller_ids: Vec<Uuid>,
    },
    Reconcile {
        attempt_ids: Vec<Uuid>,
        controller_ids: Vec<Uuid>,
        lease_ids: Vec<Uuid>,
        effect_ids: Vec<Uuid>,
        outbox_participants: Vec<Uuid>,
    },
    /// These are the remaining committed counters, never refreshed allowances.
    Ready {
        task_key: SpecIdentifier,
        loop_budget: LoopBudget,
        task_budget: RecoveryTaskBudget,
    },
}

/// Assesses the checkpoint against a complete current writer projection. Changed
/// reference sets need a refreshed checkpoint after reconciliation, while newer
/// event cursors and consumed budget are accepted monotonically. A Ready result
/// authorizes only consideration of the named task; normal admission still fences
/// its current contract, lease, controller, scope, and idempotency key.
pub fn assess_resume(
    checkpoint: &RecoveryCheckpoint,
    current: &RecoveryState,
    observations: &RecoveryObservations,
    authorization: ResumeAuthorization,
    now_ms: u64,
) -> Result<ResumeDecision, RecoveryError> {
    checkpoint.validate()?;
    current.validate()?;
    let mut current = current.clone();
    current.normalize();
    check_recovery_continuity(&checkpoint.state, &current)?;
    check_observations(&current, observations)?;
    if current.workflow_phase.is_terminal()
        || current.workflow_phase == WorkflowPhase::CancelRequested
    {
        return Ok(ResumeDecision::WorkflowStopped);
    }
    let RecoveryAction::ContinueTask { task_key } = &current.next_action else {
        return Ok(ResumeDecision::ReadOnly(current.next_action));
    };
    if current.context_settings.configured_context_tokens.is_none()
        || current
            .context_settings
            .compaction_settings_digest
            .is_none()
    {
        return Ok(ResumeDecision::ContextSettingsUnknown);
    }
    let dependency_tasks = dependency_scope(&current.contracts, task_key);
    let relevant =
        |scope: Option<&SpecIdentifier>| scope.is_none_or(|scope| dependency_tasks.contains(scope));
    let approvals: Vec<Uuid> = current
        .pending_approvals
        .iter()
        .filter(|value| relevant(Some(&value.task_key)))
        .map(|value| value.challenge.challenge_id)
        .collect();
    if !approvals.is_empty() {
        return Ok(ResumeDecision::AwaitingApproval {
            challenge_ids: approvals,
        });
    }
    let humans: Vec<Uuid> = current
        .controllers
        .iter()
        .filter(|value| {
            relevant(Some(&value.task_key))
                && matches!(value.owner, ControllerOwner::Human { .. })
                && value.phase != ControllerPhase::Closed
        })
        .map(|value| value.controller_id)
        .collect();
    if !humans.is_empty() {
        return Ok(ResumeDecision::HumanControlled {
            controller_ids: humans,
        });
    }
    let attempt_ids: Vec<Uuid> = current
        .attempts
        .iter()
        .filter(|value| {
            relevant(Some(&value.task_key)) && value.state != RecoveryAttemptState::Settled
        })
        .map(|value| value.request_id)
        .collect();
    let controller_ids: Vec<Uuid> = current
        .controllers
        .iter()
        .filter(|value| {
            relevant(Some(&value.task_key))
                && value.phase != ControllerPhase::Closed
                && (matches!(
                    value.phase,
                    ControllerPhase::Quiescing | ControllerPhase::AwaitingAttach
                ) || !observations.controllers.iter().any(|observation| {
                    observation.resource_id == value.controller_id
                        && observation.presence == RecoveryPresence::Present
                }))
        })
        .map(|value| value.controller_id)
        .collect();
    let lease_ids: Vec<Uuid> = current
        .leases
        .iter()
        .filter(|value| {
            relevant(Some(&value.task_key))
                && value.state.holds_reservation()
                && (value.state == LeaseState::Quarantined
                    || !observations.leases.iter().any(|observation| {
                        observation.resource_id == value.lease_id
                            && observation.presence == RecoveryPresence::Present
                    }))
        })
        .map(|value| value.lease_id)
        .collect();
    let effect_ids: Vec<Uuid> = current
        .unresolved_effects
        .iter()
        .filter(|value| relevant(value.task_key.as_ref()))
        .map(|value| value.effect_id)
        .collect();
    let outbox_participants: Vec<Uuid> = current
        .message_cursors
        .iter()
        .filter(|value| {
            relevant(value.task_key.as_ref())
                && value.outbox_sequence > value.acknowledged_outbox_sequence
        })
        .map(|value| value.participant_id)
        .collect();
    if !attempt_ids.is_empty()
        || !controller_ids.is_empty()
        || !lease_ids.is_empty()
        || !effect_ids.is_empty()
        || !outbox_participants.is_empty()
    {
        return Ok(ResumeDecision::Reconcile {
            attempt_ids,
            controller_ids,
            lease_ids,
            effect_ids,
            outbox_participants,
        });
    }
    let authorized = match authorization {
        ResumeAuthorization::None => false,
        ResumeAuthorization::OperatorStart {
            checkpoint_id,
            workflow_version,
        } => {
            checkpoint_id == checkpoint.checkpoint_id
                && workflow_version == current.workflow_version
        }
        ResumeAuthorization::Preapproved { policy_digest } => {
            current.preapproved_resume_policy == Some(policy_digest)
        }
    };
    if !authorized {
        return Ok(ResumeDecision::RequiresOperatorStart);
    }
    let task_budget = current
        .task_budgets
        .iter()
        .find(|value| &value.task_key == task_key)
        .ok_or(RecoveryError::ReferenceMismatch)?
        .clone();
    let budget = current.loop_budget;
    if now_ms >= budget.deadline_unix_ms
        || budget.decisions_remaining == 0
        || budget.model_requests_remaining == 0
        || task_budget.turns_used >= task_budget.max_turns
        || task_budget.model_requests_used >= task_budget.max_model_requests
    {
        return Ok(ResumeDecision::BudgetExhausted);
    }
    Ok(ResumeDecision::Ready {
        task_key: task_key.clone(),
        loop_budget: budget,
        task_budget,
    })
}

fn dependency_scope(
    contracts: &[RecoveryContractRef],
    task_key: &SpecIdentifier,
) -> BTreeSet<SpecIdentifier> {
    let mut pending = vec![task_key.clone()];
    let mut found = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if found.insert(key.clone())
            && let Some(contract) = contracts.iter().find(|contract| contract.task_key == key)
        {
            pending.extend(contract.depends_on.iter().cloned());
        }
    }
    found
}

fn check_recovery_continuity(
    saved: &RecoveryState,
    current: &RecoveryState,
) -> Result<(), RecoveryError> {
    if current.context_settings != saved.context_settings {
        return Err(RecoveryError::ContextSettingsChanged);
    }
    if (saved.workflow_phase.is_terminal() && current.workflow_phase != saved.workflow_phase)
        || (saved.workflow_phase == WorkflowPhase::CancelRequested
            && !matches!(
                current.workflow_phase,
                WorkflowPhase::CancelRequested | WorkflowPhase::Cancelled
            ))
    {
        return Err(RecoveryError::StateChanged);
    }
    let mut expected = saved.clone();
    if current.workflow_version < saved.workflow_version
        || current.committed_event_cursor < saved.committed_event_cursor
    {
        return Err(RecoveryError::StateChanged);
    }
    let before = saved.loop_budget;
    let after = current.loop_budget;
    if after.decisions_remaining > before.decisions_remaining
        || after.malformed_repairs_remaining > before.malformed_repairs_remaining
        || after.model_requests_remaining > before.model_requests_remaining
        || after.deadline_unix_ms != before.deadline_unix_ms
    {
        return Err(RecoveryError::BudgetReset);
    }
    if current.task_budgets.len() != saved.task_budgets.len() {
        return Err(RecoveryError::StateChanged);
    }
    for (before, after) in saved.task_budgets.iter().zip(&current.task_budgets) {
        if before.task_key != after.task_key
            || before.max_turns != after.max_turns
            || before.max_repairs != after.max_repairs
            || before.max_model_requests != after.max_model_requests
        {
            return Err(RecoveryError::StateChanged);
        }
        if after.turns_used < before.turns_used
            || after.repairs_used < before.repairs_used
            || after.model_requests_used < before.model_requests_used
        {
            return Err(RecoveryError::BudgetReset);
        }
    }
    if current.message_cursors.len() != saved.message_cursors.len() {
        return Err(RecoveryError::StateChanged);
    }
    for (before, after) in saved.message_cursors.iter().zip(&current.message_cursors) {
        if before.participant_id != after.participant_id
            || before.task_key != after.task_key
            || after.inbox_sequence < before.inbox_sequence
            || after.outbox_sequence < before.outbox_sequence
            || after.acknowledged_outbox_sequence < before.acknowledged_outbox_sequence
        {
            return Err(RecoveryError::StateChanged);
        }
    }
    expected.workflow_version = current.workflow_version;
    expected.workflow_phase = current.workflow_phase;
    expected.committed_event_cursor = current.committed_event_cursor;
    expected.loop_budget = current.loop_budget;
    expected.task_budgets = current.task_budgets.clone();
    expected.message_cursors = current.message_cursors.clone();
    if &expected != current {
        return Err(RecoveryError::StateChanged);
    }
    Ok(())
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::native_controller::ControllerFence;

    fn task(value: &str) -> SpecIdentifier {
        SpecIdentifier::new(value).expect("task")
    }

    fn state() -> RecoveryState {
        let task_key = task("worker_a");
        RecoveryState {
            workflow_id: Uuid::from_u128(1),
            workflow_version: 7,
            workflow_phase: WorkflowPhase::Blocked,
            plan_generation: 1,
            plan_digest: Sha256Digest::of(b"plan"),
            policy_digest: Sha256Digest::of(b"policy"),
            committed_event_cursor: 19,
            contracts: vec![RecoveryContractRef {
                task_key: task_key.clone(),
                contract_id: Sha256Digest::of(b"contract"),
                contract_version: 1,
                template_digest: Sha256Digest::of(b"template"),
                depends_on: vec![],
            }],
            attempts: vec![],
            leases: vec![RecoveryLeaseRef {
                lease_id: Uuid::from_u128(2),
                task_key: task_key.clone(),
                generation: 3,
                state: LeaseState::Active,
            }],
            controllers: vec![RecoveryControllerRef {
                controller_id: Uuid::from_u128(3),
                session_id: Uuid::from_u128(4),
                task_key: task_key.clone(),
                generation: 2,
                version: 5,
                owner: ControllerOwner::Native,
                phase: ControllerPhase::Paused,
            }],
            pending_approvals: vec![],
            message_cursors: vec![RecoveryMessageCursor {
                participant_id: Uuid::from_u128(4),
                task_key: Some(task_key.clone()),
                inbox_sequence: 2,
                outbox_sequence: 3,
                acknowledged_outbox_sequence: 3,
            }],
            artifact_digests: vec![Sha256Digest::of(b"candidate")],
            verifier_receipt_ids: vec![Uuid::from_u128(5)],
            unresolved_effects: vec![],
            loop_budget: LoopBudget {
                decisions_remaining: 4,
                malformed_repairs_remaining: 1,
                model_requests_remaining: 5,
                deadline_unix_ms: 1000,
            },
            task_budgets: vec![RecoveryTaskBudget {
                task_key: task_key.clone(),
                turns_used: 2,
                max_turns: 5,
                repairs_used: 1,
                max_repairs: 2,
                model_requests_used: 2,
                max_model_requests: 6,
            }],
            context_settings: ContextSettingsIdentity {
                configured_context_tokens: Some(1_000_000),
                compaction_settings_digest: Some(Sha256Digest::of(
                    b"original global compaction settings",
                )),
            },
            preapproved_resume_policy: None,
            next_action: RecoveryAction::ContinueTask { task_key },
        }
    }

    fn observations(state: &RecoveryState) -> RecoveryObservations {
        RecoveryObservations {
            workflow_version: state.workflow_version,
            controllers: state
                .controllers
                .iter()
                .map(|value| RecoveryObservation {
                    resource_id: value.controller_id,
                    generation: value.generation,
                    presence: RecoveryPresence::Present,
                })
                .collect(),
            leases: state
                .leases
                .iter()
                .map(|value| RecoveryObservation {
                    resource_id: value.lease_id,
                    generation: value.generation,
                    presence: RecoveryPresence::Present,
                })
                .collect(),
        }
    }

    fn checkpoint(state: RecoveryState) -> RecoveryCheckpoint {
        RecoveryCheckpoint::new(Uuid::from_u128(6), state).expect("checkpoint")
    }

    fn operator(checkpoint: &RecoveryCheckpoint, current: &RecoveryState) -> ResumeAuthorization {
        ResumeAuthorization::OperatorStart {
            checkpoint_id: checkpoint.checkpoint_id,
            workflow_version: current.workflow_version,
        }
    }

    fn active_attempt(state: &RecoveryState, status: RecoveryAttemptState) -> RecoveryAttemptRef {
        RecoveryAttemptRef {
            request_id: Uuid::from_u128(7),
            submission_id: Uuid::from_u128(8),
            task_key: state.contracts[0].task_key.clone(),
            contract_id: state.contracts[0].contract_id,
            lease_id: state.leases[0].lease_id,
            lease_generation: state.leases[0].generation,
            controller_id: state.controllers[0].controller_id,
            controller_generation: state.controllers[0].generation,
            state: status,
        }
    }

    #[test]
    fn checkpoint_roundtrip_is_distinct_from_worker_claims_and_rejects_partial_data() {
        let checkpoint = checkpoint(state());
        let bytes = serde_json::to_vec(&checkpoint).expect("bytes");
        assert_eq!(
            RecoveryCheckpoint::parse(&bytes).expect("parse"),
            checkpoint
        );
        assert_eq!(
            RecoveryCheckpoint::parse(&bytes[..bytes.len() / 2]),
            Err(RecoveryError::Invalid)
        );
        assert_eq!(
            RecoveryCheckpoint::parse(br#"{"schema_version":1,"summary":"resume approved"}"#),
            Err(RecoveryError::Invalid)
        );
        let mut tampered = checkpoint;
        tampered.state.plan_digest = Sha256Digest::of(b"new plan");
        assert_eq!(tampered.validate(), Err(RecoveryError::DigestMismatch));
    }

    #[test]
    fn resume_requires_explicit_authority_and_keeps_consumed_budgets() {
        let saved = checkpoint(state());
        let mut current = saved.state.clone();
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                ResumeAuthorization::None,
                100
            )
            .expect("decision"),
            ResumeDecision::RequiresOperatorStart
        );
        current.workflow_version += 1;
        current.committed_event_cursor += 1;
        current.loop_budget.model_requests_remaining -= 1;
        current.task_budgets[0].model_requests_used += 1;
        let decision = assess_resume(
            &saved,
            &current,
            &observations(&current),
            operator(&saved, &current),
            100,
        )
        .expect("resume");
        assert!(
            matches!(decision, ResumeDecision::Ready { loop_budget, task_budget, .. } if loop_budget.model_requests_remaining == 4 && task_budget.model_requests_used == 3)
        );
        assert_eq!(saved.state.loop_budget.model_requests_remaining, 5);
        current.task_budgets[0].turns_used = 0;
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            ),
            Err(RecoveryError::BudgetReset)
        );
    }

    #[test]
    fn submitted_unknown_and_reserved_attempts_require_reconciliation_even_if_process_is_absent() {
        for status in [
            RecoveryAttemptState::Reserved,
            RecoveryAttemptState::Submitted,
            RecoveryAttemptState::Unknown,
        ] {
            let mut current = state();
            current.attempts.push(active_attempt(&current, status));
            let saved = checkpoint(current.clone());
            let mut observed = observations(&current);
            observed.controllers[0].presence = RecoveryPresence::Absent;
            let decision =
                assess_resume(&saved, &current, &observed, operator(&saved, &current), 100)
                    .expect("decision");
            assert!(
                matches!(decision, ResumeDecision::Reconcile { attempt_ids, controller_ids, .. } if attempt_ids == vec![Uuid::from_u128(7)] && controller_ids == vec![Uuid::from_u128(3)])
            );
        }
    }

    #[test]
    fn pending_approval_survives_compaction_without_manufacturing_permission() {
        let mut current = state();
        let attempt = active_attempt(&current, RecoveryAttemptState::Unknown);
        let challenge = ApprovalChallenge {
            challenge_id: Uuid::from_u128(9),
            requester_id: Uuid::from_u128(10),
            session_id: current.controllers[0].session_id,
            controller: ControllerFence {
                controller_id: attempt.controller_id,
                generation: attempt.controller_generation,
            },
            submission_id: attempt.submission_id,
            operation_digest: Sha256Digest::of(b"operation"),
            scope_digest: Sha256Digest::of(b"scope"),
        };
        current.pending_approvals.push(RecoveryApprovalRef {
            task_key: attempt.task_key.clone(),
            challenge,
        });
        current.attempts.push(attempt);
        let saved = checkpoint(current.clone());
        let restored = RecoveryCheckpoint::parse(&serde_json::to_vec(&saved).expect("bytes"))
            .expect("restored");
        assert_eq!(
            assess_resume(
                &restored,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            )
            .expect("decision"),
            ResumeDecision::AwaitingApproval {
                challenge_ids: vec![challenge.challenge_id]
            }
        );
        current.pending_approvals[0].challenge.controller.generation += 1;
        assert_eq!(current.validate(), Err(RecoveryError::ReferenceMismatch));
    }

    #[test]
    fn unknown_other_task_does_not_block_an_independent_task() {
        let mut current = state();
        let other = task("worker_b");
        current.contracts.push(RecoveryContractRef {
            task_key: other.clone(),
            contract_id: Sha256Digest::of(b"other contract"),
            contract_version: 1,
            template_digest: Sha256Digest::of(b"template"),
            depends_on: vec![],
        });
        current.task_budgets.push(RecoveryTaskBudget {
            task_key: other.clone(),
            turns_used: 0,
            max_turns: 2,
            repairs_used: 0,
            max_repairs: 1,
            model_requests_used: 0,
            max_model_requests: 2,
        });
        current.unresolved_effects.push(RecoveryEffectRef {
            effect_id: Uuid::from_u128(11),
            task_key: Some(other),
        });
        let saved = checkpoint(current.clone());
        assert!(matches!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            )
            .expect("decision"),
            ResumeDecision::Ready { .. }
        ));
        current.contracts[0].depends_on.push(task("worker_b"));
        let dependent = checkpoint(current.clone());
        assert!(matches!(
            assess_resume(&dependent, &current, &observations(&current), operator(&dependent, &current), 100).expect("dependent"),
            ResumeDecision::Reconcile { effect_ids, .. } if effect_ids == vec![Uuid::from_u128(11)]
        ));
        current.unresolved_effects[0].task_key = None;
        let global = checkpoint(current.clone());
        assert!(
            matches!(assess_resume(&global, &current, &observations(&current), operator(&global, &current), 100).expect("decision"), ResumeDecision::Reconcile { effect_ids, .. } if effect_ids == vec![Uuid::from_u128(11)])
        );
    }

    #[test]
    fn global_context_and_compaction_identity_must_remain_exact() {
        let saved = checkpoint(state());
        let mut current = saved.state.clone();
        current.context_settings.configured_context_tokens = Some(8192);
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            ),
            Err(RecoveryError::ContextSettingsChanged)
        );
        current = saved.state.clone();
        current.context_settings.compaction_settings_digest = Some(Sha256Digest::of(b"different"));
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            ),
            Err(RecoveryError::ContextSettingsChanged)
        );
        current.context_settings = ContextSettingsIdentity {
            configured_context_tokens: None,
            compaction_settings_digest: None,
        };
        let unknown = checkpoint(current.clone());
        assert_eq!(
            assess_resume(
                &unknown,
                &current,
                &observations(&current),
                operator(&unknown, &current),
                100
            )
            .expect("unknown"),
            ResumeDecision::ContextSettingsUnknown
        );
    }

    #[test]
    fn stale_observations_duplicate_references_and_missing_lease_are_rejected() {
        let current = state();
        let saved = checkpoint(current.clone());
        let mut observed = observations(&current);
        observed.controllers[0].generation -= 1;
        assert_eq!(
            assess_resume(&saved, &current, &observed, operator(&saved, &current), 100),
            Err(RecoveryError::InvalidObservation)
        );
        let mut duplicate = current.clone();
        duplicate.contracts.push(duplicate.contracts[0].clone());
        assert_eq!(duplicate.validate(), Err(RecoveryError::ReferenceMismatch));
        let mut missing = current;
        missing
            .attempts
            .push(active_attempt(&missing, RecoveryAttemptState::Running));
        missing.leases.clear();
        assert_eq!(missing.validate(), Err(RecoveryError::ReferenceMismatch));
    }

    #[test]
    fn resume_policy_is_bound_and_cancelled_work_cannot_revive() {
        let mut current = state();
        let policy = Sha256Digest::of(b"preapproved resume");
        current.preapproved_resume_policy = Some(policy);
        let saved = checkpoint(current.clone());
        assert!(matches!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                ResumeAuthorization::Preapproved {
                    policy_digest: policy
                },
                100
            )
            .expect("allowed"),
            ResumeDecision::Ready { .. }
        ));
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                ResumeAuthorization::Preapproved {
                    policy_digest: Sha256Digest::of(b"other")
                },
                100
            )
            .expect("blocked"),
            ResumeDecision::RequiresOperatorStart
        );
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                1000
            )
            .expect("expired"),
            ResumeDecision::BudgetExhausted
        );
        current.workflow_phase = WorkflowPhase::Cancelled;
        assert_eq!(
            assess_resume(
                &saved,
                &current,
                &observations(&current),
                operator(&saved, &current),
                100
            )
            .expect("cancelled"),
            ResumeDecision::WorkflowStopped
        );
        let cancelled = checkpoint(current.clone());
        current.workflow_phase = WorkflowPhase::Running;
        assert_eq!(
            assess_resume(
                &cancelled,
                &current,
                &observations(&current),
                operator(&cancelled, &current),
                100
            ),
            Err(RecoveryError::StateChanged)
        );
    }
}

fn check_observations(
    state: &RecoveryState,
    observations: &RecoveryObservations,
) -> Result<(), RecoveryError> {
    if observations.workflow_version != state.workflow_version
        || observations.controllers.len() > MAX_RECOVERY_REFERENCES
        || observations.leases.len() > MAX_RECOVERY_REFERENCES
        || !valid_ids(
            observations
                .controllers
                .iter()
                .map(|value| value.resource_id),
        )
        || !valid_ids(observations.leases.iter().map(|value| value.resource_id))
        || observations.controllers.iter().any(|value| {
            !state.controllers.iter().any(|reference| {
                reference.controller_id == value.resource_id
                    && reference.generation == value.generation
            })
        })
        || observations.leases.iter().any(|value| {
            !state.leases.iter().any(|reference| {
                reference.lease_id == value.resource_id && reference.generation == value.generation
            })
        })
    {
        return Err(RecoveryError::InvalidObservation);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevTestOutcome {
    Passed,
    Failed,
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevTestClaim {
    pub command: String,
    pub outcome: DevTestOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileRefClaim {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutgoingMessage {
    pub kind: MessageKind,
    pub to: SpecIdentifier,
    pub text: String,
    #[serde(default)]
    pub claimed_sender: Option<String>,
    #[serde(default)]
    pub reply_to: Option<Uuid>,
    #[serde(default)]
    pub source_refs: Vec<FileRefClaim>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReport {
    pub schema_version: u32,
    pub task_spec_digest: Sha256Digest,
    pub changed_files: Vec<String>,
    pub development_tests: Vec<DevTestClaim>,
    pub unresolved_issues: Vec<String>,
    pub context_refs_consumed: Vec<Uuid>,
    pub messages: Vec<OutgoingMessage>,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Blocker,
    Major,
    Minor,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub requirement_id: Option<SpecIdentifier>,
    pub severity: FindingSeverity,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReport {
    pub schema_version: u32,
    pub task_spec_digest: Sha256Digest,
    pub verdict: ReviewVerdict,
    pub findings: Vec<ReviewFinding>,
    pub checks_executed: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportError {
    #[error("no fenced report block was found")]
    Missing,
    #[error("the report block is not valid JSON for its schema")]
    Malformed,
    #[error("the report exceeds its bounds")]
    TooLarge,
    #[error("the report names another contract")]
    WrongContract,
    #[error("a report blocks a pass verdict")]
    InconsistentVerdict,
}

impl ReportError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Missing => "report_missing",
            Self::Malformed => "report_malformed",
            Self::TooLarge => "report_too_large",
            Self::WrongContract => "report_wrong_contract",
            Self::InconsistentVerdict => "report_inconsistent_verdict",
        }
    }
}

/// The contents of the last fenced block whose info string is exactly
/// `info`, delimited by backtick or tilde fences of at least three.
#[must_use]
pub fn last_fenced_block<'a>(text: &'a str, info: &str) -> Option<&'a str> {
    let mut found = None;
    let mut offset = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let fence_char = trimmed.chars().next();
        let fence_len = trimmed
            .chars()
            .take_while(|character| Some(*character) == fence_char)
            .count();
        let is_fence = matches!(fence_char, Some('`' | '~')) && fence_len >= 3;
        let line_start = offset;
        offset += line.len();
        index += 1;
        if !is_fence || trimmed[fence_len..].trim() != info {
            continue;
        }
        let content_start = line_start + line.len();
        let mut content_end = None;
        let mut inner_offset = content_start;
        let mut inner_index = index;
        while inner_index < lines.len() {
            let inner = lines[inner_index];
            let inner_trimmed = inner.trim_end_matches(['\n', '\r']);
            let closing_len = inner_trimmed
                .chars()
                .take_while(|character| Some(*character) == fence_char)
                .count();
            if closing_len >= fence_len
                && inner_trimmed
                    .chars()
                    .all(|character| Some(character) == fence_char)
            {
                content_end = Some(inner_offset);
                break;
            }
            inner_offset += inner.len();
            inner_index += 1;
        }
        if let Some(end) = content_end {
            found = Some(&text[content_start..end]);
            offset = inner_offset;
            index = inner_index;
        }
    }
    found
}

fn bounded_text(value: &str) -> bool {
    value.len() <= MAX_REPORT_TEXT_BYTES && !value.contains('\0')
}

pub fn parse_checkpoint(
    final_text: &str,
    expected: Sha256Digest,
) -> Result<CheckpointReport, ReportError> {
    let block =
        last_fenced_block(final_text, CHECKPOINT_INFO_STRING).ok_or(ReportError::Missing)?;
    if block.len() > MAX_REPORT_BYTES {
        return Err(ReportError::TooLarge);
    }
    let report: CheckpointReport =
        serde_json::from_str(block).map_err(|_| ReportError::Malformed)?;
    if report.schema_version != CHECKPOINT_SCHEMA_VERSION {
        return Err(ReportError::Malformed);
    }
    if report.task_spec_digest != expected {
        return Err(ReportError::WrongContract);
    }
    let too_many = [
        report.changed_files.len(),
        report.development_tests.len(),
        report.unresolved_issues.len(),
        report.context_refs_consumed.len(),
        report.messages.len(),
    ]
    .into_iter()
    .any(|count| count > MAX_REPORT_ITEMS);
    let texts_ok = report.changed_files.iter().all(|text| bounded_text(text))
        && report
            .unresolved_issues
            .iter()
            .all(|text| bounded_text(text))
        && report
            .development_tests
            .iter()
            .all(|test| bounded_text(&test.command))
        && report.messages.iter().all(|message| {
            bounded_text(&message.text) && message.source_refs.len() <= MAX_REPORT_ITEMS
        })
        && bounded_text(&report.summary);
    if too_many || !texts_ok {
        return Err(ReportError::TooLarge);
    }
    Ok(report)
}

pub fn parse_review(final_text: &str, expected: Sha256Digest) -> Result<ReviewReport, ReportError> {
    let block = last_fenced_block(final_text, REVIEW_INFO_STRING).ok_or(ReportError::Missing)?;
    if block.len() > MAX_REPORT_BYTES {
        return Err(ReportError::TooLarge);
    }
    let report: ReviewReport = serde_json::from_str(block).map_err(|_| ReportError::Malformed)?;
    if report.schema_version != CHECKPOINT_SCHEMA_VERSION {
        return Err(ReportError::Malformed);
    }
    if report.task_spec_digest != expected {
        return Err(ReportError::WrongContract);
    }
    if report.findings.len() > MAX_REPORT_ITEMS
        || report.checks_executed.len() > MAX_REPORT_ITEMS
        || !report
            .findings
            .iter()
            .all(|finding| bounded_text(&finding.summary))
    {
        return Err(ReportError::TooLarge);
    }
    let has_blocker = report
        .findings
        .iter()
        .any(|finding| finding.severity == FindingSeverity::Blocker);
    if report.verdict == ReviewVerdict::Pass && has_blocker {
        return Err(ReportError::InconsistentVerdict);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> Sha256Digest {
        Sha256Digest::of(b"spec")
    }

    fn checkpoint_json() -> String {
        format!(
            r#"{{"schema_version":1,"task_spec_digest":"{}","changed_files":["src/server.mjs"],"development_tests":[{{"command":"node --test tests/worker_a","outcome":"passed"}}],"unresolved_issues":[],"context_refs_consumed":[],"messages":[{{"kind":"question","to":"track_a","text":"Which error wins?"}}],"summary":"Implemented."}}"#,
            digest()
        )
    }

    #[test]
    fn the_last_exact_block_wins_and_lookalikes_are_ignored() {
        let text = format!(
            "Echoing the instructions:\n```vibemux_checkpoint\n{{\"forged\":true}}\n```\nWork done.\n```vibemux_checkpoint_extra\n{{}}\n```\n````vibemux_checkpoint\n{}\n````\n",
            checkpoint_json()
        );
        let report = parse_checkpoint(&text, digest()).expect("checkpoint");
        assert_eq!(report.changed_files, vec!["src/server.mjs".to_string()]);
        assert_eq!(report.messages[0].kind, MessageKind::Question);
    }

    #[test]
    fn missing_malformed_wrong_contract_and_oversized_reports_are_refused() {
        assert_eq!(
            parse_checkpoint("no block", digest()),
            Err(ReportError::Missing)
        );
        assert_eq!(
            parse_checkpoint(
                "```vibemux_checkpoint\n{\"schema_version\":1}\n```",
                digest()
            ),
            Err(ReportError::Malformed)
        );
        let text = format!("```vibemux_checkpoint\n{}\n```", checkpoint_json());
        assert_eq!(
            parse_checkpoint(&text, Sha256Digest::of(b"other")),
            Err(ReportError::WrongContract)
        );
        let unknown = checkpoint_json().replace("\"summary\"", "\"accepted\":true,\"summary\"");
        assert_eq!(
            parse_checkpoint(&format!("```vibemux_checkpoint\n{unknown}\n```"), digest()),
            Err(ReportError::Malformed)
        );
        let unclosed = format!("```vibemux_checkpoint\n{}\n", checkpoint_json());
        assert_eq!(
            parse_checkpoint(&unclosed, digest()),
            Err(ReportError::Missing)
        );
    }

    #[test]
    fn a_pass_with_a_blocker_is_inconsistent() {
        let review = format!(
            "```vibemux_review\n{{\"schema_version\":1,\"task_spec_digest\":\"{}\",\"verdict\":\"pass\",\"findings\":[{{\"requirement_id\":\"r02\",\"severity\":\"blocker\",\"path\":\"src/server.mjs\",\"line\":4,\"summary\":\"121 code points accepted\"}}],\"checks_executed\":[\"read src/server.mjs\"]}}\n```",
            digest()
        );
        assert_eq!(
            parse_review(&review, digest()),
            Err(ReportError::InconsistentVerdict)
        );
        let fixed = review.replace("\"pass\"", "\"changes_requested\"");
        assert_eq!(
            parse_review(&fixed, digest()).expect("review").verdict,
            ReviewVerdict::ChangesRequested
        );
    }
}
