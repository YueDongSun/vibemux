//! Atomic, single-writer A2A binding and verified workflow transactions.
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use vibemux_events::{
    ActorName, EventDraft, EventEnvelope, EventPayload, EventSequence, EventType,
};
use vibemux_types::{
    EventId, Run, RunCompletionAuthority, RunId, RunStatus, Task, TaskId, TaskStatus,
    a2a::{
        A2A_RECORD_SCHEMA_VERSION, A2aBinding, A2aCancellationState, A2aRemoteState, A2aRunAction,
        A2aRunRecord, A2aRunStart, A2aRunUpdate, MAX_A2A_RUNS_PER_TASK, VerificationReceipt,
    },
};

use crate::{SqliteStore, StoreError};

pub(crate) const MIGRATION_V2: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE a2a_runs (
    run_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    external_task_id TEXT NOT NULL,
    worktree_path_key TEXT NOT NULL UNIQUE,
    branch_key TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    UNIQUE(project_id, peer_id, external_task_id),
    UNIQUE(project_id, branch_key)
);
CREATE INDEX a2a_runs_task ON a2a_runs(task_id);
CREATE TABLE a2a_commands (
    idempotency_key TEXT PRIMARY KEY REFERENCES events(idempotency_key),
    fingerprint TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES a2a_runs(run_id),
    record_json TEXT NOT NULL,
    event_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
INSERT INTO schema_migrations(version, applied_at)
VALUES (2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
"#;

#[derive(Clone, Debug, PartialEq)]
pub struct A2aCommitOutcome {
    pub record: A2aRunRecord,
    pub event: EventEnvelope,
    pub duplicate: bool,
}

impl SqliteStore {
    pub fn start_a2a_run(&mut self, start: A2aRunStart) -> Result<A2aCommitOutcome, StoreError> {
        start.validate().map_err(StoreError::A2aState)?;
        let fingerprint = fingerprint("start", &start)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(outcome) = duplicate(&transaction, &start.idempotency_key, &fingerprint)? {
            return Ok(outcome);
        }
        let task_id = start.task.task_id();
        let current = load_task(&transaction, task_id)?;
        let task = match current {
            Some(task) => {
                if task.project_id() != start.task.project_id()
                    || task.title() != start.task.title()
                    || task.description() != start.task.description()
                {
                    return Err(StoreError::A2aBindingConflict);
                }
                start_task(task)?
            }
            None if start.task.status() == TaskStatus::Open => start_task(start.task.clone())?,
            None => return Err(StoreError::A2aInvalidTransition),
        };
        if records_for_task(&transaction, task_id)?.len() >= MAX_A2A_RUNS_PER_TASK {
            return Err(StoreError::A2aCapacity);
        }
        let conflict: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM a2a_runs WHERE run_id=?1 OR worktree_path_key=?2 OR (project_id=?3 AND (branch_key=?4 OR (peer_id=?5 AND external_task_id=?6))))",
            params![start.run.run_id().to_string(), start.workspace.path_key(), task.project_id().to_string(),
                start.workspace.branch.to_ascii_lowercase(), start.peer_id, start.external_task_id], |row| row.get(0))?;
        let existing_run: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM projections WHERE entity_kind='run' AND entity_id=?)",
            [start.run.run_id().to_string()],
            |row| row.get(0),
        )?;
        if conflict || existing_run {
            return Err(StoreError::A2aBindingConflict);
        }
        let record = A2aRunRecord {
            schema_version: A2A_RECORD_SCHEMA_VERSION,
            binding: A2aBinding {
                project_id: task.project_id(),
                task_id,
                run_id: start.run.run_id(),
                peer_id: start.peer_id,
                external_task_id: start.external_task_id,
                transport: start.transport,
                protocol_version: start.protocol_version,
                created_at: start.timestamp,
                updated_at: start.timestamp,
                idempotency_key: start.idempotency_key.clone(),
                last_remote_state: A2aRemoteState::Submitted,
                cancellation_state: A2aCancellationState::None,
                artifacts: Vec::new(),
                verification: None,
            },
            task,
            run: start.run,
            workspace: start.workspace,
            version: 1,
        };
        record.validate().map_err(StoreError::A2aState)?;
        let event = append_event(
            &transaction,
            "a2a_run_prepared",
            &record,
            None,
            start.timestamp,
            &start.idempotency_key,
            None,
        )?;
        save_record(&transaction, &record, event.sequence().get(), true)?;
        save_task(&transaction, &record.task, event.sequence().get())?;
        save_command(
            &transaction,
            &start.idempotency_key,
            &fingerprint,
            &record,
            &event,
        )?;
        transaction.commit()?;
        Ok(A2aCommitOutcome {
            record,
            event,
            duplicate: false,
        })
    }

    pub fn update_a2a_run(&mut self, update: A2aRunUpdate) -> Result<A2aCommitOutcome, StoreError> {
        update.validate().map_err(StoreError::A2aState)?;
        let fingerprint = fingerprint("update", &update)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(outcome) = duplicate(&transaction, &update.idempotency_key, &fingerprint)? {
            return Ok(outcome);
        }
        let mut record =
            load_record(&transaction, update.run_id)?.ok_or(StoreError::A2aNotFound)?;
        if record.version != update.expected_version {
            return Err(StoreError::A2aVersionConflict);
        }
        if update.timestamp < record.binding.updated_at {
            return Err(StoreError::A2aInvalidTransition);
        }
        if is_terminal(record.run.status())
            && !matches!(update.action, A2aRunAction::FinalizeCancellation)
        {
            return Err(StoreError::A2aInvalidTransition);
        }
        let mut related = None;
        let mut failure_code = None;
        let event_type = match &update.action {
            A2aRunAction::Start => {
                if record.binding.cancellation_state != A2aCancellationState::None {
                    return Err(StoreError::A2aInvalidTransition);
                }
                record.run = transition_run(&record.run, RunStatus::Running, None)?;
                "a2a_run_started"
            }
            A2aRunAction::Observe { state, artifacts } => {
                if !matches!(
                    record.run.status(),
                    RunStatus::Preparing | RunStatus::Running
                ) || !record.binding.last_remote_state.can_transition_to(*state)
                    || (*state != A2aRemoteState::Completed && !artifacts.is_empty())
                    || (record.binding.last_remote_state == A2aRemoteState::Completed
                        && record.binding.artifacts != *artifacts)
                {
                    return Err(StoreError::A2aInvalidTransition);
                }
                record.binding.last_remote_state = *state;
                record.binding.artifacts = artifacts.clone();
                "a2a_remote_state_observed"
            }
            A2aRunAction::RequestCancellation => {
                if record.binding.cancellation_state != A2aCancellationState::None {
                    return Err(StoreError::A2aInvalidTransition);
                }
                record.binding.cancellation_state = A2aCancellationState::Requested;
                "a2a_cancellation_requested"
            }
            A2aRunAction::ConfirmCancellation => {
                if record.binding.cancellation_state != A2aCancellationState::Requested
                    || record.binding.last_remote_state != A2aRemoteState::Cancelled
                {
                    return Err(StoreError::A2aInvalidTransition);
                }
                let target = if record.run.status() == RunStatus::Preparing {
                    RunStatus::Failed
                } else {
                    RunStatus::Stopped
                };
                record.run = transition_run(&record.run, target, None)?;
                record.binding.cancellation_state = A2aCancellationState::Confirmed;
                record.task = if has_other_live_runs(
                    &transaction,
                    record.task.task_id(),
                    &[record.run.run_id()],
                )? {
                    block_task(record.task)?
                } else {
                    transition_task(record.task, TaskStatus::Cancelled)?
                };
                "a2a_cancellation_confirmed"
            }
            A2aRunAction::FinalizeCancellation => {
                if !is_terminal(record.run.status())
                    || has_other_live_runs(&transaction, record.task.task_id(), &[])?
                {
                    return Err(StoreError::A2aInvalidTransition);
                }
                record.task = transition_task(record.task, TaskStatus::Cancelled)?;
                "a2a_workflow_cancelled"
            }
            A2aRunAction::Fail { code } => {
                record.run = transition_run(&record.run, RunStatus::Failed, None)?;
                record.task = block_task(record.task)?;
                failure_code = Some(code.as_str());
                "a2a_run_failed"
            }
            A2aRunAction::Verify { receipt } => {
                let reviewer = verify_pair(&transaction, &mut record, receipt, update.timestamp)?;
                related = Some(reviewer);
                if receipt.accepted {
                    "a2a_verification_accepted"
                } else {
                    "a2a_verification_rejected"
                }
            }
        };
        record.version = record
            .version
            .checked_add(1)
            .ok_or(StoreError::A2aVersionConflict)?;
        record.binding.updated_at = update.timestamp;
        record.validate().map_err(StoreError::A2aState)?;
        let event = append_event(
            &transaction,
            event_type,
            &record,
            related.as_ref(),
            update.timestamp,
            &update.idempotency_key,
            failure_code,
        )?;
        save_record(&transaction, &record, event.sequence().get(), false)?;
        if let Some(reviewer) = &related {
            save_record(&transaction, reviewer, event.sequence().get(), false)?;
        }
        save_task(&transaction, &record.task, event.sequence().get())?;
        save_command(
            &transaction,
            &update.idempotency_key,
            &fingerprint,
            &record,
            &event,
        )?;
        transaction.commit()?;
        Ok(A2aCommitOutcome {
            record,
            event,
            duplicate: false,
        })
    }

    pub fn a2a_run(&self, run_id: RunId) -> Result<Option<A2aRunRecord>, StoreError> {
        load_record(&self.connection, run_id)
    }

    pub fn a2a_runs(&self, task_id: TaskId) -> Result<Vec<A2aRunRecord>, StoreError> {
        records_for_task(&self.connection, task_id)
    }
}

