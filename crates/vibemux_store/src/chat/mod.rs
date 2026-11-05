//! Canonical persistent-chat metadata transactions. Only the daemon writer calls
//! this boundary; private bodies are immutable blobs owned by that same writer.
mod approval_lifecycle;
mod approval_receipts;
mod approval_rows;
mod approvals;
mod attempt_records;
mod attempt_rows;
mod attempts;
mod cleanup;
mod cleanup_authority;
mod cleanup_execution;
mod cleanup_rows;
mod execution_sessions;
mod owners;
mod records;
mod recovery;
mod rows;
mod sessions;
mod turns;

pub use approval_receipts::ChatApprovalDecisionLookup;
pub(crate) use approvals::MIGRATION_V13;
pub use approvals::{
    ChatApprovalAdmission, ChatApprovalCommit, ChatApprovalDecisionInput, ChatApprovalDeliveryClaim,
};
pub use attempt_records::ChatExecutionRecordCommit;
pub(crate) use attempts::MIGRATION_V12;
pub use attempts::{ChatExecutionAttemptAdmission, ChatExecutionAttemptCommit};
pub(crate) use cleanup::MIGRATION_V14;
pub use cleanup::{
    WorkspaceCleanupCommit, WorkspaceCleanupConfirmation, WorkspaceCleanupMutation,
    WorkspaceCleanupPlan,
};
pub use cleanup_authority::{
    CleanupArmClaim, CleanupCompletionClaim, CleanupExecutionClaim, CleanupNeverSpawnedClaim,
    CleanupRegistrationClaim, CleanupReleasePermit,
};
pub use cleanup_execution::{
    CleanupEvidenceFailure, WorkspaceCleanupArmCommit, WorkspaceCleanupExecutorCommit,
    WorkspaceCleanupSettlement,
};
pub(crate) use cleanup_rows::{
    is_reserved as cleanup_is_reserved, path_is_reserved as cleanup_path_is_reserved,
};
pub use execution_sessions::{
    ChatExecutionProcessChange, ChatExecutionRecovery, ChatExecutionSessionCommit,
    ChatExecutionStart,
};
pub(crate) use owners::MIGRATION_V11;
pub use owners::{
    ChatExecutionOwnerCommit, ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation,
};

use crate::{SqliteStore, StoreError};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventEnvelope, EventPayload, EventType};
use vibemux_types::{EventId, chat::*};

