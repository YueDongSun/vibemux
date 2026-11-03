//! Owner-backed process and submission admission through the serialized service.
use super::*;
use crate::chat_writer::{ChatExecutionPrompt, PrivateExecutionRecord};
use vibemux_store::{ChatExecutionProcessChange, ChatExecutionStart};
use vibemux_types::chat_attempt::ChatExecutionAttempt;

impl ChatService {
    pub async fn execution_start_receipt(
        &self,
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
    ) -> Result<Option<ChatExecutionSessionCommit>, ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Ok(None);
        }
        self.inner
            .context
            .writer()
            .await?
            .replay_execution_start(operation_id, chat_id, generation, owner_version)
            .await
            .map_err(Into::into)
    }

    pub async fn start_execution(
        &self,
        input: ChatOwnerMutation,
        generation: u64,
    ) -> Result<ChatExecutionSessionCommit, ChatServiceError> {
        match self
            .request(Action::StartExecution { input, generation })
            .await?
        {
            Reply::ExecutionProcess(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn send_execution(
        &self,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
        request_id: Uuid,
        prompt: String,
    ) -> Result<ChatExecutionAttemptCommit, ChatServiceError> {
        match self
            .request(Action::SendExecution {
                chat_id,
                generation,
                owner_version,
                request_id,
                prompt,
            })
            .await?
        {
            Reply::ExecutionAttempt(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn cancel_execution(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<ChatExecutionAttempt, ChatServiceError> {
        match self
            .request(Action::CancelExecution {
                chat_id,
                generation,
                request_id,
            })
            .await?
        {
            Reply::CancelledExecution(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    /// Stop this process generation while retaining the lifetime owner/workspace.
    pub async fn close_execution_process(
        &self,
        chat_id: ChatId,
        generation: u64,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        match self
            .request(Action::CloseExecutionProcess {
                chat_id,
                generation,
            })
            .await?
        {
            Reply::ExecutionOwner(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn execution_attempt(
        &self,
        request_id: Uuid,
    ) -> Result<Option<ChatExecutionAttempt>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .execution_attempt(request_id)
            .await
            .map_err(Into::into)
    }

    pub async fn execution_attempts(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ChatExecutionAttempt>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .execution_attempts(chat_id, after, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn execution_records(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<PrivateExecutionRecord>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .execution_records(chat_id, after, limit)
            .await
            .map_err(Into::into)
    }
}

impl Actor {
    pub(super) async fn start_execution(
        &mut self,
        input: ChatOwnerMutation,
        generation: u64,
    ) -> Result<ChatExecutionSessionCommit, ChatServiceError> {
        let writer = self.context.writer().await?.clone();
        if let Some(replayed) = writer
            .replay_execution_start(
                input.operation_id,
                input.chat_id,
                generation,
                input.expected_version,
            )
            .await?
        {
            return Ok(replayed);
        }
        let stored = writer
            .execution_owner(input.chat_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        require_generation(&stored.session, generation)?;
        if stored.owner.version != input.expected_version {
            return Err(ChatError::Conflict.into());
        }
        if self.owners.contains_key(&input.chat_id) {
            return Err(ChatError::OwnerAlreadyClaimed.into());
        }
        let root = owner_service::verify_execution_workspace(&self.context, &stored.owner).await?;
        let target = if stored.session.vendor_session_id.is_some() {
            resume_target(&stored.session)?
        } else {
            new_target(stored.session.harness)
        };
        let prepared = self
            .context
            .dispatch
            .prepare_chat(stored.session.harness, target)
            .await?
            .for_workspace(
                stored
                    .owner
                    .workspace
                    .as_ref()
                    .ok_or(ChatError::ProjectionMismatch)?,
            )?;
        if prepared.profile_sha256 != stored.owner.profile_sha256
            || Some(&prepared.resource_key) != stored.owner.resource_key.as_ref()
        {
            return Err(ChatError::Conflict.into());
        }
        if *self.context.stopping.borrow() {
            return Err(ChatServiceError::Stopped);
        }
        let result = writer
            .start_execution_process(ChatExecutionStart {
                operation_id: input.operation_id,
                chat_id: input.chat_id,
                expected_generation: generation,
                expected_owner_version: input.expected_version,
                profile_sha256: prepared.profile_sha256,
                resource_key: prepared.resource_key,
                observed_commit: stored.owner.base_commit,
                expected_vendor_id: prepared.protocol.target.expected_id(),
                timestamp: input.timestamp,
            })
            .await?;
        let Some(fence) = result.fence else {
            return Ok(result);
        };
        if *self.context.stopping.borrow() {
            writer
                .change_execution_process(
                    mutation(input.chat_id, result.session.generation),
                    fence,
                    ChatExecutionProcessChange::Released {
                        cleanup_verified: true,
                        closed: false,
                    },
                )
                .await?;
            return Err(ChatServiceError::Stopped);
        }
        let binding = RuntimeBinding {
            chat_id: input.chat_id,
            generation: result.session.generation,
            owner_fence: fence,
        };
        let sink = Arc::new(ChatExecutionSink::new(writer.clone(), binding.clone()));
        let deadline = prepared.deadline;
        let plan = RuntimePlan {
            binding: binding.clone(),
            launch: prepared.launch,
            protocol: prepared.protocol,
            protocol_limits: SessionLimits::default(),
            limits: RuntimeLimits::default(),
        };
        match spawn_runtime(plan, sink.clone()) {
            Ok(owner) => {
                self.owners.insert(
                    input.chat_id,
                    OwnedRuntime {
                        owner,
                        binding,
                        root,
                        deadline,
                        sink: RuntimeWriterSink::Execution(sink),
                    },
                );
            }
            Err(error) => {
                writer
                    .change_execution_process(
                        mutation(input.chat_id, binding.generation),
                        fence,
                        ChatExecutionProcessChange::Released {
                            cleanup_verified: true,
                            closed: false,
                        },
                    )
                    .await?;
                return Err(error.into());
            }
        }
        Ok(result)
    }

    pub(super) async fn send_execution(
        &mut self,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
        request_id: Uuid,
        prompt: String,
    ) -> Result<ChatExecutionAttemptCommit, ChatServiceError> {
        let writer = self.context.writer().await?.clone();
        if let Some(existing) = writer.execution_attempt(request_id).await? {
            return writer
                .admit_execution_attempt(ChatExecutionPrompt {
                    chat_id,
                    request_id,
                    generation,
                    owner_version,
                    observed_commit: existing.base_commit,
                    effective_tools: existing.effective_tools,
                    prompt,
                })
                .await
                .map_err(Into::into);
        }
        let stored = writer
            .execution_owner(chat_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        require_generation(&stored.session, generation)?;
        if stored.owner.version != owner_version {
            return Err(ChatError::Conflict.into());
        }
        owner_service::verify_execution_workspace(&self.context, &stored.owner).await?;
        let owned = self
            .owners
            .get(&chat_id)
            .filter(|owned| matches!(owned.sink, RuntimeWriterSink::Execution(_)))
            .ok_or(ChatError::InvalidTransition)?;
        if owned.binding.generation != generation {
            return Err(ChatError::StaleGeneration.into());
        }
        if *self.context.stopping.borrow() {
            return Err(ChatServiceError::Stopped);
        }
        let result = writer
            .admit_execution_attempt(ChatExecutionPrompt {
                chat_id,
                request_id,
                generation,
                owner_version,
                observed_commit: stored.owner.base_commit,
                effective_tools: Default::default(),
                prompt: prompt.clone(),
            })
            .await?;
        if !result.duplicate {
            if *self.context.stopping.borrow() {
                writer
                    .cancel_execution_attempt(request_id, generation, owned.binding.owner_fence)
                    .await?;
                return Err(ChatServiceError::Stopped);
            }
            let request = TurnRequest {
                request_id,
                turn_id: *result.attempt.attempt_id.as_uuid(),
                prompt,
            };
            if let Err(error) = owned.owner.handle.queue_turn(request, owned.deadline) {
                writer
                    .cancel_execution_attempt(request_id, generation, owned.binding.owner_fence)
                    .await?;
                return Err(error.into());
            }
        }
        Ok(result)
    }

    pub(super) async fn cancel_execution(
        &mut self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<ChatExecutionAttempt, ChatServiceError> {
        let writer = self.context.writer().await?;
        let attempt = writer
            .execution_attempt(request_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        if attempt.chat_id != chat_id || attempt.generation != generation {
            return Err(ChatError::Conflict.into());
        }
        if attempt.phase.is_terminal() {
            return Ok(attempt);
        }
        let owned = self
            .owners
            .get(&chat_id)
            .filter(|owned| matches!(owned.sink, RuntimeWriterSink::Execution(_)))
            .ok_or(ChatError::InvalidTransition)?;
        if owned.binding.generation != generation {
            return Err(ChatError::StaleGeneration.into());
        }
        let result = writer
            .cancel_execution_attempt(request_id, generation, owned.binding.owner_fence)
            .await?;
        if result.attempt.phase == ChatTurnPhase::CancelRequested {
            owned
                .owner
                .handle
                .cancel(*result.attempt.attempt_id.as_uuid())
                .await?;
        }
        Ok(result.attempt)
    }

    pub(super) async fn close_execution_process(
        &mut self,
        chat_id: ChatId,
        generation: u64,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        let stored = self
            .context
            .writer()
            .await?
            .execution_owner(chat_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        require_generation(&stored.session, generation)?;
        if let Some(owned) = self.owners.get(&chat_id) {
            if !matches!(owned.sink, RuntimeWriterSink::Execution(_))
                || owned.binding.generation != generation
            {
                return Err(ChatError::Conflict.into());
            }
            let owned = self
                .owners
                .remove(&chat_id)
                .ok_or(ChatError::InvalidTransition)?;
            owned.owner.handle.close();
            end_owner(&self.context, owned).await?;
        } else if !matches!(
            stored.session.state,
            ChatState::Suspended | ChatState::Closed
        ) {
            return Err(ChatError::InvalidTransition.into());
        }
        self.context
            .writer()
            .await?
            .execution_owner(chat_id)
            .await?
            .ok_or(ChatError::NotFound.into())
    }
}
