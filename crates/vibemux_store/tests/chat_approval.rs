//! Decisions are durable receipts; only a fresh atomic claim grants delivery.
use std::collections::{BTreeMap, BTreeSet};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::{
    ChatApprovalAdmission, ChatApprovalDecisionInput, ChatExecutionAttemptAdmission,
    ChatExecutionOwnerOpen, ChatExecutionProcessChange, ChatExecutionStart, ChatMutation,
    ChatOwnerChange, ChatOwnerFence, ChatOwnerMutation, SqliteStore,
};
use vibemux_types::{
    ChatId, ToolName,
    a2a::RunWorkspace,
    chat::{ChatHarness, ContentRef},
    chat_approval::{ChatApprovalDecision, ChatApprovalDelivery, ChatApprovalState},
};

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("time")
}
fn running(store: &mut SqliteStore, tools: BTreeSet<ToolName>) -> (ChatId, Uuid, ChatOwnerFence) {
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

#[test]
fn decision_replay_never_reissues_delivery_and_changed_reuse_conflicts() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let input = admission(request, fence);
    let before_lookup = store.events().expect("events");
    assert!(
        store
            .replay_chat_approval_admission(&input)
            .expect("read-only lookup")
            .is_none()
    );
    assert_eq!(store.events().expect("no admission effect"), before_lookup);
    let challenge = store.admit_chat_approval(&input).expect("challenge");
    assert!(
        store
            .replay_chat_approval_admission(&input)
            .expect("read-only replay")
            .expect("receipt")
            .delivery_claim
            .is_none()
    );
    let duplicate = store.admit_chat_approval(&input).expect("request replay");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.approval, challenge.approval);
    let choose = decision(&challenge.approval, fence);
    let mut claimed = store.decide_chat_approval(&choose).expect("decision");
    assert_eq!(
        claimed.approval.delivery,
        Some(ChatApprovalDelivery::Claimed)
    );
    let capability = claimed
        .delivery_claim
        .take()
        .expect("one delivery authority");
    let replay = store
        .decide_chat_approval(&choose)
        .expect("decision replay");
    assert!(replay.duplicate);
    assert!(replay.delivery_claim.is_none());
    let mut changed = choose.clone();
    changed.decision = ChatApprovalDecision::Deny;
    assert!(store.decide_chat_approval(&changed).is_err());
    let completed = store
        .complete_chat_approval_delivery(&capability, ChatApprovalDelivery::Written, now())
        .expect("write receipt");
    assert_eq!(
        completed.approval.delivery,
        Some(ChatApprovalDelivery::Written)
    );
    assert!(
        store
            .decide_chat_approval(&choose)
            .expect("late replay")
            .delivery_claim
            .is_none()
    );
    assert!(
        store
            .complete_chat_approval_delivery(&capability, ChatApprovalDelivery::Unknown, now())
            .is_err()
    );
    assert!(!format!("{:?}", completed.approval).contains(&"1".repeat(64)));
}

#[test]
fn cancellation_before_claim_prevents_allow_and_after_claim_retains_decision() {
    for claim_first in [false, true] {
        let temp = tempfile::tempdir().expect("fixture");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request, fence))
            .expect("challenge")
            .approval;
        let choose = decision(&approval, fence);
        if claim_first {
            store.decide_chat_approval(&choose).expect("claim first");
        }
        store
            .cancel_chat_execution_attempt(request, 1, fence, now())
            .expect("cancel");
        let current = store
            .chat_approval(approval.approval_id)
            .expect("read")
            .expect("record");
        if claim_first {
            assert_eq!(current.decision, Some(ChatApprovalDecision::AllowOnce));
            assert_eq!(current.delivery, Some(ChatApprovalDelivery::Claimed));
            assert!(
                store
                    .decide_chat_approval(&choose)
                    .expect("historical receipt")
                    .delivery_claim
                    .is_none()
            );
        } else {
            assert_eq!(current.state, ChatApprovalState::Invalidated);
            assert!(current.decision.is_none());
            assert!(store.decide_chat_approval(&choose).is_err());
        }
    }
}