pub(crate) const MIGRATION_V10: &str = r#"
CREATE TABLE chat_sessions(
 chat_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN
 ('opening','ready','busy','suspended','recovery_required','closed','archived')),
 generation INTEGER NOT NULL CHECK(generation>0), version INTEGER NOT NULL CHECK(version>0),
 resource_key TEXT NOT NULL, owner_fence TEXT, record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
 CHECK(CASE WHEN state IN ('ready','busy') THEN owner_fence IS NOT NULL
 WHEN state IN ('suspended','closed','archived') THEN owner_fence IS NULL ELSE 1 END)
);
CREATE TABLE chat_turns(
 request_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 turn_id TEXT NOT NULL UNIQUE, task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
 generation INTEGER NOT NULL CHECK(generation>0), version INTEGER NOT NULL CHECK(version>0),
 phase TEXT NOT NULL CHECK(phase IN ('admitted','running','cancel_requested','completed','failed','cancelled','interrupted','recovery_required')),
 resource_key TEXT NOT NULL, send_claimed INTEGER NOT NULL CHECK(send_claimed IN (0,1)),send_fence TEXT,
 binding_json TEXT NOT NULL, finish_json TEXT, record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_active_turn ON chat_turns(chat_id)
 WHERE phase IN ('admitted','running','cancel_requested','recovery_required');
CREATE UNIQUE INDEX chat_active_resource ON chat_turns(resource_key)
 WHERE phase IN ('admitted','running','cancel_requested','recovery_required');
CREATE TABLE chat_content(
 blob_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),sha256 TEXT NOT NULL,
 byte_count INTEGER NOT NULL CHECK(byte_count>0),state TEXT NOT NULL CHECK(state IN ('present','released'))
);
CREATE TABLE chat_records(
 record_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 turn_id TEXT NOT NULL REFERENCES chat_turns(turn_id),sequence INTEGER NOT NULL CHECK(sequence>0),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),UNIQUE(chat_id,sequence)
);
CREATE TABLE chat_operations(
 operation_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,released_json TEXT NOT NULL,
 sequence INTEGER NOT NULL REFERENCES events(sequence)
);
INSERT INTO schema_migrations(version,applied_at) VALUES(10,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

#[derive(Clone, Debug)]
pub struct ChatOpen {
    pub operation_id: Uuid,
    pub harness: ChatHarness,
    pub profile_sha256: String,
    pub resource_key: String,
    pub base_commit: String,
    pub title_ref: Option<ContentRef>,
    pub timestamp: OffsetDateTime,
}
#[derive(Clone, Debug)]
pub struct ChatMutation {
    pub operation_id: Uuid,
    pub chat_id: ChatId,
    pub expected_generation: u64,
    pub timestamp: OffsetDateTime,
}
#[derive(Clone, Debug)]
pub struct ChatTurnAdmission {
    pub chat_id: ChatId,
    pub request_id: Uuid,
    pub expected_generation: u64,
    pub base_commit: String,
    pub prompt_ref: ContentRef,
    pub timestamp: OffsetDateTime,
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChatOwnerFence(Uuid);
impl fmt::Debug for ChatOwnerFence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChatOwnerFence(<redacted>)")
    }
}
#[derive(Clone)]
pub struct ChatTurnFinish {
    pub request_id: Uuid,
    pub generation: u64,
    pub fence: ChatOwnerFence,
    pub outcome: ChatTurnOutcome,
    pub correlated_terminal: bool,
    pub vendor_session_id: Option<String>,
    pub vendor_turn_id: Option<String>,
    pub error_code: Option<ChatFailureCode>,
    pub process_alive: bool,
    pub timestamp: OffsetDateTime,
    pub output_complete: bool,
}
impl fmt::Debug for ChatTurnFinish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatTurnFinish")
            .field("request_id", &self.request_id)
            .field("generation", &self.generation)
            .field("outcome", &self.outcome)
            .field("correlated_terminal", &self.correlated_terminal)
            .field("error_code", &self.error_code)
            .field("process_alive", &self.process_alive)
            .field("output_complete", &self.output_complete)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub struct ChatRecordAppend {
    pub request_id: Uuid,
    pub generation: u64,
    pub fence: ChatOwnerFence,
    pub record_id: Uuid,
    pub kind: ChatRecordKind,
    pub content_ref: ContentRef,
    pub original_byte_count: u64,
    pub timestamp: OffsetDateTime,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ChatSessionCommit {
    pub session: ChatSession,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
    /// Blobs no longer referenced by private chat content after this commit.
    pub released_content: Vec<ContentRef>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ChatTurnCommit {
    pub session: ChatSession,
    pub turn: ChatTurn,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ChatRecordCommit {
    pub record: ChatRecord,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
}
#[derive(Clone, Debug)]
pub struct ChatOwnerClaim {
    pub session: ChatSession,
    pub fence: ChatOwnerFence,
    pub event: EventEnvelope,
}
#[derive(Clone, Debug, Default)]
pub struct ChatRecovery {
    pub sessions: Vec<ChatSessionCommit>,
    pub turns: Vec<ChatTurnCommit>,
    pub quarantined: Vec<QuarantinedChat>,
}
#[derive(Clone, Debug)]
pub struct QuarantinedChat {
    pub chat_id: Option<ChatId>,
    pub code: &'static str,
}

fn fail(error: ChatError) -> StoreError {
    StoreError::Chat(error)
}
fn require(condition: bool, error: ChatError) -> Result<(), StoreError> {
    if condition { Ok(()) } else { Err(fail(error)) }
}
fn next_version(version: u64) -> Result<u64, StoreError> {
    version
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| fail(ChatError::Capacity))
}
fn valid_generation(generation: u64) -> Result<(), StoreError> {
    require(
        generation > 0 && generation <= i64::MAX as u64,
        ChatError::InvalidInput,
    )
}

fn emit(
    transaction: &Transaction<'_>,
    session: &ChatSession,
    turn: Option<&ChatTurn>,
    kind: &str,
    timestamp: OffsetDateTime,
    payload: Value,
) -> Result<(EventEnvelope, i64), StoreError> {
    let event_id = EventId::new();
    SqliteStore::insert_event(
        transaction,
        EventDraft {
            event_id,
            event_type: EventType::new(kind)?,
            project_id: session.project_id,
            task_id: turn.map(|turn| turn.task_id),
            run_id: turn.map(|turn| turn.run_id),
            causation_id: None,
            actor: ActorName::new("daemon_chat")?,
            timestamp: timestamp.max(session.updated_at),
            idempotency_key: Some(format!("chat_event:{event_id}")),
            payload: EventPayload::new(payload)?,
        },
    )
}
fn event_data(session: &ChatSession) -> Value {
    json!({"chat_id":session.chat_id,"generation":session.generation,"state":session.state,"version":session.version})
}
fn turn_event_data(turn: &ChatTurn) -> Value {
    json!({"chat_id":turn.chat_id,"request_id":turn.request_id,"turn_id":turn.turn_id,"generation":turn.generation,"phase":turn.phase,"version":turn.version,"error_code":turn.error_code})
}

fn check_owner(
    stored: &rows::StoredSession,
    generation: u64,
    fence: ChatOwnerFence,
) -> Result<(), StoreError> {
    require(
        stored.session.generation == generation,
        ChatError::StaleGeneration,
    )?;
    require(stored.fence == Some(fence), ChatError::StaleFence)
}
fn check_generation(session: &ChatSession, generation: u64) -> Result<(), StoreError> {
    valid_generation(generation)?;
    require(session.generation == generation, ChatError::StaleGeneration)
}
fn check_detection(transaction: &Transaction<'_>, harness: ChatHarness) -> Result<(), StoreError> {
    require(
        SqliteStore::read_registry_in_tx(transaction)?
            .state(harness.as_str())
            .is_some_and(|state| state.detected),
        ChatError::NotDetected,
    )
}

pub(crate) fn is_reserved(connection: &Connection, resource_key: &str) -> Result<bool, StoreError> {
    // Recovery clears the writable fence, not proof of the former process's
    // death. An idle unknown owner therefore retains its resource too.
    let chat_reserved: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chat_turns WHERE resource_key=?1 AND phase IN ('admitted','running','cancel_requested','recovery_required') UNION ALL SELECT 1 FROM chat_sessions WHERE resource_key=?1 AND state='recovery_required' UNION ALL SELECT 1 FROM chat_execution_owners WHERE resource_key=?1 AND phase!='finalized')",[resource_key],|row|row.get(0))?;
    Ok(chat_reserved || cleanup_is_reserved(connection, resource_key)?)
}
fn dispatch_reserved(connection: &Connection, resource_key: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM harness_dispatches WHERE resource_key=? AND phase IN ('admitted','running','cancel_requested','recovery_pending'))",[resource_key],|row|row.get(0))?)
}

