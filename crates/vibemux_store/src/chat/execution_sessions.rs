//! Process-generation authority for a lifetime owner. Workspace claims remain separate.
use super::*;
use vibemux_types::{
    Run, TaskStatus,
    chat_owner::{ChatExecutionOwner, ChatOwnerPhase, ChatWorkspaceEffect},
};

#[derive(Clone)]
pub struct ChatExecutionStart {
    pub operation_id: Uuid,
    pub chat_id: ChatId,
    pub expected_generation: u64,
    pub expected_owner_version: u64,
    pub profile_sha256: String,
    pub resource_key: String,
    pub observed_commit: String,
    pub expected_vendor_id: Option<String>,
    pub timestamp: OffsetDateTime,
}
impl fmt::Debug for ChatExecutionStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionStart")
            .field("chat_id", &self.chat_id)
            .field("expected_generation", &self.expected_generation)
            .field("expected_owner_version", &self.expected_owner_version)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub enum ChatExecutionProcessChange {
    Ready,
    VendorBound {
        vendor_id: String,
    },
    Released {
        cleanup_verified: bool,
        closed: bool,
    },
}

#[derive(Clone, Debug)]
pub struct ChatExecutionSessionCommit {
    pub owner: ChatExecutionOwner,
    pub session: ChatSession,
    pub run: Run,
    pub fence: Option<ChatOwnerFence>,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ChatExecutionRecovery {
    pub sessions: Vec<ChatExecutionSessionCommit>,
    pub quarantined: Vec<QuarantinedChat>,
}

pub(super) fn committed_session(
    connection: &Connection,
    chat_id: ChatId,
    event: Option<EventEnvelope>,
    sequence: u64,
    duplicate: bool,
    fence: Option<ChatOwnerFence>,
) -> Result<ChatExecutionSessionCommit, StoreError> {
    let stored = owners::required_owner(connection, chat_id)?;
    Ok(ChatExecutionSessionCommit {
        owner: stored.owner,
        session: stored.session,
        run: stored.run,
        event,
        sequence,
        duplicate,
        fence,
    })
}

/// Claude proves its root session/profile after the first input in a process.
/// A new UUID must be unused; an exact retained resume permits one first input
/// in the new generation. Acknowledgement and results still require root proof.
pub(super) fn permits_claude_bootstrap(
    connection: &Connection,
    session: &ChatSession,
) -> Result<bool, StoreError> {
    if session.harness != ChatHarness::Claude
        || !session
            .expected_vendor_id
            .as_ref()
            .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()))
    {
        return Ok(false);
    }
    let claimed: bool = if let Some(known) = &session.vendor_session_id {
        if session.expected_vendor_id.as_ref() != Some(known) {
            return Ok(false);
        }
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_execution_attempts WHERE chat_id=? AND generation=? AND send_claimed=1)",
            params![session.chat_id.to_string(), session.generation], |row| row.get(0))?
    } else {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_execution_attempts WHERE chat_id=? AND send_claimed=1)",
            [session.chat_id.to_string()], |row| row.get(0))?
    };
    Ok(!claimed)
}

