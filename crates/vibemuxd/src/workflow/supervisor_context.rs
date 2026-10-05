//! Bounded supervisor data reads and scoped, claim-bound broker effects.
//!
//! This module never opens a live worktree or accepts paths from the model.
//! Candidate files come from admitted immutable manifests. Retained bundles
//! require both a live per-workflow owner and a present writer content record.
//! Text is returned only as explicitly untrusted data in the next model call.

use super::{
    broker::{DEFAULT_MESSAGE_TTL_MS, grant_body, stage_bundle},
    error::WorkflowError,
    runtime::{RunContext, now_ms},
    turns::session_id,
};
use crate::harness_dispatch::call_writer;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ContentKind, ContentState, MessageRecord, WorkflowSnapshot};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    context_bundle::{
        BUNDLE_DIGEST_DOMAIN, BundleRequest, ContextBundle, FileSelection, SnapshotView,
        build_bundle, redact,
    },
    messages::{
        AdmissionFacts, MessageDraft, MessageKind, MessageState, Participant, TaskFacts, admit,
    },
    receipts::{CandidateReceipt, WorkflowReceipt},
    snapshot::SnapshotChange,
    supervisor::{
        MAX_SUPERVISOR_CONTEXT_BYTES, MAX_SUPERVISOR_CONTEXT_ITEMS, SupervisorAction,
        SupervisorActionState, SupervisorExecution, SupervisorExecutionState,
        SupervisorMessageBinding, SupervisorObservation, SupervisorTool,
        context_actions::{bundle_permitted, effect_id, path_permitted, recipient},
    },
    task_spec::{ContextSourceKind, TaskSpec, WorkflowMode},
    workflow_record::ContentStoreMode,
};

fn refused(code: &str) -> WorkflowError {
    WorkflowError::Dispatch { code: code.into() }
}

fn task_spec<'a>(
    context: &'a RunContext,
    task: &SpecIdentifier,
) -> Result<&'a TaskSpec, WorkflowError> {
    context
        .plan
        .specs
        .get(task)
        .ok_or_else(|| refused("supervisor_unknown_task"))
}

fn owns_candidate(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    candidate: &CandidateReceipt,
) -> bool {
    candidate.workflow_id == context.workflow_id
        && candidate.admitted
        && !candidate.mutated_after_checkpoint
        && candidate.manifest.as_ref().is_some_and(|manifest| {
            manifest.base_commit == snapshot.record.base_commit
                && manifest.digest().ok() == Some(candidate.candidate.candidate_digest)
        })
        && snapshot
            .record
            .task(&candidate.candidate.task_key)
            .is_some_and(|task| task.contract_id == candidate.candidate.contract_id)
        && context
            .plan
            .assignments
            .get(&candidate.candidate.task_key)
            .is_some_and(|slots| {
                slots.iter().any(|slot| {
                    session_id(context.workflow_id, &candidate.candidate.task_key, slot)
                        == candidate.candidate.worker_session_id
                })
            })
}

fn owns_bundle(context: &RunContext, snapshot: &WorkflowSnapshot, bundle: &ContextBundle) -> bool {
    bundle.workflow_id == context.workflow_id && bundle.base_commit == snapshot.record.base_commit
        && bundle.audience.contains(&Participant::Supervisor)
        && snapshot.receipts.iter().any(|receipt| matches!(receipt, WorkflowReceipt::Candidate(candidate)
            if owns_candidate(context, snapshot, candidate) && candidate.candidate.task_key == bundle.source_task
                && candidate.candidate.candidate_digest == bundle.snapshot_digest
                && bundle.source == Participant::Worker {task_key: bundle.source_task.clone(), session_id: candidate.candidate.worker_session_id}))
}

/// The content gate serializes reads with purge; expiry is checked per owner.
pub(crate) async fn retained_bundle_text(
    context: &RunContext,
    bundle: &ContextBundle,
) -> Result<Option<String>, WorkflowError> {
    let state = context.setup.state.clone();
    let _permit = state.content_permit().await?;
    retained_bundle_text_locked(context, bundle).await
}

