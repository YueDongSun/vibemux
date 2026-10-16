//! Exact claimed supervisor context effects over existing message/bundle stores.

use super::{
    BoundPlan, MAX_PLAN_BYTES, PLAN_DOMAIN, active_execution, append, load_action, millis,
    workflow_error, workflow_rows,
};
use crate::{
    MessageCommit, MessageRecord, SqliteStore, StoreError, WORKFLOW_STORE_RECORD_SCHEMA_VERSION,
};
use rusqlite::{Transaction, params};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    canonical_json::canonical_digest,
    context_bundle::ContextBundle,
    messages::{
        MAX_CORRELATION_DEPTH, MAX_MESSAGE_TTL_MS, MessageEnvelope, MessageKind, MessageState,
        Participant,
    },
    receipts::WorkflowReceipt,
    supervisor::{
        SupervisorAction, SupervisorActionState, SupervisorExecution, SupervisorTaskGate,
        SupervisorTool,
        context_actions::{bundle_permitted, effect_id, recipient},
    },
    task_spec::{ContextSourceKind, WorkflowMode},
    workflow_record::ContentStoreMode,
};

pub(super) fn is_context_action(action: &SupervisorTool) -> bool {
    matches!(
        action,
        SupervisorTool::ReadArtifact { .. }
            | SupervisorTool::QueryContext { .. }
            | SupervisorTool::SendMessage { .. }
            | SupervisorTool::ShareContext { .. }
    )
}

fn invalid() -> StoreError {
    workflow_error("store_supervisor_context_mismatch")
}

/// Deliberately conservative: no text classifier can declassify model output
/// once private tool data has reached this workflow's supervisor history.
pub(super) fn has_private_read(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
) -> Result<bool, StoreError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM workflow_supervisor_actions WHERE workflow_id=? AND state='applied' AND json_extract(record_json,'$.action.tool') IN ('read_artifact','query_context'))",
        [workflow_id.to_string()], |row| row.get(0))?)
}

pub(super) fn validate_proposal_sources(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    proposal: &vibemux_workflow::supervisor::SupervisorProposal,
) -> Result<Vec<String>, StoreError> {
    let mut codes = Vec::new();
    for decision in &proposal.decisions {
        if matches!(decision.action, SupervisorTool::SendMessage { .. })
            && has_private_read(transaction, execution.workflow_id)?
        {
            codes.push("supervisor_message_after_private_read".into());
        }
        if let SupervisorTool::ReadArtifact { artifact_digest } = &decision.action {
            let known = workflow_rows::receipts_for(transaction, execution.workflow_id, None)?
                .iter()
                .any(|receipt| match receipt {
                    WorkflowReceipt::Candidate(candidate) => {
                        candidate.admitted
                            && candidate.candidate.candidate_digest == *artifact_digest
                            && execution
                                .tasks
                                .get(&candidate.candidate.task_key)
                                .is_some_and(|binding| {
                                    binding.contract_id == candidate.candidate.contract_id
                                })
                    }
                    WorkflowReceipt::Bundle(bundle) => {
                        bundle.content_digest == *artifact_digest
                            && bundle.audience.contains(&Participant::Supervisor)
                    }
                    _ => false,
                });
            if !known {
                codes.push("supervisor_artifact_unknown".into());
            }
        }
    }
    Ok(codes)
}

pub(super) fn claim_sequence(
    transaction: &Transaction<'_>,
    action: &SupervisorAction,
) -> Result<u64, StoreError> {
    Ok(transaction.query_row(
        "SELECT updated_sequence FROM workflow_supervisor_actions WHERE action_id=?",
        [action.action_id.to_string()],
        |row| row.get(0),
    )?)
}

