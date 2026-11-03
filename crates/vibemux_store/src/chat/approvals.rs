//! Atomic approval decisions and one-use response authority. No vendor I/O here.
use super::approval_rows::{self, ApprovalBinding, DecisionBinding, StoredApproval};
use super::*;
use vibemux_types::{
    ChatApprovalId, ToolName,
    chat_approval::*,
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
};

pub(crate) const MIGRATION_V13: &str = r#"
CREATE TABLE chat_approvals(
 approval_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 request_id TEXT NOT NULL REFERENCES chat_execution_attempts(request_id),
 generation INTEGER NOT NULL CHECK(generation>0),version INTEGER NOT NULL CHECK(version>0),
 vendor_request_key TEXT NOT NULL,tool_use_key TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('pending','decided','expired','invalidated')),
 delivery TEXT CHECK(delivery IN('claimed','written','unknown')),claim_id TEXT UNIQUE,
 decision_operation_id TEXT UNIQUE REFERENCES chat_operations(operation_id),
 binding_json TEXT NOT NULL,decision_json TEXT,record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
 UNIQUE(chat_id,generation,vendor_request_key),UNIQUE(chat_id,generation,tool_use_key),
 CHECK((state='decided' AND delivery IS NOT NULL AND claim_id IS NOT NULL AND decision_operation_id IS NOT NULL AND decision_json IS NOT NULL)
 OR (state!='decided' AND delivery IS NULL AND claim_id IS NULL AND decision_operation_id IS NULL AND decision_json IS NULL))
);
CREATE UNIQUE INDEX chat_approval_pending ON chat_approvals(request_id)
 WHERE state='pending' OR delivery IN('claimed','unknown');
INSERT INTO schema_migrations(version,applied_at) VALUES(13,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

#[derive(Clone)]
pub struct ChatApprovalAdmission {
    pub request_id: Uuid,
    pub generation: u64,
    pub fence: ChatOwnerFence,
    /// Digests of exact vendor identities, computed by the trusted adapter.
    pub vendor_request_key: String,
    pub tool_use_key: String,
    pub tool: ToolName,
    pub evidence_ref: ContentRef,
    pub expires_at: OffsetDateTime,
    pub timestamp: OffsetDateTime,
}
impl fmt::Debug for ChatApprovalAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatApprovalAdmission")
            .field("request_id", &self.request_id)
            .field("generation", &self.generation)
            .field("tool", &self.tool)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub struct ChatApprovalDecisionInput {
    pub operation_id: Uuid,
    pub approval_id: ChatApprovalId,
    pub expected_version: u64,
    pub generation: u64,
    pub fence: ChatOwnerFence,
    pub decision: ChatApprovalDecision,
    pub observed_commit: String,
    pub profile_sha256: String,
    pub timestamp: OffsetDateTime,
}
/// Nonserializable authority returned only by a fresh committed decision.
/// Reopening the database or replaying an operation cannot reconstruct it.
pub struct ChatApprovalDeliveryClaim {
    pub(super) approval_id: ChatApprovalId,
    pub(super) claim_id: Uuid,
    pub(super) fence: ChatOwnerFence,
}
impl ChatApprovalDeliveryClaim {
    pub fn approval_id(&self) -> ChatApprovalId {
        self.approval_id
    }
}
impl fmt::Debug for ChatApprovalDeliveryClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChatApprovalDeliveryClaim(<redacted>)")
    }
}
#[derive(Debug)]
pub struct ChatApprovalCommit {
    pub approval: ChatApproval,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
    pub delivery_claim: Option<ChatApprovalDeliveryClaim>,
}
pub(super) fn receipt(stored: StoredApproval, duplicate: bool) -> ChatApprovalCommit {
    ChatApprovalCommit {
        approval: stored.approval,
        event: None,
        sequence: stored.sequence,
        duplicate,
        delivery_claim: None,
    }
}

pub(super) fn require_live(
    connection: &Connection,
    request_id: Uuid,
    generation: u64,
    fence: ChatOwnerFence,
    tool: ToolName,
) -> Result<
    (
        attempt_rows::StoredAttempt,
        owners::ChatExecutionOwnerCommit,
        rows::StoredSession,
    ),
    StoreError,
