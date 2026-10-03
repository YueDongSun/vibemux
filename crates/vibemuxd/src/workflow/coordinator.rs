//! The workflow coordinator (ADR 031 §4–§6).
//!
//! One coordinator drives one workflow through three stages:
//!
//! 1. implementation: every unit without a candidate runs an implement
//!    step, at most `width` at a time;
//! 2. broker rounds (bounded): a unit with a routed question answers it at
//!    its next turn boundary, the answer travels with a scoped bundle, and
//!    the asker continues with a follow-up step;
//! 3. gating: every candidate is reviewed by an independent session and
//!    verified by the trusted suites on its collected bytes. Cooperative
//!    tasks get bounded repairs and are integrated together; a comparison
//!    selects at most one gated competitor (a trusted negative control is
//!    refused alongside) and integrates it.
//!
//! The coordinator reads every decision input from the store, so a daemon
//! restart resumes from persisted receipts and inbox state instead of
//! repeating effects. It never marks anything accepted itself: the writer
//! re-checks every gate inside its transaction.

use std::collections::BTreeMap;

use futures::{StreamExt, stream};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::WorkflowSnapshot;
use vibemux_workflow::{
    SpecIdentifier,
    gates::{
        GateFailure, GateRequirements, RECEIPT_SCHEMA_VERSION, ReviewReceipt, VerifierReceipt,
        WorkflowPhase, candidate_gate,
    },
    messages::MessageKind,
    receipts::{CandidateReceipt, SelectionRecord, WorkflowReceipt},
    renderer::TurnPurpose,
    selection::{CandidateMetrics, ComparisonEntry, SelectionPolicy, select},
    snapshot::SnapshotChange,
    task_spec::WorkflowMode,
    workflow_record::{TaskProgress, WorkflowRecord, WorkflowTask},
};

use super::{
    broker::{
        AdmittedMessage, admit_step_messages, bundle_for_answer, pending_deliveries,
        settle_supervisor_messages,
    },
    candidate::{
        CollectedCandidate, StepInput, StepOutcome, UnitState, negative_control, review_candidate,
        verify_candidate, worker_step,
    },
    error::WorkflowError,
    integration::{CleanupEntry, clean_up_workspaces, integrate},
    runtime::{Control, RunContext, Unit},
    startup::finish_cancel,
};
use crate::harness_dispatch::call_writer;

/// Bound on question/answer rounds before gating.
pub const MAX_BROKER_ROUNDS: usize = 2;
pub const ESCALATED_CODE: &str = "message_escalated";
pub const NO_VALID_CANDIDATE_CODE: &str = "no_valid_candidate";
pub const REPAIRS_EXHAUSTED_CODE: &str = "task_repairs_exhausted";
/// Snapshot violations that fail the Run outright (W01).
const RUN_FAILING_CODES: [&str; 2] = ["snapshot_protected_path", "snapshot_forbidden_path"];

/// How a coordinator stopped.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "end", content = "code")]
pub enum RunEnd {
    Accepted,
    Failed(String),
    Paused,
    Cancelled,
    Blocked(String),
}

/// What a coordinator leaves for status and export.
#[derive(Clone, Debug, Serialize)]
pub struct RunReport {
    pub end: RunEnd,
    pub cleanup: Vec<CleanupEntry>,
}

