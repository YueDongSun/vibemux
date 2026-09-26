//! Row persistence for `harness_dispatches` and the dispatch-owned Task/Run
//! projections. Every load cross-checks the indexed columns against the
//! record JSON, recomputes the fingerprint, and checks that the phase allows
//! the Task/Run statuses and that a completed record keeps its evidence; any
//! disagreement fails closed.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;
use uuid::Uuid;
use vibemux_harness::dispatch::{
    AttemptOutcome, DispatchError, DispatchPhase, Sha256Digest, request::request_fingerprint,
};
use vibemux_types::{Run, Task};

use super::{
    ClaimFence, HARNESS_DISPATCH_RECORD_SCHEMA_VERSION, HarnessDispatchCommit,
    HarnessDispatchRecord, is_clean_completion,
};
use crate::StoreError;

/// A validated row with its fence and the projections it owns.
pub(super) struct StoredDispatch {
    pub(super) record: HarnessDispatchRecord,
    pub(super) fence: Option<ClaimFence>,
    pub(super) sequence: u64,
    pub(super) task: Task,
    pub(super) run: Run,
}

impl StoredDispatch {
    /// The commit for a call that wrote nothing.
    pub(super) fn unchanged(self) -> HarnessDispatchCommit {
        HarnessDispatchCommit {
            record: self.record,
            event: None,
            sequence: self.sequence,
        }
    }
}

struct PersistedRow {
    task_id: String,
    run_id: String,
    fingerprint: String,
    resource_key: String,
    phase: String,
    claim_fence: Option<String>,
    version: u64,
    record_json: String,
    updated_sequence: u64,
}

pub(super) fn load(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<StoredDispatch>, StoreError> {
    let row = connection
        .query_row(
            "SELECT task_id, run_id, fingerprint, resource_key, phase, claim_fence, version, record_json, updated_sequence FROM harness_dispatches WHERE request_id = ?",
            [request_key(request_id)],
            |row| {
                Ok(PersistedRow {
                    task_id: row.get(0)?,
                    run_id: row.get(1)?,
                    fingerprint: row.get(2)?,
                    resource_key: row.get(3)?,
                    phase: row.get(4)?,
                    claim_fence: row.get(5)?,
                    version: row.get(6)?,
                    record_json: row.get(7)?,
                    updated_sequence: row.get(8)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record: HarnessDispatchRecord = serde_json::from_str(&row.record_json)?;
    let expected_fingerprint = request_fingerprint(
        record.project_id,
        record.request_id,
        record.harness,
        record.protocol,
        record.prompt.sha256,
        record.config_sha256,
    );
    if record.schema_version != HARNESS_DISPATCH_RECORD_SCHEMA_VERSION
        || record.request_id != request_id
        || record.task_id.to_string() != row.task_id
        || record.run_id.to_string() != row.run_id
        || record.fingerprint.to_hex() != row.fingerprint
        || record.fingerprint != expected_fingerprint
        || record.resource_key.to_hex() != row.resource_key
        || record.phase.as_str() != row.phase
        || record.version != row.version
    {
        return Err(StoreError::HarnessDispatchProjectionMismatch);
    }
    let fence = row
        .claim_fence
        .map(|text| {
            Uuid::parse_str(&text)
                .map(ClaimFence)
                .map_err(|_| StoreError::HarnessDispatchProjectionMismatch)
        })
        .transpose()?;
    let task: Task = load_projection(connection, "task", &row.task_id)?;
    let run: Run = load_projection(connection, "run", &row.run_id)?;
    if task.task_id() != record.task_id
        || task.project_id() != record.project_id
        || run.run_id() != record.run_id
        || run.task_id() != record.task_id
        || run.project_id() != record.project_id
    {
        return Err(StoreError::HarnessDispatchProjectionMismatch);
    }
    if !record
        .phase
        .allowed_statuses()
        .contains(&(task.status(), run.status()))
    {
        return Err(StoreError::HarnessDispatchProjectionMismatch);
    }
    // A completed record must still carry the evidence that justified it.
    if record.phase == DispatchPhase::Completed
        && !(record.outcome == Some(AttemptOutcome::Completed)
            && record.capture.is_some()
            && record
                .process
                .is_some_and(|process| is_clean_completion(record.error_code, &process)))
    {
        return Err(StoreError::HarnessDispatchProjectionMismatch);
    }
    Ok(Some(StoredDispatch {
        record,
        fence,
        sequence: row.updated_sequence,
        task,
        run,
    }))
}

pub(super) fn load_existing(
    connection: &Connection,
    request_id: Uuid,
) -> Result<StoredDispatch, StoreError> {
    load(connection, request_id)?.ok_or(StoreError::HarnessDispatch(DispatchError::NotFound))
}

/// Whether an attempt in a reserving phase already holds `resource_key`.
pub(super) fn is_reserved(
    connection: &Connection,
    resource_key: Sha256Digest,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM harness_dispatches WHERE resource_key = ? AND phase IN ('admitted', 'running', 'cancel_requested', 'recovery_pending'))",
        [resource_key.to_hex()],
        |row| row.get(0),
    )?)
}

/// Stored keys of the attempts that startup recovery changes, in admission
/// order. They are returned unparsed so recovery can quarantine a bad key
/// without stopping.
pub(super) fn active_request_keys(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT request_id FROM harness_dispatches WHERE phase IN ('admitted', 'running', 'cancel_requested') ORDER BY rowid",
    )?;
    let keys = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(keys)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    record: &HarnessDispatchRecord,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO harness_dispatches(request_id, task_id, run_id, fingerprint, resource_key, phase, claim_fence, version, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)",
        params![
            request_key(record.request_id),
            record.task_id.to_string(),
            record.run_id.to_string(),
            record.fingerprint.to_hex(),
            record.resource_key.to_hex(),
            record.phase.as_str(),
            record.version,
            serde_json::to_string(record)?,
            sequence,
        ],
    )?;
    Ok(())
}

