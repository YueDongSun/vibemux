//! Durable harness dispatch attempts (ADR 029 §2 and §3): store schema 4 and
//! the writer transactions that apply the pure `DispatchPhase` machine.
//!
//! Every state change appends its canonical event, compare-and-sets the
//! versioned record, and saves the Task/Run projections it changes inside
//! one immediate SQLite transaction. The store never sees a prompt; the
//! record keeps only its digest. The claim fence lives in its own column and
//! never appears in the record or in an event.

mod dispatch_rows;

use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{EventDraft, EventEnvelope};
use vibemux_harness::{
    AgentKind,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchPhase, DispatchTrigger, NativeProtocol,
        OutcomeDecision, PhaseTransition, PromptDigest, Sha256Digest, TransitionResult,
        attempt::{self, ADMISSION_RUN_STATUS, ADMISSION_TASK_STATUS, FenceEffect},
        capture_budget::CaptureSummary,
        events::{
            self, AdmittedPayload, DispatchEventContext, FinishedPayload, ProcessSummary,
            RecoveredPayload,
        },
        request::request_fingerprint,
    },
};
use vibemux_types::{ProjectId, Run, RunId, RunSpec, Task, TaskId, TaskSpec};

use crate::{SqliteStore, StoreError, resolve_or_mint_project_id};

use dispatch_rows::StoredDispatch;

pub const HARNESS_DISPATCH_RECORD_SCHEMA_VERSION: u32 = 1;

/// Task title prefix; the harness command name follows.
const DISPATCH_TASK_TITLE_PREFIX: &str = "harness dispatch ";
const DISPATCH_RUN_ROLE: &str = "dispatch";

/// Adds `harness_dispatches`. `migrate` runs it and the version marker in
/// one immediate transaction, so a failure leaves the database at schema 3.
/// The partial unique index is the working-directory reservation: it covers
/// exactly the phases for which `DispatchPhase::holds_reservation` is true.
pub(crate) const MIGRATION_V4: &str = r#"
CREATE TABLE harness_dispatches (
    request_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL UNIQUE,
    run_id TEXT NOT NULL UNIQUE,
    fingerprint TEXT NOT NULL,
    resource_key TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('admitted', 'running', 'cancel_requested',
        'recovery_pending', 'completed', 'failed', 'unverified', 'cancelled')),
    claim_fence TEXT,
    version INTEGER NOT NULL CHECK(version > 0),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    CHECK(CASE
        WHEN phase IN ('running', 'cancel_requested') THEN claim_fence IS NOT NULL
        WHEN phase IN ('admitted', 'recovery_pending') THEN claim_fence IS NULL
        ELSE 1
    END)
);
CREATE UNIQUE INDEX harness_dispatches_active_resource ON harness_dispatches(resource_key)
    WHERE phase IN ('admitted', 'running', 'cancel_requested', 'recovery_pending');
INSERT INTO schema_migrations(version, applied_at)
VALUES (4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

/// The facts the writer persists at admission. The prompt itself never
/// reaches the store; the store derives the request fingerprint from these.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessDispatchAdmission {
    pub request_id: Uuid,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub prompt: PromptDigest,
    pub config_sha256: Sha256Digest,
    /// `attempt::resource_key` of the canonical working directory.
    pub resource_key: Sha256Digest,
    /// Project `HEAD` observed at admission.
    pub base_commit: String,
    pub timestamp: OffsetDateTime,
}

/// The persisted attempt. `phase` is authoritative; `outcome` is the
/// executor's evidence decision, which a committed cancellation overrides.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchRecord {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub fingerprint: Sha256Digest,
    pub resource_key: Sha256Digest,
    pub prompt: PromptDigest,
    pub config_sha256: Sha256Digest,
    pub base_commit: String,
    pub phase: DispatchPhase,
    pub version: u64,
    pub outcome: Option<AttemptOutcome>,
    /// The latest fixed failure code recorded for the attempt.
    pub error_code: Option<DispatchError>,
    pub process: Option<ProcessSummary>,
    pub capture: Option<CaptureSummary>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Result of a dispatch transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessDispatchCommit {
    pub record: HarnessDispatchRecord,
    /// The event this call appended; `None` for an idempotent repeat that
    /// wrote nothing.
    pub event: Option<EventEnvelope>,
    /// Sequence of the event that last changed the record.
    pub sequence: u64,
}

/// Proof that an executor holds the current claim. Minted by
/// [`SqliteStore::claim_harness_dispatch`] and required by
/// [`SqliteStore::finish_harness_dispatch`]. Recovery invalidates it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimFence(Uuid);