impl SqliteStore {
    /// Resolve the client-visible Start binding before current route/workspace
    /// checks. A replay observes its original receipt and never grants a fence.
    pub fn replay_chat_execution_start(
        &self,
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
    ) -> Result<Option<ChatExecutionSessionCommit>, StoreError> {
        require(!operation_id.is_nil(), ChatError::InvalidInput)?;
        valid_generation(generation)?;
        require(
            owner_version > 0 && owner_version <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        let row: Option<(String, String, Option<String>, u64)> = self.connection.query_row(
            "SELECT chat_id,kind,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,sequence FROM chat_operations WHERE operation_id=?",
            [operation_id.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
        let Some((recorded_chat, kind, binding, sequence)) = row else {
            let used: bool = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_turns WHERE request_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1 UNION ALL SELECT 1 FROM chat_execution_attempts WHERE request_id=?1)",
                [operation_id.to_string()], |row| row.get(0))?;
            require(!used, ChatError::Conflict)?;
            return Ok(None);
        };
        require(
            kind == "chat_execution_process_started" && recorded_chat == chat_id.to_string(),
            ChatError::Conflict,
        )?;
        let binding = binding.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
        let (bound_chat, bound_generation, bound_owner_version, _, _, _, _): (
            ChatId,
            u64,
            u64,
            String,
            String,
            String,
            Option<String>,
        ) = serde_json::from_str(&binding).map_err(|_| fail(ChatError::ProjectionMismatch))?;
        require(
            bound_chat == chat_id
                && bound_generation == generation
                && bound_owner_version == owner_version,
            ChatError::Conflict,
        )?;
        committed_session(&self.connection, chat_id, None, sequence, true, None).map(Some)
    }

    /// Claim process authority only after the daemon verified the stored worktree.
    /// A duplicate acknowledges its old operation and never returns launch authority.
    pub fn start_chat_execution_process(
        &mut self,
        input: &ChatExecutionStart,
    ) -> Result<ChatExecutionSessionCommit, StoreError> {
        valid_generation(input.expected_generation)?;
        require(
            input.expected_owner_version > 0
                && input
                    .expected_vendor_id
                    .as_ref()
                    .is_none_or(|id| valid_vendor_id(id)),
            ChatError::InvalidInput,
        )?;
        let binding = serde_json::to_string(&json!([
            input.chat_id,
            input.expected_generation,
            input.expected_owner_version,
            input.profile_sha256,
            input.resource_key,
            input.observed_commit,
            input.expected_vendor_id
        ]))?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let kind = "chat_execution_process_started";
        if let Some(receipt) = operation(&transaction, input.operation_id, kind, &binding)? {
            require(receipt.chat_id == input.chat_id, ChatError::Conflict)?;
            return committed_session(
                &transaction,
                input.chat_id,
                None,
                receipt.sequence,
                true,
                None,
            );
        }
        let stored = owners::required_owner(&transaction, input.chat_id)?;
        let row = rows::existing_session(&transaction, input.chat_id)?;
        if row.session.harness == ChatHarness::Claude {
            require(
                input
                    .expected_vendor_id
                    .as_ref()
                    .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|id| !id.is_nil())),
                ChatError::InvalidInput,
            )?;
        } else if row.session.vendor_session_id.is_none() {
            require(
                input.expected_vendor_id.is_none(),
                ChatError::UnconfirmedSession,
            )?;
        }
        require(
            stored.owner.phase == ChatOwnerPhase::Open
                && stored.owner.workspace_effect == Some(ChatWorkspaceEffect::Settled)
                && stored.owner.workspace_verified_at.is_some(),
            ChatError::InvalidTransition,
        )?;
        require(
            stored.owner.version == input.expected_owner_version
                && stored.owner.profile_sha256 == input.profile_sha256
                && stored.owner.resource_key.as_ref() == Some(&input.resource_key)
                && stored.owner.base_commit == input.observed_commit,
            ChatError::Conflict,
        )?;
        check_generation(&row.session, input.expected_generation)?;
        require(
            row.fence.is_none() && row.session.active_request_id.is_none(),
            ChatError::OwnerAlreadyClaimed,
        )?;
        require(
            matches!(
                row.session.state,
                ChatState::Opening | ChatState::Suspended | ChatState::Closed
            ),
            ChatError::InvalidTransition,
        )?;
        check_detection(&transaction, row.session.harness)?;
        require(
            !dispatch_reserved(&transaction, &input.resource_key)?
                && !crate::chat::cleanup_is_reserved(&transaction, &input.resource_key)?,
            ChatError::Busy,
        )?;
        let occupied:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE resource_key=?1 AND chat_id!=?2 AND (owner_fence IS NOT NULL OR state='recovery_required') UNION ALL SELECT 1 FROM chat_turns WHERE resource_key=?1 AND phase IN('admitted','running','cancel_requested','recovery_required'))",
            params![input.resource_key,input.chat_id.to_string()],|row|row.get(0))?;
        require(!occupied, ChatError::Busy)?;
        let count:u64=transaction.query_row("SELECT COUNT(*) FROM chat_sessions WHERE owner_fence IS NOT NULL OR state='recovery_required'",[],|row|row.get(0))?;
        require(count < MAX_CHAT_PROCESSES, ChatError::Capacity)?;
        if let Some(known) = &row.session.vendor_session_id {
            require(
                input.expected_vendor_id.as_ref() == Some(known),
                ChatError::Conflict,
            )?;
        } else {
            let previously_claimed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_execution_attempts WHERE chat_id=? AND send_claimed=1)",
                [input.chat_id.to_string()],|row|row.get(0))?;
            require(!previously_claimed, ChatError::UnconfirmedSession)?;
        }
        let previous_version = row.session.version;
        let mut session = row.session;
        if session.state != ChatState::Opening {
            session.generation = next_version(session.generation)?;
        }
        session.state = ChatState::Opening;
        session.expected_vendor_id = input.expected_vendor_id.clone();
        session.vendor_confirmed_generation = None;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let fence = ChatOwnerFence(Uuid::new_v4());
        let (event, sequence) = attempts::execution_event(
            &transaction,
            &stored.owner,
            &session,
            kind,
            None,
            json!({"resume":session.vendor_session_id.is_some()}),
        )?;
        rows::save_session(
            &transaction,
            &session,
            Some(fence),
            previous_version,
            sequence,
        )?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            kind,
            &binding,
            &[],
            sequence,
        )?;
        let result = committed_session(
            &transaction,
            input.chat_id,
            Some(event),
            sequence as u64,
            false,
            Some(fence),
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn change_chat_execution_process(
        &mut self,
        input: &ChatMutation,
        fence: ChatOwnerFence,
        change: &ChatExecutionProcessChange,
    ) -> Result<ChatExecutionSessionCommit, StoreError> {
        let (kind, detail) = match change {
            ChatExecutionProcessChange::Ready => ("chat_execution_process_ready", json!(null)),
            ChatExecutionProcessChange::VendorBound { vendor_id } => {
                ("chat_execution_vendor_bound", json!(vendor_id))
            }
            ChatExecutionProcessChange::Released {
                cleanup_verified,
                closed,
            } => (
                "chat_execution_cleanup_observed",
                json!([cleanup_verified, closed]),
            ),
        };
        let binding = serde_json::to_string(&json!([
            input.chat_id,
            input.expected_generation,
            fence.0,
            detail
        ]))?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(receipt) = operation(&transaction, input.operation_id, kind, &binding)? {
            require(receipt.chat_id == input.chat_id, ChatError::Conflict)?;
            return committed_session(
                &transaction,
                input.chat_id,
                None,
                receipt.sequence,
                true,
                None,
            );
        }
        let stored = owners::required_owner(&transaction, input.chat_id)?;
        require(
            stored.owner.phase == ChatOwnerPhase::Open,
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, input.chat_id)?;
        check_owner(&row, input.expected_generation, fence)?;
        let old_version = row.session.version;
        let old_session = row.session.clone();
        let mut session = row.session;
        let mut process_fence = Some(fence);
        let mut settled_attempt = None;
        match change {
            ChatExecutionProcessChange::Ready => {
                require(
                    matches!(session.state, ChatState::Opening | ChatState::Ready),
                    ChatError::InvalidTransition,
                )?;
                let confirmed = session.vendor_session_id.is_some()
                    && session.vendor_confirmed_generation == Some(session.generation);
                require(
                    confirmed || permits_claude_bootstrap(&transaction, &session)?,
                    ChatError::UnconfirmedSession,
                )?;
                session.state = ChatState::Ready;
            }
            ChatExecutionProcessChange::VendorBound { vendor_id } => {
                require(valid_vendor_id(vendor_id), ChatError::InvalidInput)?;
                require(
                    matches!(
                        session.state,
                        ChatState::Opening | ChatState::Ready | ChatState::Busy
                    ),
                    ChatError::InvalidTransition,
                )?;
                for known in [&session.expected_vendor_id, &session.vendor_session_id]
                    .into_iter()
                    .flatten()
                {
                    require(known == vendor_id, ChatError::Conflict)?;
                }
                session.vendor_session_id = Some(vendor_id.clone());
                session.vendor_confirmed_generation = Some(session.generation);
            }
            ChatExecutionProcessChange::Released {
                cleanup_verified,
                closed,
            } => {
                settled_attempt = attempts::prepare_process_end(
                    &transaction,
                    &mut session,
                    fence,
                    *cleanup_verified,
                    input.timestamp,
                )?;
                session.vendor_confirmed_generation = None;
                session.state = if *cleanup_verified {
                    if *closed {
                        ChatState::Closed
                    } else {
                        ChatState::Suspended
                    }
                } else {
                    ChatState::RecoveryRequired
                };
                if *cleanup_verified {
                    process_fence = None;
                }
            }
        }
        let changed = session != old_session || process_fence != row.fence;
        if !changed {
            record_operation(
                &transaction,
                input.operation_id,
                &session,
                kind,
                &binding,
                &[],
                row.sequence as i64,
            )?;
            let result =
                committed_session(&transaction, input.chat_id, None, row.sequence, false, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        if let Some((attempt, _)) = &settled_attempt {
            session.updated_at = session.updated_at.max(attempt.attempt.updated_at);
        }
        let (event, sequence) = attempts::execution_event(
            &transaction,
            &stored.owner,
            &session,
            kind,
            None,
            json!({"cleanup_verified":matches!(change,ChatExecutionProcessChange::Released{cleanup_verified:true,..})}),
        )?;
        rows::save_session(&transaction, &session, process_fence, old_version, sequence)?;
        if let Some((attempt, previous_version)) = settled_attempt {
            attempt_rows::save(&transaction, &attempt, previous_version, sequence)?;
        }
        let task_status = if session.state == ChatState::RecoveryRequired {
            TaskStatus::Blocked
        } else {
            TaskStatus::InProgress
        };
        if stored.task.status() != task_status {
            let task = stored
                .task
                .transitioned(task_status)
                .map_err(|_| fail(ChatError::InvalidTransition))?;
            rows::save_task(&transaction, &task, sequence)?;
        }
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            kind,
            &binding,
            &[],
            sequence,
        )?;
        let result = committed_session(
            &transaction,
            input.chat_id,
            Some(event),
            sequence as u64,
            false,
            None,
        )?;
        transaction.commit()?;
        Ok(result)
    }
}

impl SqliteStore {
    /// Startup never grants another process claim for an unfinished generation.
    /// Each chat's state, attempt and event are fenced in its own transaction.
    pub fn recover_chat_execution_processes(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<ChatExecutionRecovery, StoreError> {
        let ids = {
            let mut statement=self.connection.prepare("SELECT s.chat_id FROM chat_sessions s JOIN chat_execution_owners o ON o.chat_id=s.chat_id WHERE s.owner_fence IS NOT NULL AND s.state!='recovery_required' ORDER BY s.updated_sequence LIMIT ?")?;
            statement
                .query_map([MAX_CHAT_SESSIONS + 1], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        require(
            ids.len() as u64 <= MAX_CHAT_SESSIONS,
            ChatError::ProjectionMismatch,
        )?;
        let mut result = ChatExecutionRecovery::default();
        for raw_id in ids {
            let id = raw_id.parse::<ChatId>().ok();
            let recovered = (|| {
                let id = id.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
                let stored = rows::existing_session(&self.connection, id)?;
                let fence = stored
                    .fence
                    .ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
                self.change_chat_execution_process(
                    &ChatMutation {
                        operation_id: Uuid::new_v4(),
                        chat_id: id,
                        expected_generation: stored.session.generation,
                        timestamp,
                    },
                    fence,
                    &ChatExecutionProcessChange::Released {
                        cleanup_verified: false,
                        closed: false,
                    },
                )
            })();
            match recovered {
                Ok(recovered) => result.sessions.push(recovered),
                Err(error) if matches!(error, StoreError::Chat(_) | StoreError::Json(_)) => {
                    result.quarantined.push(QuarantinedChat {
                        chat_id: id,
                        code: error.code(),
                    })
                }
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
}