/// Compare-and-set on the version the transaction loaded.
pub(super) fn update(
    transaction: &Transaction<'_>,
    record: &HarnessDispatchRecord,
    fence: Option<ClaimFence>,
    previous_version: u64,
    sequence: i64,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE harness_dispatches SET phase = ?, claim_fence = ?, version = ?, record_json = ?, updated_sequence = ? WHERE request_id = ? AND version = ?",
        params![
            record.phase.as_str(),
            fence.map(|fence| fence.0.as_hyphenated().to_string()),
            record.version,
            serde_json::to_string(record)?,
            sequence,
            request_key(record.request_id),
            previous_version,
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::HarnessDispatchVersionConflict);
    }
    Ok(())
}

pub(super) fn save_task(
    transaction: &Transaction<'_>,
    task: &Task,
    sequence: i64,
) -> Result<(), StoreError> {
    save_projection(
        transaction,
        "task",
        &task.task_id().to_string(),
        &task.project_id().to_string(),
        &serde_json::to_string(task)?,
        sequence,
    )
}

pub(super) fn save_run(
    transaction: &Transaction<'_>,
    run: &Run,
    sequence: i64,
) -> Result<(), StoreError> {
    save_projection(
        transaction,
        "run",
        &run.run_id().to_string(),
        &run.project_id().to_string(),
        &serde_json::to_string(run)?,
        sequence,
    )
}

fn save_projection(
    transaction: &Transaction<'_>,
    entity_kind: &str,
    entity_id: &str,
    project_id: &str,
    state_json: &str,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        r#"
        INSERT INTO projections(entity_kind, entity_id, project_id, state_json, updated_sequence)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
            state_json = excluded.state_json,
            updated_sequence = excluded.updated_sequence
        "#,
        params![entity_kind, entity_id, project_id, state_json, sequence],
    )?;
    Ok(())
}

fn load_projection<T: DeserializeOwned>(
    connection: &Connection,
    entity_kind: &str,
    entity_id: &str,
) -> Result<T, StoreError> {
    let encoded: Option<String> = connection
        .query_row(
            "SELECT state_json FROM projections WHERE entity_kind = ? AND entity_id = ?",
            params![entity_kind, entity_id],
            |row| row.get(0),
        )
        .optional()?;
    let encoded = encoded.ok_or(StoreError::HarnessDispatchProjectionMismatch)?;
    Ok(serde_json::from_str(&encoded)?)
}

fn request_key(request_id: Uuid) -> String {
    request_id.as_hyphenated().to_string()
}
