//! Approval reads and decisions share the existing writer/runtime owners.
use super::*;
use crate::chat_session::{RuntimeApprovalDecision, RuntimeHandle, api::ChatContentError};
use vibemux_store::{ChatApprovalCommit, ChatApprovalDecisionLookup};
use vibemux_types::{
    ChatApprovalId,
    chat_approval::{ChatApproval, ChatApprovalState},
};

pub(super) struct LiveApproval {
    pub handle: RuntimeHandle,
    pub record: ChatApproval,
}

impl ChatService {
    async fn approval_handle(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<Option<Box<LiveApproval>>, ChatServiceError> {
        match self
            .request(Action::ApprovalHandle {
                chat_id,
                generation,
                request_id,
            })
            .await?
        {
            Reply::ApprovalHandle(value) => Ok(value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn approval_pending(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<Option<ChatApproval>, ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Ok(None);
        }
        let Some(live) = self
            .approval_handle(chat_id, generation, request_id)
            .await?
        else {
            return Ok(None);
        };
        let record = self
            .inner
            .context
            .writer()
            .await?
            .approval_metadata(chat_id, live.record.approval_id)
            .await?;
        Ok(
            (record.state == ChatApprovalState::Pending && record.version == live.record.version)
                .then_some(record),
        )
    }
    pub async fn approval_metadata(
        &self,
        chat_id: ChatId,
        approval_id: ChatApprovalId,
    ) -> Result<(ChatApproval, bool), ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Err(ChatError::NotFound.into());
        }
        let record = self
            .inner
            .context
            .writer()
            .await?
            .approval_metadata(chat_id, approval_id)
            .await?;
        let live = if record.state == ChatApprovalState::Pending {
            self.approval_handle(chat_id, record.generation, record.request_id)
                .await?
                .is_some_and(|v| {
                    v.record.approval_id == approval_id && v.record.version == record.version
                })
        } else {
            false
        };
        Ok((record, live))
    }
    pub async fn approval_receipt(
        &self,
        input: ChatApprovalDecisionLookup,
    ) -> Result<Option<ChatApprovalCommit>, ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Ok(None);
        }
        self.inner
            .context
            .writer()
            .await?
            .approval_decision_receipt(input)
            .await
            .map_err(Into::into)
    }
    /// Waiting for the runtime happens outside the serialized service command
    /// queue. Cancel/close commands can still reach the same owned actor.
    pub async fn approval_decide(
        &self,
        input: ChatApprovalDecisionLookup,
    ) -> Result<(ChatApprovalCommit, bool), ChatServiceError> {
        let writer = self.inner.context.writer().await?;
        if let Some(receipt) = writer.approval_decision_receipt(input.clone()).await? {
            return Ok((receipt, true));
        }
        let live = self
            .approval_handle(input.chat_id, input.generation, input.request_id)
            .await?
            .ok_or(ChatError::InvalidTransition)?;
        if live.record.approval_id != input.approval_id
            || live.record.version != input.expected_version
        {
            return Err(ChatError::Conflict.into());
        }
        let decision = live
            .handle
            .decide_approval(RuntimeApprovalDecision {
                approval_id: input.approval_id,
                operation_id: input.operation_id,
                expected_version: input.expected_version,
                decision: input.decision,
            })
            .await;
        // A missing client/runtime acknowledgement does not revoke an accepted
        // decision. Only this read-only receipt lookup can reconcile it here.
        let receipt = writer.approval_decision_receipt(input).await?;
        match (decision, receipt) {
            (Ok(_), Some(receipt)) => Ok((receipt, false)),
            (Err(_), Some(receipt)) => Ok((receipt, true)),
            (Err(error), None) => Err(error.into()),
            (Ok(_), None) => Err(ChatError::ProjectionMismatch.into()),
        }
    }
    pub(crate) async fn approval_input(
        &self,
        chat_id: ChatId,
        approval_id: ChatApprovalId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<(ChatApproval, Option<String>, Option<ChatContentError>), ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Err(ChatError::NotFound.into());
        }
        let value = self
            .inner
            .context
            .writer()
            .await?
            .approval_content(chat_id, approval_id)
            .await?;
        if value.approval.generation != generation || value.approval.request_id != request_id {
            return Err(ChatError::Conflict.into());
        }
        let text = value
            .evidence
            .as_ref()
            .map(|body| serde_json::to_string(&body.input))
            .transpose()
            .map_err(|_| ChatError::ProjectionMismatch)?;
        if text
            .as_ref()
            .is_some_and(|v| v.len() as u64 > vibemux_types::chat::MAX_CHAT_RECORD_BYTES)
        {
            return Err(ChatError::Capacity.into());
        }
        let error = if text.is_some() {
            None
        } else {
            Some(match value.content_error {
                Some("chat_content_purged") => ChatContentError::Purged,
                _ => ChatContentError::Unavailable,
            })
        };
        Ok((value.approval, text, error))
    }
}

