//! Executes only claimed supervisor intents through the established effect owners.

use std::collections::{BTreeMap, BTreeSet};

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::SupervisorActionFinish;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::{RECEIPT_SCHEMA_VERSION, WorkflowPhase},
    receipts::{SelectionRecord, WorkflowReceipt},
    renderer::TurnPurpose,
    supervisor::{
        SupervisorAction, SupervisorActionState, SupervisorExecutionState, SupervisorTool,
    },
    task_spec::WorkflowMode,
    workflow_record::TaskProgress,
};

use super::{
    broker::pending_deliveries,
    candidate::{StepInput, UnitState, review_candidate, verify_candidate},
    coordinator::{
        RUN_FAILING_CODES, RunEnd, after_steps, fail, restore_latest, run_steps,
        stop as stop_coordinator,
    },
    error::WorkflowError,
    integration::integrate,
    runtime::{Control, RunContext, Unit},
};
use crate::harness_dispatch::call_writer;

fn refused(code: &str) -> WorkflowError {
    WorkflowError::Dispatch { code: code.into() }
}

/// A pause is resumable only after every reserved model/effect outcome is
/// settled. Unknown work remains blocked for reconciliation, with charges and
/// intent identities retained. Cancellation remains a truthful terminal state.
pub(crate) async fn stop_or_recover(context: &RunContext) -> Result<RunEnd, WorkflowError> {
    if context.control() == Control::Pause {
        let writer = context.writer.clone();
        let workflow_id = context.workflow_id;
        let status = call_writer(move || writer.supervisor_status(workflow_id))
            .await?
            .ok_or(WorkflowError::Internal)?;
        if status.execution.state == SupervisorExecutionState::RecoveryRequired
            || status.execution.active_round.is_some()
            || status.actions.iter().any(|action| {
                matches!(
                    action.state,
                    SupervisorActionState::Claimed | SupervisorActionState::Unknown
                )
            })
        {
            let record = context.record().await?;
            if record.phase.can_transition_to(WorkflowPhase::Blocked) {
                context
                    .transition(WorkflowPhase::Blocked, Some("supervisor_recovery_required"))
                    .await?;
            }
            return Ok(RunEnd::Blocked("supervisor_recovery_required".into()));
        }
    }
    stop_coordinator(context).await
}

/// Worktrees are opened only after an intent is claimed. Candidate lookup is
/// bound to its task and worker session, including same-task comparisons.
async fn state_index(
    context: &RunContext,
    units: &[Unit],
    states: &mut Vec<UnitState>,
    task_key: &SpecIdentifier,
    slot: Option<&SpecIdentifier>,
    digest: Option<Sha256Digest>,
) -> Result<usize, WorkflowError> {
    let snapshot = context.snapshot().await?;
    let session = match digest {
        Some(digest) => Some(
            snapshot
                .receipts
                .iter()
                .rev()
                .find_map(|receipt| match receipt {
                    WorkflowReceipt::Candidate(candidate)
                        if candidate.admitted
                            && candidate.candidate.task_key == *task_key
                            && candidate.candidate.candidate_digest == digest =>
                    {
                        Some(candidate.candidate.worker_session_id)
                    }
                    _ => None,
                })
                .ok_or_else(|| refused("supervisor_candidate_unknown"))?,
        ),
        None => None,
    };
    let unit = units
        .iter()
        .find(|unit| {
            unit.task_key == *task_key
                && slot.is_none_or(|slot| unit.slot.slot_id == *slot)
                && session.is_none_or(|session| unit.session_id == session)
        })
        .ok_or(WorkflowError::SlotUnavailable)?;
    if let Some(index) = states
        .iter()
        .position(|state| state.unit.session_id == unit.session_id)
    {
        return Ok(index);
    }
    let workspace = context.workspace(&unit.key()).await?;
    let latest = restore_latest(context, &snapshot, unit).await?;
    states.push(UnitState {
        unit: unit.clone(),
        workspace,
        latest,
    });
    Ok(states.len() - 1)
}

