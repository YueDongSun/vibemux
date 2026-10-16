//! Claim-bound, content-free evidence of a daemon context read.
//!
//! The daemon owns bounded private-byte retrieval. It records these references
//! only after reading and verifying those bytes; the writer rechecks current
//! identity, policy, state and time before appending the immutable evidence.

use super::{
    BoundPlan, MAX_PLAN_BYTES, PLAN_DOMAIN, active_execution, append,
    context_actions::{bundle_for_action, claim_sequence},
    load_action, millis, workflow_error, workflow_rows,
};
use crate::{MessageRecord, SqliteStore, StoreError};
use rusqlite::Transaction;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    messages::{MAX_MESSAGE_BODY_BYTES, MAX_MESSAGE_TTL_MS, MessageState, Participant},
    supervisor::{
        MAX_SUPERVISOR_CONTEXT_BYTES, MAX_SUPERVISOR_CONTEXT_ITEMS, SupervisorAction,
        SupervisorActionState, SupervisorExecution, SupervisorTool,
        context_actions::{bundle_permitted, effect_id, participant_is_current},
    },
    task_spec::ContextSourceKind,
    workflow_record::WorkflowRecord,
};

const READ_EVENT: &str = "workflow_supervisor_context_read";
const READ_REFS_DOMAIN: &str = "vibemux.workflow.supervisor_read_references.v1";

fn invalid() -> StoreError {
    workflow_error("store_supervisor_context_read_invalid")
}

fn read_key(action_id: Uuid) -> String {
    format!("supervisor_context_read:{action_id}")
}

pub(super) fn validate_query_message(
    execution: &SupervisorExecution,
    workflow: &WorkflowRecord,
    task_key: &SpecIdentifier,
    message: &MessageRecord,
    now_ms: u64,
) -> Result<(), StoreError> {
    let envelope = &message.envelope;
    let exact_contract = envelope
        .recipient
        .task_key()
        .or_else(|| envelope.sender.task_key())
        .and_then(|key| execution.tasks.get(key).zip(workflow.task(key)))
        .is_some_and(|(binding, task)| {
            task.contract_id == binding.contract_id
                && task.contract_version == binding.contract_version
                && envelope.contract_id == binding.contract_id
                && envelope.contract_version == binding.contract_version
        });
    let current_tasks = [&envelope.sender, &envelope.recipient]
        .iter()
        .all(|participant| {
            participant_is_current(execution, participant)
                && participant.task_key().is_none_or(|key| {
                    execution
                        .tasks
                        .get(key)
                        .zip(workflow.task(key))
                        .is_some_and(|(binding, task)| {
                            binding.contract_id == task.contract_id
                                && binding.contract_version == task.contract_version
                        })
                })
        });
    let mut audience = vec![envelope.recipient.clone(), Participant::Supervisor];
    audience.sort();
    audience.dedup();
    let mut actual_audience = envelope.audience.clone();
    actual_audience.sort();
    if envelope.workflow_id != execution.workflow_id
        || workflow.workflow_id != execution.workflow_id
        || message.state != MessageState::Admitted
        || message.delivered_in_attempt.is_some()
        || message.acknowledged_in_attempt.is_some()
        || envelope.created_at_ms > now_ms
        || envelope.expires_at_ms <= now_ms
        || envelope
            .expires_at_ms
            .saturating_sub(envelope.created_at_ms)
            > MAX_MESSAGE_TTL_MS
        || envelope.body_bytes == 0
        || envelope.body_bytes > MAX_MESSAGE_BODY_BYTES as u64
        || !current_tasks
        || !exact_contract
        || actual_audience != audience
        || !execution.tasks.contains_key(task_key)
        || envelope.sender.task_key() != Some(task_key)
            && envelope.recipient.task_key() != Some(task_key)
    {
        return Err(invalid());
    }
    Ok(())
}