impl Actor {
    pub(super) fn approval_handle(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Option<Box<LiveApproval>> {
        let owned = self.owners.get(&chat_id)?;
        if !matches!(owned.sink, RuntimeWriterSink::Execution(_))
            || owned.binding.generation != generation
            || owned.owner.task.is_finished()
        {
            return None;
        }
        let handle = owned.owner.handle.clone();
        let active = handle.snapshot().active_turn?;
        let record = handle.pending_approval()?;
        if active.request_id != request_id
            || active.cancel_requested
            || record.chat_id != chat_id
            || record.generation != generation
            || record.request_id != request_id
            || *record.attempt_id.as_uuid() != active.turn_id
            || record.state != ChatApprovalState::Pending
        {
            return None;
        }
        Some(Box::new(LiveApproval { handle, record }))
    }
}

#[cfg(feature = "test_helpers")]
type FixtureSinkWrapper = Box<
    dyn FnOnce(crate::chat_session::contracts::Sink) -> crate::chat_session::contracts::Sink + Send,
>;

#[cfg(feature = "test_helpers")]
pub(super) struct FixtureApprovalStart {
    binding: RuntimeBinding,
    settings: crate::chat_session::fixture_runtime::FixtureApprovalSettings,
    protocol: vibemux_harness::chat_session::SessionConfig,
    deadline: Duration,
    wrap: FixtureSinkWrapper,
}

#[cfg(feature = "test_helpers")]
impl ChatService {
    /// Test-only enrollment always constructs its owner through the guarded
    /// fixture factory. No Control/config/renderer operation selects this path.
    pub async fn start_fixture_approval_runtime<F>(
        &self,
        binding: RuntimeBinding,
        settings: crate::chat_session::fixture_runtime::FixtureApprovalSettings,
        protocol: vibemux_harness::chat_session::SessionConfig,
        deadline: Duration,
        wrap: F,
    ) -> Result<RuntimeHandle, ChatServiceError>
    where
        F: FnOnce(
                Arc<dyn crate::chat_session::ChatRuntimeSink>,
            ) -> Arc<dyn crate::chat_session::ChatRuntimeSink>
            + Send
            + 'static,
    {
        match self
            .request(Action::StartFixtureApproval(Box::new(
                FixtureApprovalStart {
                    binding,
                    settings,
                    protocol,
                    deadline,
                    wrap: Box::new(wrap),
                },
            )))
            .await?
        {
            Reply::FixtureApprovalHandle(handle) => Ok(handle),
            _ => Err(ChatServiceError::Stopped),
        }
    }
}

#[cfg(feature = "test_helpers")]
impl Actor {
    pub(super) async fn start_fixture_approval(
        &mut self,
        input: FixtureApprovalStart,
    ) -> Result<RuntimeHandle, ChatServiceError> {
        if self.owners.contains_key(&input.binding.chat_id)
            || input.deadline.is_zero()
            || *self.context.stopping.borrow()
        {
            return Err(ChatError::InvalidTransition.into());
        }
        let writer = self.context.writer().await?.clone();
        let stored = writer
            .execution_owner(input.binding.chat_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        let root = PathBuf::from(
            &stored
                .owner
                .workspace
                .as_ref()
                .ok_or(ChatError::InvalidTransition)?
                .path,
        );
        let (owner, sink) =
            crate::chat_session::fixture_runtime::spawn_fixture_approval_runtime_owned(
                writer,
                input.binding.clone(),
                input.settings,
                input.protocol,
                input.wrap,
            )
            .await?;
        let handle = owner.handle.clone();
        self.owners.insert(
            input.binding.chat_id,
            OwnedRuntime {
                owner,
                binding: input.binding,
                root,
                deadline: input.deadline,
                sink: RuntimeWriterSink::Execution(sink),
            },
        );
        Ok(handle)
    }
}