async fn retained_bundle_text_locked(
    context: &RunContext,
    bundle: &ContextBundle,
) -> Result<Option<String>, WorkflowError> {
    let state = context.setup.state.clone();
    let record = context.record().await?;
    if !matches!(record.content_store, ContentStoreMode::Enabled { .. })
        || bundle.workflow_id != context.workflow_id
    {
        return Ok(None);
    }
    let workflow_id = context.workflow_id;
    let bundle_id = bundle.bundle_id;
    let index_state = state.clone();
    let entry = tokio::task::spawn_blocking(move || {
        Ok::<_, WorkflowError>(index_state.content_index()?.into_iter().find(|entry| {
            entry.workflow_id == workflow_id
                && entry.bundle_id == Some(bundle_id)
                && entry
                    .retain_until_ms
                    .is_some_and(|expiry| expiry > now_ms())
        }))
    })
    .await
    .map_err(|_| WorkflowError::Internal)??;
    let Some(entry) = entry else {
        return Ok(None);
    };
    let writer = context.writer.clone();
    let digest = entry.sha256;
    let Some(content) = call_writer(move || writer.workflow_content(digest)).await? else {
        return Ok(None);
    };
    // The filesystem index supplies the per-workflow owner. The global digest
    // row can belong to another workflow sharing identical bytes; it supplies
    // only the present/tombstone and exact-byte checks, never authorization.
    if content.state != ContentState::Present
        || content.kind != ContentKind::Bundle
        || content.byte_count != bundle.byte_count
        || content.retain_until_ms <= now_ms()
        || bundle.byte_count > MAX_SUPERVISOR_CONTEXT_BYTES as u64
    {
        return Ok(None);
    }
    let expected = bundle.content_digest;
    let maximum = bundle.byte_count;
    tokio::task::spawn_blocking(move || {
        let Some(bytes) = state.read_supervisor_content(digest, maximum)? else {
            return Ok(None);
        };
        if bytes.len() as u64 != maximum
            || Sha256Digest::of(&bytes) != digest
            || Sha256Digest::of_fields(BUNDLE_DIGEST_DOMAIN, &[&bytes]) != expected
        {
            return Err(refused("supervisor_context_content_mismatch"));
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| refused("supervisor_context_invalid_utf8"))
    })
    .await
    .map_err(|_| WorkflowError::Internal)?
}

async fn message_text(
    context: &RunContext,
    message: &MessageRecord,
) -> Result<Option<String>, WorkflowError> {
    let envelope = &message.envelope;
    if envelope.workflow_id != context.workflow_id
        || !envelope.audience.contains(&Participant::Supervisor)
        || envelope.expires_at_ms <= now_ms()
        || message.state != MessageState::Admitted
        || message.delivered_in_attempt.is_some()
        || message.acknowledged_in_attempt.is_some()
        || envelope.created_at_ms > now_ms()
        || envelope.body_bytes > vibemux_workflow::messages::MAX_MESSAGE_BODY_BYTES as u64
    {
        return Ok(None);
    }
    let state = context.setup.state.clone();
    let envelope = envelope.clone();
    tokio::task::spawn_blocking(move || {
        let Some(bytes) = state.read_supervisor_outbox(
            envelope.workflow_id,
            envelope.message_id,
            envelope.body_bytes,
        )?
        else {
            return Ok(None);
        };
        if bytes.len() as u64 != envelope.body_bytes
            || Sha256Digest::of(&bytes) != envelope.body_sha256
        {
            return Err(refused("supervisor_context_content_mismatch"));
        }
        String::from_utf8(bytes)
            .map(|text| Some(redact(&text).0))
            .map_err(|_| refused("supervisor_context_invalid_utf8"))
    })
    .await
    .map_err(|_| WorkflowError::Internal)?
}