#[test]
fn expiry_is_bounded_and_backdated_decisions_cannot_extend_it() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let mut input = admission(request, fence);
    input.expires_at += Duration::seconds(1);
    assert!(store.admit_chat_approval(&input).is_err());
    input.expires_at = now() + Duration::seconds(120);
    let challenge = store
        .admit_chat_approval(&input)
        .expect("challenge")
        .approval;
    let mut choose = decision(&challenge, fence);
    choose.timestamp = now() - Duration::seconds(1);
    assert!(
        store.decide_chat_approval(&choose).is_err(),
        "backdated choice"
    );
    choose.timestamp = input.expires_at;
    assert!(
        store.decide_chat_approval(&choose).is_err(),
        "expiry is exclusive"
    );
    assert!(
        store
            .expire_chat_approval(
                challenge.approval_id,
                1,
                fence,
                input.expires_at - Duration::seconds(1)
            )
            .is_err()
    );
    let expired = store
        .expire_chat_approval(challenge.approval_id, 1, fence, input.expires_at)
        .expect("expire");
    assert_eq!(expired.approval.state, ChatApprovalState::Expired);
    assert!(expired.approval.decision.is_none());
    assert!(
        store
            .expire_chat_approval(challenge.approval_id, 1, fence, input.expires_at)
            .expect("replay")
            .duplicate
    );
    choose.timestamp = now();
    assert!(store.decide_chat_approval(&choose).is_err());
}

#[test]
fn racing_decisions_have_one_winner_and_identical_replays_have_no_authority() {
    for same_operation in [true, false] {
        let temp = tempfile::tempdir().expect("fixture");
        let database = temp.path().join("state.sqlite3");
        let mut store = SqliteStore::open(&database).expect("store");
        let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request, fence))
            .expect("challenge")
            .approval;
        let choose = decision(&approval, fence);
        let before = store.events().expect("events").len();
        drop(store);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads = (0..4)
            .map(|i| {
                let database = database.clone();
                let barrier = barrier.clone();
                let mut input = choose.clone();
                if !same_operation {
                    input.operation_id = Uuid::new_v4();
                    if i % 2 == 0 {
                        input.decision = ChatApprovalDecision::Deny;
                    }
                }
                std::thread::spawn(move || {
                    let mut store = SqliteStore::open(&database).expect("connection");
                    barrier.wait();
                    store.decide_chat_approval(&input)
                })
            })
            .collect::<Vec<_>>();
        let receipts = threads
            .into_iter()
            .map(|t| t.join().expect("join"))
            .collect::<Vec<_>>();
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r.as_ref().is_ok_and(|v| v.delivery_claim.is_some()))
                .count(),
            1
        );
        assert_eq!(
            receipts.iter().filter(|r| r.is_ok()).count(),
            if same_operation { 4 } else { 1 }
        );
        let store = SqliteStore::open(&database).expect("reopen");
        assert_eq!(store.events().expect("one event").len(), before + 1);
    }
}

#[test]
fn restart_invalidates_pending_and_seals_only_unobserved_delivery_as_unknown() {
    for delivery in [
        None,
        Some(ChatApprovalDelivery::Claimed),
        Some(ChatApprovalDelivery::Written),
    ] {
        let temp = tempfile::tempdir().expect("fixture");
        let database = temp.path().join("state.sqlite3");
        let mut store = SqliteStore::open(&database).expect("store");
        let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request, fence))
            .expect("challenge")
            .approval;
        let choose = decision(&approval, fence);
        let claim = delivery.map(|_| {
            store
                .decide_chat_approval(&choose)
                .expect("decision")
                .delivery_claim
                .expect("claim")
        });
        if delivery == Some(ChatApprovalDelivery::Written) {
            store
                .complete_chat_approval_delivery(
                    claim.as_ref().expect("claim"),
                    ChatApprovalDelivery::Written,
                    now(),
                )
                .expect("written");
        }
        drop(store);
        let mut store = SqliteStore::open(&database).expect("restart");
        let recovered = store
            .recover_chat_execution_processes(now())
            .expect("recover");
        assert!(recovered.quarantined.is_empty());
        let current = store
            .chat_approval(approval.approval_id)
            .expect("read")
            .expect("record");
        match delivery {
            None => {
                assert_eq!(current.state, ChatApprovalState::Invalidated);
                assert!(store.decide_chat_approval(&choose).is_err());
            }
            Some(ChatApprovalDelivery::Claimed) => {
                assert_eq!(current.delivery, Some(ChatApprovalDelivery::Unknown));
                assert!(
                    store
                        .complete_chat_approval_delivery(
                            claim.as_ref().expect("claim"),
                            ChatApprovalDelivery::Written,
                            now()
                        )
                        .is_err()
                );
            }
            Some(ChatApprovalDelivery::Written) => assert_eq!(current.delivery, delivery),
            _ => unreachable!("matrix"),
        }
        if delivery.is_some() {
            assert!(
                store
                    .decide_chat_approval(&choose)
                    .expect("receipt")
                    .delivery_claim
                    .is_none()
            );
        }
        let events = store.events().expect("events");
        store
            .recover_chat_execution_processes(now())
            .expect("repeat recovery");
        assert_eq!(store.events().expect("no repeated event"), events);
    }
}

