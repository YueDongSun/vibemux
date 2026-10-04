//! Content-free projections of a workflow for status, export, and session
//! inspection (ADR 031 §8).
//!
//! Everything here is derived from the canonical snapshot. No view carries
//! a prompt, request text, message body, bundle text, file content, path
//! outside the repository, environment, or database path: messages appear
//! as kinds, states, and digests, bundles as their stripped receipts, and
//! verifier commands as argv templates.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use vibemux_harness::AgentKind;
use vibemux_store::{AttemptPurpose, LeaseRecord, WorkflowSnapshot};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::WorkflowPhase,
    leases::LeaseState,
    messages::{MessageKind, MessageState, Participant},
    receipts::WorkflowReceipt,
    slots::EvidenceClass,
    task_spec::WorkflowMode,
    workflow_record::{TaskProgress, WorkflowRecord},
};

use super::{coordinator::RunReport, error::WorkflowError};

pub const STATUS_SCHEMA_VERSION: u32 = 1;
pub const EXPORT_SCHEMA_VERSION: u32 = 1;
/// Encoded budget of one export page, below the Control frame bound.
pub const EXPORT_PAGE_BYTES: usize = 40 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct TaskView {
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub progress: TaskProgress,
    pub turns_used: u32,
    pub max_turns: u32,
    pub repairs_used: u32,
    pub max_repairs: u32,
    pub required_suites: Vec<SpecIdentifier>,
    pub accepted_candidate: Option<Sha256Digest>,
    pub failure_code: Option<String>,
}