fn verify_pair(
    transaction: &Transaction<'_>,
    worker: &mut A2aRunRecord,
    receipt: &VerificationReceipt,
    timestamp: time::OffsetDateTime,
) -> Result<A2aRunRecord, StoreError> {
    let mut reviewer =
        load_record(transaction, receipt.reviewer_run_id)?.ok_or(StoreError::A2aNotFound)?;
    if reviewer.version != receipt.reviewer_expected_version {
        return Err(StoreError::A2aVersionConflict);
    }
    if worker.run.role() != "worker"
        || worker.run.status() != RunStatus::Running
        || !matches!(reviewer.run.role(), "reviewer" | "verifier")
        || reviewer.run.status() != RunStatus::Running
        || worker.run.run_id() == reviewer.run.run_id()
        || worker.binding.peer_id == reviewer.binding.peer_id
        || worker.run.task_id() != reviewer.run.task_id()
        || worker.run.project_id() != reviewer.run.project_id()
        || worker.run.base_commit() != reviewer.run.base_commit()
        || worker.workspace.path_key() == reviewer.workspace.path_key()
        || worker
            .workspace
            .branch
            .eq_ignore_ascii_case(&reviewer.workspace.branch)
        || worker.binding.last_remote_state != A2aRemoteState::Completed
        || reviewer.binding.last_remote_state != A2aRemoteState::Completed
        || worker.binding.cancellation_state != A2aCancellationState::None
        || reviewer.binding.cancellation_state != A2aCancellationState::None
        || timestamp < reviewer.binding.updated_at
        || !worker
            .binding
            .artifacts
            .iter()
            .any(|artifact| artifact.sha256 == receipt.artifact_sha256)
        || !reviewer
            .binding
            .artifacts
            .iter()
            .any(|artifact| artifact.sha256 == receipt.review_sha256)
        || has_other_live_runs(
            transaction,
            worker.task.task_id(),
            &[worker.run.run_id(), reviewer.run.run_id()],
        )?
    {
        return Err(StoreError::A2aVerificationRejected);
    }
    worker.run = transition_run(
        &worker.run,
        if receipt.accepted {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        },
        Some(RunCompletionAuthority::Verifier),
    )?;
    reviewer.run = transition_run(
        &reviewer.run,
        RunStatus::Succeeded,
        Some(RunCompletionAuthority::Verifier),
    )?;
    worker.task = if receipt.accepted {
        transition_task(worker.task.clone(), TaskStatus::Done)?
    } else {
        block_task(worker.task.clone())?
    };
    reviewer.task = worker.task.clone();
    worker.binding.verification = Some(receipt.clone());
    reviewer.binding.verification = Some(receipt.clone());
    reviewer.binding.updated_at = timestamp;
    reviewer.version = reviewer
        .version
        .checked_add(1)
        .ok_or(StoreError::A2aVersionConflict)?;
    reviewer.validate().map_err(StoreError::A2aState)?;
    Ok(reviewer)
}

