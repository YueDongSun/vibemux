//! Bounded owner-attempt and record loads with lifetime binding validation.
use super::*;
use vibemux_types::chat_attempt::*;

pub(super) struct StoredAttempt {
    pub attempt: ChatExecutionAttempt,
    pub binding: String,
    pub finish: Option<String>,
    pub claimed: bool,
    pub fence: Option<ChatOwnerFence>,
    pub sequence: u64,
}

struct AttemptRow {
    chat_id: String,
    attempt_id: String,
    generation: u64,
    version: u64,
    phase: String,
    claimed: bool,
    fence: Option<String>,
    binding: Option<String>,
    finish: Option<String>,
    encoded: Option<String>,
    admitted_sequence: u64,
    sequence: u64,
    finish_valid: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FinishBinding {
    pub generation: u64,
    pub fence: Uuid,
    pub outcome: ChatTurnOutcome,
    pub correlated_terminal: bool,
    pub vendor_session_id: Option<String>,
    pub vendor_turn_id: Option<String>,
    pub error_code: Option<ChatFailureCode>,
    pub process_alive: bool,
    pub output_complete: bool,
}

pub(super) fn attempt(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<StoredAttempt>, StoreError> {
    let raw=connection.query_row("SELECT chat_id,attempt_id,generation,version,phase,send_claimed,send_fence,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,CASE WHEN length(CAST(finish_json AS BLOB))<=16384 THEN finish_json END,CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,admitted_sequence,updated_sequence,(finish_json IS NULL OR length(CAST(finish_json AS BLOB))<=16384) FROM chat_execution_attempts WHERE request_id=?",
        [request_id.to_string()],|row|Ok(AttemptRow {chat_id:row.get(0)?,attempt_id:row.get(1)?,generation:row.get(2)?,version:row.get(3)?,phase:row.get(4)?,claimed:row.get(5)?,fence:row.get(6)?,binding:row.get(7)?,finish:row.get(8)?,encoded:row.get(9)?,admitted_sequence:row.get(10)?,sequence:row.get(11)?,finish_valid:row.get(12)?})).optional()?;
    let Some(AttemptRow {
        chat_id,
        attempt_id: id,
        generation,
        version,
        phase,
        claimed,
        fence,
        binding,
        finish,
        encoded,
        admitted_sequence,
        sequence,
        finish_valid,
    }) = raw
    else {
        return Ok(None);
    };
    let attempt: ChatExecutionAttempt = rows::decode(encoded)?;
    attempt
        .validate()
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let owner_state = owners::required_owner(connection, attempt.chat_id)?;
    let owner = &owner_state.owner;
    let binding = binding.ok_or_else(|| fail(ChatError::ProjectionMismatch))?;
    let expected_binding = serde_json::to_string(&json!([
        attempt.chat_id,
        attempt.generation,
        attempt.owner_version,
        attempt.base_commit,
        attempt.effective_tools,
        attempt.prompt_ref
    ]))?;
    let fence = fence
        .map(|value| {
            Uuid::parse_str(&value)
                .ok()
                .filter(|id| !id.is_nil())
                .map(ChatOwnerFence)
                .ok_or_else(|| fail(ChatError::ProjectionMismatch))
        })
        .transpose()?;
    require(
        attempt.request_id == request_id
            && attempt.chat_id.to_string() == chat_id
            && attempt.attempt_id.to_string() == id
            && attempt.generation == generation
            && attempt.version == version
            && attempt.phase.as_str() == phase
            && attempt.admitted_sequence == admitted_sequence
            && sequence > 0
            && admitted_sequence > 0
            && attempt.project_id == owner.project_id
            && attempt.owner_task_id == owner.task_id
            && attempt.owner_run_id == owner.run_id
            && attempt.base_commit == owner.base_commit
            && attempt.owner_version < owner.version
            && attempt.generation <= owner_state.session.generation
            && attempt.effective_tools.is_subset(&owner.admitted_tools)
            && claimed == fence.is_some()
            && (!matches!(attempt.phase, ChatTurnPhase::Admitted) || !claimed)
            && (!matches!(
                attempt.phase,
                ChatTurnPhase::Running
                    | ChatTurnPhase::CancelRequested
                    | ChatTurnPhase::Completed
                    | ChatTurnPhase::Failed
                    | ChatTurnPhase::RecoveryRequired
            ) || claimed)
            && (attempt.phase != ChatTurnPhase::Cancelled
                || !claimed
                || attempt.correlated_terminal)
            && (claimed || attempt.vendor_turn_id.is_none())
            && binding == expected_binding
            && finish_valid
            && (!attempt.correlated_terminal || finish.is_some()),
        ChatError::ProjectionMismatch,
    )?;
    if let Some(encoded) = &finish {
        let receipt: FinishBinding = rows::decode(Some(encoded.clone()))?;
        require(
            receipt.generation == attempt.generation
                && Some(receipt.fence) == fence.map(|value| value.0)
                && Some(receipt.outcome) == attempt.outcome
                && receipt.correlated_terminal == attempt.correlated_terminal
                && receipt.vendor_turn_id == attempt.vendor_turn_id
                && receipt.error_code == attempt.error_code
                && receipt.output_complete == attempt.output_complete
                && receipt
                    .vendor_session_id
                    .as_ref()
                    .is_none_or(|id| valid_vendor_id(id)),
            ChatError::ProjectionMismatch,
        )?;
    }
    rows::validate_content(
        connection,
        attempt.chat_id,
        &attempt.prompt_ref,
        !owner_state.session.content_purged,
    )?;
    Ok(Some(StoredAttempt {
        attempt,
        binding,
        finish,
        claimed,
        fence,
        sequence,
    }))
}

pub(super) fn required_attempt(
    connection: &Connection,
    request_id: Uuid,
) -> Result<StoredAttempt, StoreError> {
    attempt(connection, request_id)?.ok_or_else(|| fail(ChatError::NotFound))
}

pub(super) fn save(
    transaction: &Transaction<'_>,
    stored: &StoredAttempt,
    previous_version: u64,
    sequence: i64,
) -> Result<(), StoreError> {
    stored.attempt.validate().map_err(fail)?;
    let changed=transaction.execute("UPDATE chat_execution_attempts SET version=?,phase=?,send_claimed=?,send_fence=?,finish_json=?,record_json=?,updated_sequence=? WHERE request_id=? AND version=?",
        params![stored.attempt.version,stored.attempt.phase.as_str(),stored.claimed,stored.fence.map(|fence|fence.0.to_string()),stored.finish,
            serde_json::to_string(&stored.attempt)?,sequence,stored.attempt.request_id.to_string(),previous_version])?;
    require(changed == 1, ChatError::Conflict)
}