/// One worker or reviewer session, as recorded by its attempts.
#[derive(Clone, Debug, Serialize)]
pub struct SessionView {
    pub session_id: Uuid,
    pub role: &'static str,
    pub task_key: SpecIdentifier,
    pub slot_id: Option<SpecIdentifier>,
    pub harness: Option<AgentKind>,
    pub route: Option<String>,
    pub attempts: u32,
    pub candidates: Vec<Sha256Digest>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LeaseView {
    pub lease_id: Uuid,
    pub task_key: SpecIdentifier,
    pub slot_id: SpecIdentifier,
    pub generation: u64,
    pub state: LeaseState,
    pub end_reason: Option<String>,
}

impl From<&LeaseRecord> for LeaseView {
    fn from(lease: &LeaseRecord) -> Self {
        Self {
            lease_id: lease.lease_id,
            task_key: lease.task_key.clone(),
            slot_id: lease.slot_id.clone(),
            generation: lease.generation,
            state: lease.state,
            end_reason: lease.end_reason.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MessageView {
    pub message_id: Uuid,
    pub kind: MessageKind,
    pub sender: String,
    pub recipient: String,
    pub state: MessageState,
    pub recipient_sequence: u64,
    pub depth: u32,
    pub reply_to: Option<Uuid>,
    pub escalated: bool,
    pub body_sha256: Sha256Digest,
    pub body_bytes: u64,
}

/// Receipt counts and the identities an acceptance report needs.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReceiptSummary {
    pub counts: BTreeMap<&'static str, u32>,
    pub candidates: Vec<CandidateView>,
    pub reviews: Vec<ReviewView>,
    pub verifications: Vec<VerificationView>,
    pub integration: Option<IntegrationView>,
    pub selection: Option<SelectionView>,
    pub bundles: Vec<BundleView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CandidateView {
    pub candidate_digest: Sha256Digest,
    pub task_key: SpecIdentifier,
    pub worker_session_id: Uuid,
    pub worker_route: String,
    pub worker_harness: AgentKind,
    pub origin: &'static str,
    pub admitted: bool,
    pub violation_codes: Vec<String>,
    pub mutated_after_checkpoint: bool,
    pub files: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewView {
    pub task_key: SpecIdentifier,
    pub candidate_digest: Sha256Digest,
    pub reviewer_session_id: Uuid,
    pub reviewer_route: String,
    pub verdict: vibemux_workflow::gates::ReviewVerdict,
    pub findings_count: u32,
    pub candidate_unchanged: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerificationView {
    pub task_key: Option<SpecIdentifier>,
    pub subject_digest: Sha256Digest,
    pub suites: Vec<SuiteView>,
    pub subject_unchanged: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SuiteView {
    pub suite_id: SpecIdentifier,
    pub status: vibemux_workflow::gates::SuiteStatus,
    pub tests_total: u32,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IntegrationView {
    pub applied_candidates: Vec<Sha256Digest>,
    pub integration_tree_digest: Sha256Digest,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectionView {
    pub winner: Option<Sha256Digest>,
    pub excluded: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct BundleView {
    pub bundle_id: Uuid,
    pub source_task: SpecIdentifier,
    pub audience: Vec<String>,
    pub request_message_id: Option<Uuid>,
    pub snapshot_digest: Sha256Digest,
    pub byte_count: u64,
    pub content_digest: Sha256Digest,
    pub redacted_values: u32,
}

/// The content-free status of one workflow.
#[derive(Clone, Debug, Serialize)]
pub struct WorkflowStatus {
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
    pub blocked_reason: Option<String>,
    /// The digest `workflow start` must present.
    pub start_contract: Option<Sha256Digest>,
    /// A coordinator of this daemon is driving the workflow.
    pub running_here: bool,
    pub tasks: Vec<TaskView>,
    pub sessions: Vec<SessionView>,
    pub leases: Vec<LeaseView>,
    pub messages: Vec<MessageView>,
    pub receipts: ReceiptSummary,
    pub last_run: Option<RunReport>,
}

fn task_views(record: &WorkflowRecord) -> Vec<TaskView> {
    record
        .tasks
        .iter()
        .map(|task| TaskView {
            task_key: task.task_key.clone(),
            contract_id: task.contract_id,
            contract_version: task.contract_version,
            progress: task.progress,
            turns_used: task.turns_used,
            max_turns: task.max_turns,
            repairs_used: task.repairs_used,
            max_repairs: task.max_repairs,
            required_suites: task.required_suites.clone(),
            accepted_candidate: task.accepted_candidate,
            failure_code: task.failure_code.clone(),
        })
        .collect()
}

/// A worker session the plan assigns, whether or not it ran yet.
#[derive(Clone, Debug)]
pub struct PlannedSession {
    pub session_id: Uuid,
    pub task_key: SpecIdentifier,
    pub slot_id: SpecIdentifier,
    pub harness: AgentKind,
    pub route: String,
}

/// Every planned worker session and every session that admitted an
/// attempt, with what it produced.
#[must_use]
pub fn session_views(snapshot: &WorkflowSnapshot, planned: &[PlannedSession]) -> Vec<SessionView> {
    let mut sessions: BTreeMap<Uuid, SessionView> = planned
        .iter()
        .map(|session| {
            (
                session.session_id,
                SessionView {
                    session_id: session.session_id,
                    role: "worker",
                    task_key: session.task_key.clone(),
                    slot_id: Some(session.slot_id.clone()),
                    harness: Some(session.harness),
                    route: Some(session.route.clone()),
                    attempts: 0,
                    candidates: Vec::new(),
                },
            )
        })
        .collect();
    for attempt in &snapshot.attempts {
        let lease = snapshot
            .leases
            .iter()
            .find(|lease| lease.lease_id == attempt.lease_id);
        let entry = sessions
            .entry(attempt.session_id)
            .or_insert_with(|| SessionView {
                session_id: attempt.session_id,
                role: if attempt.purpose == AttemptPurpose::Review {
                    "reviewer"
                } else {
                    "worker"
                },
                task_key: attempt.task_key.clone(),
                slot_id: lease.map(|lease| lease.slot_id.clone()),
                harness: None,
                route: None,
                attempts: 0,
                candidates: Vec::new(),
            });
        entry.attempts += 1;
    }
    for receipt in &snapshot.receipts {
        match receipt {
            WorkflowReceipt::Candidate(candidate) => {
                if let Some(session) = sessions.get_mut(&candidate.candidate.worker_session_id) {
                    session.harness = Some(candidate.candidate.worker_harness);
                    session.route = Some(candidate.candidate.worker_route.clone());
                    session
                        .candidates
                        .push(candidate.candidate.candidate_digest);
                }
            }
            WorkflowReceipt::Review(review) => {
                if let Some(session) = sessions.get_mut(&review.reviewer_session_id) {
                    session.harness = Some(review.reviewer_harness);
                    session.route = Some(review.reviewer_route.clone());
                }
            }
            _ => {}
        }
    }
    sessions.into_values().collect()
}

fn message_views(snapshot: &WorkflowSnapshot) -> Vec<MessageView> {
    snapshot
        .messages
        .iter()
        .map(|message| MessageView {
            message_id: message.envelope.message_id,
            kind: message.envelope.kind,
            sender: message.envelope.sender.label(),
            recipient: message.envelope.recipient.label(),
            state: message.state,
            recipient_sequence: message.recipient_sequence,
            depth: message.envelope.depth,
            reply_to: message.envelope.reply_to,
            escalated: message.envelope.escalated,
            body_sha256: message.envelope.body_sha256,
            body_bytes: message.envelope.body_bytes,
        })
        .collect()
}

fn origin_label(origin: &vibemux_workflow::gates::CandidateOrigin) -> &'static str {
    match origin {
        vibemux_workflow::gates::CandidateOrigin::LiveWorker => "live_worker",
        vibemux_workflow::gates::CandidateOrigin::FixtureWorker => "fixture_worker",
        vibemux_workflow::gates::CandidateOrigin::InjectedMutation { .. } => "injected_mutation",
    }
}

#[must_use]
pub fn receipt_summary(snapshot: &WorkflowSnapshot) -> ReceiptSummary {
    let mut summary = ReceiptSummary::default();
    for receipt in &snapshot.receipts {
        *summary.counts.entry(receipt.kind()).or_default() += 1;
        match receipt {
            WorkflowReceipt::Candidate(candidate) => summary.candidates.push(CandidateView {
                candidate_digest: candidate.candidate.candidate_digest,
                task_key: candidate.candidate.task_key.clone(),
                worker_session_id: candidate.candidate.worker_session_id,
                worker_route: candidate.candidate.worker_route.clone(),
                worker_harness: candidate.candidate.worker_harness,
                origin: origin_label(&candidate.candidate.origin),
                admitted: candidate.admitted,
                violation_codes: candidate.violation_codes.clone(),
                mutated_after_checkpoint: candidate.mutated_after_checkpoint,
                files: candidate
                    .manifest
                    .as_ref()
                    .map_or(0, |manifest| manifest.entries.len()),
            }),
            WorkflowReceipt::Review(review) => summary.reviews.push(ReviewView {
                task_key: review.task_key.clone(),
                candidate_digest: review.candidate_digest,
                reviewer_session_id: review.reviewer_session_id,
                reviewer_route: review.reviewer_route.clone(),
                verdict: review.verdict,
                findings_count: review.findings_count,
                candidate_unchanged: review.candidate_unchanged,
            }),
            WorkflowReceipt::Verification(verification) => {
                summary.verifications.push(VerificationView {
                    task_key: verification.task_key.clone(),
                    subject_digest: verification.subject_digest,
                    suites: verification
                        .suites
                        .iter()
                        .map(|suite| SuiteView {
                            suite_id: suite.suite_id.clone(),
                            status: suite.status,
                            tests_total: suite.tests_total,
                            tests_passed: suite.tests_passed,
                            tests_failed: suite.tests_failed,
                            blocked_reason: suite.blocked_reason.clone(),
                        })
                        .collect(),
                    subject_unchanged: verification.subject_unchanged,
                });
            }
            WorkflowReceipt::Integration(integration) => {
                summary.integration = Some(IntegrationView {
                    applied_candidates: integration.applied_candidates.clone(),
                    integration_tree_digest: integration.integration_tree_digest,
                    conflicts: integration.conflicts.clone(),
                });
            }
            WorkflowReceipt::Selection(selection) => {
                summary.selection = Some(SelectionView {
                    winner: selection.outcome.winner,
                    excluded: serde_json::to_value(&selection.outcome.excluded)
                        .unwrap_or(Value::Null),
                });
            }
            WorkflowReceipt::Bundle(bundle) => summary.bundles.push(BundleView {
                bundle_id: bundle.bundle_id,
                source_task: bundle.source_task.clone(),
                audience: bundle.audience.iter().map(Participant::label).collect(),
                request_message_id: bundle.request_message_id,
                snapshot_digest: bundle.snapshot_digest,
                byte_count: bundle.byte_count,
                content_digest: bundle.content_digest,
                redacted_values: bundle.redaction.redacted_values,
            }),
            _ => {}
        }
    }
    summary
}

#[must_use]
pub fn status_view(
    snapshot: &WorkflowSnapshot,
    planned: &[PlannedSession],
    start_contract: Option<Sha256Digest>,
    running_here: bool,
    last_run: Option<RunReport>,
) -> WorkflowStatus {
    let record = &snapshot.record;
    WorkflowStatus {
        schema_version: STATUS_SCHEMA_VERSION,
        workflow_id: record.workflow_id,
        request_key: record.request_key.clone(),
        workflow_key: record.workflow_key.clone(),
        mode: record.mode,
        phase: record.phase,
        version: record.version,
        evidence_class: record.evidence_class,
        base_commit: record.base_commit.clone(),
        policy_digest: record.policy_digest,
        verifier_digest: record.verifier_digest,
        blocked_reason: record.blocked_reason.clone(),
        start_contract,
        running_here,
        tasks: task_views(record),
        sessions: session_views(snapshot, planned),
        leases: snapshot.leases.iter().map(LeaseView::from).collect(),
        messages: message_views(snapshot),
        receipts: receipt_summary(snapshot),
        last_run,
    }
}

/// One page of the full export: content-free documents in a fixed order
/// (the record, every contract artifact, lease, attempt, receipt, and
/// message), cut to fit a Control frame.
#[derive(Clone, Debug, Serialize)]
pub struct ExportPage {
    pub schema_version: u32,
    pub workflow_id: Uuid,
    pub total_items: usize,
    pub items: Vec<Value>,
    /// Cursor of the next page; `None` on the last one.
    pub next: Option<usize>,
}

/// The export items of a snapshot, each tagged with its kind.
pub fn export_items(
    snapshot: &WorkflowSnapshot,
    status: &WorkflowStatus,
) -> Result<Vec<Value>, WorkflowError> {
    let tagged = |kind: &str, value: Result<Value, serde_json::Error>| {
        value
            .map(|body| serde_json::json!({"kind": kind, "body": body}))
            .map_err(|_| WorkflowError::Internal)
    };
    let mut items = vec![
        tagged("status", serde_json::to_value(status))?,
        tagged("record", serde_json::to_value(&snapshot.record))?,
    ];
    for contract in &snapshot.contracts {
        items.push(tagged("contract", serde_json::to_value(contract))?);
    }
    for lease in &snapshot.leases {
        items.push(tagged("lease", serde_json::to_value(lease))?);
    }
    for attempt in &snapshot.attempts {
        items.push(tagged("attempt", serde_json::to_value(attempt))?);
    }
    for receipt in &snapshot.receipts {
        items.push(tagged("receipt", serde_json::to_value(receipt))?);
    }
    for message in &snapshot.messages {
        items.push(tagged("message", serde_json::to_value(message))?);
    }
    Ok(items)
}

/// The page of `items` starting at `cursor`, at most [`EXPORT_PAGE_BYTES`]
/// encoded. An item larger than a page is refused rather than truncated.
pub fn export_page(
    workflow_id: Uuid,
    items: &[Value],
    cursor: usize,
) -> Result<ExportPage, WorkflowError> {
    if cursor > items.len() {
        return Err(WorkflowError::RequestInvalid);
    }
    let mut page = Vec::new();
    let mut bytes = 0;
    let mut next = cursor;
    for item in &items[cursor..] {
        let size = serde_json::to_vec(item)
            .map_err(|_| WorkflowError::Internal)?
            .len();
        if bytes + size > EXPORT_PAGE_BYTES {
            if page.is_empty() {
                return Err(WorkflowError::Internal);
            }
            break;
        }
        bytes += size;
        page.push(item.clone());
        next += 1;
    }
    Ok(ExportPage {
        schema_version: EXPORT_SCHEMA_VERSION,
        workflow_id,
        total_items: items.len(),
        items: page,
        next: (next < items.len()).then_some(next),
    })
}

/// What `session inspect` shows: the session's identity, its attempts by
/// purpose, and whether a native terminal can be attached (never, yet).
#[derive(Clone, Debug, Serialize)]
pub struct SessionInspection {
    pub workflow_id: Uuid,
    pub session: SessionView,
    pub attempts: Vec<AttemptView>,
    pub interface_mode: &'static str,
    pub native_tui_attachable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptView {
    pub request_id: Uuid,
    pub purpose: AttemptPurpose,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub contract_id: Sha256Digest,
    pub rendered_prompt_sha256: Sha256Digest,
}

#[must_use]
pub fn inspect_session(
    snapshot: &WorkflowSnapshot,
    planned: &[PlannedSession],
    session_id: Uuid,
) -> Option<SessionInspection> {
    let session = session_views(snapshot, planned)
        .into_iter()
        .find(|session| session.session_id == session_id)?;
    let attempts = snapshot
        .attempts
        .iter()
        .filter(|attempt| attempt.session_id == session_id)
        .map(|attempt| AttemptView {
            request_id: attempt.request_id,
            purpose: attempt.purpose,
            lease_id: attempt.lease_id,
            lease_generation: attempt.lease_generation,
            contract_id: attempt.contract_id,
            rendered_prompt_sha256: attempt.rendered_prompt_sha256,
        })
        .collect();
    Some(SessionInspection {
        workflow_id: snapshot.record.workflow_id,
        session,
        attempts,
        interface_mode: "structured",
        native_tui_attachable: false,
    })
}

/// Whether a phase allows a coordinator to run.
#[must_use]
pub const fn resumable(phase: WorkflowPhase) -> bool {
    matches!(
        phase,
        WorkflowPhase::Prepared | WorkflowPhase::Paused | WorkflowPhase::Blocked
    )
}