fn transition_task(task: Task, target: TaskStatus) -> Result<Task, StoreError> {
    task.transitioned(target)
        .map_err(|_| StoreError::A2aInvalidTransition)
}

fn start_task(task: Task) -> Result<Task, StoreError> {
    match task.status() {
        TaskStatus::Open | TaskStatus::Blocked => transition_task(task, TaskStatus::InProgress),
        TaskStatus::InProgress => Ok(task),
        _ => Err(StoreError::A2aInvalidTransition),
    }
}

fn block_task(task: Task) -> Result<Task, StoreError> {
    if task.status() == TaskStatus::Blocked {
        Ok(task)
    } else {
        transition_task(task, TaskStatus::Blocked)
    }
}

fn transition_run(
    run: &Run,
    target: RunStatus,
    authority: Option<RunCompletionAuthority>,
) -> Result<Run, StoreError> {
    run.transitioned(target, authority)
        .map_err(|_| StoreError::A2aInvalidTransition)
}

fn is_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Stopped | RunStatus::Failed | RunStatus::Succeeded
    )
}

fn has_other_live_runs(
    connection: &rusqlite::Connection,
    task_id: TaskId,
    except: &[RunId],
) -> Result<bool, StoreError> {
    Ok(records_for_task(connection, task_id)?
        .iter()
        .any(|record| !except.contains(&record.run.run_id()) && !is_terminal(record.run.status())))
}

