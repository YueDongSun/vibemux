//! Bounded metadata loads and projection checks for chat-owned rows.
use super::{ChatOwnerFence, fail, require};
use crate::{Projection, StoreError};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;
use uuid::Uuid;
use vibemux_types::{Run, RunStatus, Task, TaskStatus, chat::*};

const MAX_METADATA_BYTES: usize = 16 * 1024;
struct SessionRow {
    project_id: String,
    state: String,
    generation: u64,
    version: u64,
    resource_key: String,
    fence: Option<String>,
    encoded: Option<String>,
    sequence: u64,
}
struct TurnRow {
    chat_id: String,
    turn_id: String,
    task_id: String,
    run_id: String,
    generation: u64,
    version: u64,
    phase: String,
    claimed: bool,
    binding: Option<String>,
    finish: Option<String>,
    encoded: Option<String>,
    sequence: u64,
    fence: Option<String>,
    admitted_sequence: u64,
    resource_key: String,
}
struct RecordRow {
    chat_id: String,
    turn_id: String,
    sequence: u64,
    kind: String,
    binding: Option<String>,
    encoded: Option<String>,
    event_sequence: u64,
}

pub(super) struct StoredSession {
    pub session: ChatSession,
    pub fence: Option<ChatOwnerFence>,
    pub sequence: u64,
}
pub(super) struct StoredTurn {
    pub turn: ChatTurn,
    pub binding: String,
    pub finish: Option<String>,
    pub claimed: bool,
    pub fence: Option<ChatOwnerFence>,
    pub sequence: u64,
    pub task: Task,
    pub run: Run,
}

pub(super) fn decode<T: DeserializeOwned>(text: Option<String>) -> Result<T, StoreError> {
    let text = text.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    require(
        text.len() <= MAX_METADATA_BYTES,
        ChatError::ProjectionMismatch,
    )?;
    serde_json::from_str(&text).map_err(|_| fail(ChatError::ProjectionMismatch))
}

