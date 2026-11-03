//! Durable submissions sharing their chat's lifetime canonical Run.
use super::*;
use std::collections::BTreeSet;
use vibemux_types::{
    ChatAttemptId, Run, RunStatus, TaskStatus, ToolName,
    chat_attempt::*,
    chat_owner::{ChatExecutionOwner, ChatOwnerPhase, ChatWorkspaceEffect},
};

pub(crate) const MIGRATION_V12: &str = r#"
CREATE TABLE chat_execution_attempts(
 request_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 attempt_id TEXT NOT NULL UNIQUE,generation INTEGER NOT NULL CHECK(generation>0),
 version INTEGER NOT NULL CHECK(version>0),phase TEXT NOT NULL CHECK(phase IN
 ('admitted','running','cancel_requested','completed','failed','cancelled','interrupted','recovery_required')),
 send_claimed INTEGER NOT NULL CHECK(send_claimed IN(0,1)),send_fence TEXT,
 binding_json TEXT NOT NULL,finish_json TEXT,record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_execution_active_attempt ON chat_execution_attempts(chat_id)
 WHERE phase IN('admitted','running','cancel_requested','recovery_required');
CREATE TABLE chat_execution_records(
 record_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 attempt_id TEXT NOT NULL REFERENCES chat_execution_attempts(attempt_id),sequence INTEGER NOT NULL CHECK(sequence>0),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),UNIQUE(chat_id,sequence)
);
INSERT INTO schema_migrations(version,applied_at) VALUES(12,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

#[derive(Clone, Debug)]
pub struct ChatExecutionAttemptAdmission {
    pub chat_id: ChatId,
    pub request_id: Uuid,
    pub generation: u64,
    pub owner_version: u64,
    pub observed_commit: String,
    pub effective_tools: BTreeSet<ToolName>,
    pub prompt_ref: ContentRef,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct ChatExecutionAttemptCommit {
    pub owner: ChatExecutionOwner,
    pub session: ChatSession,
    pub run: Run,
    pub attempt: ChatExecutionAttempt,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
}

pub(super) fn execution_event(
    transaction: &Transaction<'_>,
    owner: &ChatExecutionOwner,
    session: &ChatSession,
    kind: &str,
    request_id: Option<Uuid>,
    data: Value,
) -> Result<(EventEnvelope, i64), StoreError> {
    let event_id = EventId::new();
    SqliteStore::insert_event(
        transaction,
        EventDraft {
            event_id,
            event_type: EventType::new(kind)?,
            project_id: owner.project_id,
            task_id: Some(owner.task_id),
            run_id: Some(owner.run_id),
            causation_id: None,
            actor: ActorName::new("daemon_chat_execution")?,
            timestamp: session.updated_at,
            idempotency_key: Some(format!("chat_event:{event_id}")),
            payload: EventPayload::new(
                json!({"chat_id":session.chat_id,"generation":session.generation,
            "session_state":session.state,"session_version":session.version,"owner_version":owner.version,
            "request_id":request_id,"data":data}),
            )?,
        },
    )
}

fn admission_binding(input: &ChatExecutionAttemptAdmission) -> Result<String, StoreError> {
    Ok(serde_json::to_string(&json!([
        input.chat_id,
        input.generation,
        input.owner_version,
        input.observed_commit,
        input.effective_tools,
        input.prompt_ref
    ]))?)
}

fn attempt_data(attempt: &ChatExecutionAttempt) -> Value {
    json!({"attempt_id":attempt.attempt_id,"phase":attempt.phase,"version":attempt.version,
        "prompt_sha256":attempt.prompt_ref.sha256,"prompt_bytes":attempt.prompt_ref.byte_count,
        "effective_tools":attempt.effective_tools,"error_code":attempt.error_code,
        "correlated_terminal":attempt.correlated_terminal,"output_complete":attempt.output_complete})
}

fn committed_attempt(
    connection: &Connection,
    stored: attempt_rows::StoredAttempt,
    event: Option<EventEnvelope>,
    sequence: u64,
    duplicate: bool,
) -> Result<ChatExecutionAttemptCommit, StoreError> {
    let owner = owners::required_owner(connection, stored.attempt.chat_id)?;
    Ok(ChatExecutionAttemptCommit {
        owner: owner.owner,
        session: owner.session,
        run: owner.run,
        attempt: stored.attempt,
        event,
        sequence,
        duplicate,
    })
}

impl SqliteStore {
    pub fn chat_execution_attempt(
        &self,
        request_id: Uuid,
    ) -> Result<Option<ChatExecutionAttempt>, StoreError> {
        Ok(attempt_rows::attempt(&self.connection, request_id)?.map(|stored| stored.attempt))
    }

    pub fn chat_execution_attempts(
        &self,
        chat_id: ChatId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ChatExecutionAttempt>, StoreError> {
        require(
            limit > 0 && limit <= MAX_CHAT_PAGE && after_sequence <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        owners::required_owner(&self.connection, chat_id)?;
        let mut statement=self.connection.prepare("SELECT request_id FROM chat_execution_attempts WHERE chat_id=? AND admitted_sequence>? ORDER BY admitted_sequence LIMIT ?")?;
        let ids = statement
            .query_map(params![chat_id.to_string(), after_sequence, limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let request_id =
                    Uuid::parse_str(&id).map_err(|_| fail(ChatError::ProjectionMismatch))?;
                Ok(attempt_rows::required_attempt(&self.connection, request_id)?.attempt)
            })
            .collect()
    }

    pub fn admit_chat_execution_attempt(
        &mut self,
        input: &ChatExecutionAttemptAdmission,
    ) -> Result<ChatExecutionAttemptCommit, StoreError> {
        require(!input.request_id.is_nil(), ChatError::InvalidInput)?;
        valid_generation(input.generation)?;
        input
            .prompt_ref
            .validate(MAX_CHAT_PROMPT_BYTES)
            .map_err(fail)?;
        let binding = admission_binding(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(stored) = attempt_rows::attempt(&transaction, input.request_id)? {
            require(stored.binding == binding, ChatError::Conflict)?;
            let sequence = stored.sequence;
            return committed_attempt(&transaction, stored, None, sequence, true);
        }
        let collision:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM chat_turns WHERE request_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1)",[input.request_id.to_string()],|row|row.get(0))?;
        require(!collision, ChatError::Conflict)?;
        let stored = owners::required_owner(&transaction, input.chat_id)?;
        let row = rows::existing_session(&transaction, input.chat_id)?;
        require(
            stored.owner.phase == ChatOwnerPhase::Open
                && stored.owner.workspace_effect == Some(ChatWorkspaceEffect::Settled)
                && stored.owner.workspace_verified_at.is_some(),
            ChatError::InvalidTransition,
        )?;
        require(
            stored.owner.version == input.owner_version
                && stored.owner.base_commit == input.observed_commit
                && input
                    .effective_tools
                    .is_subset(&stored.owner.admitted_tools),
            ChatError::Conflict,
        )?;
        check_generation(&row.session, input.generation)?;
        require(
            row.session.state == ChatState::Ready
                && row.fence.is_some()
                && row.session.active_request_id.is_none(),
            ChatError::Busy,
        )?;
        require(!row.session.content_purged, ChatError::ContentUnavailable)?;
        let confirmed = row.session.vendor_session_id.is_some()
            && row.session.vendor_confirmed_generation == Some(input.generation);
        require(
            confirmed || execution_sessions::permits_claude_bootstrap(&transaction, &row.session)?,
            ChatError::UnconfirmedSession,
        )?;
        check_detection(&transaction, row.session.harness)?;
        require(
            !dispatch_reserved(&transaction, &row.session.resource_key)?
                && !crate::chat::cleanup_is_reserved(&transaction, &row.session.resource_key)?,
            ChatError::Busy,
        )?;
        let total:u64=transaction.query_row("SELECT (SELECT COUNT(*) FROM chat_turns)+(SELECT COUNT(*) FROM chat_execution_attempts)",[],|row|row.get(0))?;
        require(
            total < MAX_CHAT_SUBMISSIONS && row.session.record_count < MAX_CHAT_RECORDS,
            ChatError::Capacity,
        )?;
        let old_session = row.session.version;
        let mut session = row.session;
        let mut owner = stored.owner;
        let mut run = stored.run;
        let old_owner = owner.version;
        session.updated_at = input
            .timestamp
            .max(session.updated_at)
            .max(owner.updated_at);
        session.version = next_version(session.version)?;
        session.state = ChatState::Busy;
        session.active_request_id = Some(input.request_id);
        claim_content(
            &transaction,
            &mut session,
            &input.prompt_ref,
            MAX_CHAT_PROMPT_BYTES,
        )?;
        owner.version = next_version(owner.version)?;
        owner.updated_at = session.updated_at;
        if run.status() == RunStatus::Preparing {
            run = run
                .transitioned(RunStatus::Running, None)
                .map_err(|_| fail(ChatError::InvalidTransition))?;
        }
        require(
            run.status() == RunStatus::Running,
            ChatError::InvalidTransition,
        )?;
        let mut attempt = ChatExecutionAttempt {
            schema_version: CHAT_ATTEMPT_SCHEMA_VERSION,
            chat_id: session.chat_id,
            attempt_id: ChatAttemptId::new(),
            request_id: input.request_id,
            project_id: owner.project_id,
            owner_task_id: owner.task_id,
            owner_run_id: owner.run_id,
            generation: session.generation,
            owner_version: input.owner_version,
            version: 1,
            phase: ChatTurnPhase::Admitted,
            admitted_sequence: 0,
            base_commit: input.observed_commit.clone(),
            effective_tools: input.effective_tools.clone(),
            prompt_ref: input.prompt_ref.clone(),
            vendor_turn_id: None,
            outcome: None,
            error_code: None,
            correlated_terminal: false,
            output_complete: false,
            created_at: session.updated_at,
            updated_at: session.updated_at,
        };
        let (event, sequence) = execution_event(
            &transaction,
            &owner,
            &session,
            "chat_execution_attempt_admitted",
            Some(input.request_id),
            attempt_data(&attempt),
        )?;
        attempt.admitted_sequence = sequence as u64;
        attempt.validate().map_err(fail)?;
        transaction.execute("INSERT INTO chat_execution_attempts(request_id,chat_id,attempt_id,generation,version,phase,send_claimed,send_fence,binding_json,finish_json,record_json,admitted_sequence,updated_sequence) VALUES(?,?,?,?,?,? ,0,NULL,?,NULL,?,?,?)",
            params![attempt.request_id.to_string(),attempt.chat_id.to_string(),attempt.attempt_id.to_string(),attempt.generation,attempt.version,
                attempt.phase.as_str(),binding,serde_json::to_string(&attempt)?,sequence,sequence])?;
        let timestamp = session.updated_at;
        super::attempt_records::insert_record(
            &transaction,
            &mut session,
            &attempt,
            records::RecordInput {
                id: input.request_id,
                kind: ChatRecordKind::User,
                reference: &input.prompt_ref,
                timestamp,
                original_byte_count: input.prompt_ref.byte_count,
            },
            sequence,
        )?;
        rows::save_session(&transaction, &session, row.fence, old_session, sequence)?;
        rows::save_run(&transaction, &run, sequence)?;
        let changed=transaction.execute("UPDATE chat_execution_owners SET version=?,record_json=?,updated_sequence=? WHERE chat_id=? AND version=?",
            params![owner.version,serde_json::to_string(&owner)?,sequence,owner.chat_id.to_string(),old_owner])?;
        require(changed == 1, ChatError::Conflict)?;
        let result = committed_attempt(
            &transaction,
            attempt_rows::StoredAttempt {
                attempt,
                binding,
                finish: None,
                claimed: false,
                fence: None,
                sequence: sequence as u64,
            },
            Some(event),
            sequence as u64,
            false,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn claim_chat_execution_attempt(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatExecutionAttemptCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = attempt_rows::required_attempt(&transaction, request_id)?;
        let row = rows::existing_session(&transaction, stored.attempt.chat_id)?;
        check_owner(&row, generation, fence)?;
        require(
            stored.attempt.generation == generation,
            ChatError::StaleGeneration,
        )?;
        require(
            !stored.claimed && stored.attempt.phase == ChatTurnPhase::Admitted,
            ChatError::TurnAlreadyClaimed,
        )?;
        require(
            row.session.state == ChatState::Busy
                && row.session.active_request_id == Some(request_id),
            ChatError::Busy,
        )?;
        let owner = owners::required_owner(&transaction, stored.attempt.chat_id)?.owner;
        let previous = stored.attempt.version;
        stored.attempt.version = next_version(previous)?;
        stored.attempt.phase = ChatTurnPhase::Running;
        stored.attempt.updated_at = timestamp
            .max(stored.attempt.updated_at)
            .max(row.session.updated_at);
        stored.claimed = true;
        stored.fence = Some(fence);
        let old_session = row.session.version;
        let mut session = row.session;
        session.version = next_version(session.version)?;
        session.updated_at = stored.attempt.updated_at;
        let (event, sequence) = execution_event(
            &transaction,
            &owner,
            &session,
            "chat_execution_attempt_claimed",
            Some(request_id),
            attempt_data(&stored.attempt),
        )?;
        attempt_rows::save(&transaction, &stored, previous, sequence)?;
        rows::save_session(&transaction, &session, row.fence, old_session, sequence)?;
        let result = committed_attempt(&transaction, stored, Some(event), sequence as u64, false)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn bind_chat_execution_attempt(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        vendor_turn_id: &str,
        timestamp: OffsetDateTime,
    ) -> Result<ChatExecutionAttemptCommit, StoreError> {
        require(valid_vendor_id(vendor_turn_id), ChatError::InvalidInput)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = attempt_rows::required_attempt(&transaction, request_id)?;
        require(
            stored.attempt.generation == generation && stored.fence == Some(fence),
            ChatError::StaleFence,
        )?;
        if let Some(known) = &stored.attempt.vendor_turn_id {
            require(known == vendor_turn_id, ChatError::Conflict)?;
            let sequence = stored.sequence;
            return committed_attempt(&transaction, stored, None, sequence, true);
        }
        let row = rows::existing_session(&transaction, stored.attempt.chat_id)?;
        check_owner(&row, generation, fence)?;
        require(
            row.session.active_request_id == Some(request_id)
                && matches!(
                    stored.attempt.phase,
                    ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
                ),
            ChatError::InvalidTransition,
        )?;
        require(
            row.session.vendor_confirmed_generation == Some(generation),
            ChatError::UnconfirmedSession,
        )?;
        let owner = owners::required_owner(&transaction, stored.attempt.chat_id)?.owner;
        let previous = stored.attempt.version;
        stored.attempt.vendor_turn_id = Some(vendor_turn_id.to_owned());
        stored.attempt.version = next_version(previous)?;
        stored.attempt.updated_at = timestamp
            .max(stored.attempt.updated_at)
            .max(row.session.updated_at);
        let old_session = row.session.version;
        let mut session = row.session;
        session.version = next_version(session.version)?;
        session.updated_at = stored.attempt.updated_at;
        let (event, sequence) = execution_event(
            &transaction,
            &owner,
            &session,
            "chat_execution_attempt_bound",
            Some(request_id),
            attempt_data(&stored.attempt),
        )?;
        attempt_rows::save(&transaction, &stored, previous, sequence)?;
        rows::save_session(&transaction, &session, row.fence, old_session, sequence)?;
        let result = committed_attempt(&transaction, stored, Some(event), sequence as u64, false)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn cancel_chat_execution_attempt(
        &mut self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatExecutionAttemptCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = attempt_rows::required_attempt(&transaction, request_id)?;
        require(
            stored.attempt.generation == generation
                && stored.fence.is_none_or(|known| known == fence),
            ChatError::StaleFence,
        )?;
        if stored.attempt.phase.is_terminal()
            || stored.attempt.phase == ChatTurnPhase::CancelRequested
        {
            let sequence = stored.sequence;
            return committed_attempt(&transaction, stored, None, sequence, true);
        }
        require(
            matches!(
                stored.attempt.phase,
                ChatTurnPhase::Admitted | ChatTurnPhase::Running
            ),
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, stored.attempt.chat_id)?;
        check_owner(&row, generation, fence)?;
        require(
            row.session.active_request_id == Some(request_id),
            ChatError::InvalidTransition,
        )?;
        let owner = owners::required_owner(&transaction, stored.attempt.chat_id)?.owner;
        let previous = stored.attempt.version;
        let old_session = row.session.version;
        let mut session = row.session;
        stored.attempt.version = next_version(previous)?;
        stored.attempt.updated_at = timestamp
            .max(stored.attempt.updated_at)
            .max(session.updated_at);
        if stored.claimed {
            stored.attempt.phase = ChatTurnPhase::CancelRequested;
        } else {
            stored.attempt.phase = ChatTurnPhase::Cancelled;
            stored.attempt.outcome = Some(ChatTurnOutcome::Cancelled);
            stored.attempt.error_code = Some(ChatFailureCode::Cancelled);
            session.active_request_id = None;
            session.state = ChatState::Ready;
        }
        session.version = next_version(session.version)?;
        session.updated_at = stored.attempt.updated_at;
        let (event, sequence) = execution_event(
            &transaction,
            &owner,
            &session,
            "chat_execution_attempt_cancel_requested",
            Some(request_id),
            attempt_data(&stored.attempt),
        )?;
        approval_lifecycle::invalidate(&transaction, request_id, session.updated_at, false)?;
        attempt_rows::save(&transaction, &stored, previous, sequence)?;
        rows::save_session(&transaction, &session, row.fence, old_session, sequence)?;
        let result = committed_attempt(&transaction, stored, Some(event), sequence as u64, false)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn finish_chat_execution_attempt(
        &mut self,
        input: &ChatTurnFinish,
    ) -> Result<ChatExecutionAttemptCommit, StoreError> {
        require(
            input
                .vendor_session_id
                .as_ref()
                .is_none_or(|value| valid_vendor_id(value))
                && input
                    .vendor_turn_id
                    .as_ref()
                    .is_none_or(|value| valid_vendor_id(value)),
            ChatError::InvalidInput,
        )?;
        let finish = serde_json::to_string(&attempt_rows::FinishBinding {
            generation: input.generation,
            fence: input.fence.0,
            outcome: input.outcome,
            correlated_terminal: input.correlated_terminal,
            vendor_session_id: input.vendor_session_id.clone(),
            vendor_turn_id: input.vendor_turn_id.clone(),
            error_code: input.error_code,
            process_alive: input.process_alive,
            output_complete: input.output_complete,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = attempt_rows::required_attempt(&transaction, input.request_id)?;
        require(
            stored.attempt.generation == input.generation && stored.fence == Some(input.fence),
            ChatError::StaleFence,
        )?;
        if let Some(known) = &stored.finish {
            require(known == &finish, ChatError::Conflict)?;
            let sequence = stored.sequence;
            return committed_attempt(&transaction, stored, None, sequence, true);
        }
        require(
            stored.claimed
                && matches!(
                    stored.attempt.phase,
                    ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
                ),
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, stored.attempt.chat_id)?;
        check_owner(&row, input.generation, input.fence)?;
        require(
            row.session.active_request_id == Some(input.request_id),
            ChatError::InvalidTransition,
        )?;
        if input.correlated_terminal {
            require(
                row.session.vendor_confirmed_generation == Some(input.generation)
                    && input.vendor_session_id.is_some()
                    && row.session.vendor_session_id == input.vendor_session_id
                    && input.vendor_turn_id.is_some()
                    && stored.attempt.vendor_turn_id == input.vendor_turn_id
                    && input.outcome != ChatTurnOutcome::Interrupted,
                ChatError::UnconfirmedSession,
            )?;
        } else {
            require(
                !input.process_alive && input.outcome == ChatTurnOutcome::Interrupted,
                ChatError::UnconfirmedSession,
            )?;
        }
        require(
            !input.output_complete || input.correlated_terminal,
            ChatError::InvalidInput,
        )?;
        require(
            input.outcome != ChatTurnOutcome::Completed || input.error_code.is_none(),
            ChatError::InvalidInput,
        )?;
        if input.outcome == ChatTurnOutcome::Interrupted {
            approval_lifecycle::invalidate(&transaction, input.request_id, input.timestamp, true)?;
        } else {
            approval_lifecycle::require_settled(&transaction, input.request_id)?;
        }
        let owner = owners::required_owner(&transaction, stored.attempt.chat_id)?;
        let previous = stored.attempt.version;
        let old_session = row.session.version;
        let mut session = row.session;
        stored.attempt.phase = match input.outcome {
            ChatTurnOutcome::Completed => ChatTurnPhase::Completed,
            ChatTurnOutcome::Failed => ChatTurnPhase::Failed,
            ChatTurnOutcome::Cancelled => ChatTurnPhase::Cancelled,
            ChatTurnOutcome::Interrupted => ChatTurnPhase::Interrupted,
        };
        stored.attempt.version = next_version(previous)?;
        stored.attempt.outcome = Some(input.outcome);
        stored.attempt.error_code = input.error_code;
        stored.attempt.correlated_terminal = input.correlated_terminal;
        stored.attempt.output_complete = input.output_complete;
        stored.attempt.updated_at = input
            .timestamp
            .max(stored.attempt.updated_at)
            .max(session.updated_at);
        stored.finish = Some(finish);
        session.active_request_id = None;
        session.state = if input.process_alive {
            ChatState::Ready
        } else {
            ChatState::RecoveryRequired
        };
        session.updated_at = stored.attempt.updated_at;
        session.version = next_version(session.version)?;
        let (event, sequence) = execution_event(
            &transaction,
            &owner.owner,
            &session,
            "chat_execution_attempt_finished",
            Some(input.request_id),
            attempt_data(&stored.attempt),
        )?;
        attempt_rows::save(&transaction, &stored, previous, sequence)?;
        rows::save_session(&transaction, &session, row.fence, old_session, sequence)?;
        if !input.process_alive && owner.task.status() != TaskStatus::Blocked {
            let task = owner
                .task
                .transitioned(TaskStatus::Blocked)
                .map_err(|_| fail(ChatError::InvalidTransition))?;
            rows::save_task(&transaction, &task, sequence)?;
        }
        let result = committed_attempt(&transaction, stored, Some(event), sequence as u64, false)?;
        transaction.commit()?;
        Ok(result)
    }
}

pub(super) fn prepare_process_end(
    transaction: &Transaction<'_>,
    session: &mut ChatSession,
    fence: ChatOwnerFence,
    cleanup_verified: bool,
    timestamp: OffsetDateTime,
) -> Result<Option<(attempt_rows::StoredAttempt, u64)>, StoreError> {
    let Some(request_id) = session.active_request_id else {
        return Ok(None);
    };
    let mut stored = attempt_rows::required_attempt(transaction, request_id)?;
    require(
        stored.attempt.generation == session.generation
            && stored.fence.is_none_or(|known| known == fence),
        ChatError::StaleFence,
    )?;
    approval_lifecycle::invalidate(transaction, request_id, timestamp, true)?;
    let previous = stored.attempt.version;
    stored.attempt.version = next_version(previous)?;
    stored.attempt.updated_at = timestamp
        .max(session.updated_at)
        .max(stored.attempt.updated_at);
    stored.attempt.correlated_terminal = false;
    stored.attempt.output_complete = false;
    if cleanup_verified || !stored.claimed {
        stored.attempt.phase = ChatTurnPhase::Interrupted;
        stored.attempt.outcome = Some(ChatTurnOutcome::Interrupted);
        stored.attempt.error_code = Some(ChatFailureCode::Interrupted);
        session.active_request_id = None;
    } else {
        stored.attempt.phase = ChatTurnPhase::RecoveryRequired;
        stored.attempt.error_code = Some(ChatFailureCode::OwnershipUncertain);
    }
    Ok(Some((stored, previous)))
}