fn query_message_permitted(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    task_key: &SpecIdentifier,
    message: &MessageRecord,
) -> bool {
    let Some(spec) = context.plan.specs.get(task_key) else {
        return false;
    };
    if !spec
        .context_policy
        .retrieval_order
        .contains(&ContextSourceKind::PublicMessages)
    {
        return false;
    }
    let envelope = &message.envelope;
    let known = |participant: &Participant| match participant {
        Participant::Worker {
            task_key,
            session_id: current_session,
        } => context.plan.assignments.get(task_key).is_some_and(|slots| {
            slots
                .iter()
                .any(|slot| session_id(context.workflow_id, task_key, slot) == *current_session)
        }),
        Participant::Supervisor | Participant::Operator | Participant::Broker => true,
    };
    let contract_task = envelope
        .recipient
        .task_key()
        .or_else(|| envelope.sender.task_key());
    envelope.workflow_id == context.workflow_id
        && known(&envelope.sender)
        && known(&envelope.recipient)
        && (envelope.sender.task_key() == Some(task_key)
            || envelope.recipient.task_key() == Some(task_key))
        && contract_task
            .and_then(|key| snapshot.record.task(key))
            .is_some_and(|task| {
                task.contract_id == envelope.contract_id
                    && task.contract_version == envelope.contract_version
            })
}

/// Builds from immutable owned bytes, filtered by BOTH source and recipient
/// read scopes. No live-path read, instruction interpolation or scope widening.
async fn candidate_bundle(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    action: &SupervisorAction,
    candidate: &CandidateReceipt,
    target: Option<&SpecIdentifier>,
    request_message_id: Option<Uuid>,
    audience: Vec<Participant>,
) -> Result<ContextBundle, WorkflowError> {
    if !owns_candidate(context, snapshot, candidate)
        || !matches!(
            snapshot.record.content_store,
            ContentStoreMode::Enabled { .. }
        )
    {
        return Err(refused("supervisor_context_unavailable"));
    }
    let source_spec = task_spec(context, &candidate.candidate.task_key)?;
    let target_spec = target.map(|task| task_spec(context, task)).transpose()?;
    let permitted = |spec: &TaskSpec| {
        spec.context_policy.allow_public_excerpts
            && spec
                .context_policy
                .retrieval_order
                .contains(&ContextSourceKind::CandidateSnapshot)
    };
    if !permitted(source_spec) || target_spec.is_some_and(|spec| !permitted(spec)) {
        return Err(refused("supervisor_context_policy_denied"));
    }
    let maximum = source_spec
        .context_policy
        .max_bundle_bytes
        .min(target_spec.map_or(u32::MAX, |spec| spec.context_policy.max_bundle_bytes))
        .min((MAX_SUPERVISOR_CONTEXT_BYTES - 1024) as u32);
    let mut entries = Vec::new();
    let mut total = 0_u64;
    for entry in &candidate
        .manifest
        .as_ref()
        .ok_or(WorkflowError::Snapshot)?
        .entries
    {
        if !path_permitted(
            source_spec,
            &entry.path,
            &context.plan.policy.protected_paths,
        ) || target_spec.is_some_and(|spec| {
            !path_permitted(spec, &entry.path, &context.plan.policy.protected_paths)
        }) {
            continue;
        }
        if let SnapshotChange::Write { sha256, byte_count } = entry.change {
            total = total
                .checked_add(byte_count)
                .ok_or(WorkflowError::Snapshot)?;
            if total > u64::from(maximum) {
                return Err(refused("supervisor_context_budget_exceeded"));
            }
            entries.push((entry.path.clone(), sha256, byte_count));
        }
    }
    if entries.is_empty() {
        return Err(refused("supervisor_context_policy_denied"));
    }
    let state = context.setup.state.clone();
    let files = tokio::task::spawn_blocking(move || {
        let mut files = BTreeMap::new();
        for (path, digest, bytes) in entries {
            let content = state
                .read_supervisor_candidate(digest, bytes)?
                .ok_or_else(|| refused("supervisor_context_unavailable"))?;
            if content.len() as u64 != bytes || Sha256Digest::of(&content) != digest {
                return Err(WorkflowError::Snapshot);
            }
            let text = String::from_utf8(content).map_err(|_| WorkflowError::Snapshot)?;
            if !text.is_empty() {
                files.insert(path, text);
            }
        }
        Ok::<_, WorkflowError>(files)
    })
    .await
    .map_err(|_| WorkflowError::Internal)??;
    let selections = files
        .iter()
        .map(|(path, text)| {
            Ok(FileSelection {
                path: path.clone(),
                start_line: 1,
                end_line: u32::try_from(text.split_inclusive('\n').count())
                    .map_err(|_| WorkflowError::Snapshot)?,
            })
        })
        .collect::<Result<Vec<_>, WorkflowError>>()?;
    if selections.is_empty() {
        return Err(refused("supervisor_context_unavailable"));
    }
    let source = Participant::Worker {
        task_key: candidate.candidate.task_key.clone(),
        session_id: candidate.candidate.worker_session_id,
    };
    build_bundle(
        &BundleRequest {
            bundle_id: effect_id(action.action_id, "bundle"),
            workflow_id: context.workflow_id,
            request_message_id,
            selections: &selections,
            claims: &[],
            audience,
            max_bytes: maximum,
        },
        SnapshotView {
            task_key: &candidate.candidate.task_key,
            source: &source,
            snapshot_digest: candidate.candidate.candidate_digest,
            base_commit: &snapshot.record.base_commit,
            files: &files,
        },
    )
    .map_err(|error| refused(error.code()))
}

