//! Chat metadata and private bodies share the existing authoritative writer.
//! Files are published before their references commit. A replay consults the
//! durable binding first and never recreates text that a purge removed.

#[path = "chat_execution_writer.rs"]
mod execution;
pub use execution::{ChatExecutionPrompt, PrivateExecutionRecord};
#[path = "chat_approval_writer.rs"]
mod approval;
pub use approval::{
    ChatApprovalDecisionResult, ChatApprovalDispatch, ChatApprovalEvidence, ChatApprovalWrite,
    PrivateChatApproval,
};

use std::{fmt, path::PathBuf};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{
    ChatExecutionOwnerCommit, ChatExecutionOwnerOpen, ChatMutation, ChatOpen, ChatOwnerChange,
    ChatOwnerClaim, ChatOwnerFence, ChatOwnerMutation, ChatRecordAppend, ChatRecordCommit,
    ChatSessionCommit, ChatTurnAdmission, ChatTurnCommit, ChatTurnFinish, SqliteStore, StoreError,
};
use vibemux_types::{
    ChatId, ChatTurnId,
    chat::{
        ChatError, ChatRecord, ChatRecordKind, ChatSession, ChatTurn, ContentRef,
        MAX_CHAT_PROMPT_BYTES, MAX_CHAT_TITLE_BYTES,
    },
};

pub use crate::chat_private::ChatPrivateError;
use crate::{
    WriterError, WriterHandle,
    chat_private::{BlobWrite, PrivateChatFiles},
};