#[derive(Deserialize, Serialize)]
struct OperationRow {
    chat_id: ChatId,
    released: Vec<ContentRef>,
    sequence: u64,
}
fn operation(
    transaction: &Transaction<'_>,
    id: Uuid,
    kind: &str,
    binding: &str,
) -> Result<Option<OperationRow>, StoreError> {
    require(!id.is_nil(), ChatError::InvalidInput)?;
    let row:Option<(String,String,String,String,u64)>=transaction.query_row(
  "SELECT chat_id,kind,binding_json,released_json,sequence FROM chat_operations WHERE operation_id=?",[id.to_string()],
  |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    if let Some((chat_id, stored_kind, stored_binding, released, sequence)) = row {
        require(
            stored_kind == kind && stored_binding == binding,
            ChatError::Conflict,
        )?;
        require(released.len() <= 1024 * 1024, ChatError::ProjectionMismatch)?;
        Ok(Some(OperationRow {
            chat_id: chat_id
                .parse()
                .map_err(|_| fail(ChatError::ProjectionMismatch))?,
            released: serde_json::from_str(&released)?,
            sequence,
        }))
    } else {
        let used: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_turns WHERE request_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1 UNION ALL SELECT 1 FROM chat_execution_attempts WHERE request_id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )?;
        require(!used, ChatError::Conflict)?;
        Ok(None)
    }
}
fn record_operation(
    transaction: &Transaction<'_>,
    id: Uuid,
    session: &ChatSession,
    kind: &str,
    binding: &str,
    released: &[ContentRef],
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO chat_operations(operation_id,chat_id,kind,binding_json,released_json,sequence) VALUES(?,?,?,?,?,?)",
  params![id.to_string(),session.chat_id.to_string(),kind,binding,serde_json::to_string(released)?,sequence])?;
    Ok(())
}
fn operation_commit(
    transaction: &Transaction<'_>,
    row: OperationRow,
) -> Result<ChatSessionCommit, StoreError> {
    owners::require_read_only(transaction, row.chat_id)?;
    let session = rows::session(transaction, row.chat_id)?
        .ok_or_else(|| fail(ChatError::ProjectionMismatch))?
        .session;
    Ok(ChatSessionCommit {
        session,
        event: None,
        sequence: row.sequence,
        duplicate: true,
        released_content: row.released,
    })
}

