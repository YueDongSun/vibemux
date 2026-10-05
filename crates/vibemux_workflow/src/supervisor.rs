//! The supervisor's typed tool surface and proposal validation (ADR 031 §3).
//!
//! The supervisor model proposes; the Rust runtime authorizes and commits.
//! A proposal is strict JSON naming one of a small set of tools with stable
//! identifiers, the expected workflow version, and an idempotency key.
//! There is no tool for SQL, shell, URLs, filesystem access, provider
//! switching, secrets, permission grants, contract edits, or a transition
//! to success. A malformed response gets at most the configured number of
//! repair prompts; it is never executed as prose. Decisions are bounded by
//! persisted loop limits that a daemon restart does not reset.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Sha256Digest, SpecIdentifier, selection::SelectionOutcome};

pub const MAX_DECISIONS_PER_PROPOSAL: usize = 4;
pub const MAX_RATIONALE_BYTES: usize = 1024;
pub const MAX_PROPOSAL_BYTES: usize = 16 * 1024;
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    tag = "tool",
    content = "arguments",
    deny_unknown_fields
)]
pub enum SupervisorTool {
    InspectWorkflow {},
    ListSlots {},
    ReadArtifact {
        artifact_digest: Sha256Digest,
    },
    QueryContext {
        task_key: SpecIdentifier,
    },
    ProposePlan {
        tasks: Vec<SpecIdentifier>,
    },
    AssignTask {
        task_key: SpecIdentifier,
        slot_id: SpecIdentifier,
    },
    SendMessage {
        to: SpecIdentifier,
        text: String,
    },
    ShareContext {
        from_task: SpecIdentifier,
        to_task: SpecIdentifier,
        request_message_id: Uuid,
    },
    RequestReview {
        task_key: SpecIdentifier,
        candidate_digest: Sha256Digest,
    },
    RequestVerification {
        task_key: SpecIdentifier,
        candidate_digest: Sha256Digest,
    },
    ProposeCandidateSelection {
        candidate_digest: Option<Sha256Digest>,
    },
    ProposeIntegration {
        candidate_digests: Vec<Sha256Digest>,
    },
    PauseRun {
        task_key: SpecIdentifier,
    },
    RequestCancel {
        task_key: Option<SpecIdentifier>,
    },
    RequestHandoff {
        task_key: SpecIdentifier,
    },
    ReportBlocked {
        reason_code: SpecIdentifier,
    },
}