#[derive(Debug, Error)]
pub enum ChatWriteError {
    #[error(transparent)]
    Writer(#[from] WriterError),
    #[error(transparent)]
    Content(#[from] ChatPrivateError),
    #[error("chat operation {operation_code} failed; private cleanup is pending ({cleanup})")]
    Cleanup {
        operation_code: String,
        cleanup: ChatPrivateError,
    },
}

impl ChatWriteError {
    pub fn code(&self) -> &str {
        match self {
            Self::Writer(error) => error.code(),
            Self::Content(error) => error.code(),
            Self::Cleanup { .. } => "chat_content_cleanup_pending",
        }
    }
}

impl From<StoreError> for ChatWriteError {
    fn from(error: StoreError) -> Self {
        Self::Writer(WriterError::Store {
            code: error.code().to_owned(),
        })
    }
}

pub struct ChatRecordWrite {
    pub request_id: Uuid,
    pub generation: u64,
    pub fence: ChatOwnerFence,
    pub record_id: Uuid,
    pub kind: ChatRecordKind,
    pub text: String,
    pub original_byte_count: u64,
}

impl fmt::Debug for ChatRecordWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRecordWrite")
            .field("request_id", &self.request_id)
            .field("record_id", &self.record_id)
            .field("kind", &self.kind)
            .field("bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// An explicit private-content read, not a status/event DTO.
#[derive(Clone)]
pub struct PrivateChatRecord {
    pub record: ChatRecord,
    pub text: Option<String>,
    pub content_error: Option<&'static str>,
}
impl fmt::Debug for PrivateChatRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateChatRecord")
            .field("record_id", &self.record.record_id)
            .field("available", &self.text.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct ChatWriter {
    writer: WriterHandle,
    files: PrivateChatFiles,
}

impl ChatWriter {
    /// Attaches to an existing writer and captures its project/directory
    /// identity. This reads metadata but does not create private storage.
    pub async fn attach(writer: WriterHandle, state_dir: PathBuf) -> Result<Self, ChatWriteError> {
        let handle = writer.clone();
        let files = tokio::task::spawn_blocking(move || {
            handle.with_store(move |store| {
                let project = store
                    .project_id()?
                    .ok_or(StoreError::Chat(ChatError::ProjectionMismatch))?;
                Ok(PrivateChatFiles::new(&state_dir, project))
            })
        })
        .await
        .map_err(|_| WriterError::ThreadTerminated)???;
        Ok(Self { writer, files })
    }

    async fn execute<T, F>(&self, action: F) -> Result<T, ChatWriteError>
    where
        T: Send + 'static,
        F: FnOnce(&mut SqliteStore, &PrivateChatFiles) -> Result<T, ChatWriteError>
            + Send
            + 'static,
    {
        let files = self.files.clone();
        // The outer result belongs to the writer queue; the inner result also
        // carries private-file failures without putting filesystem I/O in Store.
        // An accepted write stays attached to the actor until actual completion,
        // preserving its launch/input continuation after an RPC timeout.
        self.writer
            .enqueue_store(move |store| Ok(action(store, &files)))?
            .await
            .map_err(|_| WriterError::ThreadTerminated)??
    }

    pub async fn open(
        &self,
        mut input: ChatOpen,
        title: Option<String>,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        if title
            .as_ref()
            .is_some_and(|text| text.trim().is_empty() || text.len() as u64 > MAX_CHAT_TITLE_BYTES)
        {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        input.title_ref = title
            .as_ref()
            .map(|text| PrivateChatFiles::reference(input.operation_id, text.as_bytes()))
            .transpose()?;
        self.execute(move |store, files| {
            if store.chat_operation_known(input.operation_id)? {
                return store.open_chat(&input).map_err(Into::into);
            }
            files.initialize()?;
            match title {
                Some(text) => write_and_apply(
                    store,
                    files,
                    input.operation_id,
                    text.as_bytes(),
                    |store, reference| {
                        input.title_ref = Some(reference);
                        store.open_chat(&input)
                    },
                ),
                None => store.open_chat(&input).map_err(Into::into),
            }
        })
        .await
    }

    pub async fn admit_turn(
        &self,
        chat_id: ChatId,
        request_id: Uuid,
        generation: u64,
        base_commit: String,
        prompt: String,
    ) -> Result<ChatTurnCommit, ChatWriteError> {
        if prompt.trim().is_empty() || prompt.len() as u64 > MAX_CHAT_PROMPT_BYTES {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        let reference = PrivateChatFiles::reference(request_id, prompt.as_bytes())?;
        let mut input = ChatTurnAdmission {
            chat_id,
            request_id,
            expected_generation: generation,
            base_commit,
            prompt_ref: reference,
            timestamp: OffsetDateTime::now_utc(),
        };
        self.execute(move |store, files| {
            if store.chat_turn(request_id)?.is_some() {
                return store.admit_chat_turn(&input).map_err(Into::into);
            }
            files.initialize()?;
            write_and_apply(
                store,
                files,
                request_id,
                prompt.as_bytes(),
                |store, reference| {
                    input.prompt_ref = reference;
                    store.admit_chat_turn(&input)
                },
            )
        })
        .await
    }

    pub async fn open_execution_owner(
        &self,
        mut input: ChatExecutionOwnerOpen,
        title: Option<String>,
    ) -> Result<ChatExecutionOwnerCommit, ChatWriteError> {
        if title
            .as_ref()
            .is_some_and(|text| text.trim().is_empty() || text.len() as u64 > MAX_CHAT_TITLE_BYTES)
        {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        input.title_ref = title
            .as_ref()
            .map(|text| PrivateChatFiles::reference(input.operation_id, text.as_bytes()))
            .transpose()?;
        self.execute(move |store, files| {
            if store.chat_operation_known(input.operation_id)? {
                return store.open_chat_execution_owner(&input).map_err(Into::into);
            }
            files.initialize()?;
            match title {
                Some(text) => write_and_apply(
                    store,
                    files,
                    input.operation_id,
                    text.as_bytes(),
                    |store, reference| {
                        input.title_ref = Some(reference);
                        store.open_chat_execution_owner(&input)
                    },
                ),
                None => store.open_chat_execution_owner(&input).map_err(Into::into),
            }
        })
        .await
    }

    pub async fn change_execution_owner(
        &self,
        input: ChatOwnerMutation,
        change: ChatOwnerChange,
    ) -> Result<ChatExecutionOwnerCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .change_chat_execution_owner(&input, &change)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn execution_owner(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatExecutionOwnerCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .chat_execution_owner_state(chat_id)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn replay_execution_owner_change(
        &self,
        input: ChatOwnerMutation,
        change: ChatOwnerChange,
    ) -> Result<Option<ChatExecutionOwnerCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            if store.chat_operation_known(input.operation_id)? {
                store
                    .change_chat_execution_owner(&input, &change)
                    .map(Some)
                    .map_err(Into::into)
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn execution_owner_for_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<ChatExecutionOwnerCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .chat_execution_owner_for_operation(operation_id)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn execution_owners(
        &self,
        after: Option<ChatId>,
        limit: u32,
        finalized: bool,
    ) -> Result<Vec<ChatExecutionOwnerCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .list_chat_execution_owners(after, limit, finalized)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn execution_owner_title(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<String>, ChatWriteError> {
        self.execute(move |store, files| {
            let stored = store
                .chat_execution_owner_state(chat_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            stored
                .session
                .title_ref
                .map(|reference| {
                    String::from_utf8(files.read_blob(&reference)?)
                        .map_err(|_| ChatPrivateError::Unavailable.into())
                })
                .transpose()
        })
        .await
    }

    pub async fn append(&self, write: ChatRecordWrite) -> Result<ChatRecordCommit, ChatWriteError> {
        let reference = PrivateChatFiles::reference(write.record_id, write.text.as_bytes())?;
        self.execute(move |store, files| {
            let turn = store
                .chat_turn(write.request_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            let mut input = ChatRecordAppend {
                request_id: write.request_id,
                generation: write.generation,
                fence: write.fence,
                record_id: write.record_id,
                kind: write.kind,
                content_ref: reference,
                original_byte_count: write.original_byte_count,
                timestamp: OffsetDateTime::now_utc(),
            };
            if store
                .chat_record_by_id(turn.chat_id, write.record_id)?
                .is_some()
            {
                return store.append_chat_record(&input).map_err(Into::into);
            }
            write_and_apply(
                store,
                files,
                write.record_id,
                write.text.as_bytes(),
                |store, reference| {
                    input.content_ref = reference;
                    store.append_chat_record(&input)
                },
            )
        })
        .await
    }

    pub async fn rename(
        &self,
        input: ChatMutation,
        title: String,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        if title.trim().is_empty() || title.len() as u64 > MAX_CHAT_TITLE_BYTES {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        let reference = PrivateChatFiles::reference(input.operation_id, title.as_bytes())?;
        self.execute(move |store, files| {
            let result = if store.chat_operation_known(input.operation_id)? {
                store.rename_chat(&input, reference)?
            } else {
                write_and_apply(
                    store,
                    files,
                    input.operation_id,
                    title.as_bytes(),
                    |store, reference| store.rename_chat(&input, reference),
                )?
            };
            remove_released(store, files, &result.released_content)?;
            Ok(result)
        })
        .await
    }

    pub async fn purge(&self, input: ChatMutation) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, files| {
            let result = store.purge_chat_content(&input)?;
            remove_released(store, files, &result.released_content)?;
            // A later purge also completes failed cleanups from earlier title
            // changes. The canonical index supplies bounded, owned references.
            let mut after = None;
            loop {
                let pending = store.released_chat_content(input.chat_id, after, 128)?;
                if pending.is_empty() {
                    break;
                }
                remove_released(store, files, &pending)?;
                after = pending.last().map(|reference| reference.blob_id);
            }
            Ok(result)
        })
        .await
    }

    pub async fn records(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<PrivateChatRecord>, ChatWriteError> {
        self.execute(move |store, files| {
            let records = store.chat_records(chat_id, after, limit)?;
            Ok(records
                .into_iter()
                .map(|record| {
                    let (text, content_error) =
                        read_record_body(files, record.content_ref.as_ref());
                    PrivateChatRecord {
                        record,
                        text,
                        content_error,
                    }
                })
                .collect())
        })
        .await
    }

    pub async fn title(&self, chat_id: ChatId) -> Result<Option<String>, ChatWriteError> {
        self.execute(move |store, files| {
            let session = store
                .chat_session(chat_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            session
                .title_ref
                .as_ref()
                .map(|reference| {
                    String::from_utf8(files.read_blob(reference)?)
                        .map_err(|_| ChatPrivateError::Unavailable)
                })
                .transpose()
                .map_err(Into::into)
        })
        .await
    }

    pub async fn prompt(&self, request_id: Uuid) -> Result<String, ChatWriteError> {
        self.execute(move |store, files| {
            let turn = store
                .chat_turn(request_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if !store.chat_content_in_use(turn.prompt_ref.blob_id)? {
                return Err(ChatPrivateError::Unavailable.into());
            }
            String::from_utf8(files.read_blob(&turn.prompt_ref)?)
                .map_err(|_| ChatPrivateError::Unavailable.into())
        })
        .await
    }

    pub async fn session(&self, id: ChatId) -> Result<Option<ChatSession>, ChatWriteError> {
        self.execute(move |store, _| store.chat_session(id).map_err(Into::into))
            .await
    }
    pub async fn session_for_operation(
        &self,
        id: Uuid,
    ) -> Result<Option<ChatSession>, ChatWriteError> {
        self.execute(move |store, _| store.chat_session_for_operation(id).map_err(Into::into))
            .await
    }
    pub async fn sessions(
        &self,
        after: Option<ChatId>,
        limit: u32,
        archived: bool,
    ) -> Result<Vec<ChatSession>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .list_chat_sessions(after, limit, archived)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn turn(&self, request_id: Uuid) -> Result<Option<ChatTurn>, ChatWriteError> {
        self.execute(move |store, _| store.chat_turn(request_id).map_err(Into::into))
            .await
    }
    pub async fn turns(
        &self,
        chat_id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ChatTurn>, ChatWriteError> {
        self.execute(move |store, _| store.chat_turns(chat_id, after, limit).map_err(Into::into))
            .await
    }
    pub async fn turn_by_id(
        &self,
        chat_id: ChatId,
        turn_id: ChatTurnId,
    ) -> Result<Option<ChatTurn>, ChatWriteError> {
        self.execute(move |store, _| store.chat_turn_by_id(chat_id, turn_id).map_err(Into::into))
            .await
    }
    pub async fn claim_owner(
        &self,
        id: ChatId,
        generation: u64,
        expected: Option<String>,
    ) -> Result<ChatOwnerClaim, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .claim_chat_owner(id, generation, expected, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn transport_ready(
        &self,
        id: ChatId,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .mark_chat_transport_ready(id, generation, fence, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn bind_vendor(
        &self,
        id: ChatId,
        generation: u64,
        fence: ChatOwnerFence,
        vendor: String,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .bind_chat_vendor(id, generation, fence, &vendor, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn claim_turn(
        &self,
        id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatTurnCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .claim_chat_turn(id, generation, fence, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn bind_turn(
        &self,
        id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
        vendor: String,
    ) -> Result<ChatTurnCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .bind_chat_turn(id, generation, fence, &vendor, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn finish_turn(
        &self,
        input: ChatTurnFinish,
    ) -> Result<ChatTurnCommit, ChatWriteError> {
        self.execute(move |store, _| store.finish_chat_turn(&input).map_err(Into::into))
            .await
    }
    pub async fn cancel_turn(&self, id: Uuid) -> Result<ChatTurnCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .cancel_chat_turn(id, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn cancel_unsent(
        &self,
        id: Uuid,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatTurnCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .cancel_unsent_chat_turn(id, generation, fence, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn release_owner(
        &self,
        input: ChatMutation,
        fence: ChatOwnerFence,
        cleanup_verified: bool,
        close: bool,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .release_chat_owner(&input, fence, cleanup_verified, close)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn owner_lost(
        &self,
        id: ChatId,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .lose_chat_owner(id, generation, fence, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
    pub async fn close(&self, input: ChatMutation) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| store.close_chat(&input).map_err(Into::into))
            .await
    }
    pub async fn request_close(
        &self,
        input: ChatMutation,
    ) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| store.request_chat_close(&input).map_err(Into::into))
            .await
    }
    pub async fn archive(&self, input: ChatMutation) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| store.archive_chat(&input).map_err(Into::into))
            .await
    }
    pub async fn resume(&self, input: ChatMutation) -> Result<ChatSessionCommit, ChatWriteError> {
        self.execute(move |store, _| store.resume_chat(&input).map_err(Into::into))
            .await
    }
}

fn read_record_body(
    files: &PrivateChatFiles,
    reference: Option<&ContentRef>,
) -> (Option<String>, Option<&'static str>) {
    match reference.map(|reference| files.read_blob(reference)) {
        None => (None, None),
        Some(Err(error)) => (None, Some(error.code())),
        Some(Ok(bytes)) => match String::from_utf8(bytes) {
            Ok(text) => (Some(text), None),
            Err(_) => (None, Some("chat_content_invalid_utf8")),
        },
    }
}

#[cfg(test)]
#[path = "chat_writer_tests.rs"]
mod tests;

fn write_and_apply<T>(
    store: &mut SqliteStore,
    files: &PrivateChatFiles,
    id: Uuid,
    bytes: &[u8],
    action: impl FnOnce(&mut SqliteStore, ContentRef) -> Result<T, StoreError>,
) -> Result<T, ChatWriteError> {
    let BlobWrite { reference, .. } = files.write_blob(id, bytes)?;
    match action(store, reference.clone()) {
        Ok(result) => Ok(result),
        Err(error) => {
            // We are still on the single writer. A failed/unknown reference
            // query retains the file; a client timeout never deletes it.
            if matches!(store.chat_content_in_use(reference.blob_id), Ok(false))
                && let Err(cleanup) = files.remove_blob(&reference)
            {
                return Err(ChatWriteError::Cleanup {
                    operation_code: error.code().to_owned(),
                    cleanup,
                });
            }
            Err(error.into())
        }
    }
}

fn remove_released(
    store: &SqliteStore,
    files: &PrivateChatFiles,
    references: &[ContentRef],
) -> Result<(), ChatWriteError> {
    for reference in references {
        if !store.chat_content_in_use(reference.blob_id)? {
            files.remove_blob(reference)?;
        }
    }
    Ok(())
}