async fn stage_copy(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    action: &SupervisorAction,
    source: &ContextBundle,
) -> Result<ContextBundle, WorkflowError> {
    if !owns_bundle(context, snapshot, source)
        || !bundle_permitted(
            source,
            task_spec(context, &source.source_task)?,
            &context.plan.policy.protected_paths,
        )
    {
        return Err(refused("supervisor_context_policy_denied"));
    }
    // Copy retained text under an action-owned bundle identity without trying
    // to re-render the content-free receipt's deliberately cleared excerpts.
    let state = context.setup.state.clone();
    let _permit = state.content_permit().await?;
    retained_bundle_text_locked(context, source)
        .await?
        .ok_or_else(|| refused("supervisor_context_unavailable"))?;
    let mut bundle = source.clone();
    bundle.bundle_id = effect_id(action.action_id, "bundle");
    bundle.audience = vec![Participant::Supervisor];
    bundle.request_message_id = None;
    let workflow_id = context.workflow_id;
    let source_id = source.bundle_id;
    let bundle_id = bundle.bundle_id;
    // A read adds a receipt alias; it never renews retention or recreates a
    // purged blob. Keep the gate until the alias and receipt are committed.
    tokio::task::spawn_blocking(move || {
        let mut owner = state
            .content_index()?
            .into_iter()
            .find(|entry| {
                entry.workflow_id == workflow_id
                    && entry.bundle_id == Some(source_id)
                    && entry
                        .retain_until_ms
                        .is_some_and(|expiry| expiry > now_ms())
            })
            .ok_or_else(|| refused("supervisor_context_unavailable"))?;
        owner.bundle_id = Some(bundle_id);
        state.add_content_index(owner)
    })
    .await
    .map_err(|_| WorkflowError::Internal)??;
    let writer = context.writer.clone();
    let receipt = WorkflowReceipt::Bundle(bundle.clone());
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            receipt.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(bundle)
}