fn claim_content(
    transaction: &Transaction<'_>,
    session: &mut ChatSession,
    reference: &ContentRef,
    maximum: u64,
) -> Result<(), StoreError> {
    reference.validate(maximum).map_err(fail)?;
    let new_bytes = session
        .retained_bytes
        .checked_add(reference.byte_count)
        .ok_or_else(|| fail(ChatError::Capacity))?;
    require(new_bytes <= MAX_CHAT_BYTES, ChatError::Capacity)?;
    let project_bytes: u64 = transaction.query_row(
        "SELECT COALESCE(SUM(byte_count),0) FROM chat_content WHERE state='present'",
        [],
        |row| row.get(0),
    )?;
    require(
        project_bytes
            .checked_add(reference.byte_count)
            .is_some_and(|size| size <= MAX_PROJECT_CHAT_BYTES),
        ChatError::Capacity,
    )?;
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_content WHERE blob_id=?)",
        [reference.blob_id.to_string()],
        |row| row.get(0),
    )?;
    require(!exists, ChatError::Conflict)?;
    transaction.execute("INSERT INTO chat_content(blob_id,chat_id,sha256,byte_count,state) VALUES(?,?,?,?,'present')",params![reference.blob_id.to_string(),session.chat_id.to_string(),reference.sha256,reference.byte_count])?;
    session.retained_bytes = new_bytes;
    Ok(())
}
fn release_content(
    transaction: &Transaction<'_>,
    session: &mut ChatSession,
    reference: &ContentRef,
) -> Result<(), StoreError> {
    let changed=transaction.execute("UPDATE chat_content SET state='released' WHERE blob_id=? AND chat_id=? AND sha256=? AND byte_count=? AND state='present'",
  params![reference.blob_id.to_string(),session.chat_id.to_string(),reference.sha256,reference.byte_count])?;
    require(changed == 1, ChatError::ProjectionMismatch)?;
    session.retained_bytes = session
        .retained_bytes
        .checked_sub(reference.byte_count)
        .ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    Ok(())
}

