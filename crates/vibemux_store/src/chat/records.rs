//! Immutable private-content references and explicit content tombstones.
use super::*;
use rusqlite::TransactionBehavior;

fn binding(
    chat_id: ChatId,
    turn_id: ChatTurnId,
    kind: ChatRecordKind,
    reference: &ContentRef,
    original_byte_count: u64,
) -> Result<String, StoreError> {
    Ok(serde_json::to_string(&json!([
        chat_id,
        turn_id,
        kind,
        reference,
        original_byte_count
    ]))?)
}
pub(super) fn kind_name(kind: ChatRecordKind) -> &'static str {
    match kind {
        ChatRecordKind::User => "user",
        ChatRecordKind::Assistant => "assistant",
        ChatRecordKind::Tool => "tool",
        ChatRecordKind::Approval => "approval",
        ChatRecordKind::Notice => "notice",
    }
}
pub(super) struct RecordInput<'a> {
    pub id: Uuid,
    pub kind: ChatRecordKind,
    pub reference: &'a ContentRef,
    pub timestamp: OffsetDateTime,
    pub original_byte_count: u64,
}
pub(super) fn insert_record(
    transaction: &Transaction<'_>,
    session: &mut ChatSession,
    turn: &ChatTurn,
    input: RecordInput<'_>,
    event_sequence: i64,
) -> Result<ChatRecord, StoreError> {
    let RecordInput {
        id,
        kind,
        reference,
        timestamp,
        original_byte_count,
    } = input;
    require(
        !id.is_nil() && session.record_count < MAX_CHAT_RECORDS,
        ChatError::Capacity,
    )?;
    session.record_count = next_version(session.record_count)?;
    session.last_record_sequence = next_version(session.last_record_sequence)?;
    let record = ChatRecord {
        schema_version: CHAT_SCHEMA_VERSION,
        record_id: id,
        chat_id: session.chat_id,
        turn_id: turn.turn_id,
        sequence: session.last_record_sequence,
        kind,
        content_ref: Some(reference.clone()),
        original_byte_count,
        created_at: timestamp,
    };
    transaction.execute("INSERT INTO chat_records(record_id,chat_id,turn_id,sequence,kind,binding_json,record_json,updated_sequence) VALUES(?,?,?,?,?,?,?,?)",
  params![id.to_string(),session.chat_id.to_string(),turn.turn_id.to_string(),record.sequence,kind_name(kind),binding(record.chat_id,record.turn_id,kind,reference,original_byte_count)?,serde_json::to_string(&record)?,event_sequence])?;
    Ok(record)
}

