//! Private-content metadata for owner attempts, with separate record linkage.
use super::*;
use vibemux_types::chat_attempt::{
    CHAT_ATTEMPT_SCHEMA_VERSION, ChatExecutionAttempt, ChatExecutionRecord,
};

#[derive(Clone, Debug)]
pub struct ChatExecutionRecordCommit {
    pub record: ChatExecutionRecord,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
}

struct RecordRow {
    chat_id: String,
    attempt_id: String,
    sequence: u64,
    kind: String,
    binding: Option<String>,
    encoded: Option<String>,
    event_sequence: u64,
}

fn binding(
    attempt: &ChatExecutionAttempt,
    kind: ChatRecordKind,
    reference: &ContentRef,
    original: u64,
) -> Result<String, StoreError> {
    Ok(serde_json::to_string(&json!([
        attempt.chat_id,
        attempt.attempt_id,
        kind,
        reference,
        original
    ]))?)
}

pub(super) fn insert_record(
    transaction: &Transaction<'_>,
    session: &mut ChatSession,
    attempt: &ChatExecutionAttempt,
    input: records::RecordInput<'_>,
    sequence: i64,
) -> Result<ChatExecutionRecord, StoreError> {
    require(
        !input.id.is_nil() && session.record_count < MAX_CHAT_RECORDS,
        ChatError::Capacity,
    )?;
    session.record_count = next_version(session.record_count)?;
    session.last_record_sequence = next_version(session.last_record_sequence)?;
    let record = ChatExecutionRecord {
        schema_version: CHAT_ATTEMPT_SCHEMA_VERSION,
        record_id: input.id,
        chat_id: session.chat_id,
        attempt_id: attempt.attempt_id,
        sequence: session.last_record_sequence,
        kind: input.kind,
        content_ref: Some(input.reference.clone()),
        original_byte_count: input.original_byte_count,
        created_at: input.timestamp,
    };
    record.validate().map_err(fail)?;
    transaction.execute("INSERT INTO chat_execution_records(record_id,chat_id,attempt_id,sequence,kind,binding_json,record_json,updated_sequence) VALUES(?,?,?,?,?,?,?,?)",
        params![record.record_id.to_string(),record.chat_id.to_string(),record.attempt_id.to_string(),record.sequence,records::kind_name(record.kind),
            binding(attempt,record.kind,input.reference,record.original_byte_count)?,serde_json::to_string(&record)?,sequence])?;
    Ok(record)
}