impl SqliteStore {
    pub fn chat_operation_known(&self, operation_id: Uuid) -> Result<bool, StoreError> {
        require(!operation_id.is_nil(), ChatError::InvalidInput)?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_operations WHERE operation_id=?)",
            [operation_id.to_string()],
            |row| row.get(0),
        )?)
    }
    /// Resolve the original session binding without writing a private blob or
    /// consulting a newly changed route/HEAD. Callers still replay the original
    /// metadata operation so changed caller-controlled arguments conflict.
    pub fn chat_session_for_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<ChatSession>, StoreError> {
        require(!operation_id.is_nil(), ChatError::InvalidInput)?;
        let stored_id: Option<Option<String>> = self.connection.query_row(
            "SELECT CASE WHEN length(CAST(chat_id AS BLOB))<=36 THEN chat_id END FROM chat_operations WHERE operation_id=?",
            [operation_id.to_string()], |row| row.get(0),
        ).optional()?;
        let Some(stored_id) = stored_id else {
            return Ok(None);
        };
        let chat_id = stored_id
            .ok_or_else(|| fail(ChatError::ProjectionMismatch))?
            .parse()
            .map_err(|_| fail(ChatError::ProjectionMismatch))?;
        owners::require_read_only(&self.connection, chat_id)?;
        let stored = rows::session(&self.connection, chat_id)?
            .ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
        Ok(Some(stored.session))
    }
    pub fn chat_record_by_id(
        &self,
        chat_id: ChatId,
        record_id: Uuid,
    ) -> Result<Option<ChatRecord>, StoreError> {
        require(!record_id.is_nil(), ChatError::InvalidInput)?;
        Ok(rows::record(&self.connection, record_id)?
            .map(|entry| entry.0)
            .filter(|record| record.chat_id == chat_id))
    }
    pub fn chat_content_in_use(&self, blob_id: Uuid) -> Result<bool, StoreError> {
        require(!blob_id.is_nil(), ChatError::InvalidInput)?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_content WHERE blob_id=? AND state='present')",
            [blob_id.to_string()],
            |row| row.get(0),
        )?)
    }
    pub fn chat_session(&self, chat_id: ChatId) -> Result<Option<ChatSession>, StoreError> {
        owners::require_read_only(&self.connection, chat_id)?;
        Ok(rows::session(&self.connection, chat_id)?.map(|stored| stored.session))
    }
    pub fn chat_turn(&self, request_id: Uuid) -> Result<Option<ChatTurn>, StoreError> {
        Ok(rows::turn(&self.connection, request_id)?.map(|stored| stored.turn))
    }
    pub fn chat_turn_by_id(
        &self,
        chat_id: ChatId,
        turn_id: ChatTurnId,
    ) -> Result<Option<ChatTurn>, StoreError> {
        let key: Option<String> = self
            .connection
            .query_row(
                "SELECT request_id FROM chat_turns WHERE chat_id=? AND turn_id=?",
                params![chat_id.to_string(), turn_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        key.map(|key| {
            self.chat_turn(Uuid::parse_str(&key).map_err(|_| fail(ChatError::ProjectionMismatch))?)
        })
        .transpose()
        .map(Option::flatten)
    }
    pub fn list_chat_sessions(
        &self,
        after: Option<ChatId>,
        limit: u32,
        include_archived: bool,
    ) -> Result<Vec<ChatSession>, StoreError> {
        require(limit > 0 && limit <= MAX_CHAT_PAGE, ChatError::InvalidInput)?;
        let mut statement=self.connection.prepare("SELECT chat_id FROM chat_sessions WHERE chat_id>? AND (? OR state!='archived') AND NOT EXISTS(SELECT 1 FROM chat_execution_owners o WHERE o.chat_id=chat_sessions.chat_id) ORDER BY chat_id LIMIT ?")?;
        let keys = statement
            .query_map(
                params![
                    after.map(|id| id.to_string()).unwrap_or_default(),
                    include_archived,
                    limit
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter()
            .map(|key| {
                let id = key
                    .parse()
                    .map_err(|_| fail(ChatError::ProjectionMismatch))?;
                self.chat_session(id)?
                    .ok_or_else(|| fail(ChatError::ProjectionMismatch))
            })
            .collect()
    }
    pub fn chat_turns(
        &self,
        chat_id: ChatId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ChatTurn>, StoreError> {
        require(
            limit > 0 && limit <= MAX_CHAT_PAGE && after_sequence <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        rows::existing_session(&self.connection, chat_id)?;
        let mut statement=self.connection.prepare("SELECT request_id FROM chat_turns WHERE chat_id=? AND admitted_sequence>? ORDER BY admitted_sequence LIMIT ?")?;
        let keys = statement
            .query_map(params![chat_id.to_string(), after_sequence, limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter()
            .map(|key| {
                self.chat_turn(
                    Uuid::parse_str(&key).map_err(|_| fail(ChatError::ProjectionMismatch))?,
                )?
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
            })
            .collect()
    }
    pub fn chat_records(
        &self,
        chat_id: ChatId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ChatRecord>, StoreError> {
        require(
            limit > 0 && limit <= MAX_CHAT_PAGE && after_sequence <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        rows::existing_session(&self.connection, chat_id)?;
        rows::records(&self.connection, chat_id, after_sequence, limit)
    }
}