fn load_task(
    connection: &rusqlite::Connection,
    task_id: TaskId,
) -> Result<Option<Task>, StoreError> {
    let encoded: Option<String> = connection
        .query_row(
            "SELECT state_json FROM projections WHERE entity_kind='task' AND entity_id=?",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    encoded
        .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
        .transpose()
}

fn load_record(
    connection: &rusqlite::Connection,
    run_id: RunId,
) -> Result<Option<A2aRunRecord>, StoreError> {
    struct PersistedRow {
        encoded: String,
        version: u64,
        task_id: String,
        project_id: String,
        peer_id: String,
        external_task_id: String,
        path_key: String,
        branch_key: String,
        sequence: u64,
    }
    let row = connection.query_row(
        "SELECT record_json,version,task_id,project_id,peer_id,external_task_id,worktree_path_key,branch_key,updated_sequence FROM a2a_runs WHERE run_id=?",
        [run_id.to_string()], |row| Ok(PersistedRow { encoded: row.get(0)?, version: row.get(1)?, task_id: row.get(2)?, project_id: row.get(3)?,
            peer_id: row.get(4)?, external_task_id: row.get(5)?, path_key: row.get(6)?, branch_key: row.get(7)?, sequence: row.get(8)? })).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut record: A2aRunRecord = serde_json::from_str(&row.encoded)?;
    if record.run.run_id() != run_id
        || record.version != row.version
        || record.task.task_id().to_string() != row.task_id
        || record.task.project_id().to_string() != row.project_id
        || record.binding.peer_id != row.peer_id
        || record.binding.external_task_id != row.external_task_id
        || record.workspace.path_key() != row.path_key
        || record.workspace.branch.to_ascii_lowercase() != row.branch_key
    {
        return Err(StoreError::A2aProjectionMismatch);
    }
    let encoded_event: String = connection.query_row(
        "SELECT envelope_json FROM events WHERE sequence=?",
        [row.sequence],
        |row| row.get(0),
    )?;
    let event = EventEnvelope::from_json_slice(encoded_event.as_bytes())?;
    validate_audit_record(&record, &event)?;
    let task =
        load_task(connection, record.run.task_id())?.ok_or(StoreError::A2aProjectionMismatch)?;
    if task.title() != record.task.title() || task.description() != record.task.description() {
        return Err(StoreError::A2aProjectionMismatch);
    }
    record.task = task;
    let (run_json, sequence): (String, u64) = connection.query_row("SELECT state_json,updated_sequence FROM projections WHERE entity_kind='run' AND entity_id=?", [run_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if serde_json::from_str::<Run>(&run_json)? != record.run || sequence != row.sequence {
        return Err(StoreError::A2aProjectionMismatch);
    }
    record.validate().map_err(StoreError::A2aState)?;
    Ok(Some(record))
}

fn validate_audit_record(record: &A2aRunRecord, event: &EventEnvelope) -> Result<(), StoreError> {
    if event.project_id() != record.task.project_id()
        || event.task_id() != Some(record.task.task_id())
    {
        return Err(StoreError::A2aProjectionMismatch);
    }
    let payload = event.payload().value();
    let digest = if event.run_id() == Some(record.run.run_id()) {
        payload.get("record_sha256")
    } else if payload
        .get("related_run_id")
        .and_then(serde_json::Value::as_str)
        == Some(record.run.run_id().to_string().as_str())
    {
        payload.get("related_record_sha256")
    } else {
        return Err(StoreError::A2aProjectionMismatch);
    };
    if digest.and_then(serde_json::Value::as_str) != Some(fingerprint("record", record)?.as_str()) {
        return Err(StoreError::A2aProjectionMismatch);
    }
    Ok(())
}
fn records_for_task(
    connection: &rusqlite::Connection,
    task_id: TaskId,
) -> Result<Vec<A2aRunRecord>, StoreError> {
    let mut statement =
        connection.prepare("SELECT run_id FROM a2a_runs WHERE task_id=? ORDER BY rowid LIMIT ?")?;
    let rows = statement.query_map(
        params![task_id.to_string(), (MAX_A2A_RUNS_PER_TASK + 1) as u32],
        |row| row.get::<_, String>(0),
    )?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    if ids.len() > MAX_A2A_RUNS_PER_TASK {
        return Err(StoreError::A2aCapacity);
    }
    ids.into_iter()
        .map(|id| {
            let run_id = id.parse().map_err(|_| StoreError::A2aProjectionMismatch)?;
            load_record(connection, run_id)?.ok_or(StoreError::A2aProjectionMismatch)
        })
        .collect()
}

fn save_task(transaction: &Transaction<'_>, task: &Task, sequence: u64) -> Result<(), StoreError> {
    save_projection(
        transaction,
        "task",
        &task.task_id().to_string(),
        &task.project_id().to_string(),
        &serde_json::to_string(task)?,
        sequence,
    )
}

fn save_record(
    transaction: &Transaction<'_>,
    record: &A2aRunRecord,
    sequence: u64,
    insert: bool,
) -> Result<(), StoreError> {
    let encoded = serde_json::to_string(record)?;
    if insert {
        transaction.execute("INSERT INTO a2a_runs(run_id,task_id,project_id,peer_id,external_task_id,worktree_path_key,branch_key,version,record_json,updated_sequence) VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![record.run.run_id().to_string(), record.task.task_id().to_string(), record.task.project_id().to_string(), record.binding.peer_id,
                record.binding.external_task_id, record.workspace.path_key(), record.workspace.branch.to_ascii_lowercase(), record.version, encoded, sequence])?;
    } else {
        let changed = transaction.execute("UPDATE a2a_runs SET version=?,record_json=?,updated_sequence=? WHERE run_id=? AND version=?", params![record.version, encoded, sequence, record.run.run_id().to_string(), record.version - 1])?;
        if changed != 1 {
            return Err(StoreError::A2aVersionConflict);
        }
    }
    save_projection(
        transaction,
        "run",
        &record.run.run_id().to_string(),
        &record.run.project_id().to_string(),
        &serde_json::to_string(&record.run)?,
        sequence,
    )
}

fn save_projection(
    transaction: &Transaction<'_>,
    kind: &str,
    id: &str,
    project: &str,
    state: &str,
    sequence: u64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO projections(entity_kind,entity_id,project_id,state_json,updated_sequence) VALUES (?,?,?,?,?) ON CONFLICT(entity_kind,entity_id) DO UPDATE SET state_json=excluded.state_json,updated_sequence=excluded.updated_sequence",
        params![kind, id, project, state, sequence])?;
    Ok(())
}

fn duplicate(
    transaction: &Transaction<'_>,
    key: &str,
    fingerprint: &str,
) -> Result<Option<A2aCommitOutcome>, StoreError> {
    let saved: Option<(String, String, String)> = transaction.query_row(
        "SELECT c.fingerprint,c.record_json,e.envelope_json FROM a2a_commands c JOIN events e ON e.sequence=c.event_sequence WHERE c.idempotency_key=?",
        [key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    if let Some((previous, record, event)) = saved {
        if previous != fingerprint {
            return Err(StoreError::A2aIdempotencyConflict);
        }
        let record = serde_json::from_str(&record)?;
        let event = EventEnvelope::from_json_slice(event.as_bytes())?;
        validate_audit_record(&record, &event)?;
        return Ok(Some(A2aCommitOutcome {
            record,
            event,
            duplicate: true,
        }));
    }
    let occupied: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE idempotency_key=?)",
        [key],
        |row| row.get(0),
    )?;
    if occupied {
        return Err(StoreError::A2aIdempotencyConflict);
    }
    Ok(None)
}

fn append_event(
    transaction: &Transaction<'_>,
    event_type: &str,
    record: &A2aRunRecord,
    related: Option<&A2aRunRecord>,
    timestamp: time::OffsetDateTime,
    key: &str,
    failure_code: Option<&str>,
) -> Result<EventEnvelope, StoreError> {
    let payload = EventPayload::new(json!({
        "record_sha256": fingerprint("record", record)?, "record_version": record.version,
        "peer_id": record.binding.peer_id, "external_task_id": record.binding.external_task_id,
        "transport": record.binding.transport, "protocol_version": record.binding.protocol_version,
        "base_commit": record.run.base_commit(), "run_status": record.run.status(), "task_status": record.task.status(),
        "remote_state": record.binding.last_remote_state, "cancellation_state": record.binding.cancellation_state,
        "artifact_references": record.binding.artifacts, "verification": record.binding.verification,
        "related_run_id": related.map(|value| value.run.run_id()),
        "related_record_sha256": related.map(|value| fingerprint("record", value)).transpose()?, "failure_code": failure_code,
    }))?;
    let draft = EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new(event_type)?,
        project_id: record.task.project_id(),
        task_id: Some(record.task.task_id()),
        run_id: Some(record.run.run_id()),
        causation_id: None,
        actor: ActorName::new("daemon_a2a")?,
        timestamp,
        idempotency_key: Some(key.to_string()),
        payload,
    };
    draft.validate()?;
    transaction.execute(
        "INSERT INTO events(event_id,idempotency_key,envelope_json) VALUES (?,?,'')",
        params![draft.event_id.to_string(), key],
    )?;
    let sequence =
        u64::try_from(transaction.last_insert_rowid()).map_err(|_| StoreError::InvalidSequence)?;
    let event = EventEnvelope::commit(draft, EventSequence::new(sequence)?)?;
    transaction.execute(
        "UPDATE events SET envelope_json=? WHERE sequence=?",
        params![
            String::from_utf8(event.to_json_vec()?)
                .map_err(|_| StoreError::A2aProjectionMismatch)?,
            sequence
        ],
    )?;
    Ok(event)
}