pub(super) fn session(
    connection: &Connection,
    id: ChatId,
) -> Result<Option<StoredSession>, StoreError> {
    let row=connection.query_row(
        "SELECT project_id,state,generation,version,resource_key,owner_fence,CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,updated_sequence FROM chat_sessions WHERE chat_id=?",
        [id.to_string()],|row|Ok(SessionRow {project_id:row.get(0)?,state:row.get(1)?,generation:row.get(2)?,version:row.get(3)?,resource_key:row.get(4)?,fence:row.get(5)?,encoded:row.get(6)?,sequence:row.get(7)?})).optional()?;
    let Some(SessionRow {
        project_id,
        state,
        generation,
        version,
        resource_key,
        fence,
        encoded,
        sequence,
    }) = row
    else {
        return Ok(None);
    };
    let session: ChatSession = decode(encoded)?;
    let fence = fence
        .map(|value| {
            Uuid::parse_str(&value)
                .ok()
                .filter(|id| !id.is_nil())
                .map(ChatOwnerFence)
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
        })
        .transpose()?;
    require(
        session.schema_version == CHAT_SCHEMA_VERSION
            && session.chat_id == id
            && session.project_id.to_string() == project_id
            && session.state.as_str() == state
            && session.generation == generation
            && session.version == version
            && session.resource_key == resource_key
            && generation > 0
            && version > 0
            && sequence > 0
            && valid_digest(&session.resource_key)
            && valid_digest(&session.profile_sha256)
            && matches!(session.base_commit.len(), 40 | 64)
            && session
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && session.updated_at >= session.created_at
            && session.retained_bytes <= MAX_CHAT_BYTES
            && session.record_count <= MAX_CHAT_RECORDS
            && session.record_count == session.last_record_sequence,
        ChatError::ProjectionMismatch,
    )?;
    require(
        session
            .vendor_confirmed_generation
            .is_none_or(|confirmed| confirmed == generation && session.vendor_session_id.is_some()),
        ChatError::ProjectionMismatch,
    )?;
    for vendor in [&session.expected_vendor_id, &session.vendor_session_id]
        .into_iter()
        .flatten()
    {
        require(valid_vendor_id(vendor), ChatError::ProjectionMismatch)?;
    }
    if let (Some(expected), Some(actual)) =
        (&session.expected_vendor_id, &session.vendor_session_id)
    {
        require(expected == actual, ChatError::ProjectionMismatch)?;
    }
    require(
        match session.state {
            ChatState::Ready | ChatState::Busy => fence.is_some(),
            ChatState::Suspended | ChatState::Closed | ChatState::Archived => fence.is_none(),
            _ => true,
        },
        ChatError::ProjectionMismatch,
    )?;
    if let Some(reference) = &session.title_ref {
        reference
            .validate(MAX_CHAT_TITLE_BYTES)
            .map_err(|_| fail(ChatError::ProjectionMismatch))?;
        validate_content(connection, id, reference, true)?;
    }
    let bytes: u64 = connection.query_row(
        "SELECT COALESCE(SUM(byte_count),0) FROM chat_content WHERE chat_id=? AND state='present'",
        [id.to_string()],
        |row| row.get(0),
    )?;
    let (legacy_records, execution_records, legacy_attempts, execution_attempts): (u64,u64,u64,u64) = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM chat_records WHERE chat_id=?1),(SELECT COUNT(*) FROM chat_execution_records WHERE chat_id=?1),(SELECT COUNT(*) FROM chat_turns WHERE chat_id=?1),(SELECT COUNT(*) FROM chat_execution_attempts WHERE chat_id=?1)",
        [id.to_string()],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    )?;
    let execution_owned = super::owners::is_execution_owned(connection, id)?;
    require(
        if execution_owned {
            legacy_records == 0 && legacy_attempts == 0
        } else {
            execution_records == 0 && execution_attempts == 0
        },
        ChatError::ProjectionMismatch,
    )?;
    let count = if execution_owned {
        execution_records
    } else {
        legacy_records
    };
    let active:Option<String>=connection.query_row("SELECT request_id FROM chat_turns WHERE chat_id=?1 AND phase IN ('admitted','running','cancel_requested','recovery_required') UNION ALL SELECT request_id FROM chat_execution_attempts WHERE chat_id=?1 AND phase IN ('admitted','running','cancel_requested','recovery_required')",[id.to_string()],|row|row.get(0)).optional()?;
    require(
        bytes == session.retained_bytes
            && count == session.record_count
            && active == session.active_request_id.map(|id| id.to_string()),
        ChatError::ProjectionMismatch,
    )?;
    require(
        match session.state {
            ChatState::Busy => active.is_some(),
            ChatState::RecoveryRequired => true,
            _ => active.is_none(),
        },
        ChatError::ProjectionMismatch,
    )?;
    require(
        !session.content_purged
            || (session.state == ChatState::Archived
                && session.retained_bytes == 0
                && session.title_ref.is_none()),
        ChatError::ProjectionMismatch,
    )?;
    Ok(Some(StoredSession {
        session,
        fence,
        sequence,
    }))
}

pub(super) fn existing_session(
    connection: &Connection,
    id: ChatId,
) -> Result<StoredSession, StoreError> {
    session(connection, id)?.ok_or_else(|| fail(ChatError::NotFound))
}

