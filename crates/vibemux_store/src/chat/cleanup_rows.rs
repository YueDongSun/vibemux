//! Bounded private cleanup projection and immutable operation/audit records.
use super::{cleanup::*, *};
use serde::Serialize;
use vibemux_types::{WorkspaceCleanupId, workspace_cleanup::*};

pub(super) struct StoredCleanup {
    pub cleanup: WorkspaceCleanup,
    pub sequence: u64,
}
impl StoredCleanup {
    pub fn receipt(self, duplicate: bool) -> WorkspaceCleanupCommit {
        WorkspaceCleanupCommit {
            cleanup: self.cleanup,
            event: None,
            sequence: self.sequence,
            duplicate,
            execution_claim: None,
        }
    }
}

struct CleanupRow {
    chat_id: String,
    run_id: String,
    resource_key: String,
    path_key: String,
    branch_key: String,
    phase: String,
    version: u64,
    encoded: Option<String>,
    sequence: u64,
}

pub(super) fn encode(value: &impl Serialize) -> Result<String, StoreError> {
    let encoded = serde_json::to_string(value)?;
    ensure(
        encoded.len() <= MAX_WORKSPACE_CLEANUP_METADATA_BYTES,
        "workspace_cleanup_invalid_input",
    )?;
    Ok(encoded)
}

pub(super) fn increment(version: u64) -> Result<u64, StoreError> {
    version
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| cleanup_error("workspace_cleanup_conflict"))
}

pub(super) fn load(
    connection: &Connection,
    id: WorkspaceCleanupId,
) -> Result<Option<StoredCleanup>, StoreError> {
    let row = connection.query_row(
        "SELECT chat_id,run_id,resource_key,worktree_path_key,branch_key,phase,version,CASE WHEN length(CAST(record_json AS BLOB))<=65536 THEN record_json END,updated_sequence FROM workspace_cleanups WHERE cleanup_id=?",
        [id.to_string()], |row|Ok(CleanupRow {chat_id:row.get(0)?,run_id:row.get(1)?,resource_key:row.get(2)?,path_key:row.get(3)?,branch_key:row.get(4)?,phase:row.get(5)?,version:row.get(6)?,encoded:row.get(7)?,sequence:row.get(8)?})).optional()?;
    let Some(row) = row else { return Ok(None) };
    let encoded = row
        .encoded
        .ok_or_else(|| cleanup_error("workspace_cleanup_projection_mismatch"))?;
    ensure(
        encoded.len() <= MAX_WORKSPACE_CLEANUP_METADATA_BYTES,
        "workspace_cleanup_projection_mismatch",
    )?;
    let record: WorkspaceCleanup = serde_json::from_str(&encoded)
        .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?;
    record
        .validate()
        .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?;
    ensure(
        record.id == id
            && record.binding.chat_id.to_string() == row.chat_id
            && record.binding.run_id.to_string() == row.run_id
            && record.binding.resource_key == row.resource_key
            && record.binding.workspace.path_key() == row.path_key
            && record.binding.workspace.branch.to_ascii_lowercase() == row.branch_key
            && record.phase.as_str() == row.phase
            && record.version == row.version
            && row.sequence > 0,
        "workspace_cleanup_projection_mismatch",
    )?;
    Ok(Some(StoredCleanup {
        cleanup: record,
        sequence: row.sequence,
    }))
}

pub(super) fn required(
    connection: &Connection,
    id: WorkspaceCleanupId,
) -> Result<StoredCleanup, StoreError> {
    load(connection, id)?.ok_or_else(|| cleanup_error("workspace_cleanup_not_found"))
}

pub(crate) fn is_reserved(connection: &Connection, resource_key: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM workspace_cleanups WHERE resource_key=? AND phase IN('claimed','armed','quiescent','unknown'))",
        [resource_key],|row|row.get(0))?)
}

pub(crate) fn path_is_reserved(
    connection: &Connection,
    path_key: &str,
    branch_key: &str,
) -> Result<bool, StoreError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM workspace_cleanups WHERE (worktree_path_key=?1 OR branch_key=?2) AND phase IN('claimed','armed','quiescent','unknown'))",
        params![path_key,branch_key],|row|row.get(0))?)
}

pub(super) fn replay(
    transaction: &Transaction<'_>,
    operation_id: Uuid,
    kind: &str,
    binding: &str,
) -> Result<Option<WorkspaceCleanupCommit>, StoreError> {
    let Some(operation) = operation(transaction, operation_id, kind, binding)? else {
        return Ok(None);
    };
    let id: String = transaction
        .query_row(
            "SELECT cleanup_id FROM workspace_cleanup_commands WHERE operation_id=?",
            [operation_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| cleanup_error("workspace_cleanup_projection_mismatch"))?;
    let stored = required(
        transaction,
        id.parse()
            .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?,
    )?;
    ensure(
        stored.cleanup.binding.chat_id == operation.chat_id
            && operation.released.is_empty()
            && operation.sequence <= stored.sequence,
        "workspace_cleanup_projection_mismatch",
    )?;
    Ok(Some(stored.receipt(true)))
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    record: WorkspaceCleanup,
    previous: Option<u64>,
    operation_id: Uuid,
    kind: &str,
    binding: &str,
    session: &ChatSession,
) -> Result<WorkspaceCleanupCommit, StoreError> {
    record
        .validate()
        .map_err(|_| cleanup_error("workspace_cleanup_invalid_input"))?;
    let encoded = encode(&record)?;
    let event_id = EventId::new();
    let (event, sequence) = SqliteStore::insert_event(
        transaction,
        EventDraft {
            event_id,
            event_type: EventType::new(kind)?,
            project_id: record.binding.project_id,
            task_id: Some(record.binding.task_id),
            run_id: Some(record.binding.run_id),
            causation_id: None,
            actor: ActorName::new("daemon_workspace_cleanup")?,
            timestamp: record.updated_at,
            idempotency_key: Some(format!("workspace_cleanup_event:{event_id}")),
            payload: EventPayload::new(
                json!({"cleanup_id":record.id,"chat_id":record.binding.chat_id,
            "version":record.version,"phase":record.phase,"outcome":record.outcome,"error_code":record.error_code}),
            )?,
        },
    )?;
    if let Some(previous) = previous {
        let changed = transaction.execute("UPDATE workspace_cleanups SET phase=?,version=?,record_json=?,updated_sequence=? WHERE cleanup_id=? AND version=?",
            params![record.phase.as_str(),record.version,encoded,sequence,record.id.to_string(),previous])?;
        ensure(changed == 1, "workspace_cleanup_conflict")?;
    } else {
        transaction.execute("INSERT INTO workspace_cleanups(cleanup_id,chat_id,run_id,resource_key,worktree_path_key,branch_key,phase,version,record_json,updated_sequence) VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![record.id.to_string(),record.binding.chat_id.to_string(),record.binding.run_id.to_string(),record.binding.resource_key,
                record.binding.workspace.path_key(),record.binding.workspace.branch.to_ascii_lowercase(),record.phase.as_str(),record.version,encoded,sequence])?;
    }
    record_operation(
        transaction,
        operation_id,
        session,
        kind,
        binding,
        &[],
        sequence,
    )?;
    transaction.execute(
        "INSERT INTO workspace_cleanup_commands(operation_id,cleanup_id) VALUES(?,?)",
        params![operation_id.to_string(), record.id.to_string()],
    )?;
    Ok(WorkspaceCleanupCommit {
        cleanup: record,
        event: Some(event),
        sequence: sequence as u64,
        duplicate: false,
        execution_claim: None,
    })
}
