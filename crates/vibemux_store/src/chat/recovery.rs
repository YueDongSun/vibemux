//! Startup fencing and explicit acknowledgement of uncertain process/turn ownership.
use super::*;
use rusqlite::TransactionBehavior;

impl SqliteStore {
    /// The current process owner lost transport or cleanup proof. Fence only
    /// this generation; another chat's live owner remains untouched.
    pub fn lose_chat_owner(
        &mut self,
        chat_id: ChatId,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatSessionCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, chat_id)?;
        let stored = rows::existing_session(&transaction, chat_id)?;
        check_owner(&stored, generation, fence)?;
        let (session, _) = recover_one(&transaction, chat_id, timestamp)?;
        transaction.commit()?;
        Ok(session)
    }

    pub fn recover_chats(&mut self, timestamp: OffsetDateTime) -> Result<ChatRecovery, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement=transaction.prepare("SELECT chat_id FROM chat_sessions WHERE (owner_fence IS NOT NULL OR state IN ('opening','ready','busy')) AND NOT EXISTS(SELECT 1 FROM chat_execution_owners o WHERE o.chat_id=chat_sessions.chat_id) ORDER BY updated_sequence")?;
        let keys = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        require(
            keys.len() as u64 <= MAX_CHAT_SESSIONS,
            ChatError::ProjectionMismatch,
        )?;
        let mut result = ChatRecovery::default();
        for key in keys {
            let chat_id = key.parse::<ChatId>().ok();
            transaction.execute_batch("SAVEPOINT chat_recovery")?;
            let recovered = chat_id
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
                .and_then(|id| recover_one(&transaction, id, timestamp));
            match recovered {
                Ok((session, turn)) => {
                    transaction.execute_batch("RELEASE chat_recovery")?;
                    result.sessions.push(session);
                    if let Some(turn) = turn {
                        result.turns.push(turn);
                    }
                }
                Err(error) if matches!(error, StoreError::Chat(_) | StoreError::Json(_)) => {
                    transaction
                        .execute_batch("ROLLBACK TO chat_recovery; RELEASE chat_recovery")?;
                    result.quarantined.push(QuarantinedChat {
                        chat_id,
                        code: error.code(),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        transaction.commit()?;
        Ok(result)
    }

    /// An explicit recovery decision after the daemon has verified old-owner
    /// cleanup. Reads, cancellation and resume never call this implicitly.
    pub fn acknowledge_chat_recovery(
        &mut self,
        input: &ChatMutation,
        cleanup_verified: bool,
    ) -> Result<ChatSessionCommit, StoreError> {
        require(cleanup_verified, ChatError::InvalidInput)?;
        let binding = sessions::mutation_binding(input, json!([cleanup_verified]))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(
            &transaction,
            input.operation_id,
            "acknowledge_recovery",
            &binding,
        )? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            stored.session.state == ChatState::RecoveryRequired && stored.fence.is_none(),
            ChatError::InvalidTransition,
        )?;
        let mut active = stored
            .session
            .active_request_id
            .map(|id| rows::existing_turn(&transaction, id))
            .transpose()?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        session.state = ChatState::Closed;
        session.active_request_id = None;
        session.vendor_confirmed_generation = None;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let mut changed_turn = None;
        if let Some(stored_turn) = active.take() {
            require(
                stored_turn.turn.phase == ChatTurnPhase::RecoveryRequired,
                ChatError::InvalidTransition,
            )?;
            let previous = stored_turn.turn.version;
            let mut turn = stored_turn.turn;
            turn.phase = ChatTurnPhase::Interrupted;
            turn.outcome = Some(ChatTurnOutcome::Interrupted);
            turn.error_code = Some(ChatFailureCode::Interrupted);
            turn.version = next_version(turn.version)?;
            turn.updated_at = session.updated_at;
            let (task, run) =
                turns::finish_entities(&stored_turn.task, &stored_turn.run, turn.phase)?;
            changed_turn = Some((turn, previous, task, run, stored_turn.claimed));
        }
        let (event, sequence) = emit(
            &transaction,
            &session,
            changed_turn.as_ref().map(|value| &value.0),
            "chat_recovery_acknowledged",
            session.updated_at,
            event_data(&session),
        )?;
        if let Some((turn, previous, task, run, claimed)) = &changed_turn {
            rows::save_turn(
                &transaction,
                turn,
                *claimed,
                None,
                None,
                *previous,
                sequence,
            )?;
            rows::save_task(&transaction, task, sequence)?;
            rows::save_run(&transaction, run, sequence)?;
        }
        rows::save_session(&transaction, &session, None, previous_version, sequence)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "acknowledge_recovery",
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(sessions::committed(session, event, sequence, Vec::new()))
    }
}

fn recover_one(
    transaction: &Transaction<'_>,
    chat_id: ChatId,
    timestamp: OffsetDateTime,
) -> Result<(ChatSessionCommit, Option<ChatTurnCommit>), StoreError> {
    let stored = rows::existing_session(transaction, chat_id)?;
    let active = stored
        .session
        .active_request_id
        .map(|id| rows::existing_turn(transaction, id))
        .transpose()?;
    let previous_session = stored.session.version;
    let mut session = stored.session;
    session.state = if stored.fence.is_some() {
        ChatState::RecoveryRequired
    } else {
        ChatState::Closed
    };
    session.vendor_confirmed_generation = None;
    session.version = next_version(session.version)?;
    session.updated_at = timestamp.max(session.updated_at);
    let mut changed_turn = None;
    if let Some(stored_turn) = active {
        let previous = stored_turn.turn.version;
        let mut turn = stored_turn.turn;
        let unsent = turn.phase == ChatTurnPhase::Admitted && !stored_turn.claimed;
        require(
            matches!(
                turn.phase,
                ChatTurnPhase::Admitted | ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ),
            ChatError::ProjectionMismatch,
        )?;
        turn.phase = if unsent {
            ChatTurnPhase::Interrupted
        } else {
            ChatTurnPhase::RecoveryRequired
        };
        turn.outcome = unsent.then_some(ChatTurnOutcome::Interrupted);
        turn.error_code = Some(if unsent {
            ChatFailureCode::Interrupted
        } else {
            ChatFailureCode::OwnershipUncertain
        });
        turn.version = next_version(turn.version)?;
        turn.updated_at = session.updated_at;
        turn.output_complete = false;
        session.active_request_id = if unsent { None } else { Some(turn.request_id) };
        if !unsent {
            session.state = ChatState::RecoveryRequired;
        }
        let (task, run) = turns::finish_entities(&stored_turn.task, &stored_turn.run, turn.phase)?;
        changed_turn = Some((turn, previous, task, run, stored_turn.claimed));
    }
    let (event, sequence) = emit(
        transaction,
        &session,
        changed_turn.as_ref().map(|value| &value.0),
        "chat_owner_recovered",
        session.updated_at,
        event_data(&session),
    )?;
    if let Some((turn, previous, task, run, claimed)) = &changed_turn {
        rows::save_turn(transaction, turn, *claimed, None, None, *previous, sequence)?;
        rows::save_task(transaction, task, sequence)?;
        rows::save_run(transaction, run, sequence)?;
    }
    rows::save_session(transaction, &session, None, previous_session, sequence)?;
    let turn_commit = changed_turn.map(|(turn, _, _, _, _)| ChatTurnCommit {
        session: session.clone(),
        turn,
        event: Some(event.clone()),
        sequence: sequence as u64,
        duplicate: false,
    });
    Ok((
        sessions::committed(session, event, sequence, Vec::new()),
        turn_commit,
    ))
}