pub(super) fn turn(connection: &Connection, id: Uuid) -> Result<Option<StoredTurn>, StoreError> {
    let row=connection.query_row(
        "SELECT chat_id,turn_id,task_id,run_id,generation,version,phase,send_claimed,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,CASE WHEN finish_json IS NULL OR length(CAST(finish_json AS BLOB))<=16384 THEN finish_json END,CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,updated_sequence,send_fence,admitted_sequence,resource_key FROM chat_turns WHERE request_id=?",
        [id.to_string()],|row|Ok(TurnRow {chat_id:row.get(0)?,turn_id:row.get(1)?,task_id:row.get(2)?,run_id:row.get(3)?,generation:row.get(4)?,version:row.get(5)?,phase:row.get(6)?,claimed:row.get(7)?,binding:row.get(8)?,finish:row.get(9)?,encoded:row.get(10)?,sequence:row.get(11)?,fence:row.get(12)?,admitted_sequence:row.get(13)?,resource_key:row.get(14)?})).optional()?;
    let Some(TurnRow {
        chat_id,
        turn_id,
        task_id,
        run_id,
        generation,
        version,
        phase,
        claimed,
        binding,
        finish,
        encoded,
        sequence,
        fence,
        admitted_sequence,
        resource_key,
    }) = row
    else {
        return Ok(None);
    };
    let fence = fence
        .map(|value| {
            Uuid::parse_str(&value)
                .ok()
                .filter(|id| !id.is_nil())
                .map(ChatOwnerFence)
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
        })
        .transpose()?;
    let turn: ChatTurn = decode(encoded)?;
    super::owners::require_read_only(connection, turn.chat_id)?;
    require(
        turn.admitted_sequence == admitted_sequence
            && admitted_sequence > 0
            && admitted_sequence <= sequence,
        ChatError::ProjectionMismatch,
    )?;
    let binding = binding.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    require(
        turn.schema_version == CHAT_SCHEMA_VERSION
            && turn.request_id == id
            && !id.is_nil()
            && turn.chat_id.to_string() == chat_id
            && turn.turn_id.to_string() == turn_id
            && turn.task_id.to_string() == task_id
            && turn.run_id.to_string() == run_id
            && turn.generation == generation
            && turn.version == version
            && turn.phase.as_str() == phase
            && generation > 0
            && version > 0
            && sequence > 0,
        ChatError::ProjectionMismatch,
    )?;
    turn.prompt_ref
        .validate(MAX_CHAT_PROMPT_BYTES)
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    if let Some(vendor) = &turn.vendor_turn_id {
        require(valid_vendor_id(vendor), ChatError::ProjectionMismatch)?;
    }
    let expected_binding =
        super::turns::admission_binding(turn.chat_id, turn.generation, &turn.prompt_ref)?;
    require(binding == expected_binding, ChatError::ProjectionMismatch)?;
    require(
        matches!(turn.phase, ChatTurnPhase::Admitted) != claimed
            || matches!(
                turn.phase,
                ChatTurnPhase::Cancelled | ChatTurnPhase::Interrupted
            ),
        ChatError::ProjectionMismatch,
    )?;
    require(
        match turn.phase {
            ChatTurnPhase::Admitted | ChatTurnPhase::RecoveryRequired => fence.is_none(),
            ChatTurnPhase::Running
            | ChatTurnPhase::CancelRequested
            | ChatTurnPhase::Completed
            | ChatTurnPhase::Failed => fence.is_some(),
            _ => true,
        },
        ChatError::ProjectionMismatch,
    )?;
    require(
        turn.phase != ChatTurnPhase::Completed
            || (turn.outcome == Some(ChatTurnOutcome::Completed)
                && turn.correlated_terminal
                && turn.error_code.is_none()),
        ChatError::ProjectionMismatch,
    )?;
    require(
        !turn.phase.holds_reservation() || !turn.output_complete,
        ChatError::ProjectionMismatch,
    )?;
    require(
        turn.updated_at >= turn.created_at,
        ChatError::ProjectionMismatch,
    )?;
    let task: Task = load_projection(connection, "task", &task_id)?;
    let run: Run = load_projection(connection, "run", &run_id)?;
    require(
        task.task_id() == turn.task_id
            && run.run_id() == turn.run_id
            && run.task_id() == turn.task_id
            && task.project_id() == turn.project_id
            && run.project_id() == turn.project_id,
        ChatError::ProjectionMismatch,
    )?;
    require(
        run.base_commit() == turn.base_commit,
        ChatError::ProjectionMismatch,
    )?;
    let allowed = match turn.phase {
        ChatTurnPhase::Admitted => {
            task.status() == TaskStatus::InProgress && run.status() == RunStatus::Preparing
        }
        ChatTurnPhase::Running | ChatTurnPhase::CancelRequested => {
            task.status() == TaskStatus::InProgress && run.status() == RunStatus::Running
        }
        ChatTurnPhase::Completed => {
            task.status() == TaskStatus::Done && run.status() == RunStatus::Succeeded
        }
        ChatTurnPhase::Failed | ChatTurnPhase::Interrupted => {
            task.status() == TaskStatus::Blocked && run.status() == RunStatus::Failed
        }
        ChatTurnPhase::Cancelled => {
            task.status() == TaskStatus::Cancelled
                && matches!(run.status(), RunStatus::Stopped | RunStatus::Failed)
        }
        ChatTurnPhase::RecoveryRequired => {
            task.status() == TaskStatus::Blocked && run.status() == RunStatus::Stale
        }
    };
    require(allowed, ChatError::ProjectionMismatch)?;
    rows_content_link(connection, &turn, &resource_key)?;
    if turn.phase.is_terminal() && fence.is_some() && claimed {
        let proof: FinishProof = serde_json::from_str(
            finish
                .as_deref()
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))?,
        )
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
        require(
            proof.0 == turn.generation
                && proof.2 == turn.correlated_terminal
                && proof.4 == turn.vendor_turn_id
                && proof.7 == turn.output_complete,
            ChatError::ProjectionMismatch,
        )?;
        if turn.phase == ChatTurnPhase::Completed {
            let session = existing_session(connection, turn.chat_id)?.session;
            require(
                proof.1 == ChatTurnOutcome::Completed
                    && proof.2
                    && proof.3.is_some()
                    && proof.3 == session.vendor_session_id
                    && proof.5.is_none(),
                ChatError::ProjectionMismatch,
            )?;
        }
        if turn.phase == ChatTurnPhase::Cancelled {
            require(proof.2 || !proof.6, ChatError::ProjectionMismatch)?;
        }
    }
    Ok(Some(StoredTurn {
        turn,
        binding,
        finish,
        claimed,
        fence,
        sequence,
        task,
        run,
    }))
}
pub(super) fn existing_turn(connection: &Connection, id: Uuid) -> Result<StoredTurn, StoreError> {
    turn(connection, id)?.ok_or_else(|| fail(ChatError::NotFound))
}

