//! Read-only decision receipts never reconstruct one-use delivery authority.
use super::{
    approval_rows,
    approvals::{self, ChatApprovalCommit},
    require, valid_generation,
};
use crate::{SqliteStore, StoreError};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;
use vibemux_types::{ChatApprovalId, ChatId, chat::ChatError, chat_approval::ChatApprovalDecision};

#[derive(Clone, Debug)]
pub struct ChatApprovalDecisionLookup {
    pub operation_id: Uuid,
    pub approval_id: ChatApprovalId,
    pub chat_id: ChatId,
    pub request_id: Uuid,
    pub generation: u64,
    pub expected_version: u64,
    pub decision: ChatApprovalDecision,
}

impl SqliteStore {
    /// Observe a previously committed decision using its original client binding.
    /// Current delivery metadata is returned without an event or delivery claim.
    pub fn chat_approval_decision_receipt(
        &self,
        input: &ChatApprovalDecisionLookup,
    ) -> Result<Option<ChatApprovalCommit>, StoreError> {
        valid_generation(input.generation)?;
        require(
            !input.operation_id.is_nil()
                && !input.request_id.is_nil()
                && input.expected_version > 0
                && input.expected_version <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        let mut stored = approval_rows::required(&self.connection, input.approval_id)?;
        require(
            stored.approval.chat_id == input.chat_id,
            ChatError::NotFound,
        )?;
        require(
            stored.approval.request_id == input.request_id
                && stored.approval.generation == input.generation,
            ChatError::Conflict,
        )?;
        let operation: Option<(bool, u64)> = self
            .connection
            .query_row(
                "SELECT chat_id=?2 AND kind='chat_approval_decided',sequence FROM chat_operations WHERE operation_id=?1",
                params![input.operation_id.to_string(), input.chat_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let request_used: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_turns WHERE request_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1 UNION ALL SELECT 1 FROM chat_execution_attempts WHERE request_id=?1)",
            [input.operation_id.to_string()],
            |row| row.get(0),
        )?;
        require(!request_used, ChatError::Conflict)?;
        let Some((matches_operation, sequence)) = operation else {
            return Ok(None);
        };
        require(
            matches_operation
                && stored.approval.decision_operation_id == Some(input.operation_id)
                && stored.decision.as_ref().is_some_and(|binding| {
                    binding.approval_id == input.approval_id
                        && binding.generation == input.generation
                        && binding.expected_version == input.expected_version
                        && binding.decision == input.decision
                }),
            ChatError::Conflict,
        )?;
        require(
            sequence > stored.approval.admitted_sequence && sequence <= stored.sequence,
            ChatError::ProjectionMismatch,
        )?;
        // Delivery observations advance the approval, not this decision receipt.
        stored.sequence = sequence;
        Ok(Some(approvals::receipt(stored, true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessDispatchAdmission;
    use crate::chat::{
        ChatApprovalAdmission, ChatApprovalDecisionInput, ChatExecutionAttemptAdmission,
        ChatExecutionOwnerOpen, ChatExecutionProcessChange, ChatExecutionStart, ChatMutation,
        ChatOpen, ChatOwnerChange, ChatOwnerFence, ChatOwnerMutation, ChatTurnAdmission,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::Path,
    };
    use time::{Duration, OffsetDateTime};
    use vibemux_harness::{
        AgentKind, HarnessDetection,
        dispatch::{NativeProtocol, PromptDigest, Sha256Digest},
    };
    use vibemux_types::{
        ToolName,
        a2a::RunWorkspace,
        chat::{ChatHarness, ContentRef},
        chat_approval::ChatApprovalDelivery,
    };

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("time")
    }
    fn running(
        store: &mut SqliteStore,
        tools: BTreeSet<ToolName>,
    ) -> (ChatId, Uuid, ChatOwnerFence) {
        running_with_tools(store, tools.clone(), tools)
    }
    fn running_with_tools(
        store: &mut SqliteStore,
        admitted_tools: BTreeSet<ToolName>,
        effective_tools: BTreeSet<ToolName>,
    ) -> (ChatId, Uuid, ChatOwnerFence) {
        store
            .commit_harness_snapshot(
                &BTreeMap::from([(
                    "claude".into(),
                    HarnessDetection {
                        detected: true,
                        path: None,
                        launcher: None,
                        version: Some("fixture".into()),
                    },
                )]),
                "2026-11-01T00:00:00Z",
                "approval_detection",
            )
            .expect("detection");
        let mut owner = store
            .open_chat_execution_owner(&ChatExecutionOwnerOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Claude,
                profile_sha256: "a".repeat(64),
                base_commit: "b".repeat(40),
                admitted_tools,
                title_ref: None,
                timestamp: now(),
            })
            .expect("owner")
            .owner;
        for change in [
            ChatOwnerChange::Plan {
                workspace: RunWorkspace {
                    path: format!("/owned/{}", owner.run_id),
                    branch: format!("codex/run_{}", owner.run_id.as_uuid().simple()),
                    base_commit: owner.base_commit.clone(),
                    ownership_token: Uuid::new_v4().simple().to_string(),
                },
                resource_key: format!("{:064x}", owner.run_id.as_uuid().as_u128()),
            },
            ChatOwnerChange::ClaimCreation,
            ChatOwnerChange::VerifyWorkspace,
        ] {
            owner = store
                .change_chat_execution_owner(
                    &ChatOwnerMutation {
                        operation_id: Uuid::new_v4(),
                        chat_id: owner.chat_id,
                        expected_version: owner.version,
                        timestamp: now(),
                    },
                    &change,
                )
                .expect("workspace")
                .owner;
        }
        let vendor = Uuid::new_v4().to_string();
        let fence = store
            .start_chat_execution_process(&ChatExecutionStart {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_generation: 1,
                expected_owner_version: owner.version,
                profile_sha256: owner.profile_sha256.clone(),
                resource_key: owner.resource_key.clone().expect("resource"),
                observed_commit: owner.base_commit.clone(),
                expected_vendor_id: Some(vendor.clone()),
                timestamp: now(),
            })
            .expect("start")
            .fence
            .expect("fresh fence");
        for change in [
            ChatExecutionProcessChange::VendorBound { vendor_id: vendor },
            ChatExecutionProcessChange::Ready,
        ] {
            store
                .change_chat_execution_process(
                    &ChatMutation {
                        operation_id: Uuid::new_v4(),
                        chat_id: owner.chat_id,
                        expected_generation: 1,
                        timestamp: now(),
                    },
                    fence,
                    &change,
                )
                .expect("ready");
        }
        let request_id = Uuid::new_v4();
        store
            .admit_chat_execution_attempt(&ChatExecutionAttemptAdmission {
                chat_id: owner.chat_id,
                request_id,
                generation: 1,
                owner_version: owner.version,
                observed_commit: owner.base_commit,
                effective_tools,
                prompt_ref: ContentRef {
                    blob_id: request_id,
                    sha256: "d".repeat(64),
                    byte_count: 16,
                },
                timestamp: now(),
            })
            .expect("admission");
        store
            .claim_chat_execution_attempt(request_id, 1, fence, now())
            .expect("input claim");
        store
            .bind_chat_execution_attempt(request_id, 1, fence, "tool_turn", now())
            .expect("turn");
        (owner.chat_id, request_id, fence)
    }
    fn admission(request_id: Uuid, fence: ChatOwnerFence) -> ChatApprovalAdmission {
        ChatApprovalAdmission {
            request_id,
            generation: 1,
            fence,
            vendor_request_key: "e".repeat(64),
            tool_use_key: "f".repeat(64),
            tool: ToolName::WriteFiles,
            evidence_ref: ContentRef {
                blob_id: Uuid::new_v4(),
                sha256: "1".repeat(64),
                byte_count: 128,
            },
            expires_at: now() + Duration::seconds(120),
            timestamp: now(),
        }
    }
    fn decision(
        approval: &vibemux_types::chat_approval::ChatApproval,
        fence: ChatOwnerFence,
    ) -> ChatApprovalDecisionInput {
        ChatApprovalDecisionInput {
            operation_id: Uuid::new_v4(),
            approval_id: approval.approval_id,
            expected_version: approval.version,
            generation: 1,
            fence,
            decision: ChatApprovalDecision::AllowOnce,
            observed_commit: "b".repeat(40),
            profile_sha256: "a".repeat(64),
            timestamp: now(),
        }
    }

    fn fixture() -> (
        SqliteStore,
        ChatId,
        ChatOwnerFence,
        ChatApprovalDecisionLookup,
        ChatApprovalDecisionInput,
    ) {
        let mut store = SqliteStore::open(Path::new(":memory:")).expect("in-memory store");
        let (chat_id, request_id, fence) =
            running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request_id, fence))
            .expect("approval")
            .approval;
        let choose = decision(&approval, fence);
        let lookup = ChatApprovalDecisionLookup {
            operation_id: choose.operation_id,
            approval_id: approval.approval_id,
            chat_id,
            request_id,
            generation: choose.generation,
            expected_version: choose.expected_version,
            decision: choose.decision,
        };
        (store, chat_id, fence, lookup, choose)
    }

    fn lookup_read_only(
        store: &SqliteStore,
        input: &ChatApprovalDecisionLookup,
    ) -> Result<Option<ChatApprovalCommit>, StoreError> {
        let events = store.events().expect("events before lookup");
        let changes: u64 = store
            .connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .expect("changes");
        store
            .connection
            .pragma_update(None, "query_only", true)
            .expect("read-only connection");
        let result = store.chat_approval_decision_receipt(input);
        store
            .connection
            .pragma_update(None, "query_only", false)
            .expect("restore fixture writes");
        let after: u64 = store
            .connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .expect("changes");
        assert_eq!(changes, after, "lookup must not mutate SQLite");
        assert_eq!(store.events().expect("events after lookup"), events);
        assert!(
            store.connection.is_autocommit(),
            "lookup must leave no transaction"
        );
        result
    }

    fn assert_error(store: &SqliteStore, input: &ChatApprovalDecisionLookup, expected: ChatError) {
        assert!(
            matches!(lookup_read_only(store, input), Err(StoreError::Chat(error)) if error == expected)
        );
    }

    fn assert_receipt(receipt: &ChatApprovalCommit, sequence: u64, delivery: ChatApprovalDelivery) {
        assert_eq!(receipt.sequence, sequence);
        assert_eq!(receipt.approval.delivery, Some(delivery));
        assert!(receipt.duplicate);
        assert!(receipt.event.is_none());
        assert!(receipt.delivery_claim.is_none());
    }

    #[test]
    fn unused_lookup_before_decision_has_no_effect() {
        let (mut store, _, _, input, choose) = fixture();
        assert!(
            lookup_read_only(&store, &input)
                .expect("unused operation")
                .is_none()
        );
        assert!(
            store
                .decide_chat_approval(&choose)
                .expect("fresh decision")
                .delivery_claim
                .is_some()
        );
        let mut unused = input.clone();
        unused.operation_id = Uuid::new_v4();
        assert!(
            lookup_read_only(&store, &unused)
                .expect("different unused operation")
                .is_none()
        );
    }

    #[test]
    fn receipt_keeps_original_decision_sequence_and_current_delivery() {
        for outcome in [ChatApprovalDelivery::Written, ChatApprovalDelivery::Unknown] {
            let (mut store, _, _, input, choose) = fixture();
            let mut committed = store.decide_chat_approval(&choose).expect("fresh decision");
            let sequence = committed.sequence;
            let receipt = lookup_read_only(&store, &input)
                .expect("lookup")
                .expect("receipt");
            assert_receipt(&receipt, sequence, ChatApprovalDelivery::Claimed);
            let claim = committed.delivery_claim.take().expect("fresh authority");
            let observed = store
                .complete_chat_approval_delivery(&claim, outcome, now())
                .expect("delivery observation");
            assert!(observed.sequence > sequence);
            assert!(observed.approval.version > input.expected_version);
            let receipt = lookup_read_only(&store, &input)
                .expect("lookup after observation")
                .expect("receipt");
            assert_receipt(&receipt, sequence, outcome);
            assert_eq!(receipt.approval, observed.approval);
        }
    }

    #[test]
    fn lookup_refuses_changed_bindings_and_operation_reuse() {
        let (mut store, _, _, input, choose) = fixture();
        store.decide_chat_approval(&choose).expect("decision");
        let mut changed = input.clone();
        changed.chat_id = ChatId::new();
        assert_error(&store, &changed, ChatError::NotFound);
        let mut changed = input.clone();
        changed.approval_id = ChatApprovalId::new();
        assert_error(&store, &changed, ChatError::NotFound);
        let mut changed = input.clone();
        changed.request_id = Uuid::new_v4();
        assert_error(&store, &changed, ChatError::Conflict);
        let mut changed = input.clone();
        changed.generation += 1;
        assert_error(&store, &changed, ChatError::Conflict);
        let mut changed = input.clone();
        changed.expected_version += 1;
        assert_error(&store, &changed, ChatError::Conflict);
        let mut changed = input.clone();
        changed.decision = ChatApprovalDecision::Deny;
        assert_error(&store, &changed, ChatError::Conflict);
        let operation_id: String = store.connection.query_row(
            "SELECT operation_id FROM chat_operations WHERE kind!='chat_approval_decided' ORDER BY sequence LIMIT 1", [], |row| row.get(0),
        ).expect("another persisted operation");
        let mut changed = input.clone();
        changed.operation_id = operation_id.parse().expect("operation id");
        assert_error(&store, &changed, ChatError::Conflict);
        let (other_chat, other_request, other_fence) =
            running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let other = store
            .admit_chat_approval(&admission(other_request, other_fence))
            .expect("other challenge")
            .approval;
        let mut other_lookup = input.clone();
        other_lookup.chat_id = other_chat;
        other_lookup.request_id = other_request;
        other_lookup.approval_id = other.approval_id;
        assert_error(&store, &other_lookup, ChatError::Conflict);
    }

    #[test]
    fn lookup_rejects_invalid_operation_request_and_numeric_bindings() {
        let (store, _, _, input, _) = fixture();
        for changed in [
            ChatApprovalDecisionLookup {
                operation_id: Uuid::nil(),
                ..input.clone()
            },
            ChatApprovalDecisionLookup {
                request_id: Uuid::nil(),
                ..input.clone()
            },
            ChatApprovalDecisionLookup {
                generation: 0,
                ..input.clone()
            },
            ChatApprovalDecisionLookup {
                generation: i64::MAX as u64 + 1,
                ..input.clone()
            },
            ChatApprovalDecisionLookup {
                expected_version: 0,
                ..input.clone()
            },
            ChatApprovalDecisionLookup {
                expected_version: i64::MAX as u64 + 1,
                ..input.clone()
            },
        ] {
            assert_error(&store, &changed, ChatError::InvalidInput);
        }
    }

    #[test]
    fn turn_dispatch_and_execution_requests_cannot_be_decision_operations() {
        let (mut store, _, _, input, _) = fixture();
        let read_chat = store
            .open_chat(&ChatOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Claude,
                profile_sha256: "a".repeat(64),
                resource_key: "2".repeat(64),
                base_commit: "b".repeat(40),
                title_ref: None,
                timestamp: now(),
            })
            .expect("read-only chat")
            .session;
        let read_fence = store
            .claim_chat_owner(read_chat.chat_id, 1, None, now())
            .expect("read-only owner")
            .fence;
        store
            .mark_chat_transport_ready(read_chat.chat_id, 1, read_fence, now())
            .expect("read-only ready");
        let turn_request = Uuid::new_v4();
        store
            .admit_chat_turn(&ChatTurnAdmission {
                chat_id: read_chat.chat_id,
                request_id: turn_request,
                expected_generation: 1,
                base_commit: "b".repeat(40),
                prompt_ref: ContentRef {
                    blob_id: Uuid::new_v4(),
                    sha256: "3".repeat(64),
                    byte_count: 16,
                },
                timestamp: now(),
            })
            .expect("real turn request");
        let dispatch_request = Uuid::new_v4();
        store
            .admit_harness_dispatch(&HarnessDispatchAdmission {
                request_id: dispatch_request,
                harness: AgentKind::Claude,
                protocol: NativeProtocol::ClaudeStreamJson,
                prompt: PromptDigest {
                    sha256: Sha256Digest::of(b"fixture prompt"),
                    byte_count: 14,
                },
                config_sha256: Sha256Digest::of(b"fixture config"),
                resource_key: Sha256Digest::of(b"receipt dispatch resource"),
                base_commit: "b".repeat(40),
                timestamp: now(),
            })
            .expect("real dispatch request");
        for operation_id in [turn_request, dispatch_request, input.request_id] {
            let changed = ChatApprovalDecisionLookup {
                operation_id,
                ..input.clone()
            };
            assert_error(&store, &changed, ChatError::Conflict);
        }
    }