#[test]
fn admission_and_claim_recheck_narrowed_authority_identity_and_private_limits() {
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let (chat_id, request, fence) = running_with_tools(
        &mut store,
        BTreeSet::from([
            ToolName::WriteFiles,
            ToolName::EditFiles,
            ToolName::RunDevTests,
        ]),
        BTreeSet::from([ToolName::WriteFiles, ToolName::RunDevTests]),
    );
    let (_, _, foreign_fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let original = admission(request, fence);
    let events = store.events().expect("events");
    let before = store
        .chat_execution_owner_state(chat_id)
        .expect("state")
        .expect("owner")
        .session
        .retained_bytes;
    for variant in 0..7 {
        let mut input = original.clone();
        match variant {
            0 => input.tool = ToolName::EditFiles,
            1 => input.tool = ToolName::RunDevTests,
            2 => input.generation = 2,
            3 => input.fence = foreign_fence,
            4 => input.evidence_ref.byte_count = vibemux_types::chat::MAX_CHAT_RECORD_BYTES + 1,
            5 => input.evidence_ref.blob_id = request,
            6 => input.timestamp -= Duration::seconds(1),
            _ => unreachable!("matrix"),
        }
        assert!(
            store.admit_chat_approval(&input).is_err(),
            "variant {variant}"
        );
    }
    assert_eq!(store.events().expect("rollback events"), events);
    assert_eq!(
        store
            .chat_execution_owner_state(chat_id)
            .expect("state")
            .expect("owner")
            .session
            .retained_bytes,
        before
    );
    let approval = store
        .admit_chat_approval(&original)
        .expect("valid")
        .approval;
    assert_eq!(
        store
            .chat_execution_owner_state(chat_id)
            .expect("state")
            .expect("owner")
            .session
            .retained_bytes,
        before + 128
    );
    let mut second = original.clone();
    second.vendor_request_key = "2".repeat(64);
    second.tool_use_key = "3".repeat(64);
    second.evidence_ref.blob_id = Uuid::new_v4();
    assert!(
        store.admit_chat_approval(&second).is_err(),
        "only one outstanding challenge"
    );
    let choose = decision(&approval, fence);
    for variant in 0..5 {
        let mut input = choose.clone();
        match variant {
            0 => input.expected_version += 1,
            1 => input.generation += 1,
            2 => input.fence = foreign_fence,
            3 => input.profile_sha256 = "4".repeat(64),
            4 => input.observed_commit = "5".repeat(40),
            _ => unreachable!("matrix"),
        }
        assert!(
            store.decide_chat_approval(&input).is_err(),
            "decision variant {variant}"
        );
    }
    assert_eq!(
        store.chat_approval(approval.approval_id).expect("read"),
        Some(approval)
    );
    assert!(store.chat_approvals(chat_id, 0, 0).is_err());
    assert_eq!(store.chat_approvals(chat_id, 0, 1).expect("page").len(), 1);
}

#[test]
fn denied_delivery_is_one_use_and_retired_tool_identity_cannot_be_rebound() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let (chat_id, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let original = admission(request, fence);
    let approval = store
        .admit_chat_approval(&original)
        .expect("challenge")
        .approval;
    let mut choose = decision(&approval, fence);
    choose.decision = ChatApprovalDecision::Deny;
    let claimed = store.decide_chat_approval(&choose).expect("deny");
    assert_eq!(claimed.approval.decision, Some(ChatApprovalDecision::Deny));
    let capability = claimed.delivery_claim.expect("one denial response");
    let done = store
        .complete_chat_approval_delivery(&capability, ChatApprovalDelivery::Written, now())
        .expect("written");
    assert!(
        store
            .complete_chat_approval_delivery(&capability, ChatApprovalDelivery::Written, now())
            .expect("identical acknowledgement")
            .duplicate
    );
    let before = store.events().expect("events");
    let mut second = original.clone();
    second.vendor_request_key = "2".repeat(64);
    second.evidence_ref.blob_id = Uuid::new_v4();
    assert!(
        store.admit_chat_approval(&second).is_err(),
        "retired tool ID"
    );
    second.vendor_request_key = original.vendor_request_key.clone();
    second.tool_use_key = "3".repeat(64);
    assert!(
        store.admit_chat_approval(&second).is_err(),
        "changed original request"
    );
    assert_eq!(store.events().expect("no effect"), before);
    second.vendor_request_key = "2".repeat(64);
    let fresh = store
        .admit_chat_approval(&second)
        .expect("fresh tool request");
    assert_ne!(fresh.approval.approval_id, done.approval.approval_id);
    assert_eq!(
        store
            .chat_approvals(chat_id, done.approval.admitted_sequence, 16)
            .expect("cursor"),
        vec![fresh.approval]
    );
    let mut collision = decision(&approval, fence);
    collision.operation_id = request;
    assert!(store.decide_chat_approval(&collision).is_err());
    assert!(
        store
            .change_chat_execution_process(
                &ChatMutation {
                    operation_id: choose.operation_id,
                    chat_id,
                    expected_generation: 1,
                    timestamp: now(),
                },
                fence,
                &ChatExecutionProcessChange::Ready
            )
            .is_err(),
        "operation namespace collision"
    );
}

#[test]
fn terminal_success_refuses_unresolved_approvals_and_accepts_written_observation() {
    for complete in [false, true] {
        let temp = tempfile::tempdir().expect("fixture");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let (chat_id, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request, fence))
            .expect("challenge")
            .approval;
        let state = store
            .chat_execution_owner_state(chat_id)
            .expect("state")
            .expect("owner");
        let finish = vibemux_store::ChatTurnFinish {
            request_id: request,
            generation: 1,
            fence,
            outcome: vibemux_types::chat::ChatTurnOutcome::Completed,
            correlated_terminal: true,
            vendor_session_id: state.session.vendor_session_id,
            vendor_turn_id: Some("tool_turn".into()),
            error_code: None,
            process_alive: true,
            timestamp: now(),
            output_complete: true,
        };
        let before = store.events().expect("events");
        assert!(
            store.finish_chat_execution_attempt(&finish).is_err(),
            "pending challenge"
        );
        assert_eq!(store.events().expect("no partial finish"), before);
        let choose = decision(&approval, fence);
        let capability = store
            .decide_chat_approval(&choose)
            .expect("decision")
            .delivery_claim
            .expect("claim");
        assert!(
            store.finish_chat_execution_attempt(&finish).is_err(),
            "claimed response"
        );
        store
            .complete_chat_approval_delivery(
                &capability,
                if complete {
                    ChatApprovalDelivery::Written
                } else {
                    ChatApprovalDelivery::Unknown
                },
                now(),
            )
            .expect("observation");
        assert_eq!(
            store.finish_chat_execution_attempt(&finish).is_ok(),
            complete
        );
    }
}