pub(super) fn insert_session(
    transaction: &Transaction<'_>,
    session: &ChatSession,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO chat_sessions(chat_id,project_id,state,generation,version,resource_key,owner_fence,record_json,updated_sequence) VALUES(?,?,?,?,?,?,NULL,?,?)",params![session.chat_id.to_string(),session.project_id.to_string(),session.state.as_str(),session.generation,session.version,session.resource_key,serde_json::to_string(session)?,sequence])?;
    Ok(())
}
pub(super) fn save_session(
    transaction: &Transaction<'_>,
    session: &ChatSession,
    fence: Option<ChatOwnerFence>,
    previous_version: u64,
    sequence: i64,
) -> Result<(), StoreError> {
    let changed=transaction.execute("UPDATE chat_sessions SET state=?,generation=?,version=?,owner_fence=?,record_json=?,updated_sequence=? WHERE chat_id=? AND version=?",
  params![session.state.as_str(),session.generation,session.version,fence.map(|value|value.0.to_string()),serde_json::to_string(session)?,sequence,session.chat_id.to_string(),previous_version])?;
    require(changed == 1, ChatError::Conflict)
}
pub(super) fn insert_turn(
    transaction: &Transaction<'_>,
    turn: &ChatTurn,
    resource_key: &str,
    binding: &str,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO chat_turns(request_id,chat_id,turn_id,task_id,run_id,generation,version,phase,resource_key,send_claimed,binding_json,finish_json,record_json,admitted_sequence,updated_sequence) VALUES(?,?,?,?,?,?,?,?,?,0,?,NULL,?,?,?)",
  params![turn.request_id.to_string(),turn.chat_id.to_string(),turn.turn_id.to_string(),turn.task_id.to_string(),turn.run_id.to_string(),turn.generation,turn.version,turn.phase.as_str(),resource_key,binding,serde_json::to_string(turn)?,sequence,sequence])?;
    Ok(())
}
pub(super) fn save_turn(
    transaction: &Transaction<'_>,
    turn: &ChatTurn,
    claimed: bool,
    fence: Option<ChatOwnerFence>,
    finish: Option<&str>,
    previous_version: u64,
    sequence: i64,
) -> Result<(), StoreError> {
    let changed=transaction.execute("UPDATE chat_turns SET version=?,phase=?,send_claimed=?,send_fence=?,finish_json=?,record_json=?,updated_sequence=? WHERE request_id=? AND version=?",
  params![turn.version,turn.phase.as_str(),claimed,fence.map(|fence|fence.0.to_string()),finish,serde_json::to_string(turn)?,sequence,turn.request_id.to_string(),previous_version])?;
    require(changed == 1, ChatError::Conflict)
}
fn rows_content_link(
    connection: &Connection,
    turn: &ChatTurn,
    resource_key: &str,
) -> Result<(), StoreError> {
    validate_content(connection, turn.chat_id, &turn.prompt_ref, false)?;
    let session = existing_session(connection, turn.chat_id)?.session;
    require(
        session.project_id == turn.project_id
            && session.generation >= turn.generation
            && session.resource_key == resource_key,
        ChatError::ProjectionMismatch,
    )?;
    if turn.phase.holds_reservation() {
        require(
            session.active_request_id == Some(turn.request_id)
                && session.generation == turn.generation,
            ChatError::ProjectionMismatch,
        )?;
    }
    Ok(())
}
#[derive(serde::Deserialize)]
struct FinishProof(
    u64,
    ChatTurnOutcome,
    bool,
    Option<String>,
    Option<String>,
    Option<ChatFailureCode>,
    bool,
    bool,
);
pub(super) fn save_task(
    transaction: &Transaction<'_>,
    task: &Task,
    sequence: i64,
) -> Result<(), StoreError> {
    save_projection(transaction, Projection::Task(task), sequence)
}
pub(super) fn save_run(
    transaction: &Transaction<'_>,
    run: &Run,
    sequence: i64,
) -> Result<(), StoreError> {
    save_projection(transaction, Projection::Run(run), sequence)
}
fn save_projection(
    transaction: &Transaction<'_>,
    projection: Projection<'_>,
    sequence: i64,
) -> Result<(), StoreError> {
    let (kind, id, project, json) = projection.parts()?;
    transaction.execute("INSERT INTO projections(entity_kind,entity_id,project_id,state_json,updated_sequence) VALUES(?,?,?,?,?) ON CONFLICT(entity_kind,entity_id) DO UPDATE SET state_json=excluded.state_json,updated_sequence=excluded.updated_sequence",params![kind,id,project,json,sequence])?;
    Ok(())
}
pub(super) fn load_projection<T: DeserializeOwned>(
    connection: &Connection,
    kind: &str,
    id: &str,
) -> Result<T, StoreError> {
    let encoded:Option<String>=connection.query_row("SELECT CASE WHEN length(CAST(state_json AS BLOB))<=16384 THEN state_json END FROM projections WHERE entity_kind=? AND entity_id=?",params![kind,id],|row|row.get(0)).optional()?.flatten();
    decode(encoded)
}
pub(super) fn validate_content(
    connection: &Connection,
    chat_id: ChatId,
    reference: &ContentRef,
    present: bool,
) -> Result<(), StoreError> {
    let matches:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM chat_content WHERE blob_id=? AND chat_id=? AND sha256=? AND byte_count=? AND (?=0 OR state='present'))",params![reference.blob_id.to_string(),chat_id.to_string(),reference.sha256,reference.byte_count,present],|row|row.get(0))?;
    require(matches, ChatError::ProjectionMismatch)
}
pub(super) fn record(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<(ChatRecord, String, u64)>, StoreError> {
    let row=connection.query_row(
        "SELECT chat_id,turn_id,sequence,kind,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,updated_sequence FROM chat_records WHERE record_id=?",
        [id.to_string()],|row|Ok(RecordRow {chat_id:row.get(0)?,turn_id:row.get(1)?,sequence:row.get(2)?,kind:row.get(3)?,binding:row.get(4)?,encoded:row.get(5)?,event_sequence:row.get(6)?})).optional()?;
    let Some(RecordRow {
        chat_id,
        turn_id,
        sequence,
        kind,
        binding,
        encoded,
        event_sequence,
    }) = row
    else {
        return Ok(None);
    };
    let record: ChatRecord = decode(encoded)?;
    require(
        record.schema_version == CHAT_SCHEMA_VERSION
            && record.record_id == id
            && !id.is_nil()
            && record.chat_id.to_string() == chat_id
            && record.turn_id.to_string() == turn_id
            && record.sequence == sequence
            && sequence > 0
            && event_sequence > 0
            && serde_json::to_value(record.kind)? == kind
            && record.original_byte_count > 0
            && record.original_byte_count <= MAX_CHAT_BYTES,
        ChatError::ProjectionMismatch,
    )?;
    let binding = binding.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    let original: (ChatId, ChatTurnId, ChatRecordKind, ContentRef, u64) =
        serde_json::from_str(&binding).map_err(|_| fail(ChatError::ProjectionMismatch))?;
    require(
        original.0 == record.chat_id
            && original.1 == record.turn_id
            && original.2 == record.kind
            && original.4 == record.original_byte_count
            && original.4 >= original.3.byte_count,
        ChatError::ProjectionMismatch,
    )?;
    original
        .3
        .validate(MAX_CHAT_RECORD_BYTES)
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let linked: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_turns WHERE turn_id=? AND chat_id=?)",
        params![record.turn_id.to_string(), record.chat_id.to_string()],
        |row| row.get(0),
    )?;
    require(linked, ChatError::ProjectionMismatch)?;
    if let Some(reference) = &record.content_ref {
        reference
            .validate(MAX_CHAT_RECORD_BYTES)
            .map_err(|_| fail(ChatError::ProjectionMismatch))?;
        require(
            reference.byte_count <= record.original_byte_count,
            ChatError::ProjectionMismatch,
        )?;
        validate_content(connection, record.chat_id, reference, true)?;
    }
    if let Some(reference) = &record.content_ref {
        require(reference == &original.3, ChatError::ProjectionMismatch)?;
    } else {
        validate_content(connection, record.chat_id, &original.3, false)?;
        let released: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_content WHERE blob_id=? AND state='released')",
            [original.3.blob_id.to_string()],
            |row| row.get(0),
        )?;
        require(released, ChatError::ProjectionMismatch)?;
    }
    Ok(Some((record, binding, event_sequence)))
}
pub(super) fn records(
    connection: &Connection,
    id: ChatId,
    after: u64,
    limit: u32,
) -> Result<Vec<ChatRecord>, StoreError> {
    let mut statement=connection.prepare("SELECT record_id FROM chat_records WHERE chat_id=? AND sequence>? ORDER BY sequence LIMIT ?")?;
    let ids = statement
        .query_map(params![id.to_string(), after, limit], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| {
            record(
                connection,
                Uuid::parse_str(&id).map_err(|_| fail(ChatError::ProjectionMismatch))?,
            )?
            .map(|value| value.0)
            .ok_or_else(|| fail(ChatError::ProjectionMismatch))
        })
        .collect()
}