async fn send(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    to: &SpecIdentifier,
    body: &str,
    parent: Option<&MessageRecord>,
) -> Result<Uuid, WorkflowError> {
    let target =
        recipient(execution, to).ok_or_else(|| refused("supervisor_recipient_ambiguous"))?;
    let spec = task_spec(context, to)?;
    if spec.communication_policy.max_messages_per_turn == 0
        || !spec
            .context_policy
            .retrieval_order
            .contains(&ContextSourceKind::PublicMessages)
    {
        return Err(refused("supervisor_context_policy_denied"));
    }
    if matches!(action.action, SupervisorTool::SendMessage { .. })
        && action.message_body != Some(SupervisorMessageBinding::for_text(body))
    {
        return Err(refused("supervisor_message_body_mismatch"));
    }
    let task = snapshot.record.task(to).ok_or(WorkflowError::Internal)?;
    let facts = [TaskFacts {
        task_key: to.clone(),
        participant: target,
        contract_id: task.contract_id,
        contract_version: task.contract_version,
        may_message: spec.communication_policy.may_message.clone(),
        max_messages_per_turn: spec.communication_policy.max_messages_per_turn,
        current_snapshot: None,
    }];
    let message_id = effect_id(action.action_id, "message");
    let envelope = admit(
        &MessageDraft {
            message_id,
            kind: if parent.is_some() {
                MessageKind::ContextGrant
            } else {
                MessageKind::Question
            },
            to: to.clone(),
            claimed_sender: None,
            reply_to: parent.map(|message| message.envelope.message_id),
            contract_version: task.contract_version,
            source_refs: vec![],
            body: body.into(),
            ttl_ms: DEFAULT_MESSAGE_TTL_MS,
        },
        &Participant::Supervisor,
        AdmissionFacts {
            workflow_id: context.workflow_id,
            tasks: &facts,
            now_ms: now_ms(),
            sent_this_turn: 0,
            parent_depth: parent.map(|message| message.envelope.depth),
            parent_correlation: parent.map(|message| message.envelope.correlation_id),
        },
    )
    .map_err(|error| refused(error.code()))?;
    let state = context.setup.state.clone();
    let workflow_id = context.workflow_id;
    let staged = body.to_string();
    tokio::task::spawn_blocking(move || state.put_outbox(workflow_id, message_id, &staged))
        .await
        .map_err(|_| WorkflowError::Internal)??;
    let writer = context.writer.clone();
    let action_id = action.action_id;
    let generation = action.generation;
    let full_plan = serde_json::to_value(&context.plan).map_err(|_| WorkflowError::Internal)?;
    call_writer(move || {
        writer.admit_supervisor_message(
            action_id,
            generation,
            full_plan.clone(),
            envelope.clone(),
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(message_id)
}

pub(crate) async fn execute(
    context: &RunContext,
    action: &SupervisorAction,
    body: Option<&str>,
) -> Result<Vec<Uuid>, WorkflowError> {
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let status = call_writer(move || writer.supervisor_status(workflow_id))
        .await?
        .ok_or(WorkflowError::Internal)?;
    if context.stopping()
        || status.execution.state != SupervisorExecutionState::Running
        || status.execution.generation != action.generation
        || action.workflow_id != workflow_id
        || action.state != SupervisorActionState::Claimed
        || !status.actions.contains(action)
        || context.plan.digest()? != status.execution.plan_digest
    {
        return Err(refused("supervisor_stale_generation"));
    }
    let snapshot = context.snapshot().await?;
    if !snapshot.record.phase.admits_work() {
        return Err(refused("supervisor_task_not_active"));
    }
    let refs = match &action.action {
        SupervisorTool::SendMessage { to, .. } => Ok(vec![
            send(
                context,
                &snapshot,
                &status.execution,
                action,
                to,
                body.ok_or_else(|| refused("supervisor_message_body_unavailable"))?,
                None,
            )
            .await?,
        ]),
        SupervisorTool::ShareContext {
            from_task,
            to_task,
            request_message_id,
        } => {
            let source_spec = task_spec(context, from_task)?;
            let target_spec = task_spec(context, to_task)?;
            if snapshot.record.mode != WorkflowMode::Cooperate
                || from_task == to_task
                || !source_spec
                    .communication_policy
                    .may_message
                    .contains(to_task)
                || !target_spec
                    .communication_policy
                    .may_message
                    .contains(from_task)
            {
                return Err(refused("supervisor_context_policy_denied"));
            }
            let target = recipient(&status.execution, to_task)
                .ok_or_else(|| refused("supervisor_recipient_ambiguous"))?;
            let source = recipient(&status.execution, from_task)
                .ok_or_else(|| refused("supervisor_recipient_ambiguous"))?;
            let request = snapshot
                .messages
                .iter()
                .find(|message| {
                    message.envelope.message_id == *request_message_id
                        && message.envelope.workflow_id == workflow_id
                        && message.envelope.sender == target
                        && message.envelope.recipient == source
                        && message.envelope.expires_at_ms > now_ms()
                        && !matches!(
                            message.state,
                            MessageState::Expired | MessageState::Rejected
                        )
                        && matches!(
                            message.envelope.kind,
                            MessageKind::Question | MessageKind::ContextRequest
                        )
                })
                .ok_or_else(|| refused("supervisor_context_request_mismatch"))?;
            let candidate = snapshot
                .receipts
                .iter()
                .rev()
                .find_map(|receipt| match receipt {
                    WorkflowReceipt::Candidate(candidate)
                        if candidate.candidate.task_key == *from_task
                            && owns_candidate(context, &snapshot, candidate) =>
                    {
                        Some(candidate)
                    }
                    _ => None,
                })
                .ok_or_else(|| refused("supervisor_context_unavailable"))?;
            let bundle = candidate_bundle(
                context,
                &snapshot,
                action,
                candidate,
                Some(to_task),
                Some(*request_message_id),
                vec![target, Participant::Supervisor],
            )
            .await?;
            stage_bundle(context, &snapshot.record, &bundle).await?;
            let message_id = send(
                context,
                &snapshot,
                &status.execution,
                action,
                to_task,
                &grant_body(bundle.bundle_id),
                Some(request),
            )
            .await?;
            Ok(vec![bundle.bundle_id, message_id])
        }
        SupervisorTool::ReadArtifact { artifact_digest } => {
            if let Some(candidate) = snapshot.receipts.iter().find_map(|receipt| match receipt {
                WorkflowReceipt::Candidate(candidate)
                    if candidate.candidate.candidate_digest == *artifact_digest
                        && owns_candidate(context, &snapshot, candidate) =>
                {
                    Some(candidate)
                }
                _ => None,
            }) {
                let bundle = candidate_bundle(
                    context,
                    &snapshot,
                    action,
                    candidate,
                    None,
                    None,
                    vec![Participant::Supervisor],
                )
                .await?;
                stage_bundle(context, &snapshot.record, &bundle).await?;
                Ok(vec![bundle.bundle_id])
            } else if let Some(bundle) =
                snapshot.receipts.iter().find_map(|receipt| match receipt {
                    WorkflowReceipt::Bundle(bundle)
                        if bundle.content_digest == *artifact_digest =>
                    {
                        Some(bundle)
                    }
                    _ => None,
                })
            {
                Ok(vec![
                    stage_copy(context, &snapshot, action, bundle)
                        .await?
                        .bundle_id,
                ])
            } else {
                Err(refused("supervisor_artifact_unknown"))
            }
        }
        SupervisorTool::QueryContext { task_key } => {
            let spec = task_spec(context, task_key)?;
            let mut refs = Vec::new();
            let mut used_bytes = 0;
            if spec
                .context_policy
                .retrieval_order
                .contains(&ContextSourceKind::CandidateSnapshot)
                && matches!(
                    snapshot.record.content_store,
                    ContentStoreMode::Enabled { .. }
                )
            {
                if let Some(candidate) =
                    snapshot
                        .receipts
                        .iter()
                        .rev()
                        .find_map(|receipt| match receipt {
                            WorkflowReceipt::Candidate(candidate)
                                if candidate.candidate.task_key == *task_key
                                    && owns_candidate(context, &snapshot, candidate) =>
                            {
                                Some(candidate)
                            }
                            _ => None,
                        })
                {
                    let bundle = candidate_bundle(
                        context,
                        &snapshot,
                        action,
                        candidate,
                        None,
                        None,
                        vec![Participant::Supervisor],
                    )
                    .await?;
                    used_bytes += bundle.byte_count as usize;
                    stage_bundle(context, &snapshot.record, &bundle).await?;
                    refs.push(bundle.bundle_id);
                }
            }
            for message in
                snapshot.messages.iter().rev().filter(|message| {
                    query_message_permitted(context, &snapshot, task_key, message)
                })
            {
                if refs.len() >= MAX_SUPERVISOR_CONTEXT_ITEMS {
                    break;
                }
                if let Some(text) = message_text(context, message).await? {
                    if used_bytes.saturating_add(text.len())
                        > spec.context_policy.max_bundle_bytes as usize
                        || used_bytes + text.len() > MAX_SUPERVISOR_CONTEXT_BYTES
                    {
                        continue;
                    }
                    used_bytes += text.len();
                    refs.push(message.envelope.message_id);
                }
            }
            if refs.is_empty() {
                return Err(refused("supervisor_context_unavailable"));
            }
            Ok(refs)
        }
        _ => Err(refused("supervisor_tool_unavailable")),
    }?;
    if matches!(
        action.action,
        SupervisorTool::ReadArtifact { .. } | SupervisorTool::QueryContext { .. }
    ) {
        let writer = context.writer.clone();
        let action_id = action.action_id;
        let generation = action.generation;
        let full_plan = serde_json::to_value(&context.plan).map_err(|_| WorkflowError::Internal)?;
        let read_refs = refs.clone();
        call_writer(move || {
            writer.record_supervisor_context_read(
                action_id,
                generation,
                full_plan.clone(),
                read_refs.clone(),
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
    }
    Ok(refs)
}

/// Re-read only the immediately preceding round's admitted results. Expiry,
/// purge or scope changes yield content-free failure data, never stale text.
pub(crate) async fn model_results(
    context: &RunContext,
    observation: &SupervisorObservation,
) -> Result<Vec<Value>, WorkflowError> {
    let snapshot = context.snapshot().await?;
    let mut results = Vec::new();
    let mut remaining = MAX_SUPERVISOR_CONTEXT_BYTES;
    for action in observation.last_actions.iter().filter(|action| {
        action.state == SupervisorActionState::Applied
            && observation
                .last_round
                .as_ref()
                .is_some_and(|round| round.round_id == action.round_id)
            && matches!(
                action.action,
                SupervisorTool::ReadArtifact { .. } | SupervisorTool::QueryContext { .. }
            )
    }) {
        for id in &action.result_refs {
            if results.len() >= MAX_SUPERVISOR_CONTEXT_ITEMS {
                break;
            }
            let text = if let Some(bundle) =
                snapshot.receipts.iter().find_map(|receipt| match receipt {
                    WorkflowReceipt::Bundle(bundle) if bundle.bundle_id == *id => Some(bundle),
                    _ => None,
                }) {
                if owns_bundle(context, &snapshot, bundle)
                    && bundle_permitted(
                        bundle,
                        task_spec(context, &bundle.source_task)?,
                        &context.plan.policy.protected_paths,
                    )
                {
                    retained_bundle_text(context, bundle).await?
                } else {
                    None
                }
            } else if let SupervisorTool::QueryContext { task_key } = &action.action {
                if let Some(message) = snapshot.messages.iter().find(|message| {
                    message.envelope.message_id == *id
                        && query_message_permitted(context, &snapshot, task_key, message)
                }) {
                    message_text(context, message).await?
                } else {
                    None
                }
            } else {
                None
            };
            let value = json!({"action_id":action.action_id,"reference_id":id,"trust":"untrusted_data",
                "status":if text.is_some() {"available"} else {"unavailable"},"text":text});
            let bytes = serde_json::to_vec(&value)
                .map_err(|_| WorkflowError::Internal)?
                .len();
            if bytes <= remaining {
                remaining -= bytes;
                results.push(value);
            }
        }
    }
    Ok(results)
}
