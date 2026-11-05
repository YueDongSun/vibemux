//! Idempotent session metadata, process fencing and explicit lifecycle operations.
use super::*;
use rusqlite::TransactionBehavior;

pub(super) fn new_session(project_id: vibemux_types::ProjectId, input: &ChatOpen) -> ChatSession {
    ChatSession {
        schema_version: CHAT_SCHEMA_VERSION,
        chat_id: ChatId::new(),
        project_id,
        harness: input.harness,
        profile_sha256: input.profile_sha256.clone(),
        resource_key: input.resource_key.clone(),
        base_commit: input.base_commit.clone(),
        state: ChatState::Opening,
        generation: 1,
        version: 1,
        expected_vendor_id: None,
        vendor_session_id: None,
        vendor_confirmed_generation: None,
        title_ref: None,
        retained_bytes: 0,
        record_count: 0,
        last_record_sequence: 0,
        content_purged: false,
        active_request_id: None,
        created_at: input.timestamp,
        updated_at: input.timestamp,
    }
}

impl SqliteStore {
    pub fn open_chat(&mut self, input: &ChatOpen) -> Result<ChatSessionCommit, StoreError> {
        require(
            !input.operation_id.is_nil()
                && valid_digest(&input.profile_sha256)
                && valid_digest(&input.resource_key)
                && matches!(input.base_commit.len(), 40 | 64)
                && input.base_commit.bytes().all(|b| b.is_ascii_hexdigit()),
            ChatError::InvalidInput,
        )?;
        if let Some(reference) = &input.title_ref {
            reference.validate(MAX_CHAT_TITLE_BYTES).map_err(fail)?;
        }
        let binding = serde_json::to_string(&json!([
            input.harness,
            input.profile_sha256,
            input.resource_key,
            input.base_commit,
            input.title_ref
        ]))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(row) = operation(&transaction, input.operation_id, "open", &binding)? {
            return operation_commit(&transaction, row);
        }
        let count: u64 =
            transaction.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?;
        require(count < MAX_CHAT_SESSIONS, ChatError::Capacity)?;
        let project_id = crate::resolve_or_mint_project_id(&transaction)?;
        let mut session = new_session(project_id, input);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_opened",
            input.timestamp,
            event_data(&session),
        )?;
        rows::insert_session(&transaction, &session, sequence)?;
        if let Some(reference) = &input.title_ref {
            claim_content(&transaction, &mut session, reference, MAX_CHAT_TITLE_BYTES)?;
            session.title_ref = Some(reference.clone());
            rows::save_session(&transaction, &session, None, 1, sequence)?;
        }
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "open",
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatSessionCommit {
            session,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            released_content: Vec::new(),
        })
    }

    pub fn claim_chat_owner(
        &mut self,
        chat_id: ChatId,
        generation: u64,
        expected_vendor_id: Option<String>,
        timestamp: OffsetDateTime,
    ) -> Result<ChatOwnerClaim, StoreError> {
        if let Some(id) = &expected_vendor_id {
            require(valid_vendor_id(id), ChatError::InvalidInput)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, chat_id)?;
        let stored = rows::existing_session(&transaction, chat_id)?;
        check_generation(&stored.session, generation)?;
        require(stored.fence.is_none(), ChatError::OwnerAlreadyClaimed)?;
        require(
            stored.session.state == ChatState::Opening,
            ChatError::InvalidTransition,
        )?;
        check_detection(&transaction, stored.session.harness)?;
        require(
            !dispatch_reserved(&transaction, &stored.session.resource_key)?
                && !is_reserved(&transaction, &stored.session.resource_key)?,
            ChatError::Busy,
        )?;
        let count: u64 = transaction.query_row(
            "SELECT COUNT(*) FROM chat_sessions WHERE owner_fence IS NOT NULL OR state='recovery_required'",
            [],
            |row| row.get(0),
        )?;
        require(count < MAX_CHAT_PROCESSES, ChatError::Capacity)?;
        if let Some(known) = &stored.session.vendor_session_id {
            require(
                expected_vendor_id.as_ref() == Some(known),
                ChatError::Conflict,
            )?;
        }
        let previous_version = stored.session.version;
        let mut session = stored.session;
        session.expected_vendor_id = expected_vendor_id;
        session.vendor_confirmed_generation = None;
        session.version = next_version(session.version)?;
        session.updated_at = timestamp.max(session.updated_at);
        let fence = ChatOwnerFence(Uuid::new_v4());
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_owner_claimed",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(
            &transaction,
            &session,
            Some(fence),
            previous_version,
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatOwnerClaim {
            session,
            fence,
            event,
        })
    }

    pub fn mark_chat_transport_ready(
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
        if stored.session.state == ChatState::Ready {
            return Ok(unchanged(stored));
        }
        require(
            stored.session.state == ChatState::Opening,
            ChatError::InvalidTransition,
        )?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        session.state = ChatState::Ready;
        session.version = next_version(session.version)?;
        session.updated_at = timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_transport_ready",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(
            &transaction,
            &session,
            Some(fence),
            previous_version,
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, Vec::new()))
    }

    pub fn bind_chat_vendor(
        &mut self,
        chat_id: ChatId,
        generation: u64,
        fence: ChatOwnerFence,
        vendor_id: &str,
        timestamp: OffsetDateTime,
    ) -> Result<ChatSessionCommit, StoreError> {
        require(valid_vendor_id(vendor_id), ChatError::InvalidInput)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, chat_id)?;
        let stored = rows::existing_session(&transaction, chat_id)?;
        check_owner(&stored, generation, fence)?;
        require(
            matches!(
                stored.session.state,
                ChatState::Opening | ChatState::Ready | ChatState::Busy
            ),
            ChatError::InvalidTransition,
        )?;
        for expected in [
            &stored.session.expected_vendor_id,
            &stored.session.vendor_session_id,
        ]
        .into_iter()
        .flatten()
        {
            require(expected == vendor_id, ChatError::Conflict)?;
        }
        if stored.session.vendor_confirmed_generation == Some(generation) {
            return Ok(unchanged(stored));
        }
        let previous_version = stored.session.version;
        let mut session = stored.session;
        session.vendor_session_id = Some(vendor_id.to_owned());
        session.vendor_confirmed_generation = Some(generation);
        if session.state == ChatState::Opening {
            session.state = ChatState::Ready;
        }
        session.version = next_version(session.version)?;
        session.updated_at = timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_vendor_bound",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(
            &transaction,
            &session,
            Some(fence),
            previous_version,
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, Vec::new()))
    }

    /// Requires the supervisor's verified cleanup result, never a renderer boolean.
    pub fn release_chat_owner(
        &mut self,
        input: &ChatMutation,
        fence: ChatOwnerFence,
        cleanup_verified: bool,
        close: bool,
    ) -> Result<ChatSessionCommit, StoreError> {
        require(cleanup_verified, ChatError::InvalidInput)?;
        let binding = mutation_binding(input, json!([close, cleanup_verified]))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(&transaction, input.operation_id, "release", &binding)? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_owner(&stored, input.expected_generation, fence)?;
        require(stored.session.active_request_id.is_none(), ChatError::Busy)?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        session.state = if close {
            ChatState::Closed
        } else {
            ChatState::Suspended
        };
        session.vendor_confirmed_generation = None;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_owner_released",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(&transaction, &session, None, previous_version, sequence)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "release",
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, Vec::new()))
    }

    /// Bind an explicit close before the service stops its owned process.
    /// Replays observe this receipt and cannot close a later generation.
    pub fn request_chat_close(
        &mut self,
        input: &ChatMutation,
    ) -> Result<ChatSessionCommit, StoreError> {
        let binding = mutation_binding(input, Value::Null)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(&transaction, input.operation_id, "close", &binding)? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            stored.fence.is_some()
                && matches!(
                    stored.session.state,
                    ChatState::Opening | ChatState::Ready | ChatState::Busy
                ),
            ChatError::InvalidTransition,
        )?;
        let previous = stored.session.version;
        let mut session = stored.session;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_close_requested",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(&transaction, &session, stored.fence, previous, sequence)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "close",
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, Vec::new()))
    }

    /// Closes metadata that has no live/unknown process owner or active turn.
    pub fn close_chat(&mut self, input: &ChatMutation) -> Result<ChatSessionCommit, StoreError> {
        self.mutate_quiescent_chat(input, QuiescentAction::Close)
    }
    pub fn archive_chat(&mut self, input: &ChatMutation) -> Result<ChatSessionCommit, StoreError> {
        self.mutate_quiescent_chat(input, QuiescentAction::Archive)
    }
    pub fn resume_chat(&mut self, input: &ChatMutation) -> Result<ChatSessionCommit, StoreError> {
        self.mutate_quiescent_chat(input, QuiescentAction::Resume)
    }

    fn mutate_quiescent_chat(
        &mut self,
        input: &ChatMutation,
        action: QuiescentAction,
    ) -> Result<ChatSessionCommit, StoreError> {
        let binding = mutation_binding(input, Value::Null)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(&transaction, input.operation_id, action.name(), &binding)? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            stored.fence.is_none() && stored.session.active_request_id.is_none(),
            ChatError::Busy,
        )?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        match action {
            QuiescentAction::Close => {
                require(
                    matches!(
                        session.state,
                        ChatState::Opening | ChatState::Suspended | ChatState::Closed
                    ),
                    ChatError::InvalidTransition,
                )?;
                session.state = ChatState::Closed;
            }
            QuiescentAction::Archive => {
                require(
                    matches!(
                        session.state,
                        ChatState::Suspended | ChatState::Closed | ChatState::Archived
                    ),
                    ChatError::InvalidTransition,
                )?;
                session.state = ChatState::Archived;
            }
            QuiescentAction::Resume => {
                require(
                    matches!(session.state, ChatState::Suspended | ChatState::Closed)
                        && !session.content_purged,
                    ChatError::InvalidTransition,
                )?;
                require(
                    session.vendor_session_id.is_some(),
                    ChatError::UnconfirmedSession,
                )?;
                session.generation = next_version(session.generation)?;
                session.expected_vendor_id = session.vendor_session_id.clone();
                session.state = ChatState::Opening;
            }
        }
        session.vendor_confirmed_generation = None;
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            action.event(),
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(&transaction, &session, None, previous_version, sequence)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            action.name(),
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, Vec::new()))
    }

    pub fn rename_chat(
        &mut self,
        input: &ChatMutation,
        title_ref: ContentRef,
    ) -> Result<ChatSessionCommit, StoreError> {
        title_ref.validate(MAX_CHAT_TITLE_BYTES).map_err(fail)?;
        let binding = mutation_binding(input, serde_json::to_value(&title_ref)?)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        owners::require_read_only(&transaction, input.chat_id)?;
        if let Some(row) = operation(&transaction, input.operation_id, "rename", &binding)? {
            return operation_commit(&transaction, row);
        }
        let stored = rows::existing_session(&transaction, input.chat_id)?;
        check_generation(&stored.session, input.expected_generation)?;
        require(
            !stored.session.content_purged,
            ChatError::ContentUnavailable,
        )?;
        let previous_version = stored.session.version;
        let mut session = stored.session;
        let mut released = Vec::new();
        if session.title_ref.as_ref() != Some(&title_ref) {
            if let Some(old) = session.title_ref.take() {
                release_content(&transaction, &mut session, &old)?;
                released.push(old);
            }
            claim_content(&transaction, &mut session, &title_ref, MAX_CHAT_TITLE_BYTES)?;
            session.title_ref = Some(title_ref);
        }
        session.version = next_version(session.version)?;
        session.updated_at = input.timestamp.max(session.updated_at);
        let (event, sequence) = emit(
            &transaction,
            &session,
            None,
            "chat_title_changed",
            session.updated_at,
            event_data(&session),
        )?;
        rows::save_session(
            &transaction,
            &session,
            stored.fence,
            previous_version,
            sequence,
        )?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "rename",
            &binding,
            &released,
            sequence,
        )?;
        transaction.commit()?;
        Ok(committed(session, event, sequence, released))
    }
}