pub(super) fn bundle_for_action(
    transaction: &Transaction<'_>,
    action: &SupervisorAction,
) -> Result<ContextBundle, StoreError> {
    let bundle_id = effect_id(action.action_id, "bundle");
    let (json, sequence): (String, u64) = transaction.query_row(
        "SELECT record_json,created_sequence FROM workflow_receipts WHERE workflow_id=? AND receipt_id=?",
        params![action.workflow_id.to_string(), bundle_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| invalid())?;
    let WorkflowReceipt::Bundle(bundle) = serde_json::from_str(&json)? else {
        return Err(invalid());
    };
    if sequence <= claim_sequence(transaction, action)?
        || bundle.bundle_id != bundle_id
        || bundle.workflow_id != action.workflow_id
        || !bundle.audience.contains(&Participant::Supervisor)
    {
        return Err(invalid());
    }
    let workflow = workflow_rows::load_existing_workflow(transaction, action.workflow_id)?.record;
    let known = workflow_rows::receipts_for(transaction, action.workflow_id, Some("candidate"))?.iter().any(|receipt| {
        matches!(receipt, WorkflowReceipt::Candidate(candidate) if candidate.admitted
            && candidate.candidate.task_key == bundle.source_task && candidate.candidate.candidate_digest == bundle.snapshot_digest
            && workflow.task(&bundle.source_task).is_some_and(|task| task.contract_id == candidate.candidate.contract_id)
            && bundle.source == Participant::Worker {task_key: bundle.source_task.clone(), session_id: candidate.candidate.worker_session_id})
    });
    if !known || bundle.base_commit != workflow.base_commit {
        return Err(invalid());
    }
    Ok(bundle)
}

/// Checks fixed effect identity, current target contract and exact body binding.
fn validate_message(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    envelope: &MessageEnvelope,
    now_ms: u64,
) -> Result<(), StoreError> {
    let to = match &action.action {
        SupervisorTool::SendMessage { to, text } if text.is_empty() => to,
        SupervisorTool::ShareContext { to_task, .. } => to_task,
        _ => return Err(invalid()),
    };
    let target = recipient(execution, to).ok_or_else(invalid)?;
    let binding = execution.tasks.get(to).ok_or_else(invalid)?;
    let workflow = workflow_rows::load_existing_workflow(transaction, action.workflow_id)?.record;
    let task = workflow.task(to).ok_or_else(invalid)?;
    if envelope.message_id != effect_id(action.action_id, "message")
        || envelope.workflow_id != action.workflow_id
        || envelope.sender != Participant::Supervisor
        || envelope.recipient != target
        || envelope.contract_id != binding.contract_id
        || envelope.contract_version != binding.contract_version
        || task.contract_id != binding.contract_id
        || task.contract_version != binding.contract_version
        || !envelope.source_refs.is_empty()
        || envelope.escalated
        || envelope.audience.len() != 2
        || !envelope.audience.contains(&Participant::Supervisor)
        || !envelope.audience.contains(&target)
        || envelope.created_at_ms > now_ms
        || envelope.expires_at_ms <= now_ms
        || envelope
            .expires_at_ms
            .saturating_sub(envelope.created_at_ms)
            > MAX_MESSAGE_TTL_MS
        || execution.task_gates.get(to) != Some(&SupervisorTaskGate::Active)
    {
        return Err(invalid());
    }
    match &action.action {
        SupervisorTool::SendMessage { .. } => {
            let body = action.message_body.as_ref().ok_or_else(invalid)?;
            if body.body_sha256 != envelope.body_sha256
                || body.body_bytes != envelope.body_bytes
                || body.body_bytes == 0
                || body.body_bytes > vibemux_workflow::messages::MAX_MESSAGE_BODY_BYTES as u64
                || envelope.kind != MessageKind::Question
                || envelope.reply_to.is_some()
                || envelope.correlation_id != envelope.message_id
                || envelope.depth != 0
            {
                return Err(invalid());
            }
        }
        SupervisorTool::ShareContext {
            from_task,
            to_task,
            request_message_id,
        } => {
            if workflow.mode != WorkflowMode::Cooperate || action.message_body.is_some() {
                return Err(invalid());
            }
            let source = recipient(execution, from_task).ok_or_else(invalid)?;
            let request = workflow_rows::load_message(transaction, *request_message_id)?
                .ok_or_else(invalid)?;
            let source_binding = execution.tasks.get(from_task).ok_or_else(invalid)?;
            if request.envelope.workflow_id != action.workflow_id
                || request.envelope.sender != target
                || request.envelope.recipient != source
                || request.envelope.contract_id != source_binding.contract_id
                || request.envelope.contract_version != source_binding.contract_version
                || !matches!(
                    request.envelope.kind,
                    MessageKind::Question | MessageKind::ContextRequest
                )
                || matches!(
                    request.state,
                    MessageState::Rejected | MessageState::Expired
                )
                || request.envelope.expires_at_ms <= now_ms
                || request.envelope.depth >= MAX_CORRELATION_DEPTH
            {
                return Err(invalid());
            }
            let bundle = bundle_for_action(transaction, action)?;
            let body = format!("bundle:{}", bundle.bundle_id);
            if bundle.source_task != *from_task
                || bundle.source != source
                || !bundle.audience.contains(&target)
                || bundle.audience.len() != 2
                || bundle.request_message_id != Some(*request_message_id)
                || envelope.kind != MessageKind::ContextGrant
                || envelope.reply_to != Some(*request_message_id)
                || envelope.correlation_id != request.envelope.correlation_id
                || envelope.depth != request.envelope.depth + 1
                || envelope.body_sha256 != vibemux_workflow::Sha256Digest::of(body.as_bytes())
                || envelope.body_bytes != body.len() as u64
                || from_task == to_task
            {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

impl SqliteStore {
    /// One writer transaction fences the claim, exact plan, policy and admission.
    pub fn admit_supervisor_message(
        &mut self,
        action_id: Uuid,
        generation: u64,
        full_plan: &Value,
        envelope: &MessageEnvelope,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, StoreError> {
        let bytes = serde_json::to_vec(full_plan)?;
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(invalid());
        }
        let plan: BoundPlan = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let transaction = self.immediate()?;
        let action = load_action(&transaction, action_id)?;
        let execution = active_execution(&transaction, action.workflow_id, generation)?;
        let workflow =
            workflow_rows::load_existing_workflow(&transaction, action.workflow_id)?.record;
        if action.generation != generation
            || action.state != SupervisorActionState::Claimed
            || !workflow.phase.admits_work()
            || canonical_digest(PLAN_DOMAIN, full_plan).map_err(|_| invalid())?
                != execution.plan_digest
        {
            return Err(invalid());
        }
        if matches!(action.action, SupervisorTool::SendMessage { .. })
            && has_private_read(&transaction, action.workflow_id)?
        {
            return Err(workflow_error("supervisor_message_after_private_read"));
        }
        validate_message(
            &transaction,
            &execution,
            &action,
            envelope,
            millis(timestamp),
        )?;
        let to = envelope.recipient.task_key().ok_or_else(invalid)?;
        let target_spec = plan.specs.get(to).ok_or_else(invalid)?;
        if target_spec.communication_policy.max_messages_per_turn == 0
            || !target_spec
                .context_policy
                .retrieval_order
                .contains(&ContextSourceKind::PublicMessages)
        {
            return Err(workflow_error("supervisor_context_policy_denied"));
        }
        if let SupervisorTool::ShareContext { from_task, .. } = &action.action {
            let source_spec = plan.specs.get(from_task).ok_or_else(invalid)?;
            let bundle = bundle_for_action(&transaction, &action)?;
            if !matches!(workflow.content_store, ContentStoreMode::Enabled { .. })
                || !source_spec.communication_policy.may_message.contains(to)
                || !target_spec
                    .communication_policy
                    .may_message
                    .contains(from_task)
                || !bundle_permitted(&bundle, source_spec, &plan.policy.protected_paths)
                || !bundle_permitted(&bundle, target_spec, &plan.policy.protected_paths)
            {
                return Err(workflow_error("supervisor_context_policy_denied"));
            }
        }
        let fingerprint = super::super::message_fingerprint(envelope)?;
        if let Some(existing) = workflow_rows::load_message(&transaction, envelope.message_id)? {
            if existing.fingerprint != fingerprint {
                return Err(workflow_error("store_workflow_message_conflict"));
            }
            // An existing message is evidence only if this action created it after its claim.
            check_message_sequence(&transaction, &action, envelope.message_id)?;
            return Ok(MessageCommit {
                message: existing,
                event: None,
            });
        }
        let outstanding = workflow_rows::messages_for(&transaction, action.workflow_id)?
            .into_iter()
            .filter(|message| {
                message.envelope.sender == Participant::Supervisor
                    && message.envelope.recipient == envelope.recipient
                    && message.envelope.expires_at_ms > millis(timestamp)
                    && matches!(
                        message.state,
                        MessageState::Admitted | MessageState::Delivered
                    )
            })
            .count();
        if outstanding >= target_spec.communication_policy.max_messages_per_turn as usize {
            return Err(workflow_error("supervisor_message_budget_exhausted"));
        }
        let recipient_label = envelope.recipient.label();
        let message = MessageRecord {
            schema_version: WORKFLOW_STORE_RECORD_SCHEMA_VERSION,
            envelope: envelope.clone(),
            recipient_sequence: workflow_rows::next_recipient_sequence(
                &transaction,
                action.workflow_id,
                &recipient_label,
            )?,
            recipient_label,
            state: MessageState::Admitted,
            fingerprint,
            delivered_in_attempt: None,
            acknowledged_in_attempt: None,
            updated_at_ms: millis(timestamp),
        };
        let (event, sequence) = append(
            &transaction,
            "workflow_message_admitted",
            format!("workflow_message:{}:admitted", envelope.message_id),
            json!({"workflow_id":action.workflow_id,"action_id":action_id,"generation":generation,
                "message_id":envelope.message_id,"kind":envelope.kind,"sender":"supervisor",
                "recipient":message.recipient_label,"recipient_sequence":message.recipient_sequence,
                "body_sha256":envelope.body_sha256,"body_bytes":envelope.body_bytes}),
            timestamp,
        )?;
        workflow_rows::insert_message(&transaction, &message, sequence)?;
        transaction.commit()?;
        Ok(MessageCommit {
            message,
            event: Some(event),
        })
    }
}

fn check_message_sequence(
    transaction: &Transaction<'_>,
    action: &SupervisorAction,
    message_id: Uuid,
) -> Result<(), StoreError> {
    let event = SqliteStore::find_duplicate(
        transaction,
        &format!("workflow_message:{message_id}:admitted"),
    )?
    .ok_or_else(invalid)?;
    if event.sequence().get() <= claim_sequence(transaction, action)?
        || event.payload().value()["action_id"] != json!(action.action_id)
        || event.payload().value()["generation"] != json!(action.generation)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn validate_result(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    refs: &[Uuid],
    timestamp: OffsetDateTime,
) -> Result<(), StoreError> {
    let workflow = workflow_rows::load_existing_workflow(transaction, action.workflow_id)?.record;
    match &action.action {
        SupervisorTool::SendMessage { .. } | SupervisorTool::ShareContext { .. } => {
            let message_id = effect_id(action.action_id, "message");
            let expected = if matches!(action.action, SupervisorTool::ShareContext { .. }) {
                vec![effect_id(action.action_id, "bundle"), message_id]
            } else {
                vec![message_id]
            };
            if refs != expected {
                return Err(invalid());
            }
            let message =
                workflow_rows::load_message(transaction, message_id)?.ok_or_else(invalid)?;
            check_message_sequence(transaction, action, message_id)?;
            // A terminal receipt proves admission, not delivery or acknowledgement.
            validate_message(
                transaction,
                execution,
                action,
                &message.envelope,
                message.envelope.created_at_ms,
            )?;
        }
        SupervisorTool::ReadArtifact { .. } | SupervisorTool::QueryContext { .. } => {
            super::context_reads::validate_read_evidence(
                transaction,
                execution,
                action,
                refs,
                timestamp,
            )?;
            if refs.is_empty()
                || refs.len() > vibemux_workflow::supervisor::MAX_SUPERVISOR_CONTEXT_ITEMS
            {
                return Err(invalid());
            }
            for id in refs {
                if *id == effect_id(action.action_id, "bundle") {
                    let bundle = bundle_for_action(transaction, action)?;
                    if bundle.base_commit != workflow.base_commit {
                        return Err(invalid());
                    }
                    match &action.action {
                        SupervisorTool::ReadArtifact { artifact_digest } => {
                            let source_known =
                                workflow_rows::receipts_for(transaction, action.workflow_id, None)?
                                    .iter()
                                    .any(|receipt| match receipt {
                                        WorkflowReceipt::Candidate(candidate) => {
                                            candidate.admitted
                                                && candidate.candidate.candidate_digest
                                                    == *artifact_digest
                                                && candidate.candidate.candidate_digest
                                                    == bundle.snapshot_digest
                                                && candidate.candidate.task_key
                                                    == bundle.source_task
                                                && execution
                                                    .tasks
                                                    .get(&bundle.source_task)
                                                    .is_some_and(|binding| {
                                                        binding.contract_id
                                                            == candidate.candidate.contract_id
                                                    })
                                        }
                                        WorkflowReceipt::Bundle(source) => {
                                            source.bundle_id != bundle.bundle_id
                                                && source.content_digest == *artifact_digest
                                                && source.content_digest == bundle.content_digest
                                                && source.snapshot_digest == bundle.snapshot_digest
                                                && source.source == bundle.source
                                                && source.source_task == bundle.source_task
                                                && source
                                                    .audience
                                                    .contains(&Participant::Supervisor)
                                        }
                                        _ => false,
                                    });
                            if !source_known {
                                return Err(invalid());
                            }
                        }
                        SupervisorTool::QueryContext { task_key }
                            if bundle.source_task == *task_key => {}
                        _ => return Err(invalid()),
                    }
                } else {
                    let SupervisorTool::QueryContext { task_key } = &action.action else {
                        return Err(invalid());
                    };
                    let message =
                        workflow_rows::load_message(transaction, *id)?.ok_or_else(invalid)?;
                    super::context_reads::validate_query_message(
                        execution,
                        &workflow,
                        task_key,
                        &message,
                        millis(timestamp),
                    )?;
                }
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
