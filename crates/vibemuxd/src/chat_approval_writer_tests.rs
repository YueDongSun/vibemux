//! The real private-file/writer boundary, without a vendor or public API.
use super::*;
use serde_json::json;
use std::collections::BTreeSet;
use vibemux_store::ChatApprovalDecisionInput;
use vibemux_types::{
    ToolName,
    chat_approval::{ChatApprovalDecision, ChatApprovalDelivery},
};

const PRIVATE_PATH: &str = "private_workspace/approval_中文.txt";
const PRIVATE_CONTENT: &str = "PRIVATE_APPROVAL_BODY_中文";
const PRIVATE_RPC: &str = "private_vendor_request_identity";
const PRIVATE_TOOL: &str = "private_vendor_tool_identity";

async fn running(
    chats: &ChatWriter,
) -> (
    vibemux_types::chat_owner::ChatExecutionOwner,
    Uuid,
    ChatOwnerFence,
    String,
) {
    let (owner, fence) =
        execution_ready_with_tools(chats, BTreeSet::from([ToolName::WriteFiles])).await;
    let request_id = Uuid::new_v4();
    chats
        .admit_execution_attempt(ChatExecutionPrompt {
            chat_id: owner.chat_id,
            request_id,
            generation: 1,
            owner_version: owner.version,
            observed_commit: owner.base_commit.clone(),
            effective_tools: BTreeSet::from([ToolName::WriteFiles]),
            prompt: PROMPT.into(),
        })
        .await
        .expect("admit turn");
    chats
        .claim_execution_attempt(request_id, 1, fence)
        .await
        .expect("claim turn");
    chats
        .bind_execution_attempt(request_id, 1, fence, request_id.to_string())
        .await
        .expect("acknowledgement");
    let vendor = chats
        .execution_owner(owner.chat_id)
        .await
        .expect("state")
        .expect("owner")
        .session
        .vendor_session_id
        .expect("vendor");
    (owner, request_id, fence, vendor)
}
fn evidence(
    owner: &vibemux_types::chat_owner::ChatExecutionOwner,
    request_id: Uuid,
    fence: ChatOwnerFence,
    vendor: String,
) -> ChatApprovalWrite {
    ChatApprovalWrite {
        evidence_id: Uuid::new_v4(),
        fence,
        expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(120),
        evidence: ChatApprovalEvidence {
            schema_version: 1,
            chat_id: owner.chat_id,
            request_id,
            generation: 1,
            vendor_session_id: vendor,
            vendor_request_id: PRIVATE_RPC.into(),
            tool_use_id: PRIVATE_TOOL.into(),
            tool: ToolName::WriteFiles,
            vendor_tool_name: "Write".into(),
            input: json!({"file_path":PRIVATE_PATH,"content":PRIVATE_CONTENT}),
        },
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
        timestamp: OffsetDateTime::now_utc(),
    }
}
async fn shutdown(worker: WriterWorker) {
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_approval_evidence_survives_reopen_and_decision_replay_has_no_dispatch() {
    let temp = tempfile::tempdir().expect("fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let write = evidence(&owner, request_id, fence, vendor);
    let admitted = chats
        .admit_approval(write.clone())
        .await
        .expect("private admission");
    let replay = chats
        .admit_approval(write.clone())
        .await
        .expect("admission replay");
    assert!(replay.duplicate);
    assert_eq!(replay.approval, admitted.approval);
    let choose = decision(&admitted.approval, fence);
    let mut decided = chats
        .decide_approval(choose.clone())
        .await
        .expect("decision");
    let dispatch = decided.dispatch.take().expect("one private dispatch");
    assert!(!format!("{dispatch:?}").contains(PRIVATE_CONTENT));
    let (claim, body) = dispatch.into_parts();
    assert_eq!(body, write.evidence);
    chats
        .complete_approval_delivery(claim, ChatApprovalDelivery::Written)
        .await
        .expect("write receipt");
    assert!(
        chats
            .decide_approval(choose.clone())
            .await
            .expect("same choice")
            .dispatch
            .is_none()
    );
    drop(chats);
    shutdown(worker).await;
    let canonical = std::fs::read(temp.path().join("state.sqlite3")).expect("database");
    for marker in [PRIVATE_PATH, PRIVATE_CONTENT, PRIVATE_RPC, PRIVATE_TOOL] {
        assert!(
            !canonical
                .windows(marker.len())
                .any(|part| part == marker.as_bytes()),
            "private evidence in canonical DB"
        );
    }
    let (worker, chats) = start(temp.path().to_owned()).await;
    let retained = chats
        .approval_content(owner.chat_id, admitted.approval.approval_id)
        .await
        .expect("private read");
    assert_eq!(retained.evidence, Some(write.evidence));
    assert!(retained.content_error.is_none());
    assert!(
        chats
            .approval_content(ChatId::new(), admitted.approval.approval_id)
            .await
            .is_err()
    );
    let replay = chats
        .decide_approval(choose)
        .await
        .expect("historical decision");
    assert!(replay.receipt.duplicate);
    assert!(replay.dispatch.is_none());
    assert_eq!(
        replay.receipt.approval.delivery,
        Some(ChatApprovalDelivery::Written)
    );
    drop(chats);
    shutdown(worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_changed_private_evidence_prevents_a_fresh_decision() {
    for missing in [true, false] {
        let temp = tempfile::tempdir().expect("fixture");
        let (worker, chats) = start(temp.path().to_owned()).await;
        let (owner, request_id, fence, vendor) = running(&chats).await;
        let write = evidence(&owner, request_id, fence, vendor);
        let approval = chats
            .admit_approval(write.clone())
            .await
            .expect("admit")
            .approval;
        let path = temp
            .path()
            .join("chat_private")
            .join(format!("{}.blob", write.evidence_id.simple()));
        if missing {
            std::fs::remove_file(&path).expect("owned missing body");
        } else {
            std::fs::write(&path, b"changed owned body").expect("owned tamper fixture");
        }
        let before = chats
            .execute(|store, _| store.events().map_err(Into::into))
            .await
            .expect("events");
        for choice in [ChatApprovalDecision::AllowOnce, ChatApprovalDecision::Deny] {
            let mut choose = decision(&approval, fence);
            choose.decision = choice;
            assert!(chats.decide_approval(choose).await.is_err());
        }
        let current = chats
            .approval_content(owner.chat_id, approval.approval_id)
            .await
            .expect("explicit private read");
        assert!(current.evidence.is_none());
        assert!(current.content_error.is_some());
        assert_eq!(current.approval, approval);
        assert_eq!(
            chats
                .execute(|store, _| store.events().map_err(Into::into))
                .await
                .expect("events"),
            before
        );
        let replay = chats
            .admit_approval(write)
            .await
            .expect("old admission receipt");
        assert!(replay.duplicate);
        if missing {
            assert!(!path.exists(), "replay must not recreate body");
        } else {
            assert_eq!(
                std::fs::read(&path).expect("retain tampered file"),
                b"changed owned body"
            );
        }
        drop(chats);
        shutdown(worker).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decided_replay_needs_no_private_body_and_changed_reuse_preserves_original_bytes() {
    let temp = tempfile::tempdir().expect("fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let write = evidence(&owner, request_id, fence, vendor);
    let approval = chats
        .admit_approval(write.clone())
        .await
        .expect("admit")
        .approval;
    let path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", write.evidence_id.simple()));
    let original = std::fs::read(&path).expect("evidence bytes");
    let mut changed = write.clone();
    changed.evidence.input["content"] = json!("different");
    assert!(chats.admit_approval(changed).await.is_err());
    assert_eq!(std::fs::read(&path).expect("retained bytes"), original);
    let choose = decision(&approval, fence);
    let result = chats
        .decide_approval(choose.clone())
        .await
        .expect("fresh choice");
    let (claim, _) = result.dispatch.expect("one dispatch").into_parts();
    chats
        .complete_approval_delivery(claim, ChatApprovalDelivery::Unknown)
        .await
        .expect("unknown observation");
    std::fs::remove_file(&path).expect("owned missing body");
    let replay = chats
        .decide_approval(choose.clone())
        .await
        .expect("historical receipt");
    assert!(replay.receipt.duplicate);
    assert!(replay.dispatch.is_none());
    assert!(!path.exists());
    let mut changed = choose;
    changed.decision = ChatApprovalDecision::Deny;
    assert!(chats.decide_approval(changed).await.is_err());
    assert!(!path.exists());
    drop(chats);
    shutdown(worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_private_admission_removes_only_its_unreferenced_blob() {
    let temp = tempfile::tempdir().expect("fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let original = evidence(&owner, request_id, fence, vendor);
    let prompt = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", request_id.simple()));
    let prompt_bytes = std::fs::read(&prompt).expect("prompt");
    let before = chats
        .execute(|store, _| store.events().map_err(Into::into))
        .await
        .expect("events");
    for variant in 0..6 {
        let mut write = original.clone();
        write.evidence_id = Uuid::new_v4();
        let path = temp
            .path()
            .join("chat_private")
            .join(format!("{}.blob", write.evidence_id.simple()));
        match variant {
            0 => {
                write.evidence.tool = ToolName::EditFiles;
                write.evidence.vendor_tool_name = "Edit".into();
            }
            1 => write.evidence.vendor_tool_name = "Bash".into(),
            2 => write.evidence.vendor_session_id = Uuid::new_v4().to_string(),
            3 => write.evidence.chat_id = ChatId::new(),
            4 => {
                write.evidence.input = json!({"content":"x".repeat(vibemux_types::chat::MAX_CHAT_RECORD_BYTES as usize+1)})
            }
            5 => write.expires_at = OffsetDateTime::now_utc() - time::Duration::seconds(1),
            _ => unreachable!("matrix"),
        }
        assert!(
            chats.admit_approval(write).await.is_err(),
            "variant {variant}"
        );
        assert!(!path.exists(), "failed body retained for variant {variant}");
        assert_eq!(
            std::fs::read(&prompt).expect("untouched prompt"),
            prompt_bytes
        );
    }
    assert_eq!(
        chats
            .execute(|store, _| store.events().map_err(Into::into))
            .await
            .expect("events"),
        before
    );
    assert_eq!(
        chats
            .execution_prompt(request_id)
            .await
            .expect("private prompt"),
        PROMPT
    );
    drop(chats);
    shutdown(worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_private_decisions_return_only_one_payload_and_claim() {
    for same_operation in [true, false] {
        let temp = tempfile::tempdir().expect("fixture");
        let (worker, chats) = start(temp.path().to_owned()).await;
        let (owner, request_id, fence, vendor) = running(&chats).await;
        let write = evidence(&owner, request_id, fence, vendor);
        let approval = chats.admit_approval(write).await.expect("admit").approval;
        let left = decision(&approval, fence);
        let mut right = left.clone();
        if !same_operation {
            right.operation_id = Uuid::new_v4();
            right.decision = ChatApprovalDecision::Deny;
        }
        let (left, right) = tokio::join!(chats.decide_approval(left), chats.decide_approval(right));
        let results = [left, right];
        assert_eq!(
            results
                .iter()
                .filter(|r| r.as_ref().is_ok_and(|v| v.dispatch.is_some()))
                .count(),
            1
        );
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if same_operation { 2 } else { 1 }
        );
        assert!(
            results
                .iter()
                .filter_map(|r| r.as_ref().ok())
                .all(|v| v.receipt.delivery_claim.is_none())
        );
        drop(chats);
        shutdown(worker).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_private_decision_keeps_its_dispatch_beyond_the_sync_writer_deadline() {
    let temp = tempfile::tempdir().expect("fixture");
    let (worker, mut chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let approval = chats
        .admit_approval(evidence(&owner, request_id, fence, vendor))
        .await
        .expect("admit")
        .approval;
    let choose = decision(&approval, fence);
    let replay = choose.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let handle = worker.handle().expect("handle");
    let blocker = tokio::task::spawn_blocking(move || {
        handle.with_store(move |_| {
            let _ = entered.send(());
            let _ = blocked.recv();
            Ok(())
        })
    });
    observed.await.expect("writer blocked");
    chats.writer.response_timeout = std::time::Duration::from_millis(5);
    let queued = chats.clone();
    let mut task = tokio::spawn(async move { queued.decide_approval(choose).await });
    let early = tokio::time::timeout(std::time::Duration::from_millis(100), &mut task).await;
    release.send(()).expect("release");
    blocker.await.expect("blocker task").expect("blocker");
    assert!(
        early.is_err(),
        "accepted decision must wait for its actual result"
    );
    let result = task
        .await
        .expect("decision task")
        .expect("actual completion");
    assert!(result.dispatch.is_some());
    assert!(!result.receipt.duplicate);
    assert!(
        chats
            .decide_approval(replay)
            .await
            .expect("receipt")
            .dispatch
            .is_none()
    );
    drop(chats);
    shutdown(worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_private_decision_checks_expiry_at_commit_instead_of_client_submission() {
    let temp = tempfile::tempdir().expect("fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let mut write = evidence(&owner, request_id, fence, vendor);
    write.expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    let approval = chats.admit_approval(write).await.expect("admit").approval;
    let choose = decision(&approval, fence);
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let handle = worker.handle().expect("handle");
    let blocker = tokio::task::spawn_blocking(move || {
        handle.with_store(move |_| {
            let _ = entered.send(());
            let _ = blocked.recv();
            Ok(())
        })
    });
    observed.await.expect("writer blocked");
    let queued = chats.clone();
    let task = tokio::spawn(async move { queued.decide_approval(choose).await });
    let remaining = (approval.expires_at - OffsetDateTime::now_utc())
        .try_into()
        .unwrap_or(std::time::Duration::ZERO);
    tokio::time::sleep(remaining + std::time::Duration::from_millis(50)).await;
    release.send(()).expect("release");
    blocker.await.expect("blocker task").expect("blocker");
    assert!(task.await.expect("decision task").is_err());
    let current = chats
        .approval_content(owner.chat_id, approval.approval_id)
        .await
        .expect("state");
    assert_eq!(current.approval, approval);
    assert!(current.evidence.is_some());
    drop(chats);
    shutdown(worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_vendor_identity_must_match_the_immutable_ledger_key() {
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, request_id, fence, vendor) = running(&chats).await;
    let approval = chats
        .admit_approval(evidence(&owner, request_id, fence, vendor))
        .await
        .expect("admit")
        .approval;
    let choose = decision(&approval, fence);
    drop(chats);
    shutdown(worker).await;
    let raw = rusqlite::Connection::open(&database).expect("owned closed corruption fixture");
    raw.execute("UPDATE chat_approvals SET vendor_request_key=?1,binding_json=json_set(binding_json,'$.vendor_request_key',?1)",["9".repeat(64)]).expect("inject key mismatch");
    drop(raw);
    let (worker, chats) = start(temp.path().to_owned()).await;
    let before = chats
        .execute(|store, _| store.events().map_err(Into::into))
        .await
        .expect("events");
    let current = chats
        .approval_content(owner.chat_id, approval.approval_id)
        .await
        .expect("private read");
    assert!(current.evidence.is_none());
    assert!(current.content_error.is_some());
    assert!(chats.decide_approval(choose).await.is_err());
    assert_eq!(
        chats
            .execute(|store, _| store.events().map_err(Into::into))
            .await
            .expect("events"),
        before
    );
    drop(chats);
    shutdown(worker).await;
}
