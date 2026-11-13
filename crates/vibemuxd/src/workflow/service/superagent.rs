//! Operator intake, bounded follow-ups, and worker report reads (ADR 057).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vibemux_store::{ContentState, MessageRecord};
use vibemux_workflow::{
    messages::{AdmissionFacts, MessageDraft, MessageKind, Participant, TaskFacts, admit},
    worker_report::{WorkerReport, WorkerReportRecord},
};

use super::*;
use crate::workflow::{broker::DEFAULT_MESSAGE_TTL_MS, turns::derived_id};

/// Only an authenticated local operator may submit this envelope. Source
/// metadata and message text never select an identity or grant authority.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeRequest {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub request: Value,
    pub policy: Value,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FollowupRequest {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub workflow_id: Uuid,
    pub expected_workflow_version: u64,
    pub task_key: SpecIdentifier,
    pub session_id: Uuid,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub kind: MessageKind,
    pub text: String,
}

// Both envelopes contain user text. Keep their debug views content-free.
impl std::fmt::Debug for IntakeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntakeRequest")
            .field("event_id", &self.event_id)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for FollowupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FollowupRequest")
            .field("event_id", &self.event_id)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FollowupResponse {
    pub message: MessageRecord,
    pub duplicate: bool,
    pub context_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportView {
    pub sequence: u64,
    pub claim: WorkerReportRecord,
    /// Returned only on an explicit content read with opt-in retention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<WorkerReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportsResponse {
    pub schema_version: u32,
    pub reports: Vec<ReportView>,
    pub next: u64,
    pub more: bool,
}

impl WorkflowService {
    pub async fn intake(&self, mut intake: IntakeRequest) -> Result<Value, WorkflowError> {
        if intake.schema_version != 1 || intake.event_id.is_nil() {
            return Err(WorkflowError::RequestInvalid);
        }
        // Derive the key from the immutable intake event, never from prose.
        let request = intake
            .request
            .as_object_mut()
            .ok_or(WorkflowError::RequestInvalid)?;
        request.insert(
            "request_key".into(),
            json!(format!("intake_{}", intake.event_id.simple())),
        );
        let original = request
            .get("request_text")
            .and_then(Value::as_str)
            .ok_or(WorkflowError::RequestInvalid)?;
        let source_digest = Sha256Digest::of(original.as_bytes());
        let prepared = self.prepare(intake.request, intake.policy).await?;
        let snapshot = self.snapshot(prepared.workflow_id).await?;
        Ok(json!({"schema_version":1, "event_id":intake.event_id,
            "authority":"authenticated_operator", "prepared":prepared,
            "mandate":{"original_request_sha256":source_digest,
                "policy_digest":snapshot.record.policy_digest,
                "base_commit":snapshot.record.base_commit,
                "contracts":snapshot.contracts}}))
    }

    /// Stages context and atomically binds its admission to current contracts.
    /// This never resumes a terminal workflow or changes a task's scope.
    pub async fn followup(
        &self,
        request: FollowupRequest,
    ) -> Result<FollowupResponse, WorkflowError> {
        let _gate = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::ShuttingDown)?;
        self.accepting()?;
        let setup = self.inner.setup()?;
        if request.schema_version != 1
            || request.event_id.is_nil()
            || request.session_id.is_nil()
            || !matches!(
                request.kind,
                MessageKind::Question | MessageKind::Progress | MessageKind::Finding
            )
            || request.expected_workflow_version == 0
            || request.contract_version == 0
        {
            return Err(WorkflowError::FollowupInvalid);
        }
        let snapshot = self.snapshot(request.workflow_id).await?;
        let task = snapshot
            .record
            .task(&request.task_key)
            .ok_or(WorkflowError::FollowupInvalid)?;
        if request.contract_id != task.contract_id
            || request.contract_version != task.contract_version
        {
            return Err(WorkflowError::ContractMismatch);
        }
        let participant = Participant::Worker {
            task_key: request.task_key.clone(),
            session_id: request.session_id,
        };
        let facts = [TaskFacts {
            task_key: request.task_key.clone(),
            participant,
            contract_id: task.contract_id,
            contract_version: task.contract_version,
            may_message: Vec::new(),
            max_messages_per_turn: 0,
            current_snapshot: None,
        }];
        let message_id = derived_id(
            "vibemux.workflow.operator_followup.v1",
            &[request.workflow_id.as_bytes(), request.event_id.as_bytes()],
        );
        let draft = MessageDraft {
            message_id,
            kind: request.kind,
            to: request.task_key,
            claimed_sender: None,
            reply_to: None,
            contract_version: request.contract_version,
            source_refs: Vec::new(),
            body: request.text,
            ttl_ms: DEFAULT_MESSAGE_TTL_MS,
        };
        let envelope = admit(
            &draft,
            &Participant::Operator,
            AdmissionFacts {
                workflow_id: request.workflow_id,
                tasks: &facts,
                now_ms: now_ms(),
                sent_this_turn: 0,
                parent_depth: None,
                parent_correlation: None,
            },
        )
        .map_err(|_| WorkflowError::FollowupInvalid)?;
        let writer = self.writer().clone();
        if call_writer(move || writer.workflow_message(message_id))
            .await?
            .is_some()
        {
            // The writer compares every immutable envelope field. It returns
            // the original receipt without restaging acknowledged content.
            let writer = self.writer().clone();
            let version = request.expected_workflow_version;
            let commit = call_writer(move || {
                writer.admit_workflow_followup(envelope.clone(), version, OffsetDateTime::now_utc())
            })
            .await?;
            return Ok(FollowupResponse {
                message: commit.message,
                duplicate: commit.event.is_none(),
                context_only: true,
            });
        }
        let state = setup.state.clone();
        let workflow_id = request.workflow_id;
        let body = draft.body;
        tokio::task::spawn_blocking(move || state.put_outbox(workflow_id, message_id, &body))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let writer = self.writer().clone();
        let version = request.expected_workflow_version;
        let result = call_writer(move || {
            writer.admit_workflow_followup(envelope.clone(), version, OffsetDateTime::now_utc())
        })
        .await;
        match result {
            Ok(commit) => Ok(FollowupResponse {
                duplicate: commit.event.is_none(),
                message: commit.message,
                context_only: true,
            }),
            Err(error) => {
                // Remove only a definitely unadmitted body. A failed writer
                // receipt may be ambiguous; retain those bytes for reconciliation.
                let writer = self.writer().clone();
                if matches!(
                    call_writer(move || writer.workflow_message(message_id)).await,
                    Ok(None)
                ) {
                    let state = setup.state.clone();
                    tokio::task::spawn_blocking(move || {
                        state.delete_outbox(workflow_id, message_id)
                    })
                    .await
                    .map_err(|_| WorkflowError::Internal)??;
                }
                Err(error.into())
            }
        }
    }

    pub async fn worker_reports(
        &self,
        workflow_id: Uuid,
        after: u64,
        include_content: bool,
    ) -> Result<ReportsResponse, WorkflowError> {
        let writer = self.writer().clone();
        let page = call_writer(move || writer.workflow_worker_reports(workflow_id, after)).await?;
        let mut reports = Vec::new();
        for (sequence, claim) in page.items {
            let content = if include_content {
                self.report_content(&claim).await?
            } else {
                None
            };
            reports.push(ReportView {
                sequence,
                claim,
                content,
            });
        }
        Ok(ReportsResponse {
            schema_version: 1,
            reports,
            next: page.next,
            more: page.more,
        })
    }

    async fn report_content(
        &self,
        claim: &WorkerReportRecord,
    ) -> Result<Option<WorkerReport>, WorkflowError> {
        let snapshot = self.snapshot(claim.workflow_id).await?;
        if !matches!(
            snapshot.record.content_store,
            vibemux_workflow::workflow_record::ContentStoreMode::Enabled { .. }
        ) {
            return Ok(None);
        }
        let setup = self.inner.setup()?;
        let _gate = setup.state.content_permit().await?;
        let writer = self.writer().clone();
        let digest = claim.content_sha256;
        let Some(entry) = call_writer(move || writer.workflow_content(digest)).await? else {
            return Ok(None);
        };
        if entry.state != ContentState::Present || entry.retain_until_ms <= now_ms() {
            return Ok(None);
        }
        let state = setup.state.clone();
        let workflow_id = claim.workflow_id;
        let bytes = tokio::task::spawn_blocking(move || {
            let owns_content = state.content_index()?.iter().any(|entry| {
                entry.workflow_id == workflow_id
                    && entry.sha256 == digest
                    && entry
                        .retain_until_ms
                        .is_some_and(|deadline| deadline > now_ms())
            });
            if !owns_content {
                return Ok(None);
            }
            state.read_supervisor_content(digest, 8192)
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if Sha256Digest::of(&bytes) != digest {
            return Err(WorkflowError::FollowupInvalid);
        }
        let report: WorkerReport =
            serde_json::from_slice(&bytes).map_err(|_| WorkflowError::FollowupInvalid)?;
        if !report.valid() || report.digest().ok() != Some(claim.report_digest) {
            return Err(WorkflowError::FollowupInvalid);
        }
        Ok(Some(report))
    }
}