/// Drives one workflow until it is accepted, failed, paused, cancelled, or
/// blocked. Never panics on a failure: every error ends in a phase.
pub(crate) async fn run(context: &RunContext, units: Vec<Unit>, width: usize) -> RunReport {
    let end = match drive(context, units, width.max(1)).await {
        Ok(end) => end,
        Err(error) if context.stopping() => {
            let _ = error;
            stop(context)
                .await
                .unwrap_or_else(|error| RunEnd::Failed(error.code().to_string()))
        }
        Err(error) => fail(context, error.code()).await,
    };
    let terminal = matches!(
        end,
        RunEnd::Accepted | RunEnd::Failed(_) | RunEnd::Cancelled
    );
    let cleanup = if terminal {
        clean_up_workspaces(context).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    RunReport { end, cleanup }
}

async fn drive(
    context: &RunContext,
    units: Vec<Unit>,
    width: usize,
) -> Result<RunEnd, WorkflowError> {
    let snapshot = context.snapshot().await?;
    let mut states = Vec::with_capacity(units.len());
    for unit in units {
        let workspace = context.workspace(&unit.key()).await?;
        let latest = restore_latest(context, &snapshot, &unit).await?;
        states.push(UnitState {
            unit,
            workspace,
            latest,
        });
    }
    // Stage 1: implementation.
    let record = context.record().await?;
    let inputs: Vec<(usize, StepInput)> = states
        .iter()
        .enumerate()
        .filter(|(_, state)| state.latest.is_none() && task_open(&record, &state.unit.task_key))
        .map(|(index, _)| {
            (
                index,
                StepInput {
                    purpose: TurnPurpose::Implement,
                    notes: Vec::new(),
                    deliveries: Vec::new(),
                },
            )
        })
        .collect();
    let outcomes = run_steps(context, &mut states, inputs, width).await;
    if let Some(end) = after_steps(context, &states, outcomes).await? {
        return Ok(end);
    }
    if context.stopping() {
        return stop(context).await;
    }
    // Stage 2: broker rounds.
    if record.mode == WorkflowMode::Cooperate {
        for _ in 0..MAX_BROKER_ROUNDS {
            let mut progressed = false;
            for purpose in [TurnPurpose::Answer, TurnPurpose::Implement] {
                let inputs = routed_inputs(context, &states, purpose).await?;
                progressed |= !inputs.is_empty();
                let outcomes = run_steps(context, &mut states, inputs, width).await;
                if let Some(end) = after_steps(context, &states, outcomes).await? {
                    return Ok(end);
                }
                if context.stopping() {
                    return stop(context).await;
                }
            }
            if !progressed {
                break;
            }
        }
    }
    // Stage 3: gating, selection, and integration.
    match record.mode {
        WorkflowMode::Compare => compare(context, &mut states).await,
        _ => cooperate(context, &mut states, width).await,
    }
}

fn task_open(record: &WorkflowRecord, task_key: &SpecIdentifier) -> bool {
    record
        .task(task_key)
        .is_some_and(|task| task.progress != TaskProgress::Accepted)
}

fn turns_left(record: &WorkflowRecord, task_key: &SpecIdentifier) -> bool {
    record
        .task(task_key)
        .is_some_and(|task| task.turns_used < task.max_turns)
}

/// The candidate a unit recorded last, with its texts reloaded from the
/// blob store, so a resumed coordinator continues from it.
async fn restore_latest(
    context: &RunContext,
    snapshot: &WorkflowSnapshot,
    unit: &Unit,
) -> Result<Option<CollectedCandidate>, WorkflowError> {
    let Some(receipt) = snapshot
        .receipts
        .iter()
        .rev()
        .find_map(|receipt| match receipt {
            WorkflowReceipt::Candidate(candidate)
                if candidate.candidate.task_key == unit.task_key
                    && candidate.candidate.worker_session_id == unit.session_id =>
            {
                Some(candidate.clone())
            }
            _ => None,
        })
    else {
        return Ok(None);
    };
    let texts = match receipt.manifest.as_ref().filter(|_| receipt.admitted) {
        Some(manifest) => {
            let blobs = context.setup.state.blobs();
            let entries = manifest.entries.clone();
            tokio::task::spawn_blocking(move || {
                let mut texts = BTreeMap::new();
                for entry in entries {
                    if let SnapshotChange::Write { sha256, .. } = entry.change {
                        if let Ok(text) = String::from_utf8(blobs.get(sha256)?) {
                            texts.insert(entry.path, text);
                        }
                    }
                }
                Ok::<_, WorkflowError>(texts)
            })
            .await
            .map_err(|_| WorkflowError::Internal)??
        }
        None => BTreeMap::new(),
    };
    Ok(Some(CollectedCandidate { receipt, texts }))
}

/// Runs the given steps, at most `width` at a time, each under its own
/// lease. Results come back in unit order.
async fn run_steps(
    context: &RunContext,
    states: &mut [UnitState],
    inputs: Vec<(usize, StepInput)>,
    width: usize,
) -> Vec<(usize, Result<StepOutcome, WorkflowError>)> {
    let mut inputs: BTreeMap<usize, StepInput> = inputs.into_iter().collect();
    let steps: Vec<_> = states
        .iter_mut()
        .enumerate()
        .filter_map(|(index, state)| {
            let input = inputs.remove(&index)?;
            Some(async move { (index, worker_step(context, state, input).await) })
        })
        .collect();
    let mut outcomes: Vec<_> = stream::iter(steps).buffer_unordered(width).collect().await;
    outcomes.sort_by_key(|(index, _)| *index);
    outcomes
}

/// Admits the steps' checkpoint messages in unit order, stages a bundle
/// for every answer, and settles supervisor-bound messages. A fresh
/// escalating message blocks the workflow for the operator.
async fn after_steps(
    context: &RunContext,
    states: &[UnitState],
    outcomes: Vec<(usize, Result<StepOutcome, WorkflowError>)>,
) -> Result<Option<RunEnd>, WorkflowError> {
    let mut first_error = None;
    let mut escalated = false;
    let references: Vec<&UnitState> = states.iter().collect();
    for (index, outcome) in outcomes {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        let Ok(checkpoint) = &outcome.checkpoint else {
            continue;
        };
        if !outcome.turn.completed() {
            continue;
        }
        let sender = &states[index];
        let admitted = admit_step_messages(
            context,
            &references,
            sender,
            outcome.turn.request_id,
            checkpoint,
            &outcome.delivered,
        )
        .await?;
        settle_supervisor_messages(context, &admitted).await?;
        for answer in admitted
            .iter()
            .filter(|message| message.kind() == MessageKind::Answer)
        {
            if let Some(asker) = answer
                .recipient_task()
                .and_then(|task| states.iter().find(|state| &state.unit.task_key == task))
            {
                bundle_for_answer(context, sender, asker, answer).await?;
            }
        }
        escalated |= admitted
            .iter()
            .any(|message: &AdmittedMessage| message.fresh && message.record.envelope.escalated);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    if escalated && !context.stopping() {
        context
            .transition(WorkflowPhase::Blocked, Some(ESCALATED_CODE))
            .await?;
        return Ok(Some(RunEnd::Blocked(ESCALATED_CODE.to_string())));
    }
    Ok(None)
}

/// The steps a broker round runs: answer steps for units with a routed
/// question or context request, then follow-up steps for units with any
/// other routed message.
async fn routed_inputs(
    context: &RunContext,
    states: &[UnitState],
    purpose: TurnPurpose,
) -> Result<Vec<(usize, StepInput)>, WorkflowError> {
    let record = context.record().await?;
    let mut inputs = Vec::new();
    for (index, state) in states.iter().enumerate() {
        if !task_open(&record, &state.unit.task_key)
            || !turns_left(&record, &state.unit.task_key)
            || fails_run(state.latest.as_ref())
        {
            continue;
        }
        let deliveries = pending_deliveries(context, state).await?;
        let asks = deliveries.iter().any(|delivery| {
            matches!(
                delivery.record.envelope.kind,
                MessageKind::Question | MessageKind::ContextRequest
            )
        });
        let wanted = match purpose {
            TurnPurpose::Answer => asks,
            _ => !deliveries.is_empty() && !asks,
        };
        if wanted {
            inputs.push((
                index,
                StepInput {
                    purpose,
                    notes: Vec::new(),
                    deliveries,
                },
            ));
        }
    }
    Ok(inputs)
}

fn fails_run(candidate: Option<&CollectedCandidate>) -> bool {
    candidate.is_some_and(|candidate| {
        candidate
            .receipt
            .violation_codes
            .iter()
            .any(|code| RUN_FAILING_CODES.contains(&code.as_str()))
    })
}

/// What gating one unit ended with.
enum UnitVerdict {
    Accepted,
    Failed(String),
    Stopped,
}

async fn cooperate(
    context: &RunContext,
    states: &mut [UnitState],
    width: usize,
) -> Result<RunEnd, WorkflowError> {
    let settles: Vec<_> = states
        .iter_mut()
        .map(|state| async move { settle_unit(context, state).await })
        .collect();
    let verdicts: Vec<Result<UnitVerdict, WorkflowError>> =
        stream::iter(settles).buffered(width).collect().await;
    if context.stopping() {
        return stop(context).await;
    }
    for verdict in verdicts {
        match verdict? {
            UnitVerdict::Accepted => {}
            UnitVerdict::Failed(code) => return Ok(fail(context, &code).await),
            UnitVerdict::Stopped => return stop(context).await,
        }
    }
    let accepted: Vec<(Unit, CollectedCandidate)> = states
        .iter()
        .filter_map(|state| Some((state.unit.clone(), state.latest.clone()?)))
        .collect();
    Ok(match integrate(context, &accepted).await? {
        None => RunEnd::Accepted,
        Some(code) => RunEnd::Failed(code),
    })
}

/// Reviews, verifies, and accepts one cooperative unit's candidate, with
/// bounded repairs (A03).
async fn settle_unit(
    context: &RunContext,
    state: &mut UnitState,
) -> Result<UnitVerdict, WorkflowError> {
    loop {
        if context.stopping() {
            return Ok(UnitVerdict::Stopped);
        }
        let record = context.record().await?;
        let task = record
            .task(&state.unit.task_key)
            .ok_or(WorkflowError::Internal)?
            .clone();
        if task.progress == TaskProgress::Accepted {
            return Ok(UnitVerdict::Accepted);
        }
        let Some(candidate) = state.latest.clone() else {
            if !turns_left(&record, &task.task_key) {
                return Ok(UnitVerdict::Failed(REPAIRS_EXHAUSTED_CODE.to_string()));
            }
            step(context, state, TurnPurpose::Implement, Vec::new()).await?;
            continue;
        };
        if !candidate.receipt.admitted {
            if fails_run(Some(&candidate)) {
                let code = candidate
                    .receipt
                    .violation_codes
                    .iter()
                    .find(|code| RUN_FAILING_CODES.contains(&code.as_str()))
                    .cloned()
                    .unwrap_or_default();
                return Ok(UnitVerdict::Failed(code));
            }
            let notes = vec![format!(
                "candidate refused: {}",
                candidate.receipt.violation_codes.join(", ")
            )];
            if !repair_left(&task) {
                return Ok(UnitVerdict::Failed(
                    candidate
                        .receipt
                        .violation_codes
                        .first()
                        .cloned()
                        .unwrap_or_else(|| REPAIRS_EXHAUSTED_CODE.to_string()),
                ));
            }
            step(context, state, TurnPurpose::Repair, notes).await?;
            continue;
        }
        let judged = judge(context, &state.unit, &candidate, &record, &task).await?;
        match &judged.gate {
            Ok(()) => {
                let workflow_id = context.workflow_id;
                let task_key = task.task_key.clone();
                let digest = candidate.digest();
                context
                    .versioned(move |handle, version| {
                        handle.accept_workflow_candidate(
                            workflow_id,
                            version,
                            task_key.clone(),
                            digest,
                            OffsetDateTime::now_utc(),
                        )
                    })
                    .await?;
                return Ok(UnitVerdict::Accepted);
            }
            Err(failures) => {
                if context.stopping() {
                    return Ok(UnitVerdict::Stopped);
                }
                if !repair_left(&task) {
                    return Ok(UnitVerdict::Failed(REPAIRS_EXHAUSTED_CODE.to_string()));
                }
                let notes = repair_notes(failures, &judged);
                step(context, state, TurnPurpose::Repair, notes).await?;
            }
        }
    }
}

fn repair_left(task: &WorkflowTask) -> bool {
    task.repairs_used < task.max_repairs && task.turns_used < task.max_turns
}

/// One worker step outside a broker round; routed messages still waiting
/// for the unit travel with it.
async fn step(
    context: &RunContext,
    state: &mut UnitState,
    purpose: TurnPurpose,
    notes: Vec<String>,
) -> Result<(), WorkflowError> {
    let deliveries = pending_deliveries(context, state).await?;
    let outcome = worker_step(
        context,
        state,
        StepInput {
            purpose,
            notes,
            deliveries,
        },
    )
    .await?;
    if !outcome.turn.completed() && !context.stopping() && outcome.recorded.is_none() {
        return Err(WorkflowError::Dispatch {
            code: "workflow_turn_not_completed".to_string(),
        });
    }
    Ok(())
}

/// The review and verification of one candidate and its gate.
struct Judgement {
    review: Option<ReviewReceipt>,
    findings: Vec<String>,
    verification: Option<VerifierReceipt>,
    gate: Result<(), Vec<GateFailure>>,
}

/// Reviews and verifies `candidate`, reusing receipts a previous run of
/// this coordinator already recorded for the same bytes.
async fn judge(
    context: &RunContext,
    unit: &Unit,
    candidate: &CollectedCandidate,
    record: &WorkflowRecord,
    task: &WorkflowTask,
) -> Result<Judgement, WorkflowError> {
    let digest = candidate.digest();
    let snapshot = context.snapshot().await?;
    let mut findings = Vec::new();
    let review = match snapshot
        .receipts
        .iter()
        .rev()
        .find_map(|receipt| match receipt {
            WorkflowReceipt::Review(review)
                if review.candidate_digest == digest && review.task_key == unit.task_key =>
            {
                Some(review.clone())
            }
            _ => None,
        }) {
        Some(review) => review,
        None => {
            let outcome = review_candidate(context, unit, candidate).await?;
            findings = outcome
                .findings
                .iter()
                .map(|finding| {
                    let location = match (&finding.path, finding.line) {
                        (Some(path), Some(line)) => format!(" {path}:{line}"),
                        (Some(path), None) => format!(" {path}"),
                        _ => String::new(),
                    };
                    let requirement = finding
                        .requirement_id
                        .as_ref()
                        .map_or(String::new(), |id| format!(" [{id}]"));
                    format!(
                        "review finding ({:?}){requirement}{location}: {}",
                        finding.severity, finding.summary
                    )
                })
                .collect();
            outcome.receipt
        }
    };
    let verification = match snapshot
        .receipts
        .iter()
        .rev()
        .find_map(|receipt| match receipt {
            WorkflowReceipt::Verification(verification)
                if verification.subject_digest == digest
                    && verification.task_key.as_ref() == Some(&unit.task_key) =>
            {
                Some(verification.clone())
            }
            _ => None,
        }) {
        Some(verification) => verification,
        None => verify_candidate(context, unit, candidate).await?,
    };
    let gate = candidate_gate(
        &candidate.receipt.candidate,
        Some(&review),
        Some(&verification),
        GateRequirements {
            required_suites: &task.required_suites,
            admitted_verifier_digest: record.verifier_digest,
        },
    )
    .map(|_| ());
    Ok(Judgement {
        review: Some(review),
        findings,
        verification: Some(verification),
        gate,
    })
}

/// Content-free gate facts plus the reviewer's findings, as turn data.
fn repair_notes(failures: &[GateFailure], judged: &Judgement) -> Vec<String> {
    let mut notes = vec![format!(
        "gate failures: {}",
        failures
            .iter()
            .map(|failure| match failure {
                GateFailure::SuiteMissing(suite)
                | GateFailure::SuiteFailed(suite)
                | GateFailure::SuiteBlocked(suite) => format!("{}({suite})", failure.code()),
                _ => failure.code().to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    )];
    if let Some(verification) = &judged.verification {
        for suite in &verification.suites {
            if !suite.failed_tests.is_empty() {
                notes.push(format!(
                    "verifier suite {} failed tests: {}",
                    suite.suite_id,
                    suite.failed_tests.join("; ")
                ));
            }
        }
    }
    if let Some(review) = &judged.review {
        notes.push(format!(
            "review verdict: {:?} with {} findings",
            review.verdict, review.findings_count
        ));
    }
    notes.extend(judged.findings.iter().cloned());
    notes
}

/// Gates every competitor, adds the trusted negative control when asked,
/// selects at most one winner, and integrates it (V04).
async fn compare(context: &RunContext, states: &mut [UnitState]) -> Result<RunEnd, WorkflowError> {
    let record = context.record().await?;
    let task = record.tasks.first().ok_or(WorkflowError::Internal)?.clone();
    let mut entries = Vec::new();
    let mut admitted: Vec<(Unit, CollectedCandidate)> = Vec::new();
    for state in states.iter() {
        if context.stopping() {
            return stop(context).await;
        }
        let Some(candidate) = state
            .latest
            .clone()
            .filter(|candidate| candidate.receipt.admitted)
        else {
            continue;
        };
        let judged = judge(context, &state.unit, &candidate, &record, &task).await?;
        entries.push(comparison_entry(
            &candidate.receipt,
            &judged,
            &task,
            &record,
        ));
        admitted.push((state.unit.clone(), candidate));
    }
    if context.plan.negative_control {
        let mut sources: Vec<&(Unit, CollectedCandidate)> = admitted.iter().collect();
        sources.sort_by_key(|(_, candidate)| candidate.digest());
        if let Some((unit, source)) = sources.first() {
            if let Some(control) = negative_control(context, unit, source).await? {
                let gate = candidate_gate(
                    &control.record,
                    None,
                    Some(&control.verification),
                    GateRequirements {
                        required_suites: &task.required_suites,
                        admitted_verifier_digest: record.verifier_digest,
                    },
                );
                entries.push(ComparisonEntry {
                    candidate: control.record,
                    gate,
                    metrics: CandidateMetrics {
                        tests_passed: tests_passed(Some(&control.verification)),
                        model_requests: None,
                        elapsed_ms: None,
                        changed_bytes: control.changed_bytes,
                    },
                });
            }
        }
    }
    let outcome = select(&entries, &SelectionPolicy::baseline(), task.contract_id);
    let selection = SelectionRecord {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: context.workflow_id,
        contract_id: task.contract_id,
        outcome: outcome.clone(),
    };
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let receipt = WorkflowReceipt::Selection(selection);
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            receipt.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    let Some(winner) = outcome.winner else {
        return Ok(fail(context, NO_VALID_CANDIDATE_CODE).await);
    };
    let chosen = admitted
        .into_iter()
        .find(|(_, candidate)| candidate.digest() == winner)
        .ok_or(WorkflowError::Internal)?;
    if task.progress != TaskProgress::Accepted {
        let task_key = task.task_key.clone();
        context
            .versioned(move |handle, version| {
                handle.accept_workflow_candidate(
                    workflow_id,
                    version,
                    task_key.clone(),
                    winner,
                    OffsetDateTime::now_utc(),
                )
            })
            .await?;
    }
    Ok(match integrate(context, &[chosen]).await? {
        None => RunEnd::Accepted,
        Some(code) => RunEnd::Failed(code),
    })
}

fn tests_passed(verification: Option<&VerifierReceipt>) -> u32 {
    verification.map_or(0, |verification| {
        verification
            .suites
            .iter()
            .map(|suite| suite.tests_passed)
            .sum()
    })
}

fn comparison_entry(
    receipt: &CandidateReceipt,
    judged: &Judgement,
    task: &WorkflowTask,
    record: &WorkflowRecord,
) -> ComparisonEntry {
    ComparisonEntry {
        candidate: receipt.candidate.clone(),
        gate: candidate_gate(
            &receipt.candidate,
            judged.review.as_ref(),
            judged.verification.as_ref(),
            GateRequirements {
                required_suites: &task.required_suites,
                admitted_verifier_digest: record.verifier_digest,
            },
        ),
        metrics: CandidateMetrics {
            tests_passed: tests_passed(judged.verification.as_ref()),
            model_requests: None,
            elapsed_ms: None,
            changed_bytes: receipt
                .manifest
                .as_ref()
                .map_or(0, |manifest| manifest.total_bytes()),
        },
    }
}

/// Moves a workflow that could not continue to `failed`.
async fn fail(context: &RunContext, code: &str) -> RunEnd {
    if context.stopping() {
        return stop(context)
            .await
            .unwrap_or_else(|error| RunEnd::Failed(error.code().to_string()));
    }
    let phase = context.record().await.map(|record| record.phase);
    match phase {
        Ok(phase) if phase.is_terminal() => {}
        Ok(phase) if phase.can_transition_to(WorkflowPhase::Failed) => {
            let _ = context.transition(WorkflowPhase::Failed, Some(code)).await;
        }
        Ok(WorkflowPhase::Paused) => {
            let _ = context.transition(WorkflowPhase::Running, None).await;
            let _ = context.transition(WorkflowPhase::Failed, Some(code)).await;
        }
        _ => {}
    }
    RunEnd::Failed(code.to_string())
}

/// Ends a paused or cancelled run once every lease it held is settled.
async fn stop(context: &RunContext) -> Result<RunEnd, WorkflowError> {
    match context.control() {
        Control::Pause => {
            let record = context.record().await?;
            if record.phase == WorkflowPhase::Running {
                context.transition(WorkflowPhase::Paused, None).await?;
            }
            Ok(RunEnd::Paused)
        }
        Control::Cancel | Control::Run => {
            let record = context.record().await?;
            if record.phase == WorkflowPhase::Cancelled {
                return Ok(RunEnd::Cancelled);
            }
            if record.phase != WorkflowPhase::CancelRequested {
                context
                    .transition(WorkflowPhase::CancelRequested, Some("operator_cancel"))
                    .await?;
            }
            finish_cancel(
                &context.writer,
                context.workflow_id,
                context.setup.config.config.heartbeat_ms,
            )
            .await?;
            Ok(RunEnd::Cancelled)
        }
    }
}