    #[test]
    fn process_release_and_recovery_preserve_historical_receipts_without_authority() {
        for clean_release in [true, false] {
            let (mut store, chat_id, fence, input, choose) = fixture();
            let mut committed = store.decide_chat_approval(&choose).expect("decision");
            let claim = committed.delivery_claim.take().expect("fresh authority");
            if clean_release {
                store
                    .change_chat_execution_process(
                        &ChatMutation {
                            operation_id: Uuid::new_v4(),
                            chat_id,
                            expected_generation: 1,
                            timestamp: now(),
                        },
                        fence,
                        &ChatExecutionProcessChange::Released {
                            cleanup_verified: true,
                            closed: false,
                        },
                    )
                    .expect("verified process release");
            } else {
                let recovered = store
                    .recover_chat_execution_processes(now())
                    .expect("recovery");
                assert!(recovered.quarantined.is_empty());
                assert_eq!(recovered.sessions.len(), 1);
            }
            let receipt = lookup_read_only(&store, &input)
                .expect("historical lookup")
                .expect("receipt");
            assert_receipt(&receipt, committed.sequence, ChatApprovalDelivery::Unknown);
            assert!(
                store
                    .complete_chat_approval_delivery(&claim, ChatApprovalDelivery::Written, now())
                    .is_err()
            );
            let receipt = lookup_read_only(&store, &input)
                .expect("repeat historical lookup")
                .expect("receipt");
            assert_receipt(&receipt, committed.sequence, ChatApprovalDelivery::Unknown);
        }
    }
}