enum QuiescentAction {
    Close,
    Archive,
    Resume,
}
impl QuiescentAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Archive => "archive",
            Self::Resume => "resume",
        }
    }
    fn event(&self) -> &'static str {
        match self {
            Self::Close => "chat_closed",
            Self::Archive => "chat_archived",
            Self::Resume => "chat_resume_requested",
        }
    }
}
pub(super) fn mutation_binding(input: &ChatMutation, extra: Value) -> Result<String, StoreError> {
    require(!input.operation_id.is_nil(), ChatError::InvalidInput)?;
    valid_generation(input.expected_generation)?;
    Ok(serde_json::to_string(&json!([
        input.chat_id,
        input.expected_generation,
        extra
    ]))?)
}
pub(super) fn unchanged(stored: rows::StoredSession) -> ChatSessionCommit {
    ChatSessionCommit {
        session: stored.session,
        event: None,
        sequence: stored.sequence,
        duplicate: true,
        released_content: Vec::new(),
    }
}
pub(super) fn committed(
    session: ChatSession,
    event: EventEnvelope,
    sequence: i64,
    released_content: Vec<ContentRef>,
) -> ChatSessionCommit {
    ChatSessionCommit {
        session,
        event: Some(event),
        sequence: sequence as u64,
        duplicate: false,
        released_content,
    }
}
