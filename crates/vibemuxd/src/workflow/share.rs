//! Operator context shares (ADR 031 §5).
//!
//! `context share` grants one recorded bundle to one worker session of the
//! same cooperative workflow. The grant is an operator-sent message: it is
//! admitted through the pure rules, persisted by the writer, and delivered
//! only at the recipient's next turn boundary, where the bundle text is
//! read from the opt-in content store and checked against its receipt.
//! Competitors of a comparison never receive each other's context.

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{MessageRecord, WorkflowSnapshot};
use vibemux_workflow::{
    SpecIdentifier,
    context_bundle::ContextBundle,
    gates::WorkflowPhase,
    messages::{AdmissionFacts, MessageDraft, MessageKind, Participant, TaskFacts, admit},
    receipts::WorkflowReceipt,
    task_spec::WorkflowMode,
    workflow_record::ContentStoreMode,
};

use super::{
    broker::{DEFAULT_MESSAGE_TTL_MS, grant_body, stored_bundle_text},
    error::WorkflowError,
    prepare::WorkflowPlan,
    runtime::now_ms,
    state_files::StateFiles,
    turns::{derived_id, session_id},
};
use crate::{WriterHandle, harness_dispatch::call_writer};

const SHARE_ID_DOMAIN: &str = "vibemux.workflow.operator_share.v1";

/// The worker session a share targets.
pub(crate) struct ShareTarget {
    pub task_key: SpecIdentifier,
    pub session_id: Uuid,
}

/// The worker units of a plan as (task, session) pairs.
pub(crate) fn worker_sessions(plan: &WorkflowPlan, workflow_id: Uuid) -> Vec<ShareTarget> {
    plan.assignments
        .iter()
        .flat_map(|(task_key, slots)| {
            slots.iter().map(move |slot_id| ShareTarget {
                task_key: task_key.clone(),
                session_id: session_id(workflow_id, task_key, slot_id),
            })
        })
        .collect()
}

/// The bundle receipt `bundle_id` of a snapshot.
pub(crate) fn find_bundle(snapshot: &WorkflowSnapshot, bundle_id: Uuid) -> Option<&ContextBundle> {
    snapshot.receipts.iter().find_map(|receipt| match receipt {
        WorkflowReceipt::Bundle(bundle) if bundle.bundle_id == bundle_id => Some(bundle),
        _ => None,
    })
}

/// Admits the operator grant of `bundle` to `target`. A repeated share of
/// the same bundle to the same session returns the recorded message.
pub(crate) async fn share_bundle(
    writer: &WriterHandle,
    state: &StateFiles,
    plan: &WorkflowPlan,
    snapshot: &WorkflowSnapshot,
    bundle: &ContextBundle,
    target_session: Uuid,
) -> Result<(MessageRecord, bool), WorkflowError> {
    let record = &snapshot.record;
    let workflow_id = record.workflow_id;
    if record.mode != WorkflowMode::Cooperate
        || !matches!(
            record.phase,
            WorkflowPhase::Running | WorkflowPhase::Paused | WorkflowPhase::Blocked
        )
        || !matches!(record.content_store, ContentStoreMode::Enabled { .. })
    {
        return Err(WorkflowError::ShareInvalid);
    }
    let sessions = worker_sessions(plan, workflow_id);
    let target = sessions
        .iter()
        .find(|session| session.session_id == target_session)
        .ok_or(WorkflowError::SessionNotFound)?;
    if target.task_key == bundle.source_task {
        return Err(WorkflowError::ShareInvalid);
    }
    let max_bytes = plan
        .specs
        .get(&target.task_key)
        .ok_or(WorkflowError::Internal)?
        .context_policy
        .max_bundle_bytes;
    let text = {
        let state = state.clone();
        let bundle = bundle.clone();
        tokio::task::spawn_blocking(move || stored_bundle_text(&state, workflow_id, &bundle))
            .await
            .map_err(|_| WorkflowError::Internal)??
            .ok_or(WorkflowError::ShareInvalid)?
    };
    if text.len() as u64 > u64::from(max_bytes) {
        return Err(WorkflowError::ShareInvalid);
    }
    let message_id = derived_id(
        SHARE_ID_DOMAIN,
        &[bundle.bundle_id.as_bytes(), target_session.as_bytes()],
    );
    let reader = writer.clone();
    if let Some(existing) = call_writer(move || reader.workflow_message(message_id)).await? {
        return Ok((existing, true));
    }
    let facts = task_facts(plan, snapshot, &sessions);
    let task = record
        .task(&target.task_key)
        .ok_or(WorkflowError::Internal)?;
    let body = grant_body(bundle.bundle_id);
    let draft = MessageDraft {
        message_id,
        kind: MessageKind::ContextGrant,
        to: target.task_key.clone(),
        claimed_sender: None,
        reply_to: None,
        contract_version: task.contract_version,
        source_refs: Vec::new(),
        body: body.clone(),
        ttl_ms: DEFAULT_MESSAGE_TTL_MS,
    };
    let envelope = admit(
        &draft,
        &Participant::Operator,
        AdmissionFacts {
            workflow_id,
            tasks: &facts,
            now_ms: now_ms(),
            sent_this_turn: 0,
            parent_depth: None,
            parent_correlation: None,
        },
    )
    .map_err(|_| WorkflowError::ShareInvalid)?;
    let staged = state.clone();
    tokio::task::spawn_blocking(move || staged.put_outbox(workflow_id, message_id, &body))
        .await
        .map_err(|_| WorkflowError::Internal)??;
    let handle = writer.clone();
    let commit = call_writer(move || {
        handle.admit_workflow_message(envelope.clone(), OffsetDateTime::now_utc())
    })
    .await?;
    Ok((commit.message, false))
}

fn task_facts(
    plan: &WorkflowPlan,
    snapshot: &WorkflowSnapshot,
    sessions: &[ShareTarget],
) -> Vec<TaskFacts> {
    sessions
        .iter()
        .filter_map(|session| {
            let task = snapshot.record.task(&session.task_key)?;
            let spec = plan.specs.get(&session.task_key)?;
            let current_snapshot =
                snapshot
                    .receipts
                    .iter()
                    .rev()
                    .find_map(|receipt| match receipt {
                        WorkflowReceipt::Candidate(candidate)
                            if candidate.admitted
                                && candidate.candidate.worker_session_id == session.session_id =>
                        {
                            Some(candidate.candidate.candidate_digest)
                        }
                        _ => None,
                    });
            Some(TaskFacts {
                task_key: session.task_key.clone(),
                participant: Participant::Worker {
                    task_key: session.task_key.clone(),
                    session_id: session.session_id,
                },
                contract_id: task.contract_id,
                contract_version: task.contract_version,
                may_message: spec.communication_policy.may_message.clone(),
                max_messages_per_turn: spec.communication_policy.max_messages_per_turn,
                current_snapshot,
            })
        })
        .collect()
}