#[test]
fn schema_twelve_upgrade_is_atomic_and_preserves_owner_attempt_and_event_history() {
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let (chat_id, request, _) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let owner = store
        .chat_execution_owner_state(chat_id)
        .expect("state")
        .expect("owner");
    let attempt = store.chat_execution_attempt(request).expect("attempt");
    let events = store.events().expect("events");
    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("closed fixture");
    let original: Vec<String> = {
        let mut statement=raw.prepare("SELECT record_json FROM chat_sessions UNION ALL SELECT record_json FROM chat_execution_owners UNION ALL SELECT record_json FROM chat_execution_attempts").expect("query");
        statement
            .query_map([], |r| r.get(0))
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("history")
    };
    raw.execute_batch("DROP TABLE workspace_cleanup_commands; DROP TABLE workspace_cleanups; DROP TABLE chat_approvals; DELETE FROM schema_migrations WHERE version>=13; CREATE INDEX chat_approval_pending ON chat_sessions(resource_key);").expect("schema12 and late fault");
    assert!(SqliteStore::open(&database).is_err());
    assert_eq!(
        raw.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r
            .get::<_, u32>(0))
            .expect("version"),
        12
    );
    assert!(!raw.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chat_approvals' AND type='table')",[],|r|r.get::<_,bool>(0)).expect("atomic rollback"));
    raw.execute_batch("DROP INDEX chat_approval_pending;")
        .expect("remove fault");
    drop(raw);
    let store = SqliteStore::open(&database).expect("forward upgrade");
    let restored = store
        .chat_execution_owner_state(chat_id)
        .expect("state")
        .expect("owner");
    assert_eq!(restored.owner, owner.owner);
    assert_eq!(restored.session, owner.session);
    assert_eq!(restored.task, owner.task);
    assert_eq!(restored.run, owner.run);
    assert_eq!(
        store.chat_execution_attempt(request).expect("attempt"),
        attempt
    );
    assert_eq!(store.events().expect("events"), events);
    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("raw verification");
    let after: Vec<String> = {
        let mut statement=raw.prepare("SELECT record_json FROM chat_sessions UNION ALL SELECT record_json FROM chat_execution_owners UNION ALL SELECT record_json FROM chat_execution_attempts").expect("query");
        statement
            .query_map([], |r| r.get(0))
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("history")
    };
    assert_eq!(after, original, "historical bytes");
}

