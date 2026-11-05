//! Bounded approval loads validate immutable admission and operation bindings.
use super::approvals::{ChatApprovalAdmission, ChatApprovalDecisionInput};
use super::*;
use vibemux_types::{ChatApprovalId, ToolName, chat_approval::*};

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct ApprovalBinding {
    pub request_id: Uuid,
    pub generation: u64,
    pub fence: Uuid,
    pub vendor_request_key: String,
    pub tool_use_key: String,
    pub tool: ToolName,
    pub evidence_ref: ContentRef,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}
impl From<&ChatApprovalAdmission> for ApprovalBinding {
    fn from(v: &ChatApprovalAdmission) -> Self {
        Self {
            request_id: v.request_id,
            generation: v.generation,
            fence: v.fence.0,
            vendor_request_key: v.vendor_request_key.clone(),
            tool_use_key: v.tool_use_key.clone(),
            tool: v.tool,
            evidence_ref: v.evidence_ref.clone(),
            expires_at: v.expires_at,
        }
    }
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct DecisionBinding {
    pub approval_id: ChatApprovalId,
    pub expected_version: u64,
    pub generation: u64,
    pub fence: Uuid,
    pub decision: ChatApprovalDecision,
    pub observed_commit: String,
    pub profile_sha256: String,
}
impl From<&ChatApprovalDecisionInput> for DecisionBinding {
    fn from(v: &ChatApprovalDecisionInput) -> Self {
        Self {
            approval_id: v.approval_id,
            expected_version: v.expected_version,
            generation: v.generation,
            fence: v.fence.0,
            decision: v.decision,
            observed_commit: v.observed_commit.clone(),
            profile_sha256: v.profile_sha256.clone(),
        }
    }
}
pub(super) struct StoredApproval {
    pub approval: ChatApproval,
    pub binding: ApprovalBinding,
    pub decision: Option<DecisionBinding>,
    pub claim_id: Option<Uuid>,
    pub sequence: u64,
}
struct ApprovalRow {
    chat_id: String,
    request_id: String,
    generation: u64,
    version: u64,
    vendor_request_key: String,
    tool_use_key: String,
    state: String,
    delivery: Option<String>,
    claim_id: Option<String>,
    operation_id: Option<String>,
    binding: Option<String>,
    decision: Option<String>,
    encoded: Option<String>,
    admitted_sequence: u64,
    sequence: u64,
    decision_valid: bool,
}
pub(super) fn load(
    connection: &Connection,
    id: ChatApprovalId,
) -> Result<Option<StoredApproval>, StoreError> {
    let raw = connection.query_row("SELECT chat_id,request_id,generation,version,
        CASE WHEN length(CAST(vendor_request_key AS BLOB))=64 THEN vendor_request_key ELSE '' END,
        CASE WHEN length(CAST(tool_use_key AS BLOB))=64 THEN tool_use_key ELSE '' END,state,delivery,claim_id,decision_operation_id,
        CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,
        CASE WHEN length(CAST(decision_json AS BLOB))<=16384 THEN decision_json END,
        CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,
        admitted_sequence,updated_sequence,(decision_json IS NULL OR length(CAST(decision_json AS BLOB))<=16384)
        FROM chat_approvals WHERE approval_id=?", [id.to_string()], |r| Ok(ApprovalRow {
            chat_id:r.get(0)?,request_id:r.get(1)?,generation:r.get(2)?,version:r.get(3)?,
            vendor_request_key:r.get(4)?,tool_use_key:r.get(5)?,state:r.get(6)?,delivery:r.get(7)?,
            claim_id:r.get(8)?,operation_id:r.get(9)?,binding:r.get(10)?,decision:r.get(11)?,encoded:r.get(12)?,
            admitted_sequence:r.get(13)?,sequence:r.get(14)?,decision_valid:r.get(15)?,
        })).optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let approval: ChatApproval = rows::decode(raw.encoded)?;
    approval
        .validate()
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let binding: ApprovalBinding = rows::decode(raw.binding)?;
    let decision: Option<DecisionBinding> =
        raw.decision.map(|s| rows::decode(Some(s))).transpose()?;
    let claim_id = raw
        .claim_id
        .map(|s| {
            Uuid::parse_str(&s)
                .ok()
                .filter(|v| !v.is_nil())
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
        })
        .transpose()?;
    let attempt = attempt_rows::required_attempt(connection, approval.request_id)?;
    let owner = owners::required_owner(connection, approval.chat_id)?;
    require(
        approval.approval_id == id
            && approval.chat_id.to_string() == raw.chat_id
            && approval.request_id.to_string() == raw.request_id
            && approval.generation == raw.generation
            && approval.version == raw.version
            && approval.state.as_str() == raw.state
            && approval.delivery.map(|v| v.as_str().to_owned()) == raw.delivery
            && approval.decision_operation_id.map(|v| v.to_string()) == raw.operation_id
            && approval.admitted_sequence == raw.admitted_sequence
            && raw.sequence >= raw.admitted_sequence
            && raw.decision_valid
            && binding.request_id == approval.request_id
            && binding.generation == approval.generation
            && Some(binding.fence) == attempt.fence.map(|f| f.0)
            && valid_digest(&binding.vendor_request_key)
            && valid_digest(&binding.tool_use_key)
            && binding.vendor_request_key == raw.vendor_request_key
            && binding.tool_use_key == raw.tool_use_key
            && binding.tool == approval.tool
            && binding.evidence_ref == approval.evidence_ref
            && binding.expires_at == approval.expires_at
            && attempt.attempt.chat_id == approval.chat_id
            && attempt.attempt.attempt_id == approval.attempt_id
            && attempt.attempt.generation == approval.generation
            && attempt.attempt.effective_tools.contains(&approval.tool)
            && approval.project_id == owner.owner.project_id
            && approval.owner_task_id == owner.owner.task_id
            && approval.owner_run_id == owner.owner.run_id
            && approval.profile_sha256 == owner.owner.profile_sha256
            && approval.created_at >= attempt.attempt.created_at
            && (approval.state == ChatApprovalState::Decided) == claim_id.is_some()
            && (approval.state == ChatApprovalState::Decided) == decision.is_some(),
        ChatError::ProjectionMismatch,
    )?;
    if let Some(value) = &decision {
        require(
            value.approval_id == id
                && value.expected_version > 0
                && value.expected_version < approval.version
                && value.generation == approval.generation
                && value.fence == binding.fence
                && Some(value.decision) == approval.decision
                && value.observed_commit == owner.owner.base_commit
                && value.profile_sha256 == approval.profile_sha256,
            ChatError::ProjectionMismatch,
        )?;
        let op: Option<(String, String, String)> = connection
            .query_row(
            "SELECT chat_id,kind,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json ELSE '' END FROM chat_operations WHERE operation_id=?",
                [approval
                    .decision_operation_id
                    .ok_or_else(|| fail(ChatError::ProjectionMismatch))?
                    .to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        require(
            op == Some((
                approval.chat_id.to_string(),
                "chat_approval_decided".into(),
                serde_json::to_string(value)?,
            )),
            ChatError::ProjectionMismatch,
        )?;
    }
    rows::validate_content(connection, approval.chat_id, &approval.evidence_ref, true)?;
    Ok(Some(StoredApproval {
        approval,
        binding,
        decision,
        claim_id,
        sequence: raw.sequence,
    }))
}
pub(super) fn required(
    connection: &Connection,
    id: ChatApprovalId,
) -> Result<StoredApproval, StoreError> {
    load(connection, id)?.ok_or_else(|| fail(ChatError::NotFound))
}
pub(super) fn change_event(
    transaction: &Transaction<'_>,
    stored: &mut StoredApproval,
    timestamp: OffsetDateTime,
    kind: &str,
) -> Result<(EventEnvelope, i64), StoreError> {
    let latest = latest_timestamp(transaction, stored.approval.chat_id)?;
    let owner = owners::required_owner(transaction, stored.approval.chat_id)?;
    let approval = &mut stored.approval;
    approval.version = next_version(approval.version)?;
    approval.updated_at = timestamp
        .max(approval.updated_at)
        .max(owner.session.updated_at);
    if let Some(latest) = latest {
        approval.updated_at = approval.updated_at.max(latest);
    }
    approval.validate().map_err(fail)?;
    let mut session = owner.session;
    session.updated_at = approval.updated_at.max(session.updated_at);
    attempts::execution_event(
        transaction,
        &owner.owner,
        &session,
        kind,
        Some(approval.request_id),
        json!({"approval_id":approval.approval_id,"version":approval.version,"tool":approval.tool,
            "state":approval.state,"decision":approval.decision,"delivery":approval.delivery}),
    )
}

// Event order is the watermark: each approval mutation advances this clock.
// Decode one bounded canonical record rather than comparing RFC3339 strings.
pub(super) fn latest_timestamp(
    connection: &Connection,
    chat_id: ChatId,
) -> Result<Option<OffsetDateTime>, StoreError> {
    let encoded: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END FROM chat_approvals WHERE chat_id=? ORDER BY updated_sequence DESC LIMIT 1",
        [chat_id.to_string()], |r| r.get(0)).optional()?;
    encoded
        .map(|encoded| {
            let value: ChatApproval = rows::decode(encoded)?;
            value
                .validate()
                .map_err(|_| fail(ChatError::ProjectionMismatch))?;
            require(value.chat_id == chat_id, ChatError::ProjectionMismatch)?;
            Ok(value.updated_at)
        })
        .transpose()
}
pub(super) fn save(
    transaction: &Transaction<'_>,
    stored: &StoredApproval,
    sequence: i64,
) -> Result<(), StoreError> {
    let v = &stored.approval;
    v.validate().map_err(fail)?;
    let changed = transaction.execute("UPDATE chat_approvals SET version=?,state=?,delivery=?,claim_id=?,decision_operation_id=?,decision_json=?,record_json=?,updated_sequence=? WHERE approval_id=? AND version=?",
        params![v.version,v.state.as_str(),v.delivery.map(|s|s.as_str()),stored.claim_id.map(|id|id.to_string()),
            v.decision_operation_id.map(|id|id.to_string()),stored.decision.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(v)?,sequence,v.approval_id.to_string(),v.version-1])?;
    require(changed == 1, ChatError::Conflict)
}