fn save_command(
    transaction: &Transaction<'_>,
    key: &str,
    fingerprint: &str,
    record: &A2aRunRecord,
    event: &EventEnvelope,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO a2a_commands(idempotency_key,fingerprint,run_id,record_json,event_sequence) VALUES (?,?,?,?,?)",
        params![key, fingerprint, record.run.run_id().to_string(), serde_json::to_string(record)?, event.sequence().get()])?;
    Ok(())
}

fn fingerprint(kind: &str, value: &impl serde::Serialize) -> Result<String, StoreError> {
    let mut hasher = Sha256::new();
    hasher.update(b"vibemux.a2a.state.v1\0");
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(value)?);
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut encoded, "{byte:02x}").map_err(|_| StoreError::A2aProjectionMismatch)?;
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::{
        ArtifactId, ProjectId, RunSpec, TaskSpec,
        a2a::{ArtifactReference, RunWorkspace},
    };

    fn fixture_draft(project_id: ProjectId, key: &str, event_type: &str) -> EventDraft {
        EventDraft {
            event_id: EventId::new(),
            event_type: EventType::new(event_type).expect("event type"),
            project_id,
            task_id: None,
            run_id: None,
            causation_id: None,
            actor: ActorName::new("test").expect("actor"),
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: Some(key.to_string()),
            payload: EventPayload::new(json!({"fixture":true})).expect("payload"),
        }
    }
    fn task() -> Task {
        Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "bounded A2A workflow".to_string(),
            description: String::new(),
        })
        .expect("task")
    }

    fn start_request(task: &Task, role: &str, peer: &str) -> A2aRunStart {
        let run = Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: role.to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run");
        let id = run.run_id();
        A2aRunStart {
            task: task.clone(),
            run,
            peer_id: peer.to_string(),
            external_task_id: format!("external_{id}"),
            transport: "http_json".to_string(),
            protocol_version: "1.0".to_string(),
            workspace: RunWorkspace {
                path: format!("/vibemux_test/{id}"),
                branch: format!("codex/run_{id}"),
                base_commit: "a".repeat(40),
                ownership_token: format!("owner_{id}"),
            },
            timestamp: time::OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("timestamp"),
            idempotency_key: format!("start_{id}"),
        }
    }

    fn artifact(digit: char) -> ArtifactReference {
        ArtifactReference {
            artifact_id: ArtifactId::new(),
            sha256: digit.to_string().repeat(64),
            size_bytes: 32,
            media_type: "application/json".to_string(),
        }
    }

    fn update(
        store: &mut SqliteStore,
        record: &A2aRunRecord,
        action: A2aRunAction,
    ) -> A2aRunRecord {
        store
            .update_a2a_run(A2aRunUpdate {
                run_id: record.run.run_id(),
                expected_version: record.version,
                timestamp: record.binding.updated_at + time::Duration::milliseconds(1),
                idempotency_key: format!("update_{}_{}", record.run.run_id(), record.version),
                action,
            })
            .expect("update")
            .record
    }

    fn completed_pair(
        store: &mut SqliteStore,
        task: &Task,
        reviewer_peer: &str,
    ) -> (A2aRunRecord, A2aRunRecord) {
        let worker = store
            .start_a2a_run(start_request(task, "worker", "worker_peer"))
            .expect("worker")
            .record;
        let reviewer = store
            .start_a2a_run(start_request(task, "reviewer", reviewer_peer))
            .expect("reviewer")
            .record;
        let worker = update(store, &worker, A2aRunAction::Start);
        let reviewer = update(store, &reviewer, A2aRunAction::Start);
        let worker = update(
            store,
            &worker,
            A2aRunAction::Observe {
                state: A2aRemoteState::Completed,
                artifacts: vec![artifact('a')],
            },
        );
        let reviewer = update(
            store,
            &reviewer,
            A2aRunAction::Observe {
                state: A2aRemoteState::Completed,
                artifacts: vec![artifact('b')],
            },
        );
        (worker, reviewer)
    }

    fn verification(
        worker: &A2aRunRecord,
        reviewer: &A2aRunRecord,
        accepted: bool,
    ) -> A2aRunUpdate {
        A2aRunUpdate {
            run_id: worker.run.run_id(),
            expected_version: worker.version,
            timestamp: worker.binding.updated_at + time::Duration::seconds(1),
            idempotency_key: format!("verify_{}", worker.run.run_id()),
            action: A2aRunAction::Verify {
                receipt: VerificationReceipt {
                    reviewer_run_id: reviewer.run.run_id(),
                    reviewer_expected_version: reviewer.version,
                    artifact_sha256: "a".repeat(64),
                    review_sha256: "b".repeat(64),
                    verification_sha256: "c".repeat(64),
                    accepted,
                },
            },
        }
    }

    #[test]
    fn create_duplicate_conflict_and_transaction_rollback_are_atomic() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let request = start_request(&task(), "worker", "worker_peer");
        store.connection.execute_batch("CREATE TRIGGER reject_binding BEFORE INSERT ON a2a_runs BEGIN SELECT RAISE(ABORT,'fixture'); END;").expect("trigger");
        assert!(store.start_a2a_run(request.clone()).is_err());
        assert!(store.events().expect("events").is_empty());
        assert!(
            store
                .projection("task", &request.task.task_id().to_string())
                .expect("projection")
                .is_none()
        );
        store
            .connection
            .execute_batch("DROP TRIGGER reject_binding;")
            .expect("remove trigger");
        let first = store.start_a2a_run(request.clone()).expect("start");
        assert_eq!(first.record.version, 1);
        assert_eq!(first.record.task.status(), TaskStatus::InProgress);
        let duplicate = store.start_a2a_run(request.clone()).expect("duplicate");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.record, first.record);
        assert_eq!(duplicate.event, first.event);
        let mut conflict = request;
        conflict.peer_id = "different_peer".to_string();
        assert!(matches!(
            store.start_a2a_run(conflict),
            Err(StoreError::A2aIdempotencyConflict)
        ));
        assert_eq!(store.events().expect("events").len(), 1);
    }

    #[test]
    fn completed_remote_state_does_not_grant_success_and_stale_commands_fail() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let initial = store
            .start_a2a_run(start_request(&task(), "worker", "worker_peer"))
            .expect("start")
            .record;
        let running = update(&mut store, &initial, A2aRunAction::Start);
        let completed = update(
            &mut store,
            &running,
            A2aRunAction::Observe {
                state: A2aRemoteState::Completed,
                artifacts: vec![artifact('a')],
            },
        );
        assert_eq!(completed.run.status(), RunStatus::Running);
        assert_eq!(completed.task.status(), TaskStatus::InProgress);
        let before = store.events().expect("events").len();
        assert!(matches!(
            store.update_a2a_run(A2aRunUpdate {
                run_id: initial.run.run_id(),
                expected_version: 1,
                timestamp: completed.binding.updated_at,
                idempotency_key: "stale_update".to_string(),
                action: A2aRunAction::Fail {
                    code: "failure".to_string()
                }
            }),
            Err(StoreError::A2aVersionConflict)
        ));
        assert!(matches!(
            store.update_a2a_run(A2aRunUpdate {
                run_id: completed.run.run_id(),
                expected_version: completed.version,
                timestamp: completed.binding.updated_at,
                idempotency_key: "artifact_replacement".to_string(),
                action: A2aRunAction::Observe {
                    state: A2aRemoteState::Completed,
                    artifacts: vec![artifact('d')]
                }
            }),
            Err(StoreError::A2aInvalidTransition)
        ));
        assert_eq!(store.events().expect("events").len(), before);
    }

    #[test]
    fn independent_verification_commits_pair_and_task_and_survives_reopen() {
        let directory = tempfile::tempdir().expect("directory");
        let database = directory.path().join("state.sqlite3");
        let mut store = SqliteStore::open(&database).expect("store");
        let task = task();
        let (worker, reviewer) = completed_pair(&mut store, &task, "reviewer_peer");
        let command = verification(&worker, &reviewer, true);
        let result = store.update_a2a_run(command.clone()).expect("verified");
        assert_eq!(result.record.run.status(), RunStatus::Succeeded);
        assert_eq!(result.record.task.status(), TaskStatus::Done);
        let review = store
            .a2a_run(reviewer.run.run_id())
            .expect("query")
            .expect("reviewer");
        assert_eq!(review.run.status(), RunStatus::Succeeded);
        assert_eq!(review.task.status(), TaskStatus::Done);
        assert_eq!(review.version, reviewer.version + 1);
        let events = store.events().expect("events");
        let encoded = serde_json::to_string(&events).expect("serialize");
        assert!(!encoded.contains("/vibemux_test/"));
        assert!(!encoded.contains("owner_"));
        assert!(!encoded.contains("bounded A2A workflow"));
        let duplicate = store
            .update_a2a_run(command)
            .expect("retry after completed");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.event, result.event);
        drop(store);
        let reopened = SqliteStore::open(&database).expect("reopen");
        let records = reopened.a2a_runs(task.task_id()).expect("replay query");
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .all(|record| record.task.status() == TaskStatus::Done
                    && record.run.status() == RunStatus::Succeeded)
        );
        assert_eq!(reopened.events().expect("replayed events"), events);
    }

    #[test]
    fn verification_rejects_same_peer_missing_evidence_stale_reviewer_and_other_live_run() {
        for failure in [
            "same_peer",
            "missing_evidence",
            "stale_reviewer",
            "other_live",
            "pending_cancellation",
        ] {
            let directory = tempfile::tempdir().expect("directory");
            let mut store =
                SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
            let task = task();
            let (worker, reviewer) = completed_pair(
                &mut store,
                &task,
                if failure == "same_peer" {
                    "worker_peer"
                } else {
                    "reviewer_peer"
                },
            );
            let worker = if failure == "pending_cancellation" {
                update(&mut store, &worker, A2aRunAction::RequestCancellation)
            } else {
                worker
            };
            let mut command = verification(&worker, &reviewer, true);
            if let A2aRunAction::Verify { receipt } = &mut command.action {
                if failure == "missing_evidence" {
                    receipt.review_sha256 = "d".repeat(64);
                }
                if failure == "stale_reviewer" {
                    receipt.reviewer_expected_version -= 1;
                }
            }
            if failure == "other_live" {
                store
                    .start_a2a_run(start_request(&task, "worker", "other_peer"))
                    .expect("other run");
            }
            let before = store.events().expect("events").len();
            assert!(
                matches!(
                    store.update_a2a_run(command),
                    Err(StoreError::A2aVerificationRejected | StoreError::A2aVersionConflict)
                ),
                "{failure}"
            );
            assert_eq!(store.events().expect("events").len(), before);
            assert_eq!(
                store
                    .a2a_run(worker.run.run_id())
                    .expect("query")
                    .expect("worker")
                    .run
                    .status(),
                RunStatus::Running
            );
        }
    }

    #[test]
    fn rejected_verification_retains_failed_attempt_and_allows_new_repair_runs() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let task = task();
        let (worker, reviewer) = completed_pair(&mut store, &task, "reviewer_peer");
        let failed = store
            .update_a2a_run(verification(&worker, &reviewer, false))
            .expect("rejected verdict")
            .record;
        assert_eq!(failed.run.status(), RunStatus::Failed);
        assert_eq!(failed.task.status(), TaskStatus::Blocked);
        let history = store.events().expect("history");
        let (repair, repair_review) = completed_pair(&mut store, &task, "reviewer_peer");
        let repaired = store
            .update_a2a_run(verification(&repair, &repair_review, true))
            .expect("repair verified")
            .record;
        assert_eq!(repaired.task.status(), TaskStatus::Done);
        assert_ne!(repair.run.run_id(), failed.run.run_id());
        assert_eq!(
            store
                .a2a_run(failed.run.run_id())
                .expect("query")
                .expect("failed")
                .run
                .status(),
            RunStatus::Failed
        );
        assert_eq!(store.a2a_runs(task.task_id()).expect("runs").len(), 4);
        assert_eq!(&store.events().expect("events")[..history.len()], &history);
    }

    #[test]
    fn cancellation_requires_request_and_observation_and_preserves_canonical_graph() {
        for started in [false, true] {
            let directory = tempfile::tempdir().expect("directory");
            let mut store =
                SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
            let mut record = store
                .start_a2a_run(start_request(&task(), "worker", "worker_peer"))
                .expect("start")
                .record;
            if started {
                record = update(&mut store, &record, A2aRunAction::Start);
            }
            let bad = A2aRunUpdate {
                run_id: record.run.run_id(),
                expected_version: record.version,
                timestamp: record.binding.updated_at,
                idempotency_key: "premature_confirmation".to_string(),
                action: A2aRunAction::ConfirmCancellation,
            };
            assert!(matches!(
                store.update_a2a_run(bad),
                Err(StoreError::A2aInvalidTransition)
            ));
            record = update(&mut store, &record, A2aRunAction::RequestCancellation);
            record = update(
                &mut store,
                &record,
                A2aRunAction::Observe {
                    state: A2aRemoteState::Cancelled,
                    artifacts: vec![],
                },
            );
            record = update(&mut store, &record, A2aRunAction::ConfirmCancellation);
            assert_eq!(
                record.run.status(),
                if started {
                    RunStatus::Stopped
                } else {
                    RunStatus::Failed
                }
            );
            assert_eq!(record.task.status(), TaskStatus::Cancelled);
            assert_eq!(
                record.binding.cancellation_state,
                A2aCancellationState::Confirmed
            );
        }
    }

    #[test]
    fn legacy_snapshot_api_cannot_bypass_a2a_authority_or_reuse_workspace() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let task = task();
        let initial = store
            .start_a2a_run(start_request(&task, "worker", "worker_peer"))
            .expect("start")
            .record;
        let mut reused = start_request(&task, "reviewer", "reviewer_peer");
        reused.workspace.path = initial.workspace.path.to_ascii_uppercase();
        assert!(matches!(
            store.start_a2a_run(reused),
            Err(StoreError::A2aBindingConflict)
        ));
        let running = update(&mut store, &initial, A2aRunAction::Start);
        let counterfeit = running
            .run
            .transitioned(RunStatus::Succeeded, Some(RunCompletionAuthority::Verifier))
            .expect("domain transition");
        assert!(matches!(
            store.commit_run(
                &counterfeit,
                fixture_draft(task.project_id(), "legacy_bypass", "run_succeeded")
            ),
            Err(StoreError::A2aBoundProjection)
        ));
        assert!(matches!(
            store.commit_task(
                &running.task,
                fixture_draft(task.project_id(), "legacy_task", "task_updated")
            ),
            Err(StoreError::A2aBoundProjection)
        ));
    }

    #[test]
    fn failed_verification_write_rolls_back_both_runs_task_and_event() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let task = task();
        let (worker, reviewer) = completed_pair(&mut store, &task, "reviewer_peer");
        let original_events = store.events().expect("events");
        // The worker write succeeds first; fail the second row inside the transaction.
        store.connection.execute_batch(&format!("CREATE TRIGGER reject_review_update BEFORE UPDATE ON a2a_runs WHEN NEW.run_id='{}' BEGIN SELECT RAISE(ABORT,'fixture'); END;", reviewer.run.run_id())).expect("trigger");
        assert!(
            store
                .update_a2a_run(verification(&worker, &reviewer, true))
                .is_err()
        );
        assert_eq!(
            store
                .a2a_run(worker.run.run_id())
                .expect("worker query")
                .expect("worker"),
            worker
        );
        assert_eq!(
            store
                .a2a_run(reviewer.run.run_id())
                .expect("reviewer query")
                .expect("reviewer"),
            reviewer
        );
        assert_eq!(
            store.events().expect("events after rollback"),
            original_events
        );
    }

    #[test]
    fn corrupted_binding_columns_and_audit_digest_fail_closed() {
        for corruption in ["column", "digest"] {
            let directory = tempfile::tempdir().expect("directory");
            let mut store =
                SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
            let record = store
                .start_a2a_run(start_request(&task(), "worker", "worker_peer"))
                .expect("start")
                .record;
            if corruption == "column" {
                store
                    .connection
                    .execute(
                        "UPDATE a2a_runs SET peer_id='different_peer' WHERE run_id=?",
                        [record.run.run_id().to_string()],
                    )
                    .expect("fixture corruption");
            } else {
                let mut changed = record.clone();
                changed.workspace.ownership_token = "different_owner".to_string();
                store
                    .connection
                    .execute(
                        "UPDATE a2a_runs SET record_json=? WHERE run_id=?",
                        params![
                            serde_json::to_string(&changed).expect("record JSON"),
                            record.run.run_id().to_string()
                        ],
                    )
                    .expect("fixture corruption");
            }
            assert!(
                matches!(
                    store.a2a_run(record.run.run_id()),
                    Err(StoreError::A2aProjectionMismatch)
                ),
                "{corruption}"
            );
        }
    }

    #[test]
    fn task_cancels_only_after_all_runs_stop() {
        let directory = tempfile::tempdir().expect("directory");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let task = task();
        let first = store
            .start_a2a_run(start_request(&task, "worker", "worker_peer"))
            .expect("first")
            .record;
        let second = store
            .start_a2a_run(start_request(&task, "reviewer", "reviewer_peer"))
            .expect("second")
            .record;
        let first = update(&mut store, &first, A2aRunAction::Start);
        let second = update(&mut store, &second, A2aRunAction::Start);
        let mut cancelled = first;
        cancelled = update(&mut store, &cancelled, A2aRunAction::RequestCancellation);
        cancelled = update(
            &mut store,
            &cancelled,
            A2aRunAction::Observe {
                state: A2aRemoteState::Cancelled,
                artifacts: vec![],
            },
        );
        cancelled = update(&mut store, &cancelled, A2aRunAction::ConfirmCancellation);
        assert_eq!(cancelled.task.status(), TaskStatus::Blocked);
        let mut cancelled = second;
        cancelled = update(&mut store, &cancelled, A2aRunAction::RequestCancellation);
        cancelled = update(
            &mut store,
            &cancelled,
            A2aRunAction::Observe {
                state: A2aRemoteState::Cancelled,
                artifacts: vec![],
            },
        );
        cancelled = update(&mut store, &cancelled, A2aRunAction::ConfirmCancellation);
        assert_eq!(cancelled.task.status(), TaskStatus::Cancelled);
    }
    #[test]
    fn v1_migration_preserves_historical_event_bytes_and_projection() {
        let directory = tempfile::tempdir().expect("directory");
        let database = directory.path().join("state.sqlite3");
        let connection = rusqlite::Connection::open(&database).expect("connection");
        connection
            .execute_batch(super::super::INITIAL_SCHEMA)
            .expect("v1 schema");
        let task = task();
        let event = EventEnvelope::commit(
            fixture_draft(task.project_id(), "historical", "task_created"),
            EventSequence::new(1).expect("sequence"),
        )
        .expect("event");
        let encoded = serde_json::to_string(&event).expect("JSON");
        connection.execute("INSERT INTO events(sequence,event_id,idempotency_key,envelope_json) VALUES (1,?,'historical',?)", params![event.event_id().to_string(), encoded]).expect("historical event");
        connection
            .execute(
                "INSERT INTO projections VALUES ('task',?,?,?,1)",
                params![
                    task.task_id().to_string(),
                    task.project_id().to_string(),
                    serde_json::to_string(&task).expect("task JSON")
                ],
            )
            .expect("projection");
        drop(connection);
        let migrated = SqliteStore::open(&database).expect("migration");
        let preserved: String = migrated
            .connection
            .query_row(
                "SELECT envelope_json FROM events WHERE sequence=1",
                [],
                |row| row.get(0),
            )
            .expect("preserved bytes");
        assert_eq!(preserved, encoded);
        // The v3 migration appends its harness seed after the historical
        // event; the historical bytes at sequence 1 are unchanged.
        let events = migrated.events().expect("events");
        assert_eq!(events[0], event);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event_type().as_str(), "v3_harness_seed");
        assert_eq!(
            migrated
                .projection("task", &task.task_id().to_string())
                .expect("projection")
                .expect("task"),
            serde_json::to_value(task).expect("task JSON")
        );
        assert_eq!(
            migrated
                .connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
                    .get::<_, u32>(
                    0
                ))
                .expect("version"),
            crate::STORE_SCHEMA_VERSION
        );
    }
    #[test]
    fn owner_cancellation_after_peer_completion_never_forges_remote_cancellation() {
        let directory = tempfile::tempdir().expect("temp");
        let mut store = SqliteStore::open(&directory.path().join("state.sqlite3")).expect("store");
        let task = task();
        let (worker, reviewer) = completed_pair(&mut store, &task, "reviewer_peer");
        let premature = A2aRunUpdate {
            run_id: worker.run.run_id(),
            expected_version: worker.version,
            timestamp: worker.binding.updated_at,
            idempotency_key: "premature_owner_cancel".into(),
            action: A2aRunAction::FinalizeCancellation,
        };
        assert!(matches!(
            store.update_a2a_run(premature),
            Err(StoreError::A2aInvalidTransition)
        ));
        let worker = update(
            &mut store,
            &worker,
            A2aRunAction::Fail {
                code: "parent_cancelled".into(),
            },
        );
        update(
            &mut store,
            &reviewer,
            A2aRunAction::Fail {
                code: "parent_cancelled".into(),
            },
        );
        let worker = store
            .a2a_run(worker.run.run_id())
            .expect("query")
            .expect("worker");
        let canceled = update(&mut store, &worker, A2aRunAction::FinalizeCancellation);
        assert_eq!(canceled.task.status(), TaskStatus::Cancelled);
        assert_eq!(
            canceled.binding.last_remote_state,
            A2aRemoteState::Completed
        );
        assert_eq!(
            canceled.binding.cancellation_state,
            A2aCancellationState::None
        );
        assert!(
            store
                .a2a_runs(task.task_id())
                .expect("runs")
                .iter()
                .all(|record| record.task.status() == TaskStatus::Cancelled
                    && record.run.status() == RunStatus::Failed)
        );
    }
}