#[derive(Clone, Debug, PartialEq)]
pub struct HarnessDispatchClaim {
    pub record: HarnessDispatchRecord,
    pub event: EventEnvelope,
    pub fence: ClaimFence,
}

/// The executor's fenced report once the vendor process ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessDispatchFinish {
    pub request_id: Uuid,
    pub fence: ClaimFence,
    pub decision: OutcomeDecision,
    pub process: ProcessSummary,
    pub capture: CaptureSummary,
    pub timestamp: OffsetDateTime,
}

impl SqliteStore {
    /// Durably admits a dispatch: a new Task (`in_progress`), a new Run
    /// (`preparing`), the record, and `harness_dispatch_admitted`. The same
    /// request UUID with the same fingerprint returns the existing record
    /// and writes nothing; a different fingerprint is a conflict. Nothing is
    /// written on any rejection.
    pub fn admit_harness_dispatch(
        &mut self,
        admission: &HarnessDispatchAdmission,
    ) -> Result<HarnessDispatchCommit, StoreError> {
        if admission.request_id.is_nil() || !admission.protocol.accepts(admission.harness) {
            return Err(StoreError::HarnessDispatch(DispatchError::InvalidRequest));
        }
        if !admission.protocol.supports_execution() {
            return Err(StoreError::HarnessDispatch(
                DispatchError::ExecutionDisabled,
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project_id = resolve_or_mint_project_id(&transaction)?;
        let fingerprint = request_fingerprint(
            project_id,
            admission.request_id,
            admission.harness,
            admission.protocol,
            admission.prompt.sha256,
            admission.config_sha256,
        );
        if let Some(stored) = dispatch_rows::load(&transaction, admission.request_id)? {
            if stored.record.fingerprint != fingerprint {
                return Err(StoreError::HarnessDispatch(DispatchError::Conflict));
            }
            return Ok(stored.unchanged());
        }
        let command_name = admission.harness.command_name();
        let detected = Self::read_registry_in_tx(&transaction)?
            .state(command_name)
            .is_some_and(|state| state.detected);
        if !detected {
            return Err(StoreError::HarnessDispatch(DispatchError::NotDetected));
        }
        if dispatch_rows::is_reserved(&transaction, admission.resource_key)? {
            return Err(StoreError::HarnessDispatch(DispatchError::Busy));
        }
        let (task, run) = admitted_entities(project_id, admission)?;
        let record = HarnessDispatchRecord {
            schema_version: HARNESS_DISPATCH_RECORD_SCHEMA_VERSION,
            request_id: admission.request_id,
            project_id,
            task_id: task.task_id(),
            run_id: run.run_id(),
            harness: admission.harness,
            protocol: admission.protocol,
            fingerprint,
            resource_key: admission.resource_key,
            prompt: admission.prompt,
            config_sha256: admission.config_sha256,
            base_commit: admission.base_commit.clone(),
            phase: DispatchPhase::Admitted,
            version: 1,
            outcome: None,
            error_code: None,
            process: None,
            capture: None,
            created_at: admission.timestamp,
            updated_at: admission.timestamp,
        };
        let draft = events::admitted(
            &event_context(&record, admission.timestamp),
            &AdmittedPayload::new(
                record.request_id,
                record.harness,
                record.protocol,
                record.prompt,
                record.config_sha256,
                record.base_commit.clone(),
            ),
        )
        .map_err(StoreError::HarnessDispatch)?;
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        dispatch_rows::insert(&transaction, &record, raw_sequence)?;
        dispatch_rows::save_task(&transaction, &task, raw_sequence)?;
        dispatch_rows::save_run(&transaction, &run, raw_sequence)?;
        transaction.commit()?;
        Ok(HarnessDispatchCommit {
            record,
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    /// Single-use claim, committed before the process is spawned: `admitted
    /// → running`, Run `preparing → running`, and a fresh fence.
    pub fn claim_harness_dispatch(
        &mut self,
        request_id: Uuid,
        timestamp: OffsetDateTime,
    ) -> Result<HarnessDispatchClaim, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = dispatch_rows::load_existing(&transaction, request_id)?;
        let (commit, fence) = commit_transition(
            &transaction,
            stored,
            DispatchTrigger::Claim,
            None,
            timestamp,
        )?;
        let (Some(event), Some(fence)) = (commit.event, fence) else {
            return Err(StoreError::HarnessDispatch(DispatchError::Internal));
        };
        transaction.commit()?;
        Ok(HarnessDispatchClaim {
            record: commit.record,
            event,
            fence,
        })
    }

    /// Fenced finish. A finish that repeats the fence of an attempt it
    /// already finished writes nothing; any other fence is stale.
    /// `Completed` is accepted only with a clean, error-free exit.
    pub fn finish_harness_dispatch(
        &mut self,
        finish: &HarnessDispatchFinish,
    ) -> Result<HarnessDispatchCommit, StoreError> {
        validate_finish(finish)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = dispatch_rows::load_existing(&transaction, finish.request_id)?;
        if stored.record.phase.is_terminal() && stored.fence == Some(finish.fence) {
            return Ok(stored.unchanged());
        }
        let (commit, _) = commit_transition(
            &transaction,
            stored,
            DispatchTrigger::Finish(finish.decision.outcome),
            Some(finish),
            finish.timestamp,
        )?;
        transaction.commit()?;
        Ok(commit)
    }

    /// Operator cancellation. Before the claim it ends the attempt; while
    /// running it records the request for the executor; in
    /// `recovery_pending` it is the acknowledgement that releases the
    /// reservation. Repeats write nothing; a finished attempt is terminal.
    pub fn cancel_harness_dispatch(
        &mut self,
        request_id: Uuid,
        timestamp: OffsetDateTime,
    ) -> Result<HarnessDispatchCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = dispatch_rows::load_existing(&transaction, request_id)?;
        let (commit, _) = commit_transition(
            &transaction,
            stored,
            DispatchTrigger::Cancel,
            None,
            timestamp,
        )?;
        transaction.commit()?;
        Ok(commit)
    }

    /// Startup recovery in one transaction: `admitted` attempts fail as
    /// interrupted (nothing was launched); `running` and `cancel_requested`
    /// attempts become `recovery_pending`, keep the reservation, and lose
    /// their fence. Returns only the attempts it changed.
    pub fn recover_harness_dispatches(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<Vec<HarnessDispatchCommit>, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut commits = Vec::new();
        for request_id in dispatch_rows::active_request_ids(&transaction)? {
            let stored = dispatch_rows::load_existing(&transaction, request_id)?;
            let (commit, _) = commit_transition(
                &transaction,
                stored,
                DispatchTrigger::Recover,
                None,
                timestamp,
            )?;
            if commit.event.is_some() {
                commits.push(commit);
            }
        }
        transaction.commit()?;
        Ok(commits)
    }

    /// The validated record for `request_id`, if one was admitted.
    pub fn harness_dispatch(
        &self,
        request_id: Uuid,
    ) -> Result<Option<HarnessDispatchRecord>, StoreError> {
        Ok(dispatch_rows::load(&self.connection, request_id)?.map(|stored| stored.record))
    }
}

/// Applies `trigger` through the pure machine and commits the change: the
/// Task/Run transitions, the fence effect, the record, and the event. An
/// idempotent repeat returns the stored record and writes nothing. Also
/// returns the fence column's new value.
fn commit_transition(
    transaction: &Transaction<'_>,
    stored: StoredDispatch,
    trigger: DispatchTrigger,
    finish: Option<&HarnessDispatchFinish>,
    timestamp: OffsetDateTime,
) -> Result<(HarnessDispatchCommit, Option<ClaimFence>), StoreError> {
    let transition =
        match attempt::apply(stored.record.phase, trigger).map_err(StoreError::HarnessDispatch)? {
            TransitionResult::Changed(transition) => transition,
            TransitionResult::Unchanged => {
                let fence = stored.fence;
                return Ok((stored.unchanged(), fence));
            }
        };
    let fence = match transition.fence {
        FenceEffect::Keep => stored.fence,
        FenceEffect::Mint => Some(ClaimFence(Uuid::new_v4())),
        // Kept after the finish so a retried finish is recognized.
        FenceEffect::Require => match (stored.fence, finish) {
            (Some(current), Some(finish)) if current == finish.fence => Some(current),
            _ => return Err(StoreError::HarnessDispatch(DispatchError::StaleFence)),
        },
        FenceEffect::Clear => None,
    };
    let task = transition
        .task_status
        .map(|status| stored.task.transitioned(status))
        .transpose()
        .map_err(|_| StoreError::HarnessDispatchProjectionMismatch)?;
    let run = transition
        .run_status
        .map(|status| {
            stored
                .run
                .transitioned(status, transition.completion_authority)
        })
        .transpose()
        .map_err(|_| StoreError::HarnessDispatchProjectionMismatch)?;

    let previous_version = stored.record.version;
    let mut record = stored.record;
    let error_code = finish.map_or(transition.error_code, |finish| finish.decision.error_code);
    record.phase = transition.to;
    record.version = previous_version
        .checked_add(1)
        .ok_or(StoreError::HarnessDispatchVersionConflict)?;
    record.error_code = error_code.or(record.error_code);
    if let Some(finish) = finish {
        record.outcome = Some(finish.decision.outcome);
        record.process = Some(finish.process);
        record.capture = Some(finish.capture);
    }
    record.updated_at = record.updated_at.max(timestamp);

    let draft = transition_event(&record, trigger, &transition, finish, error_code, timestamp)
        .map_err(StoreError::HarnessDispatch)?;
    let (event, raw_sequence) = SqliteStore::insert_event(transaction, draft)?;
    dispatch_rows::update(transaction, &record, fence, previous_version, raw_sequence)?;
    if let Some(task) = &task {
        dispatch_rows::save_task(transaction, task, raw_sequence)?;
    }
    if let Some(run) = &run {
        dispatch_rows::save_run(transaction, run, raw_sequence)?;
    }
    let commit = HarnessDispatchCommit {
        record,
        sequence: event.sequence().get(),
        event: Some(event),
    };
    Ok((commit, fence))
}

fn transition_event(
    record: &HarnessDispatchRecord,
    trigger: DispatchTrigger,
    transition: &PhaseTransition,
    finish: Option<&HarnessDispatchFinish>,
    error_code: Option<DispatchError>,
    timestamp: OffsetDateTime,
) -> Result<EventDraft, DispatchError> {
    let context = event_context(record, timestamp);
    match trigger {
        DispatchTrigger::Claim => events::started(&context),
        DispatchTrigger::Recover => events::recovered(
            &context,
            &RecoveredPayload {
                request_id: record.request_id,
                from_phase: transition.from,
                phase: transition.to,
                error_code: error_code.ok_or(DispatchError::Internal)?,
            },
        ),
        DispatchTrigger::Cancel if transition.to == DispatchPhase::CancelRequested => {
            events::cancel_requested(&context, transition.from)
        }
        DispatchTrigger::Cancel | DispatchTrigger::Finish(_) => events::finished(
            &context,
            &FinishedPayload {
                request_id: record.request_id,
                from_phase: transition.from,
                phase: transition.to,
                outcome: finish.map(|finish| finish.decision.outcome),
                error_code,
                process: finish.map(|finish| finish.process),
                capture: finish.map(|finish| finish.capture),
            },
        ),
    }
}

const fn event_context(
    record: &HarnessDispatchRecord,
    timestamp: OffsetDateTime,
) -> DispatchEventContext {
    DispatchEventContext {
        project_id: record.project_id,
        task_id: record.task_id,
        run_id: record.run_id,
        request_id: record.request_id,
        timestamp,
    }
}

/// The admission's canonical Task (`in_progress`) and Run (`preparing`).
fn admitted_entities(
    project_id: ProjectId,
    admission: &HarnessDispatchAdmission,
) -> Result<(Task, Run), StoreError> {
    let invalid = |_| StoreError::HarnessDispatch(DispatchError::InvalidRequest);
    let command_name = admission.harness.command_name();
    let task = Task::new(TaskSpec {
        project_id,
        title: format!("{DISPATCH_TASK_TITLE_PREFIX}{command_name}"),
        description: String::new(),
    })
    .and_then(|task| task.transitioned(ADMISSION_TASK_STATUS))
    .map_err(invalid)?;
    let run = Run::new(RunSpec {
        project_id,
        task_id: task.task_id(),
        harness: command_name.to_owned(),
        role: DISPATCH_RUN_ROLE.to_owned(),
        protocol: admission.protocol.as_str().to_owned(),
        base_commit: admission.base_commit.clone(),
    })
    .map_err(invalid)?;
    if run.status() != ADMISSION_RUN_STATUS {
        return Err(StoreError::HarnessDispatch(DispatchError::Internal));
    }
    Ok((task, run))
}

/// `Succeeded` is written only for a clean, error-free exit; a probe is
/// never a dispatch outcome.
fn validate_finish(finish: &HarnessDispatchFinish) -> Result<(), StoreError> {
    match finish.decision.outcome {
        AttemptOutcome::Probed => Err(StoreError::HarnessDispatch(
            DispatchError::InvalidTransition,
        )),
        AttemptOutcome::Completed
            if finish.decision.error_code.is_some()
                || finish.process.exit_code != Some(0)
                || finish.process.forced_termination =>
        {
            Err(StoreError::HarnessDispatch(DispatchError::InvalidRequest))
        }
        _ => Ok(()),
    }
}
