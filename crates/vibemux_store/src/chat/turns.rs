//! Per-turn canonical Task/Run admission and fenced, non-replayable send claims.
use super::*;
use rusqlite::TransactionBehavior;
use vibemux_types::{Run, RunCompletionAuthority, RunSpec, RunStatus, Task, TaskSpec, TaskStatus};

pub(super) fn admission_binding(
    chat_id: ChatId,
    generation: u64,
    reference: &ContentRef,
) -> Result<String, StoreError> {
    Ok(serde_json::to_string(&json!([
        chat_id,
        generation,
        reference.sha256,
        reference.byte_count
    ]))?)
}

impl SqliteStore {
    pub fn admit_chat_turn(
        &mut self,
        input: &ChatTurnAdmission,
    ) -> Result<ChatTurnCommit, StoreError> {
        require(!input.request_id.is_nil(), ChatError::InvalidInput)?;
        valid_generation(input.expected_generation)?;
        input
            .prompt_ref
            .validate(MAX_CHAT_PROMPT_BYTES)
            .map_err(fail)?;
        let binding =
            admission_binding(input.chat_id, input.expected_generation, &input.prompt_ref)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(stored) = rows::turn(&transaction, input.request_id)? {
            require(stored.binding == binding, ChatError::Conflict)?;
            return unchanged_turn(&transaction, stored);
        }
        require(
            matches!(input.base_commit.len(), 40 | 64)
                && input
                    .base_commit
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            ChatError::InvalidInput,
        )?;
        let collision:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1 UNION ALL SELECT 1 FROM chat_execution_attempts WHERE request_id=?1)",[input.request_id.to_string()],|row|row.get(0))?;
        require(!collision, ChatError::Conflict)?;
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            stored.session.state == ChatState::Ready
                && stored.fence.is_some()
                && stored.session.active_request_id.is_none(),
            ChatError::Busy,
        )?;
        require(
            !stored.session.content_purged,
            ChatError::ContentUnavailable,
        )?;
        check_detection(&transaction, stored.session.harness)?;
        require(
            !dispatch_reserved(&transaction, &stored.session.resource_key)?
                && !is_reserved(&transaction, &stored.session.resource_key)?,
            ChatError::Busy,
        )?;
        let count: u64 =
            transaction.query_row("SELECT COUNT(*) FROM chat_turns", [], |row| row.get(0))?;
        require(
            count < MAX_CHAT_SUBMISSIONS && stored.session.record_count < MAX_CHAT_RECORDS,
            ChatError::Capacity,
        )?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        claim_content(
            &transaction,
            &mut session,
            &input.prompt_ref,
            MAX_CHAT_PROMPT_BYTES,
        )?;
        let task = Task::new(TaskSpec {
            project_id: session.project_id,
            title: format!("chat turn {}", session.harness.as_str()),
            description: String::new(),
        })
        .and_then(|task| task.transitioned(TaskStatus::InProgress))
        .map_err(|_| fail(ChatError::InvalidInput))?;
        let run = Run::new(RunSpec {
            project_id: session.project_id,
            task_id: task.task_id(),
            harness: session.harness.as_str().to_owned(),
            role: "chat_turn".to_owned(),
            protocol: "chat_read_only_v1".to_owned(),
            base_commit: input.base_commit.clone(),
        })
        .map_err(|_| fail(ChatError::InvalidInput))?;
        let timestamp = input.timestamp.max(session.updated_at);
        let mut turn = ChatTurn {
            schema_version: CHAT_SCHEMA_VERSION,
            chat_id: session.chat_id,
            turn_id: ChatTurnId::new(),
            request_id: input.request_id,
            project_id: session.project_id,
            task_id: task.task_id(),
            run_id: run.run_id(),
            generation: session.generation,
            version: 1,
            phase: ChatTurnPhase::Admitted,
            admitted_sequence: 0,
            base_commit: input.base_commit.clone(),
            prompt_ref: input.prompt_ref.clone(),
            vendor_turn_id: None,
            outcome: None,
            error_code: None,
            correlated_terminal: false,
            output_complete: false,
            created_at: timestamp,
            updated_at: timestamp,
        };
        session.active_request_id = Some(input.request_id);
        session.state = ChatState::Busy;
        session.version = next_version(session.version)?;
        session.updated_at = timestamp;
        let (event, sequence) = emit(
            &transaction,
            &session,
            Some(&turn),
            "chat_turn_admitted",
            timestamp,
            turn_event_data(&turn),
        )?;
        turn.admitted_sequence = sequence as u64;
        rows::save_task(&transaction, &task, sequence)?;
        rows::save_run(&transaction, &run, sequence)?;
        rows::insert_turn(
            &transaction,
            &turn,
            &session.resource_key,
            &binding,
            sequence,
        )?;
        records::insert_record(
            &transaction,
            &mut session,
            &turn,
            records::RecordInput {
                id: input.request_id,
                kind: ChatRecordKind::User,
                reference: &input.prompt_ref,
                timestamp,
                original_byte_count: input.prompt_ref.byte_count,
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
        Ok(ChatTurnCommit {
            session,
            turn,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }

    /// The only send claim. A duplicate admission is not permission to send again.
    pub fn claim_chat_turn(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatTurnCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = rows::existing_turn(&transaction, request_id)?;
        require(!stored.claimed, ChatError::TurnAlreadyClaimed)?;
        require(
            stored.turn.phase == ChatTurnPhase::Admitted,
            ChatError::InvalidTransition,
        )?;
        let session = rows::existing_session(&transaction, stored.turn.chat_id)?;
        check_owner(&session, generation, fence)?;
        require(
            stored.turn.generation == generation,
            ChatError::StaleGeneration,
        )?;
        let previous_turn = stored.turn.version;
        let previous_session = session.session.version;
        let mut turn = stored.turn;
        let mut session_record = session.session;
        turn.phase = ChatTurnPhase::Running;
        turn.version = next_version(turn.version)?;
        turn.updated_at = timestamp.max(turn.updated_at);
        session_record.version = next_version(session_record.version)?;
        session_record.updated_at = turn.updated_at.max(session_record.updated_at);
        let run = stored
            .run
            .transitioned(RunStatus::Running, None)
            .map_err(|_| fail(ChatError::ProjectionMismatch))?;
        let (event, sequence) = emit(
            &transaction,
            &session_record,
            Some(&turn),
            "chat_turn_started",
            session_record.updated_at,
            turn_event_data(&turn),
        )?;
        rows::save_run(&transaction, &run, sequence)?;
        rows::save_turn(
            &transaction,
            &turn,
            true,
            Some(fence),
            None,
            previous_turn,
            sequence,
        )?;
        rows::save_session(
            &transaction,
            &session_record,
            session.fence,
            previous_session,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatTurnCommit {
            session: session_record,
            turn,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }

    pub fn bind_chat_turn(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        vendor_turn_id: &str,
        timestamp: OffsetDateTime,
    ) -> Result<ChatTurnCommit, StoreError> {
        require(valid_vendor_id(vendor_turn_id), ChatError::InvalidInput)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = rows::existing_turn(&transaction, request_id)?;
        let session = rows::existing_session(&transaction, stored.turn.chat_id)?;
        check_owner(&session, generation, fence)?;
        require(
            stored.fence == Some(fence) && stored.turn.generation == generation,
            ChatError::StaleFence,
        )?;
        require(
            matches!(
                stored.turn.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ),
            ChatError::InvalidTransition,
        )?;
        if let Some(existing) = &stored.turn.vendor_turn_id {
            require(existing == vendor_turn_id, ChatError::Conflict)?;
            return unchanged_turn(&transaction, stored);
        }
        let previous_turn = stored.turn.version;
        let previous_session = session.session.version;
        let mut turn = stored.turn;
        let mut session_record = session.session;
        turn.vendor_turn_id = Some(vendor_turn_id.to_owned());
        turn.version = next_version(turn.version)?;
        turn.updated_at = timestamp.max(turn.updated_at);
        session_record.version = next_version(session_record.version)?;
        session_record.updated_at = turn.updated_at.max(session_record.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session_record,
            Some(&turn),
            "chat_turn_bound",
            session_record.updated_at,
            turn_event_data(&turn),
        )?;
        rows::save_turn(
            &transaction,
            &turn,
            true,
            Some(fence),
            None,
            previous_turn,
            sequence,
        )?;
        rows::save_session(
            &transaction,
            &session_record,
            session.fence,
            previous_session,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatTurnCommit {
            session: session_record,
            turn,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }

    pub fn finish_chat_turn(
        &mut self,
        input: &ChatTurnFinish,
    ) -> Result<ChatTurnCommit, StoreError> {
        let binding = finish_binding(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = rows::existing_turn(&transaction, input.request_id)?;
        require(
            stored.fence == Some(input.fence) && stored.turn.generation == input.generation,
            ChatError::StaleFence,
        )?;
        if stored.turn.phase.is_terminal() {
            require(
                stored.finish.as_deref() == Some(&binding),
                ChatError::Conflict,
            )?;
            return unchanged_turn(&transaction, stored);
        }
        let owner = rows::existing_session(&transaction, stored.turn.chat_id)?;
        check_owner(&owner, input.generation, input.fence)?;
        require(
            matches!(
                stored.turn.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ),
            ChatError::InvalidTransition,
        )?;
        if input.correlated_terminal {
            require(
                owner.session.vendor_confirmed_generation == Some(input.generation)
                    && owner.session.vendor_session_id.is_some()
                    && owner.session.vendor_session_id == input.vendor_session_id
                    && stored.turn.vendor_turn_id == input.vendor_turn_id,
                ChatError::UnconfirmedSession,
            )?;
            if owner.session.harness == ChatHarness::Codex {
                require(input.vendor_turn_id.is_some(), ChatError::InvalidInput)?;
            }
        }
        require(
            input.outcome != ChatTurnOutcome::Completed
                || (input.correlated_terminal && input.error_code.is_none()),
            ChatError::InvalidInput,
        )?;
        require(
            input.outcome == ChatTurnOutcome::Completed || input.error_code.is_some(),
            ChatError::InvalidInput,
        )?;
        require(
            input.outcome != ChatTurnOutcome::Cancelled
                || input.correlated_terminal
                || !input.process_alive,
            ChatError::InvalidInput,
        )?;
        let previous_turn = stored.turn.version;
        let previous_session = owner.session.version;
        let mut turn = stored.turn;
        let mut session = owner.session;
        let uncertain = !input.correlated_terminal && input.process_alive;
        let cancelled = turn.phase == ChatTurnPhase::CancelRequested && !uncertain;
        turn.phase = if uncertain {
            ChatTurnPhase::RecoveryRequired
        } else if cancelled {
            ChatTurnPhase::Cancelled
        } else {
            match input.outcome {
                ChatTurnOutcome::Completed => ChatTurnPhase::Completed,
                ChatTurnOutcome::Failed => ChatTurnPhase::Failed,
                ChatTurnOutcome::Cancelled => ChatTurnPhase::Cancelled,
                ChatTurnOutcome::Interrupted => ChatTurnPhase::Interrupted,
            }
        };
        turn.outcome = if uncertain {
            None
        } else if cancelled {
            Some(ChatTurnOutcome::Cancelled)
        } else {
            Some(input.outcome)
        };
        turn.error_code = if uncertain {
            Some(ChatFailureCode::OwnershipUncertain)
        } else if cancelled {
            Some(ChatFailureCode::Cancelled)
        } else {
            input.error_code
        };
        turn.correlated_terminal = input.correlated_terminal;
        turn.output_complete = input.output_complete && !uncertain;
        turn.version = next_version(turn.version)?;
        turn.updated_at = input.timestamp.max(turn.updated_at);
        let (task, run) = finish_entities(&stored.task, &stored.run, turn.phase)?;
        session.state = if uncertain {
            ChatState::RecoveryRequired
        } else if input.process_alive {
            ChatState::Ready
        } else {
            ChatState::Suspended
        };
        session.active_request_id = uncertain.then_some(turn.request_id);
        if !input.process_alive || uncertain {
            session.vendor_confirmed_generation = None;
        }
        session.version = next_version(session.version)?;
        session.updated_at = turn.updated_at.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            Some(&turn),
            "chat_turn_finished",
            session.updated_at,
            turn_event_data(&turn),
        )?;
        rows::save_task(&transaction, &task, sequence)?;
        rows::save_run(&transaction, &run, sequence)?;
        let turn_fence = if uncertain { None } else { Some(input.fence) };
        rows::save_turn(
            &transaction,
            &turn,
            true,
            turn_fence,
            Some(&binding),
            previous_turn,
            sequence,
        )?;
        rows::save_session(
            &transaction,
            &session,
            if input.process_alive && !uncertain {
                owner.fence
            } else {
                None
            },
            previous_session,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatTurnCommit {
            session,
            turn,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }

    pub fn cancel_chat_turn(
        &mut self,
        request_id: Uuid,
        timestamp: OffsetDateTime,
    ) -> Result<ChatTurnCommit, StoreError> {
        self.cancel_turn_owned(request_id, timestamp, None)
    }

    /// Runtime cleanup may cancel only its own unclaimed admission. User
    /// cancellation uses the separate request-ID operation above.
    pub fn cancel_unsent_chat_turn(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatTurnCommit, StoreError> {
        self.cancel_turn_owned(request_id, timestamp, Some((generation, fence)))
    }

    fn cancel_turn_owned(
        &mut self,
        request_id: Uuid,
        timestamp: OffsetDateTime,
        required_owner: Option<(u64, ChatOwnerFence)>,
    ) -> Result<ChatTurnCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = rows::existing_turn(&transaction, request_id)?;
        if let Some((generation, fence)) = required_owner {
            let owner = rows::existing_session(&transaction, stored.turn.chat_id)?;
            check_owner(&owner, generation, fence)?;
            require(
                stored.turn.generation == generation,
                ChatError::StaleGeneration,
            )?;
            require(
                stored.turn.phase == ChatTurnPhase::Admitted && !stored.claimed,
                ChatError::InvalidTransition,
            )?;
        }
        if stored.turn.phase.is_terminal() || stored.turn.phase == ChatTurnPhase::CancelRequested {
            return unchanged_turn(&transaction, stored);
        }
        require(
            stored.turn.phase != ChatTurnPhase::RecoveryRequired,
            ChatError::InvalidTransition,
        )?;
        let owner = rows::existing_session(&transaction, stored.turn.chat_id)?;
        require(
            owner.session.generation == stored.turn.generation,
            ChatError::StaleGeneration,
        )?;
        let previous_turn = stored.turn.version;
        let previous_session = owner.session.version;
        let mut turn = stored.turn;
        let mut session = owner.session;
        let unsent = turn.phase == ChatTurnPhase::Admitted;
        turn.phase = if unsent {
            ChatTurnPhase::Cancelled
        } else {
            ChatTurnPhase::CancelRequested
        };
        if unsent {
            turn.outcome = Some(ChatTurnOutcome::Cancelled);
            turn.error_code = Some(ChatFailureCode::Cancelled);
            session.active_request_id = None;
            session.state = ChatState::Ready;
        }
        turn.version = next_version(turn.version)?;
        turn.updated_at = timestamp.max(turn.updated_at);
        session.version = next_version(session.version)?;
        session.updated_at = turn.updated_at.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            Some(&turn),
            "chat_turn_cancel_requested",
            session.updated_at,
            turn_event_data(&turn),
        )?;
        if unsent {
            let (task, run) = finish_entities(&stored.task, &stored.run, ChatTurnPhase::Cancelled)?;
            rows::save_task(&transaction, &task, sequence)?;
            rows::save_run(&transaction, &run, sequence)?;
        }
        rows::save_turn(
            &transaction,
            &turn,
            stored.claimed,
            stored.fence,
            None,
            previous_turn,
            sequence,
        )?;
        rows::save_session(
            &transaction,
            &session,
            owner.fence,
            previous_session,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatTurnCommit {
            session,
            turn,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
        })
    }
}

pub(super) fn unchanged_turn(
    transaction: &Transaction<'_>,
    stored: rows::StoredTurn,
) -> Result<ChatTurnCommit, StoreError> {
    let session = rows::existing_session(transaction, stored.turn.chat_id)?.session;
    Ok(ChatTurnCommit {
        session,
        turn: stored.turn,
        event: None,
        sequence: stored.sequence,
        duplicate: true,
    })
}
pub(super) fn finish_entities(
    task: &Task,
    run: &Run,
    phase: ChatTurnPhase,
) -> Result<(Task, Run), StoreError> {
    let (task_state, run_state, authority) = match phase {
        ChatTurnPhase::Completed => (
            TaskStatus::Done,
            RunStatus::Succeeded,
            Some(RunCompletionAuthority::StructuredAdapter),
        ),
        ChatTurnPhase::Cancelled => (
            TaskStatus::Cancelled,
            if run.status() == RunStatus::Preparing {
                RunStatus::Failed
            } else {
                RunStatus::Stopped
            },
            None,
        ),
        ChatTurnPhase::RecoveryRequired => (TaskStatus::Blocked, RunStatus::Stale, None),
        ChatTurnPhase::Failed | ChatTurnPhase::Interrupted => {
            (TaskStatus::Blocked, RunStatus::Failed, None)
        }
        _ => return Err(fail(ChatError::InvalidTransition)),
    };
    let task = if task.status() == task_state {
        task.clone()
    } else {
        task.transitioned(task_state)
            .map_err(|_| fail(ChatError::ProjectionMismatch))?
    };
    let run = if run.status() == run_state {
        run.clone()
    } else {
        run.transitioned(run_state, authority)
            .map_err(|_| fail(ChatError::ProjectionMismatch))?
    };
    Ok((task, run))
}
fn finish_binding(input: &ChatTurnFinish) -> Result<String, StoreError> {
    require(!input.request_id.is_nil(), ChatError::InvalidInput)?;
    valid_generation(input.generation)?;
    for id in [&input.vendor_session_id, &input.vendor_turn_id]
        .into_iter()
        .flatten()
    {
        require(valid_vendor_id(id), ChatError::InvalidInput)?;
    }
    Ok(serde_json::to_string(&json!([
        input.generation,
        input.outcome,
        input.correlated_terminal,
        input.vendor_session_id,
        input.vendor_turn_id,
        input.error_code,
        input.process_alive,
        input.output_complete
    ]))?)
}