impl SupervisorTool {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::InspectWorkflow {} => "inspect_workflow",
            Self::ListSlots {} => "list_slots",
            Self::ReadArtifact { .. } => "read_artifact",
            Self::QueryContext { .. } => "query_context",
            Self::ProposePlan { .. } => "propose_plan",
            Self::AssignTask { .. } => "assign_task",
            Self::SendMessage { .. } => "send_message",
            Self::ShareContext { .. } => "share_context",
            Self::RequestReview { .. } => "request_review",
            Self::RequestVerification { .. } => "request_verification",
            Self::ProposeCandidateSelection { .. } => "propose_candidate_selection",
            Self::ProposeIntegration { .. } => "propose_integration",
            Self::PauseRun { .. } => "pause_run",
            Self::RequestCancel { .. } => "request_cancel",
            Self::RequestHandoff { .. } => "request_handoff",
            Self::ReportBlocked { .. } => "report_blocked",
        }
    }

    /// Read-only tools never change state or spend budget beyond the call.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::InspectWorkflow {}
                | Self::ListSlots {}
                | Self::ReadArtifact { .. }
                | Self::QueryContext { .. }
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorDecision {
    pub idempotency_key: String,
    pub expected_workflow_version: u64,
    pub action: SupervisorTool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorProposal {
    pub decisions: Vec<SupervisorDecision>,
    /// Concise public rationale; never hidden reasoning.
    pub rationale: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalRejection {
    #[error("proposal is not strict JSON for the tool schema")]
    Malformed,
    #[error("proposal is too large or has too many decisions")]
    TooLarge,
    #[error("idempotency key is missing or invalid")]
    InvalidIdempotencyKey,
    #[error("expected workflow version is stale")]
    StaleVersion,
    #[error("task is unknown")]
    UnknownTask,
    #[error("slot is not in the validated eligible set")]
    IneligibleSlot,
    #[error("candidate is unknown or did not pass its gate")]
    CandidateNotAccepted,
    #[error("proposed selection differs from the deterministic selector")]
    SelectionMismatch,
    #[error("integration names unapproved candidates")]
    IntegrationNotApproved,
    #[error("decision budget is exhausted")]
    BudgetExhausted,
    #[error("workflow does not admit this action now")]
    PhaseForbids,
    #[error("message text is empty or too large")]
    InvalidText,
    #[error("rationale is too large")]
    RationaleTooLarge,
}

impl ProposalRejection {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "supervisor_proposal_malformed",
            Self::TooLarge => "supervisor_proposal_too_large",
            Self::InvalidIdempotencyKey => "supervisor_invalid_idempotency_key",
            Self::StaleVersion => "supervisor_stale_version",
            Self::UnknownTask => "supervisor_unknown_task",
            Self::IneligibleSlot => "supervisor_ineligible_slot",
            Self::CandidateNotAccepted => "supervisor_candidate_not_accepted",
            Self::SelectionMismatch => "supervisor_selection_mismatch",
            Self::IntegrationNotApproved => "supervisor_integration_not_approved",
            Self::BudgetExhausted => "supervisor_budget_exhausted",
            Self::PhaseForbids => "supervisor_phase_forbids",
            Self::InvalidText => "supervisor_invalid_text",
            Self::RationaleTooLarge => "supervisor_rationale_too_large",
        }
    }
}

/// Persisted remaining allowances; restarts and repair Runs never reset them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoopBudget {
    pub decisions_remaining: u32,
    pub malformed_repairs_remaining: u32,
    pub model_requests_remaining: u32,
    pub deadline_unix_ms: u64,
}

impl LoopBudget {
    /// Charges one model request and one decision; refuses when exhausted.
    pub fn charge_decision(&mut self, now_unix_ms: u64) -> Result<(), ProposalRejection> {
        if now_unix_ms >= self.deadline_unix_ms
            || self.decisions_remaining == 0
            || self.model_requests_remaining == 0
        {
            return Err(ProposalRejection::BudgetExhausted);
        }
        self.decisions_remaining -= 1;
        self.model_requests_remaining -= 1;
        Ok(())
    }

    /// Charges one malformed-response repair.
    pub fn charge_repair(&mut self) -> Result<(), ProposalRejection> {
        if self.malformed_repairs_remaining == 0 || self.model_requests_remaining == 0 {
            return Err(ProposalRejection::BudgetExhausted);
        }
        self.malformed_repairs_remaining -= 1;
        self.model_requests_remaining -= 1;
        Ok(())
    }
}

/// Committed facts the validator checks proposals against.
#[derive(Clone, Debug)]
pub struct SupervisorFacts<'a> {
    pub workflow_version: u64,
    pub admits_work: bool,
    pub tasks: &'a [SpecIdentifier],
    pub eligible_slots: &'a [SpecIdentifier],
    /// Candidates whose gate passed, by task.
    pub accepted_candidates: &'a [(SpecIdentifier, Sha256Digest)],
    /// Candidates collected but not yet gated, by task.
    pub collected_candidates: &'a [(SpecIdentifier, Sha256Digest)],
    pub selection: Option<&'a SelectionOutcome>,
}

