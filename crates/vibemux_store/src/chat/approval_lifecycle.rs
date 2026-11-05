//! Pending approval invalidation shares the owning attempt/process transaction.
use super::*;
use super::{
    approval_rows,
    approvals::{ChatApprovalCommit, receipt},
};
use vibemux_types::{ChatApprovalId, chat_approval::*};

/// Cancellation cannot revoke a committed response claim. Cleanup, including
/// restart recovery, seals any still-unobserved claim as Unknown.
pub(super) fn invalidate(
    transaction: &Transaction<'_>,
    request_id: Uuid,
    timestamp: OffsetDateTime,
    process_ended: bool,
) -> Result<(), StoreError> {
    let ids = {
        let mut statement = transaction.prepare("SELECT approval_id FROM chat_approvals WHERE request_id=? AND (state='pending' OR delivery='claimed') ORDER BY admitted_sequence")?;
        statement
            .query_map([request_id.to_string()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    require(
        ids.len() <= MAX_CHAT_RECORDS as usize,
        ChatError::ProjectionMismatch,
    )?;
    for id in ids {
        let mut stored = approval_rows::required(
            transaction,
            id.parse()
                .map_err(|_| fail(ChatError::ProjectionMismatch))?,
        )?;
        match stored.approval.state {
            ChatApprovalState::Pending => stored.approval.state = ChatApprovalState::Invalidated,
            ChatApprovalState::Decided if process_ended => {
                stored.approval.delivery = Some(ChatApprovalDelivery::Unknown)
            }
            _ => continue,
        }
        let (_, sequence) = approval_rows::change_event(
            transaction,
            &mut stored,
            timestamp,
            "chat_approval_invalidated",
        )?;
        approval_rows::save(transaction, &stored, sequence)?;
    }
    Ok(())
}
pub(super) fn require_settled(connection: &Connection, request_id: Uuid) -> Result<(), StoreError> {
    let unresolved: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chat_approvals WHERE request_id=? AND (state='pending' OR delivery IN('claimed','unknown')))", [request_id.to_string()], |r| r.get(0))?;
    require(!unresolved, ChatError::InvalidTransition)
}
impl SqliteStore {
    /// The live runtime owns monotonic deadlines and interruption. This only
    /// persists expiry of an unclaimed challenge; it never changes a decision.
    pub fn expire_chat_approval(
        &mut self,
        id: ChatApprovalId,
        generation: u64,
        fence: ChatOwnerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ChatApprovalCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stored = approval_rows::required(&transaction, id)?;
        require(
            stored.approval.generation == generation && stored.binding.fence == fence.0,
            ChatError::StaleFence,
        )?;
        if stored.approval.state == ChatApprovalState::Expired {
            return Ok(receipt(stored, true));
        }
        require(
            stored.approval.state == ChatApprovalState::Pending
                && timestamp >= stored.approval.expires_at,
            ChatError::InvalidTransition,
        )?;
        let row = rows::existing_session(&transaction, stored.approval.chat_id)?;
        check_owner(&row, generation, fence)?;
        stored.approval.state = ChatApprovalState::Expired;
        let (event, sequence) = approval_rows::change_event(
            &transaction,
            &mut stored,
            timestamp,
            "chat_approval_expired",
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
