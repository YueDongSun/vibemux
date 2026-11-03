//! Owner-attempt persistence through the existing authoritative writer and body store.
use super::*;
use vibemux_store::{
    ChatExecutionAttemptAdmission, ChatExecutionAttemptCommit, ChatExecutionProcessChange,
    ChatExecutionRecordCommit, ChatExecutionSessionCommit, ChatExecutionStart,
};
use vibemux_types::{
    ToolName,
    chat_attempt::{ChatExecutionAttempt, ChatExecutionRecord},
};

#[derive(Clone)]
pub struct ChatExecutionPrompt {
    pub chat_id: ChatId,
    pub request_id: Uuid,
    pub generation: u64,
    pub owner_version: u64,
    pub observed_commit: String,
    pub effective_tools: std::collections::BTreeSet<ToolName>,
    pub prompt: String,
}
impl fmt::Debug for ChatExecutionPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionPrompt")
            .field("chat_id", &self.chat_id)
            .field("request_id", &self.request_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct PrivateExecutionRecord {
    pub record: ChatExecutionRecord,
    pub text: Option<String>,
    pub content_error: Option<&'static str>,
}
impl fmt::Debug for PrivateExecutionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateExecutionRecord")
            .field("record_id", &self.record.record_id)
            .field("available", &self.text.is_some())
            .finish_non_exhaustive()
    }
}

impl ChatWriter {
    pub async fn replay_execution_start(
        &self,
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
    ) -> Result<Option<ChatExecutionSessionCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .replay_chat_execution_start(operation_id, chat_id, generation, owner_version)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn start_execution_process(
        &self,
        input: ChatExecutionStart,
    ) -> Result<ChatExecutionSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .start_chat_execution_process(&input)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn change_execution_process(
        &self,
        input: ChatMutation,
        fence: ChatOwnerFence,
        change: ChatExecutionProcessChange,
    ) -> Result<ChatExecutionSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .change_chat_execution_process(&input, fence, &change)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn execution_attempt(
        &self,
        request_id: Uuid,
    ) -> Result<Option<ChatExecutionAttempt>, ChatWriteError> {
        self.execute(move |store, _| store.chat_execution_attempt(request_id).map_err(Into::into))
            .await
    }
    pub async fn execution_attempts(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ChatExecutionAttempt>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .chat_execution_attempts(chat_id, after, limit)
                .map_err(Into::into)
        })
        .await
    }
    /// Private text is published before the metadata reference in the same writer queue.
    /// Replays consult the stored receipt before any filesystem effect.
    pub async fn admit_execution_attempt(
        &self,
        write: ChatExecutionPrompt,
    ) -> Result<ChatExecutionAttemptCommit, ChatWriteError> {
        let ChatExecutionPrompt {
            chat_id,
            request_id,
            generation,
            owner_version,
            observed_commit,
            effective_tools,
            prompt,
        } = write;
        if prompt.trim().is_empty() || prompt.len() as u64 > MAX_CHAT_PROMPT_BYTES {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        let mut input = ChatExecutionAttemptAdmission {
            chat_id,
            request_id,
            generation,
            owner_version,
            observed_commit,
            effective_tools,
            prompt_ref: PrivateChatFiles::reference(request_id, prompt.as_bytes())?,
            timestamp: OffsetDateTime::now_utc(),
        };
        self.execute(move |store, files| {
            if store.chat_execution_attempt(request_id)?.is_some() {
                return store
                    .admit_chat_execution_attempt(&input)
                    .map_err(Into::into);
            }
            files.initialize()?;
            write_and_apply(
                store,
                files,
                request_id,
                prompt.as_bytes(),
                |store, reference| {
                    input.prompt_ref = reference;
                    store.admit_chat_execution_attempt(&input)
                },
            )
        })
        .await
    }
    pub async fn claim_execution_attempt(
        &self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatExecutionAttemptCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .claim_chat_execution_attempt(
                    request_id,
                    generation,
                    fence,
                    OffsetDateTime::now_utc(),
                )
                .map_err(Into::into)
        })
        .await
    }
    pub async fn bind_execution_attempt(
        &self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        vendor_turn_id: String,
    ) -> Result<ChatExecutionAttemptCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .bind_chat_execution_attempt(
                    request_id,
                    generation,
                    fence,
                    &vendor_turn_id,
                    OffsetDateTime::now_utc(),
                )
                .map_err(Into::into)
        })
        .await
    }
    pub async fn cancel_execution_attempt(
        &self,
        request_id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatExecutionAttemptCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .cancel_chat_execution_attempt(
                    request_id,
                    generation,
                    fence,
                    OffsetDateTime::now_utc(),
                )
                .map_err(Into::into)
        })
        .await
    }
    pub async fn finish_execution_attempt(
        &self,
        input: ChatTurnFinish,
    ) -> Result<ChatExecutionAttemptCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .finish_chat_execution_attempt(&input)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn execution_prompt(&self, request_id: Uuid) -> Result<String, ChatWriteError> {
        self.execute(move |store, files| {
            let attempt = store
                .chat_execution_attempt(request_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if !store.chat_content_in_use(attempt.prompt_ref.blob_id)? {
                return Err(ChatPrivateError::Unavailable.into());
            }
            String::from_utf8(files.read_blob(&attempt.prompt_ref)?)
                .map_err(|_| ChatPrivateError::Unavailable.into())
        })
        .await
    }
    pub async fn append_execution_record(
        &self,
        write: ChatRecordWrite,
    ) -> Result<ChatExecutionRecordCommit, ChatWriteError> {
        let mut input = ChatRecordAppend {
            request_id: write.request_id,
            generation: write.generation,
            fence: write.fence,
            record_id: write.record_id,
            kind: write.kind,
            content_ref: PrivateChatFiles::reference(write.record_id, write.text.as_bytes())?,
            original_byte_count: write.original_byte_count,
            timestamp: OffsetDateTime::now_utc(),
        };
        self.execute(move |store, files| {
            if store.chat_execution_record(write.record_id)?.is_some() {
                return store
                    .append_chat_execution_record(&input)
                    .map_err(Into::into);
            }
            write_and_apply(
                store,
                files,
                write.record_id,
                write.text.as_bytes(),
                |store, reference| {
                    input.content_ref = reference;
                    store.append_chat_execution_record(&input)
                },
            )
        })
        .await
    }
    /// Explicit private record reads; transport adapters still apply chunk/frame limits.
    pub async fn execution_records(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<PrivateExecutionRecord>, ChatWriteError> {
        self.execute(move |store, files| {
            Ok(store
                .chat_execution_records(chat_id, after, limit)?
                .into_iter()
                .map(|record| {
                    let (text, content_error) =
                        read_record_body(files, record.content_ref.as_ref());
                    PrivateExecutionRecord {
                        record,
                        text,
                        content_error,
                    }
                })
                .collect())
        })
        .await
    }
}