> {
    let attempt = attempt_rows::required_attempt(connection, request_id)?;
    let owner = owners::required_owner(connection, attempt.attempt.chat_id)?;
    let row = rows::existing_session(connection, attempt.attempt.chat_id)?;
    check_owner(&row, generation, fence)?;
    require(
        attempt.fence == Some(fence) && attempt.attempt.generation == generation,
        ChatError::StaleFence,
    )?;
    require(
        attempt.attempt.phase == ChatTurnPhase::Running
            && row.session.state == ChatState::Busy
            && row.session.active_request_id == Some(request_id)
            && attempt.attempt.vendor_turn_id.is_some()
            && row.session.vendor_confirmed_generation == Some(generation),
        ChatError::InvalidTransition,
    )?;
    require(
        owner.owner.phase == ChatOwnerPhase::Open
            && row.session.harness == ChatHarness::Claude
            && tool != ToolName::RunDevTests
            && owner.owner.workspace_effect == Some(ChatWorkspaceEffect::Settled)
            && owner.owner.workspace_verified_at.is_some()
            && attempt.attempt.effective_tools.contains(&tool)
            && owner.owner.admitted_tools.contains(&tool),
        ChatError::Conflict,
    )?;
    Ok((attempt, owner, row))
}

impl SqliteStore {
    /// Read-only admission lookup for publish-before-metadata writers. This
    /// cannot create a challenge or return response authority.
    pub fn replay_chat_approval_admission(
        &self,
        input: &ChatApprovalAdmission,
    ) -> Result<Option<ChatApprovalCommit>, StoreError> {
        admission_replay(&self.connection, input)
    }
    /// Verify private vendor identities against the immutable ledger binding.
    /// The raw identities stay in the daemon's protected evidence body.
    pub fn validate_chat_approval_evidence(
        &self,
        id: ChatApprovalId,
        vendor_request_key: &str,
        tool_use_key: &str,
    ) -> Result<(), StoreError> {
        let stored = approval_rows::required(&self.connection, id)?;
        require(
            stored.binding.vendor_request_key == vendor_request_key
                && stored.binding.tool_use_key == tool_use_key,
            ChatError::ProjectionMismatch,
        )
    }
    pub fn chat_approval(&self, id: ChatApprovalId) -> Result<Option<ChatApproval>, StoreError> {
        Ok(approval_rows::load(&self.connection, id)?.map(|s| s.approval))
    }
    pub fn chat_approvals(
        &self,
        chat_id: ChatId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ChatApproval>, StoreError> {
        require(
            limit > 0 && limit <= MAX_CHAT_PAGE && after_sequence <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        owners::required_owner(&self.connection, chat_id)?;
        let mut statement = self.connection.prepare("SELECT approval_id FROM chat_approvals WHERE chat_id=? AND admitted_sequence>? ORDER BY admitted_sequence LIMIT ?")?;
        let ids = statement
            .query_map(params![chat_id.to_string(), after_sequence, limit], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                approval_rows::required(
                    &self.connection,
                    id.parse()
                        .map_err(|_| fail(ChatError::ProjectionMismatch))?,
                )
                .map(|s| s.approval)
            })
            .collect()
    }
    pub fn admit_chat_approval(
        &mut self,
        input: &ChatApprovalAdmission,
    ) -> Result<ChatApprovalCommit, StoreError> {
        input
            .evidence_ref
            .validate(MAX_CHAT_RECORD_BYTES)
            .map_err(fail)?;
        require(
            valid_digest(&input.vendor_request_key) && valid_digest(&input.tool_use_key),
            ChatError::InvalidInput,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(receipt) = admission_replay(&transaction, input)? {
            return Ok(receipt);
        }
        let binding = ApprovalBinding::from(input);
        let (attempt, owner, row) = require_live(
            &transaction,
            input.request_id,
            input.generation,
            input.fence,
            input.tool,
        )?;
        let count: u64 = transaction.query_row(
            "SELECT COUNT(*) FROM chat_approvals WHERE chat_id=?",
            [row.session.chat_id.to_string()],
            |r| r.get(0),
        )?;
        require(count < MAX_CHAT_RECORDS, ChatError::Capacity)?;
        let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_approvals WHERE request_id=? AND (state='pending' OR delivery IN('claimed','unknown')))", [input.request_id.to_string()], |r| r.get(0))?;
        require(!active, ChatError::Busy)?;
        let reused: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chat_approvals WHERE chat_id=? AND generation=? AND tool_use_key=?)",
            params![row.session.chat_id.to_string(), input.generation, input.tool_use_key], |r| r.get(0))?;
        require(!reused, ChatError::Conflict)?;
        require(
            approval_rows::latest_timestamp(&transaction, row.session.chat_id)?
                .is_none_or(|latest| input.timestamp >= latest),
            ChatError::InvalidInput,
        )?;
        let previous = row.session.version;
        let mut session = row.session;
        require(
            input.timestamp >= session.updated_at && input.timestamp >= attempt.attempt.updated_at,
            ChatError::InvalidInput,
        )?;
        let timestamp = input
            .timestamp
            .max(session.updated_at)
            .max(attempt.attempt.updated_at);
        require(
            input.expires_at > timestamp
                && input.expires_at - timestamp
                    <= time::Duration::seconds(MAX_CHAT_APPROVAL_SECONDS),
            ChatError::InvalidInput,
        )?;
        claim_content(
            &transaction,
            &mut session,
            &input.evidence_ref,
            MAX_CHAT_RECORD_BYTES,
        )?;
        session.updated_at = timestamp;
        session.version = next_version(previous)?;
        let approval_id = ChatApprovalId::new();
        let (event, sequence) = attempts::execution_event(
            &transaction,
            &owner.owner,
            &session,
            "chat_approval_admitted",
            Some(input.request_id),
            json!({"approval_id":approval_id,"tool":input.tool,"bytes":input.evidence_ref.byte_count}),
        )?;
        let approval = ChatApproval {
            schema_version: CHAT_APPROVAL_SCHEMA_VERSION,
            approval_id,
            chat_id: session.chat_id,
            attempt_id: attempt.attempt.attempt_id,
            request_id: input.request_id,
            project_id: owner.owner.project_id,
            owner_task_id: owner.owner.task_id,
            owner_run_id: owner.owner.run_id,
            generation: input.generation,
            version: 1,
            admitted_sequence: sequence as u64,
            tool: input.tool,
            profile_sha256: owner.owner.profile_sha256,
            evidence_ref: input.evidence_ref.clone(),
            state: ChatApprovalState::Pending,
            decision: None,
            decision_operation_id: None,
            delivery: None,
            expires_at: input.expires_at,
            created_at: timestamp,
            updated_at: timestamp,
        };
        approval.validate().map_err(fail)?;
        transaction.execute("INSERT INTO chat_approvals(approval_id,chat_id,request_id,generation,version,vendor_request_key,tool_use_key,state,binding_json,record_json,admitted_sequence,updated_sequence) VALUES(?,?,?,?,?,?,?,'pending',?,?,?,?)",
            params![approval_id.to_string(),approval.chat_id.to_string(),input.request_id.to_string(),input.generation,1,input.vendor_request_key,input.tool_use_key,serde_json::to_string(&binding)?,serde_json::to_string(&approval)?,sequence,sequence])?;
        rows::save_session(&transaction, &session, row.fence, previous, sequence)?;
        transaction.commit()?;
        Ok(ChatApprovalCommit {
            approval,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            delivery_claim: None,
        })
    }
    pub fn decide_chat_approval(
        &mut self,
        input: &ChatApprovalDecisionInput,
    ) -> Result<ChatApprovalCommit, StoreError> {
        let binding = DecisionBinding::from(input);
        let encoded = serde_json::to_string(&binding)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = approval_rows::required(&transaction, input.approval_id)?;
        if let Some(operation) = operation(
            &transaction,
            input.operation_id,
            "chat_approval_decided",
            &encoded,
        )? {
            require(
                operation.chat_id == stored.approval.chat_id
                    && stored.approval.decision_operation_id == Some(input.operation_id)
                    && stored.decision.as_ref() == Some(&binding),
                ChatError::Conflict,
            )?;
            let mut result = receipt(stored, true);
            result.sequence = operation.sequence;
            return Ok(result);
        }
        require(
            stored.approval.state == ChatApprovalState::Pending
                && stored.approval.version == input.expected_version,
            ChatError::Conflict,
        )?;
        let (attempt, owner, row) = require_live(
            &transaction,
            stored.approval.request_id,
            input.generation,
            input.fence,
            stored.approval.tool,
        )?;
        require(
            stored.binding.fence == input.fence.0
                && stored.approval.generation == input.generation
                && input.observed_commit == owner.owner.base_commit
                && input.profile_sha256 == owner.owner.profile_sha256
                && stored.approval.profile_sha256 == input.profile_sha256,
            ChatError::Conflict,
        )?;
        require(
            input.timestamp >= row.session.updated_at
                && input.timestamp >= stored.approval.updated_at
                && input.timestamp >= attempt.attempt.updated_at,
            ChatError::InvalidInput,
        )?;
        let timestamp = input.timestamp;
        require(
            approval_rows::latest_timestamp(&transaction, stored.approval.chat_id)?
                .is_none_or(|latest| timestamp >= latest),
            ChatError::InvalidInput,
        )?;
        require(
            timestamp < stored.approval.expires_at,
            ChatError::InvalidTransition,
        )?;
        stored.approval.state = ChatApprovalState::Decided;
        stored.approval.decision = Some(input.decision);
        stored.approval.decision_operation_id = Some(input.operation_id);
        stored.approval.delivery = Some(ChatApprovalDelivery::Claimed);
        let claim_id = Uuid::new_v4();
        stored.claim_id = Some(claim_id);
        stored.decision = Some(binding);
        // The operation and outbox claim become visible in the same transaction.
        let (event, sequence) = approval_rows::change_event(
            &transaction,
            &mut stored,
            timestamp,
            "chat_approval_decided",
        )?;
        record_operation(
            &transaction,
            input.operation_id,
            &row.session,
            "chat_approval_decided",
            &encoded,
            &[],
            sequence,
        )?;
        approval_rows::save(&transaction, &stored, sequence)?;
        transaction.commit()?;
        Ok(ChatApprovalCommit {
            approval: stored.approval,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            delivery_claim: Some(ChatApprovalDeliveryClaim {
                approval_id: input.approval_id,
                claim_id,
                fence: input.fence,
            }),
        })
    }
    pub fn complete_chat_approval_delivery(
        &mut self,
        claim: &ChatApprovalDeliveryClaim,
        outcome: ChatApprovalDelivery,
        timestamp: OffsetDateTime,
    ) -> Result<ChatApprovalCommit, StoreError> {
        require(
            outcome != ChatApprovalDelivery::Claimed,
            ChatError::InvalidInput,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = approval_rows::required(&transaction, claim.approval_id)?;
        require(
            stored.claim_id == Some(claim.claim_id) && stored.binding.fence == claim.fence.0,
            ChatError::StaleFence,
        )?;
        if stored.approval.delivery == Some(outcome) {
            return Ok(receipt(stored, true));
        }
        require(
            stored.approval.delivery == Some(ChatApprovalDelivery::Claimed),
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, stored.approval.chat_id)?;
        check_owner(&row, stored.approval.generation, claim.fence)?;
        let attempt = attempt_rows::required_attempt(&transaction, stored.approval.request_id)?;
        require(
            timestamp >= stored.approval.updated_at
                && timestamp >= row.session.updated_at
                && timestamp >= attempt.attempt.updated_at
                && approval_rows::latest_timestamp(&transaction, stored.approval.chat_id)?
                    .is_none_or(|latest| timestamp >= latest),
            ChatError::InvalidInput,
        )?;
        stored.approval.delivery = Some(outcome);
        let (event, sequence) = approval_rows::change_event(
            &transaction,
            &mut stored,
            timestamp,
            "chat_approval_delivery_observed",
        )?;
        approval_rows::save(&transaction, &stored, sequence)?;
        transaction.commit()?;
        Ok(ChatApprovalCommit {
            approval: stored.approval,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            delivery_claim: None,
        })
    }
}

fn admission_replay(
    connection: &Connection,
    input: &ChatApprovalAdmission,
) -> Result<Option<ChatApprovalCommit>, StoreError> {
    input
        .evidence_ref
        .validate(MAX_CHAT_RECORD_BYTES)
        .map_err(fail)?;
    valid_generation(input.generation)?;
    require(
        valid_digest(&input.vendor_request_key) && valid_digest(&input.tool_use_key),
        ChatError::InvalidInput,
    )?;
    let attempt = attempt_rows::required_attempt(connection, input.request_id)?;
    let existing: Option<String> = connection.query_row(
        "SELECT approval_id FROM chat_approvals WHERE chat_id=? AND generation=? AND vendor_request_key=?",
        params![attempt.attempt.chat_id.to_string(), input.generation, input.vendor_request_key], |r| r.get(0)).optional()?;
    existing
        .map(|id| {
            let stored = approval_rows::required(
                connection,
                id.parse()
                    .map_err(|_| fail(ChatError::ProjectionMismatch))?,
            )?;
            require(
                stored.binding == ApprovalBinding::from(input),
                ChatError::Conflict,
            )?;
            Ok(receipt(stored, true))
        })
        .transpose()
}
