//! Private approval evidence and one-use dispatch stay on the daemon writer.
//! This boundary does not qualify native correlation or construct vendor frames.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_store::{
    ChatApprovalAdmission, ChatApprovalCommit, ChatApprovalDecisionInput, ChatApprovalDeliveryClaim,
};
use vibemux_types::{
    ChatApprovalId, ToolName,
    chat::{MAX_CHAT_RECORD_BYTES, valid_vendor_id},
    chat_approval::{ChatApproval, ChatApprovalDelivery, ChatApprovalState},
};

const EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Only a validated adapter/runtime producer supplies this private body. Its
/// contents never establish correlation merely by claiming the canonical IDs.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApprovalEvidence {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub request_id: Uuid,
    pub generation: u64,
    pub vendor_session_id: String,
    pub vendor_request_id: String,
    pub tool_use_id: String,
    pub tool: ToolName,
    pub vendor_tool_name: String,
    pub input: Value,
}
impl fmt::Debug for ChatApprovalEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatApprovalEvidence")
            .field("chat_id", &self.chat_id)
            .field("request_id", &self.request_id)
            .field("generation", &self.generation)
            .field("tool", &self.tool)
            .finish_non_exhaustive()
    }
}
impl ChatApprovalEvidence {
    fn validate(&self) -> Result<(), ChatWriteError> {
        let tool_matches = match self.tool {
            ToolName::ReadFiles => self.vendor_tool_name == "Read",
            ToolName::SearchFiles => matches!(self.vendor_tool_name.as_str(), "Glob" | "Grep"),
            ToolName::EditFiles => self.vendor_tool_name == "Edit",
            ToolName::WriteFiles => self.vendor_tool_name == "Write",
            ToolName::RunDevTests => false,
        };
        if self.schema_version != EVIDENCE_SCHEMA_VERSION
            || self.request_id.is_nil()
            || self.generation == 0
            || self.generation > i64::MAX as u64
            || !valid_vendor_id(&self.vendor_session_id)
            || !valid_vendor_id(&self.vendor_request_id)
            || !valid_vendor_id(&self.tool_use_id)
            || !tool_matches
            || !self.input.is_object()
        {
            return Err(StoreError::Chat(ChatError::InvalidInput).into());
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>, ChatWriteError> {
        self.validate()?;
        let bytes =
            serde_json::to_vec(self).map_err(|_| StoreError::Chat(ChatError::InvalidInput))?;
        if bytes.len() as u64 > MAX_CHAT_RECORD_BYTES {
            return Err(StoreError::Chat(ChatError::Capacity).into());
        }
        Ok(bytes)
    }
    fn check_record(&self, approval: &ChatApproval) -> Result<(), ChatWriteError> {
        self.validate()?;
        if self.chat_id != approval.chat_id
            || self.request_id != approval.request_id
            || self.generation != approval.generation
            || self.tool != approval.tool
        {
            return Err(StoreError::Chat(ChatError::ProjectionMismatch).into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ChatApprovalWrite {
    pub evidence_id: Uuid,
    pub fence: ChatOwnerFence,
    pub evidence: ChatApprovalEvidence,
    pub expires_at: OffsetDateTime,
}
/// Explicit private read; public transports must still apply their chunk limits.
pub struct PrivateChatApproval {
    pub approval: ChatApproval,
    pub evidence: Option<ChatApprovalEvidence>,
    pub content_error: Option<&'static str>,
}
impl fmt::Debug for PrivateChatApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateChatApproval")
            .field("approval", &self.approval)
            .field("available", &self.evidence.is_some())
            .field("content_error", &self.content_error)
            .finish()
    }
}
/// No Clone or serde: only the fresh decision transfers the verified snapshot
/// and its sole claim to the owned runtime. A receipt replay has neither.
pub struct ChatApprovalDispatch {
    claim: ChatApprovalDeliveryClaim,
    evidence: ChatApprovalEvidence,
}
impl ChatApprovalDispatch {
    pub fn evidence(&self) -> &ChatApprovalEvidence {
        &self.evidence
    }
    pub fn into_parts(self) -> (ChatApprovalDeliveryClaim, ChatApprovalEvidence) {
        (self.claim, self.evidence)
    }
}
impl fmt::Debug for ChatApprovalDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChatApprovalDispatch(<private>)")
    }
}
#[derive(Debug)]
pub struct ChatApprovalDecisionResult {
    pub receipt: ChatApprovalCommit,
    pub dispatch: Option<ChatApprovalDispatch>,
}

fn read_evidence(
    store: &SqliteStore,
    files: &PrivateChatFiles,
    approval: &ChatApproval,
) -> Result<ChatApprovalEvidence, ChatWriteError> {
    let bytes = files.read_blob(&approval.evidence_ref)?;
    let body: ChatApprovalEvidence = serde_json::from_slice(&bytes)
        .map_err(|_| StoreError::Chat(ChatError::ProjectionMismatch))?;
    body.check_record(approval)?;
    store.validate_chat_approval_evidence(
        approval.approval_id,
        &Sha256Digest::of(body.vendor_request_id.as_bytes()).to_hex(),
        &Sha256Digest::of(body.tool_use_id.as_bytes()).to_hex(),
    )?;
    Ok(body)
}

impl ChatWriter {
    /// Metadata-only observation: never reads private files or reacquires a claim.
    pub async fn approval_decision_receipt(
        &self,
        input: vibemux_store::ChatApprovalDecisionLookup,
    ) -> Result<Option<ChatApprovalCommit>, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .chat_approval_decision_receipt(&input)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn approval_metadata(
        &self,
        chat_id: ChatId,
        id: ChatApprovalId,
    ) -> Result<ChatApproval, ChatWriteError> {
        self.execute(move |store, _| {
            let value = store
                .chat_approval(id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if value.chat_id != chat_id {
                return Err(StoreError::Chat(ChatError::NotFound).into());
            }
            Ok(value)
        })
        .await
    }
    /// Called only after the owned runtime's monotonic expiry. A claim that
    /// won the race remains Decided, and cleanup reconciles its delivery.
    pub async fn expire_approval(
        &self,
        chat_id: ChatId,
        id: ChatApprovalId,
        generation: u64,
        fence: ChatOwnerFence,
    ) -> Result<ChatApproval, ChatWriteError> {
        self.execute(move |store, _| {
            let value = store
                .chat_approval(id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if value.chat_id != chat_id || value.generation != generation {
                return Err(StoreError::Chat(ChatError::StaleGeneration).into());
            }
            if value.state != ChatApprovalState::Pending {
                return Ok(value);
            }
            store
                .expire_chat_approval(
                    id,
                    generation,
                    fence,
                    OffsetDateTime::now_utc().max(value.expires_at),
                )
                .map(|v| v.approval)
                .map_err(Into::into)
        })
        .await
    }
    pub async fn admit_approval(
        &self,
        write: ChatApprovalWrite,
    ) -> Result<ChatApprovalCommit, ChatWriteError> {
        let bytes = write.evidence.bytes()?;
        let mut input = ChatApprovalAdmission {
            request_id: write.evidence.request_id,
            generation: write.evidence.generation,
            fence: write.fence,
            vendor_request_key: Sha256Digest::of(write.evidence.vendor_request_id.as_bytes())
                .to_hex(),
            tool_use_key: Sha256Digest::of(write.evidence.tool_use_id.as_bytes()).to_hex(),
            tool: write.evidence.tool,
            evidence_ref: PrivateChatFiles::reference(write.evidence_id, &bytes)?,
            expires_at: write.expires_at,
            timestamp: OffsetDateTime::now_utc(),
        };
        self.execute(move |store, files| {
            let attempt = store
                .chat_execution_attempt(input.request_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if attempt.chat_id != write.evidence.chat_id
                || attempt.generation != write.evidence.generation
            {
                return Err(StoreError::Chat(ChatError::Conflict).into());
            }
            if let Some(receipt) = store.replay_chat_approval_admission(&input)? {
                return Ok(receipt);
            }
            let owner = store
                .chat_execution_owner_state(write.evidence.chat_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if owner.session.vendor_session_id.as_deref()
                != Some(write.evidence.vendor_session_id.as_str())
            {
                return Err(StoreError::Chat(ChatError::UnconfirmedSession).into());
            }
            files.initialize()?;
            write_and_apply(
                store,
                files,
                write.evidence_id,
                &bytes,
                |store, reference| {
                    input.evidence_ref = reference;
                    input.timestamp = OffsetDateTime::now_utc();
                    store.admit_chat_approval(&input)
                },
            )
        })
        .await
    }
    pub async fn approval_content(
        &self,
        chat_id: ChatId,
        id: ChatApprovalId,
    ) -> Result<PrivateChatApproval, ChatWriteError> {
        self.execute(move |store, files| {
            let approval = store
                .chat_approval(id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if approval.chat_id != chat_id {
                return Err(StoreError::Chat(ChatError::NotFound).into());
            }
            let (evidence, content_error) = match read_evidence(store, files, &approval) {
                Ok(body) => (Some(body), None),
                Err(ChatWriteError::Content(error)) => (None, Some(error.code())),
                Err(_) => (None, Some("chat_projection_mismatch")),
            };
            Ok(PrivateChatApproval {
                approval,
                evidence,
                content_error,
            })
        })
        .await
    }
    pub async fn decide_approval(
        &self,
        mut input: ChatApprovalDecisionInput,
    ) -> Result<ChatApprovalDecisionResult, ChatWriteError> {
        self.execute(move |store, files| {
            let approval = store
                .chat_approval(input.approval_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            input.timestamp = OffsetDateTime::now_utc();
            if approval.state != ChatApprovalState::Pending {
                return Ok(ChatApprovalDecisionResult {
                    receipt: store.decide_chat_approval(&input)?,
                    dispatch: None,
                });
            }
            let body = read_evidence(store, files, &approval)?;
            let owner = store
                .chat_execution_owner_state(approval.chat_id)?
                .ok_or(StoreError::Chat(ChatError::NotFound))?;
            if owner.session.vendor_session_id.as_deref() != Some(body.vendor_session_id.as_str()) {
                return Err(StoreError::Chat(ChatError::UnconfirmedSession).into());
            }
            input.timestamp = OffsetDateTime::now_utc();
            let mut receipt = store.decide_chat_approval(&input)?;
            let dispatch = receipt
                .delivery_claim
                .take()
                .map(|claim| ChatApprovalDispatch {
                    claim,
                    evidence: body,
                });
            Ok(ChatApprovalDecisionResult { receipt, dispatch })
        })
        .await
    }
    pub async fn complete_approval_delivery(
        &self,
        claim: ChatApprovalDeliveryClaim,
        outcome: ChatApprovalDelivery,
    ) -> Result<ChatApprovalCommit, ChatWriteError> {
        self.execute(move |store, _| {
            store
                .complete_chat_approval_delivery(&claim, outcome, OffsetDateTime::now_utc())
                .map_err(Into::into)
        })
        .await
    }
}