#[test]
fn approval_clock_does_not_regress_between_tools_or_behind_cancellation() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
    let original = admission(request, fence);
    let approval = store
        .admit_chat_approval(&original)
        .expect("challenge")
        .approval;
    let capability = store
        .decide_chat_approval(&decision(&approval, fence))
        .expect("decision")
        .delivery_claim
        .expect("claim");
    store
        .complete_chat_approval_delivery(
            &capability,
            ChatApprovalDelivery::Written,
            now() + Duration::seconds(50),
        )
        .expect("written");
    let mut next = original;
    next.vendor_request_key = "2".repeat(64);
    next.tool_use_key = "3".repeat(64);
    next.evidence_ref.blob_id = Uuid::new_v4();
    next.timestamp = now() + Duration::seconds(1);
    assert!(
        store.admit_chat_approval(&next).is_err(),
        "previous tool has a newer clock"
    );
    next.timestamp = now() + Duration::seconds(50);
    let approval = store
        .admit_chat_approval(&next)
        .expect("next tool")
        .approval;
    let mut choose = decision(&approval, fence);
    choose.timestamp = next.timestamp;
    let capability = store
        .decide_chat_approval(&choose)
        .expect("decision")
        .delivery_claim
        .expect("claim");
    store
        .cancel_chat_execution_attempt(request, 1, fence, now() + Duration::seconds(100))
        .expect("cancel");
    assert!(
        store
            .complete_chat_approval_delivery(
                &capability,
                ChatApprovalDelivery::Written,
                now() + Duration::seconds(51)
            )
            .is_err(),
        "newer lifecycle clock"
    );
    store
        .complete_chat_approval_delivery(
            &capability,
            ChatApprovalDelivery::Written,
            now() + Duration::seconds(100),
        )
        .expect("actual later observation");
}

#[test]
fn corrupt_or_oversized_persisted_approval_bindings_fail_closed() {
    for mutation in [
        "UPDATE chat_approvals SET binding_json=json_set(binding_json,'$.tool','edit_files')",
        "UPDATE chat_approvals SET record_json=printf('%20000s','x')",
        "UPDATE chat_approvals SET vendor_request_key=printf('%20000s','x')",
        "UPDATE chat_approvals SET record_json=json_set(record_json,'$.evidence_ref.byte_count',127)",
    ] {
        let temp = tempfile::tempdir().expect("fixture");
        let database = temp.path().join("state.sqlite3");
        let mut store = SqliteStore::open(&database).expect("store");
        let (_, request, fence) = running(&mut store, BTreeSet::from([ToolName::WriteFiles]));
        let approval = store
            .admit_chat_approval(&admission(request, fence))
            .expect("challenge")
            .approval;
        let choose = decision(&approval, fence);
        let before = store.events().expect("events");
        drop(store);
        let raw = rusqlite::Connection::open(&database).expect("owned corruption fixture");
        raw.execute_batch(mutation).expect("inject fault");
        drop(raw);
        let mut store = SqliteStore::open(&database).expect("reopen");
        assert!(
            store.chat_approval(approval.approval_id).is_err(),
            "{mutation}"
        );
        assert!(store.decide_chat_approval(&choose).is_err(), "{mutation}");
        assert_eq!(store.events().expect("no decision event"), before);
    }
}