pub(super) fn record(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<(ChatExecutionRecord, String, u64)>, StoreError> {
    let row = connection
        .query_row(
            "SELECT chat_id,attempt_id,sequence,kind,
         CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,
         CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,updated_sequence
         FROM chat_execution_records WHERE record_id=?",
            [id.to_string()],
            |row| {
                Ok(RecordRow {
                    chat_id: row.get(0)?,
                    attempt_id: row.get(1)?,
                    sequence: row.get(2)?,
                    kind: row.get(3)?,
                    binding: row.get(4)?,
                    encoded: row.get(5)?,
                    event_sequence: row.get(6)?,
                })
            },
        )
        .optional()?;
    let Some(RecordRow {
        chat_id,
        attempt_id,
        sequence,
        kind,
        binding: binding_json,
        encoded,
        event_sequence,
    }) = row
    else {
        return Ok(None);
    };
    let value: ChatExecutionRecord = rows::decode(encoded)?;
    value
        .validate()
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let stored_binding = binding_json.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    let request_id: String = connection.query_row(
        "SELECT request_id FROM chat_execution_attempts WHERE attempt_id=?",
        [attempt_id.clone()],
        |row| row.get(0),
    )?;
    let request_id =
        Uuid::parse_str(&request_id).map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let attempt = attempt_rows::required_attempt(connection, request_id)?.attempt;
    require(
        value.record_id == id
            && value.chat_id.to_string() == chat_id
            && value.attempt_id.to_string() == attempt_id
            && attempt.chat_id == value.chat_id
            && value.sequence == sequence
            && records::kind_name(value.kind) == kind
            && event_sequence > 0,
        ChatError::ProjectionMismatch,
    )?;
    if let Some(reference) = &value.content_ref {
        require(
            stored_binding == binding(&attempt, value.kind, reference, value.original_byte_count)?,
            ChatError::ProjectionMismatch,
        )?;
        rows::validate_content(connection, value.chat_id, reference, true)?;
        if value.kind == ChatRecordKind::User {
            require(
                value.record_id == attempt.request_id && reference == &attempt.prompt_ref,
                ChatError::ProjectionMismatch,
            )?;
        }
    } else {
        require(
            rows::existing_session(connection, value.chat_id)?
                .session
                .content_purged,
            ChatError::ProjectionMismatch,
        )?;
    }
    if value.kind == ChatRecordKind::User {
        require(
            value.original_byte_count == attempt.prompt_ref.byte_count,
            ChatError::ProjectionMismatch,
        )?;
    }
    Ok(Some((value, stored_binding, event_sequence)))
}

impl SqliteStore {
    pub fn chat_execution_record(
        &self,
        id: Uuid,
    ) -> Result<Option<ChatExecutionRecord>, StoreError> {
        Ok(record(&self.connection, id)?.map(|(record, _, _)| record))
    }

    pub fn chat_execution_records(
        &self,
        chat_id: ChatId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ChatExecutionRecord>, StoreError> {
        require(
            limit > 0 && limit <= MAX_CHAT_PAGE && after_sequence <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        owners::required_owner(&self.connection, chat_id)?;
        let mut statement=self.connection.prepare("SELECT record_id FROM chat_execution_records WHERE chat_id=? AND sequence>? ORDER BY sequence LIMIT ?")?;
        let ids = statement
            .query_map(params![chat_id.to_string(), after_sequence, limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let id = Uuid::parse_str(&id).map_err(|_| fail(ChatError::ProjectionMismatch))?;
                record(&self.connection, id)?
                    .map(|(record, _, _)| record)
                    .ok_or_else(|| fail(ChatError::ProjectionMismatch))
            })
            .collect()
    }

    pub fn append_chat_execution_record(
        &mut self,
        input: &ChatRecordAppend,
    ) -> Result<ChatExecutionRecordCommit, StoreError> {
        require(
            !input.record_id.is_nil() && input.kind != ChatRecordKind::User,
            ChatError::InvalidInput,
        )?;
        input
            .content_ref
            .validate(MAX_CHAT_RECORD_BYTES)
            .map_err(fail)?;
        require(
            input.original_byte_count >= input.content_ref.byte_count
                && input.original_byte_count <= MAX_CHAT_BYTES,
            ChatError::InvalidInput,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let attempt = attempt_rows::required_attempt(&transaction, input.request_id)?;
        require(
            attempt.attempt.generation == input.generation && attempt.fence == Some(input.fence),
            ChatError::StaleFence,
        )?;
        let expected = binding(
            &attempt.attempt,
            input.kind,
            &input.content_ref,
            input.original_byte_count,
        )?;
        if let Some((record, known, sequence)) = record(&transaction, input.record_id)? {
            require(known == expected, ChatError::Conflict)?;
            return Ok(ChatExecutionRecordCommit {
                record,
                event: None,
                sequence,
                duplicate: true,
            });
        }
        require(
            matches!(
                attempt.attempt.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ),
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, attempt.attempt.chat_id)?;
        check_owner(&row, input.generation, input.fence)?;
        require(
            row.session.active_request_id == Some(input.request_id),
            ChatError::InvalidTransition,
        )?;
        let owner = owners::required_owner(&transaction, attempt.attempt.chat_id)?.owner;
        let old_version = row.session.version;
        let mut session = row.session;
        claim_content(
            &transaction,
            &mut session,
            &input.content_ref,
            MAX_CHAT_RECORD_BYTES,
        )?;
        session.version = next_version(session.version)?;
        session.updated_at = input
            .timestamp
            .max(session.updated_at)
            .max(attempt.attempt.updated_at);
        let (event, sequence) = attempts::execution_event(
            &transaction,
            &owner,
            &session,
            "chat_execution_record_appended",
            Some(input.request_id),
            json!({"record_id":input.record_id,"kind":input.kind,"bytes":input.content_ref.byte_count,"original_bytes":input.original_byte_count}),
        )?;
        let timestamp = session.updated_at;
        let record = insert_record(
            &transaction,
            &mut session,
            &attempt.attempt,
            records::RecordInput {
                id: input.record_id,
                kind: input.kind,
                reference: &input.content_ref,
                timestamp,
                original_byte_count: input.original_byte_count,
            },
            sequence,
        )?;
        rows::save_session(&transaction, &session, row.fence, old_version, sequence)?;
        transaction.commit()?;
        Ok(ChatExecutionRecordCommit {
            record,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }
}