fn reference_fingerprint(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    refs: &[Uuid],
    plan: Option<&BoundPlan>,
    now_ms: u64,
) -> Result<Sha256Digest, StoreError> {
    if refs.is_empty()
        || refs.len() > MAX_SUPERVISOR_CONTEXT_ITEMS
        || refs.iter().collect::<BTreeSet<_>>().len() != refs.len()
    {
        return Err(invalid());
    }
    let workflow = workflow_rows::load_existing_workflow(transaction, action.workflow_id)?.record;
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    for id in refs {
        if *id == effect_id(action.action_id, "bundle") {
            let bundle = bundle_for_action(transaction, action)?;
            let binding = execution
                .tasks
                .get(&bundle.source_task)
                .ok_or_else(invalid)?;
            let task = workflow.task(&bundle.source_task).ok_or_else(invalid)?;
            if task.contract_id != binding.contract_id
                || task.contract_version != binding.contract_version
                || !participant_is_current(execution, &bundle.source)
            {
                return Err(invalid());
            }
            match &action.action {
                SupervisorTool::ReadArtifact { artifact_digest }
                    if *artifact_digest == bundle.snapshot_digest
                        || *artifact_digest == bundle.content_digest => {}
                SupervisorTool::QueryContext { task_key } if *task_key == bundle.source_task => {}
                _ => return Err(invalid()),
            }
            if let Some(plan) = plan {
                let spec = plan.specs.get(&bundle.source_task).ok_or_else(invalid)?;
                if !bundle_permitted(&bundle, spec, &plan.policy.protected_paths) {
                    return Err(workflow_error("supervisor_context_policy_denied"));
                }
            }
            total_bytes = total_bytes
                .checked_add(bundle.byte_count)
                .ok_or_else(invalid)?;
            entries.push(json!({"kind":"bundle","id":id,"record":bundle}));
        } else {
            let SupervisorTool::QueryContext { task_key } = &action.action else {
                return Err(invalid());
            };
            let message = workflow_rows::load_message(transaction, *id)?.ok_or_else(invalid)?;
            validate_query_message(execution, &workflow, task_key, &message, now_ms)?;
            if let Some(plan) = plan
                && !plan.specs.get(task_key).is_some_and(|spec| {
                    spec.context_policy
                        .retrieval_order
                        .contains(&ContextSourceKind::PublicMessages)
                })
            {
                return Err(workflow_error("supervisor_context_policy_denied"));
            }
            total_bytes = total_bytes
                .checked_add(message.envelope.body_bytes)
                .ok_or_else(invalid)?;
            entries.push(json!({"kind":"message","id":id,"envelope":message.envelope,"fingerprint":message.fingerprint}));
        }
    }
    let task_limit = match (&action.action, plan) {
        (SupervisorTool::QueryContext { task_key }, Some(plan)) => u64::from(
            plan.specs
                .get(task_key)
                .ok_or_else(invalid)?
                .context_policy
                .max_bundle_bytes,
        ),
        _ => MAX_SUPERVISOR_CONTEXT_BYTES as u64,
    };
    if total_bytes > task_limit.min(MAX_SUPERVISOR_CONTEXT_BYTES as u64) {
        return Err(workflow_error("supervisor_context_budget_exceeded"));
    }
    canonical_digest(READ_REFS_DOMAIN, &entries).map_err(|_| invalid())
}

fn evidence_payload(
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    refs: &[Uuid],
    fingerprint: Sha256Digest,
) -> Value {
    json!({"workflow_id":action.workflow_id,"action_id":action.action_id,"generation":action.generation,
        "round_id":action.round_id,"plan_digest":execution.plan_digest,"result_refs":refs,"references_digest":fingerprint})
}

impl SqliteStore {
    /// Records a trusted daemon's completed private read, never model-supplied
    /// text. An applied read requires this exact post-claim evidence.
    pub fn record_supervisor_context_read(
        &mut self,
        action_id: Uuid,
        generation: u64,
        full_plan: &Value,
        refs: &[Uuid],
        timestamp: OffsetDateTime,
    ) -> Result<(), StoreError> {
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
            || !matches!(
                action.action,
                SupervisorTool::ReadArtifact { .. } | SupervisorTool::QueryContext { .. }
            )
            || !workflow.phase.admits_work()
            || canonical_digest(PLAN_DOMAIN, full_plan).map_err(|_| invalid())?
                != execution.plan_digest
        {
            return Err(invalid());
        }
        let fingerprint = reference_fingerprint(
            &transaction,
            &execution,
            &action,
            refs,
            Some(&plan),
            millis(timestamp),
        )?;
        let payload = evidence_payload(&execution, &action, refs, fingerprint);
        if let Some(existing) = SqliteStore::find_duplicate(&transaction, &read_key(action_id))? {
            if existing.payload().value() != &payload
                || existing.sequence().get() <= claim_sequence(&transaction, &action)?
            {
                return Err(workflow_error("store_supervisor_context_read_conflict"));
            }
            return Ok(());
        }
        append(
            &transaction,
            READ_EVENT,
            read_key(action_id),
            payload,
            timestamp,
        )?;
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn validate_read_evidence(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    refs: &[Uuid],
    timestamp: OffsetDateTime,
) -> Result<(), StoreError> {
    let fingerprint = reference_fingerprint(
        transaction,
        execution,
        action,
        refs,
        None,
        millis(timestamp),
    )?;
    let event = SqliteStore::find_duplicate(transaction, &read_key(action.action_id))?
        .ok_or_else(invalid)?;
    if event.sequence().get() <= claim_sequence(transaction, action)?
        || event.payload().value() != &evidence_payload(execution, action, refs, fingerprint)
    {
        return Err(invalid());
    }
    Ok(())
}