/// Parses a model response strictly.
pub fn parse_proposal(bytes: &[u8]) -> Result<SupervisorProposal, ProposalRejection> {
    if bytes.len() > MAX_PROPOSAL_BYTES {
        return Err(ProposalRejection::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ProposalRejection::Malformed)?;
    // Models often wrap JSON in a fence; accept exactly one fenced block or
    // a bare object, nothing else.
    let json = crate::checkpoint::last_fenced_block(text, "json").unwrap_or(text);
    let proposal: SupervisorProposal =
        serde_json::from_str(json.trim()).map_err(|_| ProposalRejection::Malformed)?;
    if proposal.decisions.is_empty() || proposal.decisions.len() > MAX_DECISIONS_PER_PROPOSAL {
        return Err(ProposalRejection::TooLarge);
    }
    if proposal.rationale.len() > MAX_RATIONALE_BYTES || proposal.evidence_refs.len() > 16 {
        return Err(ProposalRejection::RationaleTooLarge);
    }
    Ok(proposal)
}

/// Validates each decision against committed facts. Every rejection is
/// reported; nothing in a rejected proposal is executed.
pub fn validate_proposal(
    proposal: &SupervisorProposal,
    facts: &SupervisorFacts<'_>,
) -> Result<(), Vec<ProposalRejection>> {
    let mut rejections = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for decision in &proposal.decisions {
        let key = &decision.idempotency_key;
        if key.is_empty()
            || key.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !keys.insert(key.clone())
        {
            rejections.insert(ProposalRejection::InvalidIdempotencyKey);
        }
        if decision.expected_workflow_version != facts.workflow_version {
            rejections.insert(ProposalRejection::StaleVersion);
        }
        if !decision.action.is_read_only() && !facts.admits_work {
            let allowed_when_stopped = matches!(
                decision.action,
                SupervisorTool::ReportBlocked { .. } | SupervisorTool::RequestCancel { .. }
            );
            if !allowed_when_stopped {
                rejections.insert(ProposalRejection::PhaseForbids);
            }
        }
        let known_task = |task: &SpecIdentifier| facts.tasks.contains(task);
        match &decision.action {
            SupervisorTool::AssignTask { task_key, slot_id } => {
                if !known_task(task_key) {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
                if !facts.eligible_slots.contains(slot_id) {
                    rejections.insert(ProposalRejection::IneligibleSlot);
                }
            }
            SupervisorTool::QueryContext { task_key }
            | SupervisorTool::PauseRun { task_key }
            | SupervisorTool::RequestHandoff { task_key } => {
                if !known_task(task_key) {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
            }
            SupervisorTool::ProposePlan { tasks } => {
                if tasks.iter().any(|task| !known_task(task)) {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
            }
            SupervisorTool::ShareContext {
                from_task, to_task, ..
            } => {
                if !known_task(from_task) || !known_task(to_task) || from_task == to_task {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
            }
            SupervisorTool::SendMessage { to, text } => {
                if !known_task(to) {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
                if text.trim().is_empty() || text.len() > crate::messages::MAX_MESSAGE_BODY_BYTES {
                    rejections.insert(ProposalRejection::InvalidText);
                }
            }
            SupervisorTool::RequestReview {
                task_key,
                candidate_digest,
            }
            | SupervisorTool::RequestVerification {
                task_key,
                candidate_digest,
            } => {
                let collected = facts
                    .collected_candidates
                    .iter()
                    .chain(facts.accepted_candidates)
                    .any(|(task, digest)| task == task_key && digest == candidate_digest);
                if !collected {
                    rejections.insert(ProposalRejection::CandidateNotAccepted);
                }
            }
            SupervisorTool::ProposeCandidateSelection { candidate_digest } => {
                let deterministic = facts.selection.map(|selection| selection.winner);
                if deterministic != Some(*candidate_digest) {
                    rejections.insert(ProposalRejection::SelectionMismatch);
                }
            }
            SupervisorTool::ProposeIntegration { candidate_digests } => {
                let approved = !candidate_digests.is_empty()
                    && candidate_digests.iter().all(|digest| {
                        facts
                            .accepted_candidates
                            .iter()
                            .any(|(_, accepted)| accepted == digest)
                    });
                if !approved {
                    rejections.insert(ProposalRejection::IntegrationNotApproved);
                }
            }
            SupervisorTool::RequestCancel {
                task_key: Some(task_key),
            } => {
                if !known_task(task_key) {
                    rejections.insert(ProposalRejection::UnknownTask);
                }
            }
            SupervisorTool::InspectWorkflow {}
            | SupervisorTool::ListSlots {}
            | SupervisorTool::ReadArtifact { .. }
            | SupervisorTool::RequestCancel { task_key: None }
            | SupervisorTool::ReportBlocked { .. } => {}
        }
    }
    if rejections.is_empty() {
        Ok(())
    } else {
        Err(rejections.into_iter().collect())
    }
}

/// The content-free record of one supervisor decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub decision_id: Uuid,
    pub tool: String,
    pub idempotency_key: String,
    pub workflow_version: u64,
    pub affected: Vec<String>,
    pub rationale_sha256: Sha256Digest,
    pub rationale_bytes: u64,
    pub evidence_refs: Vec<String>,
    pub accepted: bool,
    pub rejection_codes: Vec<String>,
}

/// Driver selection is operator-owned. Merely configuring a model route never
/// switches a deterministic workflow into model-driven execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorMode {
    Deterministic,
    Model,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorExecutionState {
    Running,
    Stopped,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorTaskGate {
    Active,
    Paused,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorTaskBinding {
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub allowed_slots: Vec<SpecIdentifier>,
    pub dependencies: Vec<SpecIdentifier>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorExecution {
    pub workflow_id: Uuid,
    pub generation: u64,
    pub start_request_id: Uuid,
    pub mode: SupervisorMode,
    pub state: SupervisorExecutionState,
    pub plan_digest: Sha256Digest,
    pub config_digest: Sha256Digest,
    pub tasks: BTreeMap<SpecIdentifier, SupervisorTaskBinding>,
    pub slot_harnesses: BTreeMap<SpecIdentifier, vibemux_harness::AgentKind>,
    pub reviewer_slot: SpecIdentifier,
    pub task_order: Vec<SpecIdentifier>,
    pub task_gates: BTreeMap<SpecIdentifier, SupervisorTaskGate>,
    pub next_round: u64,
    pub max_model_calls: u32,
    pub deadline_unix_ms: u64,
    pub active_round: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorCandidate {
    pub candidate: crate::gates::CandidateRecord,
    pub request_id: Uuid,
    pub reviewed: bool,
    pub verified: bool,
    pub gate_passed: bool,
    pub tests_passed: u32,
    pub changed_bytes: u64,
}

/// Complete bounded facts used by the writer; no raw source, prompts, or secrets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorObservation {
    pub execution: SupervisorExecution,
    pub workflow: crate::workflow_record::WorkflowRecord,
    pub event_cursor: u64,
    pub eligible_slots: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    pub candidates: Vec<SupervisorCandidate>,
    pub selection: Option<SelectionOutcome>,
    pub last_round: Option<SupervisorRound>,
    pub last_actions: Vec<SupervisorAction>,
}

impl SupervisorObservation {
    pub fn digest(&self) -> Result<Sha256Digest, crate::canonical_json::CanonicalError> {
        crate::canonical_json::canonical_digest("vibemux.workflow.supervisor_observation.v1", self)
    }

    /// Fenced target facts checked after this round's own call accounting.
    /// Reservation validates the full observation first; only the explicitly
    /// owned round counter, active-round pointer and accounting timestamp are
    /// omitted here. Existing action results remain fenced throughout the call.
    pub fn target_digest(&self) -> Result<Sha256Digest, crate::canonical_json::CanonicalError> {
        let mut execution = self.execution.clone();
        execution.active_round = None;
        execution.next_round = 0;
        let mut workflow = self.workflow.clone();
        workflow.updated_at_ms = 0;
        crate::canonical_json::canonical_digest(
            "vibemux.workflow.supervisor_targets.v1",
            &serde_json::json!({
                "execution":execution,"workflow":workflow,"eligible_slots":self.eligible_slots,
                "candidates":self.candidates,"selection":self.selection,"last_actions":self.last_actions,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRoundState {
    Reserved,
    Accepted,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorRound {
    pub round_id: Uuid,
    pub workflow_id: Uuid,
    pub generation: u64,
    pub call_id: Uuid,
    pub observed_version: u64,
    pub reservation_version: u64,
    pub observation_digest: Sha256Digest,
    pub target_digest: Sha256Digest,
    pub context_digest: Sha256Digest,
    pub state: SupervisorRoundState,
    pub proposal_digest: Option<Sha256Digest>,
    pub rationale_digest: Option<Sha256Digest>,
    pub rejection_codes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorActionState {
    Pending,
    Claimed,
    Applied,
    Blocked,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorAction {
    pub action_id: Uuid,
    pub workflow_id: Uuid,
    pub generation: u64,
    pub round_id: Uuid,
    pub idempotency_key: String,
    pub fingerprint: Sha256Digest,
    /// Only supported content-free actions are persisted. Unsupported message
    /// bodies remain transient and contribute only to the rejected digest.
    pub action: SupervisorTool,
    pub state: SupervisorActionState,
    pub dispatch_request_id: Option<Uuid>,
    pub result_refs: Vec<Uuid>,
    pub result_code: Option<SpecIdentifier>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorStatus {
    pub schema_version: u32,
    pub execution: SupervisorExecution,
    pub rounds: Vec<SupervisorRound>,
    pub actions: Vec<SupervisorAction>,
}

/// The first model driver supports bounded orchestration effects. It does not
/// pretend unavailable content-sharing or native capabilities were executed.
#[must_use]
pub const fn runtime_tool_supported(tool: &SupervisorTool) -> bool {
    !matches!(
        tool,
        SupervisorTool::ReadArtifact { .. }
            | SupervisorTool::QueryContext { .. }
            | SupervisorTool::SendMessage { .. }
            | SupervisorTool::ShareContext { .. }
            | SupervisorTool::RequestHandoff { .. }
    )
}

pub fn validate_runtime_proposal(
    proposal: &SupervisorProposal,
    observation: &SupervisorObservation,
) -> Result<(), Vec<String>> {
    let tasks: Vec<_> = observation.execution.tasks.keys().cloned().collect();
    let eligible: Vec<_> = observation
        .eligible_slots
        .values()
        .flatten()
        .cloned()
        .collect();
    let collected: Vec<_> = observation
        .candidates
        .iter()
        .map(|value| {
            (
                value.candidate.task_key.clone(),
                value.candidate.candidate_digest,
            )
        })
        .collect();
    let accepted: Vec<_> = observation
        .candidates
        .iter()
        .filter(|value| value.gate_passed)
        .map(|value| {
            (
                value.candidate.task_key.clone(),
                value.candidate.candidate_digest,
            )
        })
        .collect();
    let mut codes: BTreeSet<String> = validate_proposal(
        proposal,
        &SupervisorFacts {
            workflow_version: observation.workflow.version,
            admits_work: observation.workflow.phase.admits_work(),
            tasks: &tasks,
            eligible_slots: &eligible,
            accepted_candidates: &accepted,
            collected_candidates: &collected,
            selection: observation.selection.as_ref(),
        },
    )
    .err()
    .unwrap_or_default()
    .into_iter()
    .map(|error| error.code().to_string())
    .collect();
    if proposal.decisions.is_empty()
        || proposal.decisions.len() > MAX_DECISIONS_PER_PROPOSAL
        || proposal.rationale.len() > MAX_RATIONALE_BYTES
        || proposal.evidence_refs.len() > 16
    {
        codes.insert("supervisor_proposal_too_large".into());
    }
    let mut targeted = BTreeSet::new();
    for decision in &proposal.decisions {
        if !runtime_tool_supported(&decision.action) {
            codes.insert("supervisor_tool_unavailable".into());
        }
        let target = match &decision.action {
            SupervisorTool::AssignTask { task_key, slot_id } => {
                if !observation
                    .eligible_slots
                    .get(task_key)
                    .is_some_and(|slots| slots.contains(slot_id))
                {
                    codes.insert("supervisor_ineligible_slot".into());
                }
                if observation.workflow.task(task_key).is_none_or(|task| {
                    matches!(
                        task.progress,
                        crate::workflow_record::TaskProgress::Accepted
                            | crate::workflow_record::TaskProgress::Failed
                    ) || task.turns_used >= task.max_turns
                }) {
                    codes.insert("supervisor_task_not_active".into());
                }
                if observation
                    .execution
                    .tasks
                    .get(task_key)
                    .is_some_and(|binding| {
                        binding.dependencies.iter().any(|dependency| {
                            observation
                                .workflow
                                .task(dependency)
                                .is_none_or(|task| task.accepted_candidate.is_none())
                        })
                    })
                {
                    codes.insert("supervisor_dependencies_unready".into());
                }
                Some(task_key)
            }
            SupervisorTool::RequestReview { task_key, .. }
            | SupervisorTool::RequestVerification { task_key, .. }
            | SupervisorTool::PauseRun { task_key }
            | SupervisorTool::RequestCancel {
                task_key: Some(task_key),
            } => Some(task_key),
            _ => None,
        };
        if let Some(task) = target {
            if !targeted.insert(task) {
                codes.insert("supervisor_conflicting_actions".into());
            }
            if observation.execution.task_gates.get(task) != Some(&SupervisorTaskGate::Active)
                && !matches!(decision.action, SupervisorTool::RequestCancel { .. })
            {
                codes.insert("supervisor_task_not_active".into());
            }
        }
        match &decision.action {
            SupervisorTool::ProposePlan { tasks: order } => {
                let positions: BTreeMap<_, _> = order
                    .iter()
                    .enumerate()
                    .map(|(index, task)| (task, index))
                    .collect();
                if order.len() != tasks.len()
                    || positions.len() != tasks.len()
                    || tasks.iter().any(|task| !positions.contains_key(task))
                    || observation.execution.tasks.iter().any(|(task, binding)| {
                        binding.dependencies.iter().any(|dependency| {
                            positions
                                .get(dependency)
                                .zip(positions.get(task))
                                .is_none_or(|(before, after)| before >= after)
                        })
                    })
                {
                    codes.insert("supervisor_plan_invalid".into());
                }
            }
            SupervisorTool::ProposeIntegration { candidate_digests } => {
                let unique: BTreeSet<_> = candidate_digests.iter().copied().collect();
                let expected =
                    if observation.workflow.mode == crate::task_spec::WorkflowMode::Compare {
                        observation
                            .selection
                            .as_ref()
                            .and_then(|value| value.winner)
                            .into_iter()
                            .collect::<BTreeSet<_>>()
                    } else {
                        observation
                            .workflow
                            .tasks
                            .iter()
                            .filter_map(|task| task.accepted_candidate)
                            .collect::<BTreeSet<_>>()
                    };
                let all_tasks_ready = observation.workflow.tasks.iter().all(|task| {
                    task.accepted_candidate.is_some()
                        && observation.execution.task_gates.get(&task.task_key)
                            == Some(&SupervisorTaskGate::Active)
                });
                if unique.len() != candidate_digests.len()
                    || unique.is_empty()
                    || unique != expected
                    || !all_tasks_ready
                {
                    codes.insert("supervisor_integration_not_approved".into());
                }
            }
            _ => {}
        }
        if proposal.decisions.len() > 1
            && matches!(
                decision.action,
                SupervisorTool::ProposePlan { .. }
                    | SupervisorTool::ProposeIntegration { .. }
                    | SupervisorTool::ProposeCandidateSelection { .. }
                    | SupervisorTool::RequestCancel { task_key: None }
                    | SupervisorTool::ReportBlocked { .. }
            )
        {
            codes.insert("supervisor_conflicting_actions".into());
        }
    }
    if codes.is_empty() {
        Ok(())
    } else {
        Err(codes.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::DecidedBy;

    fn id(value: &str) -> SpecIdentifier {
        SpecIdentifier::new(value).expect("id")
    }

    fn proposal(json: &str) -> Result<SupervisorProposal, ProposalRejection> {
        parse_proposal(json.as_bytes())
    }

    struct Fixture {
        tasks: Vec<SpecIdentifier>,
        slots: Vec<SpecIdentifier>,
        accepted: Vec<(SpecIdentifier, Sha256Digest)>,
        collected: Vec<(SpecIdentifier, Sha256Digest)>,
        selection: SelectionOutcome,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                tasks: vec![id("track_a"), id("track_b")],
                slots: vec![id("claude_a")],
                accepted: vec![(id("track_a"), Sha256Digest::of(b"good"))],
                collected: vec![(id("track_b"), Sha256Digest::of(b"pending"))],
                selection: SelectionOutcome {
                    winner: Some(Sha256Digest::of(b"good")),
                    decided_by: DecidedBy::OnlyValidCandidate,
                    ranking: vec![Sha256Digest::of(b"good")],
                    excluded: vec![],
                },
            }
        }

        fn facts(&self) -> SupervisorFacts<'_> {
            SupervisorFacts {
                workflow_version: 7,
                admits_work: true,
                tasks: &self.tasks,
                eligible_slots: &self.slots,
                accepted_candidates: &self.accepted,
                collected_candidates: &self.collected,
                selection: Some(&self.selection),
            }
        }
    }

    #[test]
    fn a_valid_assignment_passes() {
        let fixture = Fixture::new();
        let parsed = proposal(
            r#"```json
{"decisions":[{"idempotency_key":"assign_track_a_1","expected_workflow_version":7,"action":{"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"claude_a"}}}],"rationale":"Track A is ready.","evidence_refs":["slot:claude_a"]}
```"#,
        )
        .expect("parse");
        validate_proposal(&parsed, &fixture.facts()).expect("valid");
    }

    #[test]
    fn prose_unknown_tools_and_success_transitions_cannot_parse() {
        assert_eq!(
            proposal("Please assign track_a to claude_a."),
            Err(ProposalRejection::Malformed)
        );
        for tool in [
            "mark_accepted",
            "run_shell",
            "switch_provider",
            "edit_contract",
            "grant_permission",
        ] {
            let json = format!(
                r#"{{"decisions":[{{"idempotency_key":"k1","expected_workflow_version":7,"action":{{"tool":"{tool}","arguments":{{}}}}}}],"rationale":"x","evidence_refs":[]}}"#
            );
            assert_eq!(proposal(&json), Err(ProposalRejection::Malformed), "{tool}");
        }
        let extra = r#"{"decisions":[{"idempotency_key":"k1","expected_workflow_version":7,"action":{"tool":"list_slots","arguments":{}},"force":true}],"rationale":"x","evidence_refs":[]}"#;
        assert_eq!(proposal(extra), Err(ProposalRejection::Malformed));
    }

    #[test]
    fn ineligible_slots_stale_versions_and_bad_keys_are_rejected() {
        let fixture = Fixture::new();
        let parsed = proposal(
            r#"{"decisions":[
                {"idempotency_key":"k1","expected_workflow_version":6,"action":{"tool":"assign_task","arguments":{"task_key":"track_c","slot_id":"codex_x"}}},
                {"idempotency_key":"k1","expected_workflow_version":7,"action":{"tool":"list_slots","arguments":{}}}
            ],"rationale":"x","evidence_refs":[]}"#,
        )
        .expect("parse");
        let rejections = validate_proposal(&parsed, &fixture.facts()).expect_err("rejected");
        assert_eq!(
            rejections,
            vec![
                ProposalRejection::InvalidIdempotencyKey,
                ProposalRejection::StaleVersion,
                ProposalRejection::UnknownTask,
                ProposalRejection::IneligibleSlot,
            ]
        );
    }

    #[test]
    fn selection_and_integration_must_follow_the_gates() {
        let fixture = Fixture::new();
        let pick_other = format!(
            r#"{{"decisions":[{{"idempotency_key":"select_1","expected_workflow_version":7,"action":{{"tool":"propose_candidate_selection","arguments":{{"candidate_digest":"{}"}}}}}}],"rationale":"Its report is more convincing.","evidence_refs":[]}}"#,
            Sha256Digest::of(b"pending")
        );
        assert_eq!(
            validate_proposal(&proposal(&pick_other).expect("parse"), &fixture.facts()),
            Err(vec![ProposalRejection::SelectionMismatch])
        );
        let integrate_pending = format!(
            r#"{{"decisions":[{{"idempotency_key":"integrate_1","expected_workflow_version":7,"action":{{"tool":"propose_integration","arguments":{{"candidate_digests":["{}","{}"]}}}}}}],"rationale":"x","evidence_refs":[]}}"#,
            Sha256Digest::of(b"good"),
            Sha256Digest::of(b"pending")
        );
        assert_eq!(
            validate_proposal(
                &proposal(&integrate_pending).expect("parse"),
                &fixture.facts()
            ),
            Err(vec![ProposalRejection::IntegrationNotApproved])
        );
    }

    #[test]
    fn a_stopped_workflow_only_accepts_reads_cancel_and_blocked_reports() {
        let fixture = Fixture::new();
        let mut facts = fixture.facts();
        facts.admits_work = false;
        let assign = proposal(r#"{"decisions":[{"idempotency_key":"k1","expected_workflow_version":7,"action":{"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"claude_a"}}}],"rationale":"x","evidence_refs":[]}"#).expect("parse");
        assert_eq!(
            validate_proposal(&assign, &facts),
            Err(vec![ProposalRejection::PhaseForbids])
        );
        let blocked = proposal(r#"{"decisions":[{"idempotency_key":"k2","expected_workflow_version":7,"action":{"tool":"report_blocked","arguments":{"reason_code":"no_eligible_slot"}}},{"idempotency_key":"k3","expected_workflow_version":7,"action":{"tool":"inspect_workflow","arguments":{}}}],"rationale":"x","evidence_refs":[]}"#).expect("parse");
        validate_proposal(&blocked, &facts).expect("allowed");
    }

    #[test]
    fn loop_budget_is_bounded_and_counts_repairs() {
        let mut budget = LoopBudget {
            decisions_remaining: 1,
            malformed_repairs_remaining: 1,
            model_requests_remaining: 2,
            deadline_unix_ms: 1_000,
        };
        budget.charge_repair().expect("one repair");
        assert_eq!(
            budget.charge_repair(),
            Err(ProposalRejection::BudgetExhausted)
        );
        budget.charge_decision(10).expect("one decision");
        assert_eq!(
            budget.charge_decision(10),
            Err(ProposalRejection::BudgetExhausted)
        );
        let mut late = LoopBudget {
            decisions_remaining: 5,
            malformed_repairs_remaining: 0,
            model_requests_remaining: 5,
            deadline_unix_ms: 1_000,
        };
        assert_eq!(
            late.charge_decision(1_000),
            Err(ProposalRejection::BudgetExhausted)
        );
    }
}