impl SqliteStore {
    /// Bounded cleanup references retained by canonical release transactions.
    /// This does not enumerate or adopt unknown filesystem entries.
    pub fn released_chat_content(
        &self,
        chat_id: ChatId,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<ContentRef>, StoreError> {
        require(limit > 0 && limit <= MAX_CHAT_PAGE, ChatError::InvalidInput)?;
        rows::existing_session(&self.connection, chat_id)?;
        let mut statement = self.connection.prepare(
            "SELECT blob_id,sha256,byte_count FROM chat_content WHERE chat_id=?1 AND state='released' AND (?2 IS NULL OR blob_id>?2) ORDER BY blob_id LIMIT ?3",
        )?;
        statement
            .query_map(
                params![chat_id.to_string(), after.map(|id| id.to_string()), limit],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )?
            .map(|row| {
                let (id, sha256, byte_count) = row?;
                let reference = ContentRef {
                    blob_id: Uuid::parse_str(&id)
                        .map_err(|_| fail(ChatError::ProjectionMismatch))?,
                    sha256,
                    byte_count,
                };
                reference.validate(MAX_CHAT_RECORD_BYTES).map_err(fail)?;
                Ok(reference)
            })
            .collect()
    }

    pub fn append_chat_record(
        &mut self,
        input: &ChatRecordAppend,
    ) -> Result<ChatRecordCommit, StoreError> {
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
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let turn = rows::existing_turn(&transaction, input.request_id)?;
        require(
            turn.turn.generation == input.generation,
            ChatError::StaleGeneration,
        )?;
        require(turn.fence == Some(input.fence), ChatError::StaleFence)?;
        let expected_binding = binding(
            turn.turn.chat_id,
            turn.turn.turn_id,
            input.kind,
            &input.content_ref,
            input.original_byte_count,
        )?;
        if let Some((record, stored_binding, sequence)) =
            rows::record(&transaction, input.record_id)?
        {
            require(stored_binding == expected_binding, ChatError::Conflict)?;
            return Ok(ChatRecordCommit {
                record,
                event: None,
                sequence,
                duplicate: true,
            });
        }
        require(
            matches!(
                turn.turn.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ),
            ChatError::InvalidTransition,
        )?;
        let stored = rows::existing_session(&transaction, turn.turn.chat_id)?;
        check_owner(&stored, input.generation, input.fence)?;
        require(
            !stored.session.content_purged,
            ChatError::ContentUnavailable,
        )?;
        require(
            stored.session.record_count < MAX_CHAT_RECORDS,
            ChatError::Capacity,
        )?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        claim_content(
            &transaction,
            &mut session,
            &input.content_ref,
            MAX_CHAT_RECORD_BYTES,
        )?;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            Some(&turn.turn),
            "chat_record_appended",
            session.updated_at,
            json!({"chat_id":session.chat_id,"turn_id":turn.turn.turn_id,"record_id":input.record_id,"kind":input.kind,"byte_count":input.content_ref.byte_count}),
        )?;
        let timestamp = session.updated_at;
        let record = insert_record(
            &transaction,
            &mut session,
            &turn.turn,
            RecordInput {
                id: input.record_id,
                kind: input.kind,
                reference: &input.content_ref,
                timestamp,
                original_byte_count: input.original_byte_count,
            },
            sequence,
        )?;
        rows::save_session(
            &transaction,
            &session,
            stored.fence,
            previous_version,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatRecordCommit {
            record,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }

    pub fn purge_chat_content(
        &mut self,
        input: &ChatMutation,
    ) -> Result<ChatSessionCommit, StoreError> {
        let binding = sessions::mutation_binding(input, Value::Null)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(&transaction, input.operation_id, "purge", &binding)? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            stored.session.state == ChatState::Archived
                && stored.fence.is_none()
                && stored.session.active_request_id.is_none(),
            ChatError::InvalidTransition,
        )?;
        let mut records = rows::records(&transaction, input.chat_id, 0, MAX_CHAT_RECORDS as u32)?;
        let count: u64 = transaction.query_row(
            "SELECT COUNT(*) FROM chat_content WHERE chat_id=? AND state='present'",
            [input.chat_id.to_string()],
            |row| row.get(0),
        )?;
        require(count <= MAX_CHAT_RECORDS + 1, ChatError::ProjectionMismatch)?;
        let mut statement=transaction.prepare("SELECT blob_id,sha256,byte_count FROM chat_content WHERE chat_id=? AND state='present'")?;
        let released = statement
            .query_map([input.chat_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?
            .map(|row| {
                let (id, sha256, byte_count) = row?;
                Ok(ContentRef {
                    blob_id: Uuid::parse_str(&id)
                        .map_err(|_| fail(ChatError::ProjectionMismatch))?,
                    sha256,
                    byte_count,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        drop(statement);
        let previous_version = stored.session.version;
        let mut session = stored.session;
        for reference in &released {
            release_content(&transaction, &mut session, reference)?;
        }
        session.title_ref = None;
        session.content_purged = true;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_content_purged",
            session.updated_at,
            event_data(&session),
        )?;
        for record in &mut records {
            record.content_ref = None;
            transaction.execute(
                "UPDATE chat_records SET record_json=?,updated_sequence=? WHERE record_id=?",
                params![
                    serde_json::to_string(record)?,
                    sequence,
                    record.record_id.to_string()
                ],
            )?;
        }
        rows::save_session(&transaction, &session, None, previous_version, sequence)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "purge",
            &binding,
            &released,
            sequence,
        )?;
        transaction.commit()?;
        Ok(sessions::committed(session, event, sequence, released))
    }
}