async fn finish(
    context: &RunContext,
    action: &SupervisorAction,
    state: SupervisorActionState,
    result_refs: Vec<Uuid>,
    code: &str,
) -> Result<(), WorkflowError> {
    let writer = context.writer.clone();
    let finish = SupervisorActionFinish {
        action_id: action.action_id,
        generation: action.generation,
        state,
        result_refs,
        result_code: Some(SpecIdentifier::new(code).map_err(|_| WorkflowError::Internal)?),
        timestamp: OffsetDateTime::now_utc(),
    };
    call_writer(move || writer.finish_supervisor_action(finish.clone())).await?;
    Ok(())
}

async fn accept_if_ready(
    context: &RunContext,
    task_key: &SpecIdentifier,
    digest: Sha256Digest,
) -> Result<(), WorkflowError> {
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let observation = call_writer(move || writer.supervisor_observation(workflow_id)).await?;
    if observation.workflow.mode == WorkflowMode::Compare
        || observation
            .workflow
            .task(task_key)
            .is_some_and(|task| task.progress == TaskProgress::Accepted)
        || !observation.candidates.iter().any(|candidate| {
            candidate.gate_passed
                && candidate.candidate.task_key == *task_key
                && candidate.candidate.candidate_digest == digest
        })
    {
        return Ok(());
    }
    let task_key = task_key.clone();
    context
        .versioned(move |writer, version| {
            writer.accept_workflow_candidate(
                workflow_id,
                version,
                task_key.clone(),
                digest,
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
    Ok(())
}

/// A proposal is validated as one batch before any claim. Independent worker
/// assignments run concurrently within the existing width; every future joins
/// before the next model observation is built.
pub(crate) async fn execute_batch(
    context: &RunContext,
    units: &[Unit],
    states: &mut Vec<UnitState>,
    actions: Vec<SupervisorAction>,
    width: usize,
) -> Result<Option<RunEnd>, WorkflowError> {
    let mut claimed = Vec::new();
    for action in actions {
        if action.state == SupervisorActionState::Applied
            || action.state == SupervisorActionState::Blocked
        {
            continue;
        }
        if context.stopping() {
            return stop_or_recover(context).await.map(Some);
        }
        let writer = context.writer.clone();
        let action_id = action.action_id;
        let generation = action.generation;
        claimed.push(
            call_writer(move || {
                writer.claim_supervisor_action(action_id, generation, OffsetDateTime::now_utc())
            })
            .await?,
        );
    }
    let mut assignments = BTreeMap::new();
    let mut inputs = Vec::new();
    for action in &claimed {
        if let SupervisorTool::AssignTask { task_key, slot_id } = &action.action {
            let index = state_index(context, units, states, task_key, Some(slot_id), None).await?;
            let state = &states[index];
            let deliveries = pending_deliveries(context, state).await?;
            let purpose = if state.latest.is_some() {
                TurnPurpose::Repair
            } else {
                TurnPurpose::Implement
            };
            let notes = if purpose == TurnPurpose::Repair {
                vec!["Repair the current candidate against the unchanged contract and trusted gate receipts.".into()]
            } else {
                Vec::new()
            };
            inputs.push((
                index,
                StepInput {
                    request_id: action.dispatch_request_id,
                    purpose,
                    notes,
                    deliveries,
                },
            ));
            assignments.insert(index, action);
        }
    }
    if !inputs.is_empty() {
        let outcomes = run_steps(context, states, inputs, width.max(1)).await;
        let forbidden_write = outcomes.iter().find_map(|(_, outcome)| {
            outcome
                .as_ref()
                .ok()?
                .recorded
                .as_ref()?
                .receipt
                .violation_codes
                .iter()
                .find(|code| RUN_FAILING_CODES.contains(&code.as_str()))
                .cloned()
        });
        for (index, outcome) in &outcomes {
            let action = assignments.get(index).ok_or(WorkflowError::Internal)?;
            match outcome {
                Ok(outcome) => {
                    let refs = outcome
                        .recorded
                        .as_ref()
                        .map(|candidate| candidate.receipt.receipt_id)
                        .into_iter()
                        .collect();
                    let (state, code) = if outcome.turn.completed() {
                        (
                            SupervisorActionState::Applied,
                            "supervisor_worker_completed",
                        )
                    } else {
                        (
                            SupervisorActionState::Blocked,
                            "supervisor_worker_not_completed",
                        )
                    };
                    finish(context, action, state, refs, code).await?;
                }
                Err(_) => {
                    finish(
                        context,
                        action,
                        SupervisorActionState::Unknown,
                        vec![],
                        "supervisor_effect_unresolved",
                    )
                    .await?
                }
            }
        }
        if let Some(end) = after_steps(context, states, outcomes).await? {
            return Ok(Some(end));
        }
        if let Some(code) = forbidden_write {
            if context.stopping() {
                return stop_or_recover(context).await.map(Some);
            }
            return Ok(Some(fail(context, &code).await));
        }
    }
    for action in claimed
        .iter()
        .filter(|action| !matches!(action.action, SupervisorTool::AssignTask { .. }))
    {
        if context.stopping() {
            return stop_or_recover(context).await.map(Some);
        }
        match execute_one(context, units, states, action).await {
            Ok((refs, end)) => {
                let (state, code) = if matches!(end, Some(RunEnd::Failed(_))) {
                    (SupervisorActionState::Blocked, "supervisor_action_failed")
                } else {
                    (SupervisorActionState::Applied, "supervisor_action_applied")
                };
                finish(context, action, state, refs, code).await?;
                if end.is_some() {
                    return Ok(end);
                }
            }
            Err(error) => {
                finish(
                    context,
                    action,
                    SupervisorActionState::Unknown,
                    vec![],
                    "supervisor_effect_unresolved",
                )
                .await?;
                return Err(error);
            }
        }
    }
    Ok(None)
}

async fn execute_one(
    context: &RunContext,
    units: &[Unit],
    states: &mut Vec<UnitState>,
    action: &SupervisorAction,
) -> Result<(Vec<Uuid>, Option<RunEnd>), WorkflowError> {
    match &action.action {
        SupervisorTool::InspectWorkflow {} | SupervisorTool::ListSlots {} => {
            let writer = context.writer.clone();
            let workflow_id = context.workflow_id;
            call_writer(move || writer.supervisor_observation(workflow_id)).await?;
            Ok((vec![], None))
        }
        SupervisorTool::ProposePlan { .. }
        | SupervisorTool::PauseRun { .. }
        | SupervisorTool::RequestCancel { task_key: Some(_) } => Ok((vec![], None)),
        SupervisorTool::RequestCancel { task_key: None } => {
            Ok((vec![], Some(stop_or_recover(context).await?)))
        }
        SupervisorTool::ReportBlocked { reason_code } => {
            Ok((vec![], Some(RunEnd::Blocked(reason_code.to_string()))))
        }
        SupervisorTool::RequestReview {
            task_key,
            candidate_digest,
        }
        | SupervisorTool::RequestVerification {
            task_key,
            candidate_digest,
        } => {
            let index = state_index(
                context,
                units,
                states,
                task_key,
                None,
                Some(*candidate_digest),
            )
            .await?;
            let state = &states[index];
            let candidate = state
                .latest
                .as_ref()
                .filter(|candidate| candidate.digest() == *candidate_digest)
                .ok_or_else(|| refused("supervisor_candidate_stale"))?;
            let receipt_id = if matches!(action.action, SupervisorTool::RequestReview { .. }) {
                review_candidate(context, &state.unit, candidate, action.dispatch_request_id)
                    .await?
                    .receipt
                    .receipt_id
            } else {
                verify_candidate(context, &state.unit, candidate)
                    .await?
                    .receipt_id
            };
            accept_if_ready(context, task_key, *candidate_digest).await?;
            Ok((vec![receipt_id], None))
        }
        SupervisorTool::ProposeCandidateSelection { candidate_digest } => {
            let writer = context.writer.clone();
            let workflow_id = context.workflow_id;
            let observation =
                call_writer(move || writer.supervisor_observation(workflow_id)).await?;
            let outcome = observation
                .selection
                .ok_or_else(|| refused("supervisor_selection_unavailable"))?;
            if outcome.winner != *candidate_digest {
                return Err(refused("supervisor_selection_mismatch"));
            }
            let task = observation
                .workflow
                .tasks
                .first()
                .ok_or(WorkflowError::Internal)?;
            let receipt_id = Uuid::new_v4();
            let receipt = WorkflowReceipt::Selection(SelectionRecord {
                schema_version: RECEIPT_SCHEMA_VERSION,
                receipt_id,
                workflow_id,
                contract_id: task.contract_id,
                outcome,
            });
            let writer = context.writer.clone();
            call_writer(move || {
                writer.record_workflow_receipt(
                    workflow_id,
                    receipt.clone(),
                    None,
                    OffsetDateTime::now_utc(),
                )
            })
            .await?;
            if let Some(digest) = candidate_digest {
                let task_key = task.task_key.clone();
                let digest = *digest;
                context
                    .versioned(move |writer, version| {
                        writer.accept_workflow_candidate(
                            workflow_id,
                            version,
                            task_key.clone(),
                            digest,
                            OffsetDateTime::now_utc(),
                        )
                    })
                    .await?;
            }
            Ok((vec![receipt_id], None))
        }
        SupervisorTool::ProposeIntegration { candidate_digests } => {
            let before = context.snapshot().await?;
            let mut accepted = Vec::new();
            for task in &before.record.tasks {
                let digest = task
                    .accepted_candidate
                    .filter(|digest| candidate_digests.contains(digest))
                    .ok_or_else(|| refused("supervisor_integration_not_approved"))?;
                let index =
                    state_index(context, units, states, &task.task_key, None, Some(digest)).await?;
                let state = &states[index];
                accepted.push((
                    state.unit.clone(),
                    state
                        .latest
                        .clone()
                        .filter(|candidate| candidate.digest() == digest)
                        .ok_or_else(|| refused("supervisor_candidate_stale"))?,
                ));
            }
            let outcome = integrate(context, &accepted).await?;
            let old: BTreeSet<_> = before
                .receipts
                .iter()
                .map(WorkflowReceipt::receipt_id)
                .collect();
            let refs = context
                .snapshot()
                .await?
                .receipts
                .iter()
                .filter(|receipt| {
                    matches!(
                        receipt,
                        WorkflowReceipt::Integration(_) | WorkflowReceipt::Verification(_)
                    ) && !old.contains(&receipt.receipt_id())
                })
                .map(WorkflowReceipt::receipt_id)
                .collect();
            Ok((
                refs,
                Some(match outcome {
                    None => RunEnd::Accepted,
                    Some(code) => RunEnd::Failed(code),
                }),
            ))
        }
        _ => Err(refused("supervisor_tool_unavailable")),
    }
}

pub(crate) async fn end_from_phase(context: &RunContext) -> Result<Option<RunEnd>, WorkflowError> {
    let record = context.record().await?;
    Ok(match record.phase {
        WorkflowPhase::Accepted => Some(RunEnd::Accepted),
        WorkflowPhase::Failed => Some(RunEnd::Failed(
            record
                .blocked_reason
                .unwrap_or_else(|| "workflow_failed".into()),
        )),
        WorkflowPhase::Blocked => Some(RunEnd::Blocked(
            record
                .blocked_reason
                .unwrap_or_else(|| "workflow_blocked".into()),
        )),
        WorkflowPhase::Cancelled => Some(RunEnd::Cancelled),
        WorkflowPhase::Paused => Some(RunEnd::Paused),
        WorkflowPhase::CancelRequested => Some(stop_or_recover(context).await?),
        _ => None,
    })
}
