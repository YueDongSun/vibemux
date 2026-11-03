use super::*;
use crate::WriterWorker;
use std::collections::BTreeMap;
use vibemux_harness::HarnessDetection;
use vibemux_types::chat::{ChatHarness, ChatState, ChatTurnOutcome};

const PROMPT: &str = "PRIVATE_PROMPT_中文";
const REPLY: &str = "PRIVATE_REPLY_中文";
const TITLE: &str = "PRIVATE_TITLE";
// ACL initialization waits for the shared Windows helper and its 15s budget.
// These fixtures exercise durable content semantics. Writer deadline behavior
// has separate tests in lib.rs.
const FIXTURE_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[path = "chat_approval_writer_tests.rs"]
mod approval_tests;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_execution_admission_retains_completion_beyond_the_writer_response_deadline() {
    let temp = tempfile::tempdir().expect("temporary writer");
    let (worker, mut chats) = start(temp.path().to_path_buf()).await;
    let (owner, _) = execution_ready(&chats).await;
    let request_id = Uuid::new_v4();
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
    observed.await.expect("writer blocker entered");
    chats.writer.response_timeout = std::time::Duration::from_millis(5);
    let mut operation = tokio::spawn(async move {
        chats
            .admit_execution_attempt(ChatExecutionPrompt {
                chat_id: owner.chat_id,
                request_id,
                generation: 1,
                owner_version: owner.version,
                observed_commit: owner.base_commit,
                effective_tools: Default::default(),
                prompt: PROMPT.into(),
            })
            .await
    });
    let early_result =
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut operation).await;
    release.send(()).expect("release writer");
    blocker
        .await
        .expect("blocker task")
        .expect("blocker completed");
    assert!(
        early_result.is_err(),
        "accepted write completion must stay owned while its writer closure is queued: {early_result:?}"
    );
    let admitted = operation
        .await
        .expect("completion task")
        .expect("actual writer admission");
    assert_eq!(admitted.attempt.request_id, request_id);
    assert!(!admitted.duplicate);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_execution_start_retains_its_launch_grant_beyond_the_writer_response_deadline() {
    use vibemux_store::{ChatExecutionProcessChange, ChatExecutionStart};
    let temp = tempfile::tempdir().expect("temporary writer");
    let (worker, mut chats) = start(temp.path().to_path_buf()).await;
    let (owner, fence) = execution_ready(&chats).await;
    chats
        .change_execution_process(
            mutation(owner.chat_id, 1),
            fence,
            ChatExecutionProcessChange::Released {
                cleanup_verified: true,
                closed: true,
            },
        )
        .await
        .expect("release fixture generation");
    let retained = chats
        .execution_owner(owner.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
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
    observed.await.expect("writer blocker entered");
    chats.writer.response_timeout = std::time::Duration::from_millis(5);
    let mut operation = tokio::spawn(async move {
        chats
            .start_execution_process(ChatExecutionStart {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_generation: 1,
                expected_owner_version: retained.owner.version,
                profile_sha256: owner.profile_sha256,
                resource_key: owner.resource_key.expect("resource"),
                observed_commit: owner.base_commit,
                expected_vendor_id: retained.session.vendor_session_id,
                timestamp: OffsetDateTime::now_utc(),
            })
            .await
    });
    let early_result =
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut operation).await;
    release.send(()).expect("release writer");
    blocker
        .await
        .expect("blocker task")
        .expect("blocker completed");
    assert!(
        early_result.is_err(),
        "accepted process claim must retain its fresh grant: {early_result:?}"
    );
    let started = operation
        .await
        .expect("completion task")
        .expect("actual process claim");
    assert!(started.fence.is_some());
    assert!(!started.duplicate);
    assert_eq!(started.session.generation, 2);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

async fn start(state_dir: PathBuf) -> (WriterWorker, ChatWriter) {
    let database = state_dir.join("state.sqlite3");
    let worker = tokio::task::spawn_blocking(move || {
        WriterWorker::start_with_config(
            &database,
            crate::DEFAULT_WRITER_QUEUE_CAPACITY,
            FIXTURE_RESPONSE_TIMEOUT,
        )
    })
    .await
    .expect("start task")
    .expect("writer");
    let handle = worker.handle().expect("handle");
    let seed = handle.clone();
    tokio::task::spawn_blocking(move || {
        seed.commit_harness_snapshot(
            BTreeMap::from([(
                "claude".to_owned(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("fixture".to_owned()),
                },
            )]),
            "2026-10-31T00:00:00Z".to_owned(),
            "chat_writer_test_detection".to_owned(),
        )
    })
    .await
    .expect("seed task")
    .expect("seed");
    let chats = ChatWriter::attach(handle, state_dir)
        .await
        .expect("chat writer");
    (worker, chats)
}

fn mutation(chat_id: ChatId, generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_generation: generation,
        timestamp: OffsetDateTime::now_utc(),
    }
}

async fn execution_ready(
    chats: &ChatWriter,
) -> (
    vibemux_types::chat_owner::ChatExecutionOwner,
    ChatOwnerFence,
) {
    execution_ready_with_tools(chats, Default::default()).await
}

async fn execution_ready_with_tools(
    chats: &ChatWriter,
    admitted_tools: std::collections::BTreeSet<vibemux_types::ToolName>,
) -> (
    vibemux_types::chat_owner::ChatExecutionOwner,
    ChatOwnerFence,
) {
    use vibemux_store::{
        ChatExecutionOwnerOpen, ChatExecutionProcessChange, ChatExecutionStart, ChatOwnerChange,
        ChatOwnerMutation,
    };
    let mut owner = chats
        .open_execution_owner(
            ChatExecutionOwnerOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Claude,
                title_ref: None,
                profile_sha256: "a".repeat(64),
                base_commit: "b".repeat(40),
                admitted_tools,
                timestamp: OffsetDateTime::now_utc(),
            },
            None,
        )
        .await
        .expect("lifetime owner")
        .owner;
    let plan = vibemux_types::a2a::RunWorkspace {
        path: format!("/owned/{}", owner.run_id),
        branch: format!("codex/run_{}", owner.run_id.as_uuid().simple()),
        base_commit: owner.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    for change in [
        ChatOwnerChange::Plan {
            workspace: plan,
            resource_key: "c".repeat(64),
        },
        ChatOwnerChange::ClaimCreation,
        ChatOwnerChange::VerifyWorkspace,
    ] {
        owner = chats
            .change_execution_owner(
                ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.chat_id,
                    expected_version: owner.version,
                    timestamp: OffsetDateTime::now_utc(),
                },
                change,
            )
            .await
            .expect("owned fixture metadata")
            .owner;
    }
    let vendor = Uuid::new_v4().to_string();
    let process = chats
        .start_execution_process(ChatExecutionStart {
            operation_id: Uuid::new_v4(),
            chat_id: owner.chat_id,
            expected_generation: 1,
            expected_owner_version: owner.version,
            profile_sha256: owner.profile_sha256.clone(),
            resource_key: owner.resource_key.clone().expect("resource"),
            observed_commit: owner.base_commit.clone(),
            expected_vendor_id: Some(vendor.clone()),
            timestamp: OffsetDateTime::now_utc(),
        })
        .await
        .expect("process authority");
    let fence = process.fence.expect("fresh fence");
    chats
        .change_execution_process(
            mutation(owner.chat_id, 1),
            fence,
            ChatExecutionProcessChange::VendorBound { vendor_id: vendor },
        )
        .await
        .expect("identity proof");
    chats
        .change_execution_process(
            mutation(owner.chat_id, 1),
            fence,
            ChatExecutionProcessChange::Ready,
        )
        .await
        .expect("ready");
    (owner, fence)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execution_history_uses_private_bodies_and_replays_never_recreate_missing_text() {
    use vibemux_store::ChatTurnFinish;
    let temp = tempfile::tempdir().expect("private writer fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, fence) = execution_ready(&chats).await;
    let write = ChatExecutionPrompt {
        chat_id: owner.chat_id,
        request_id: Uuid::new_v4(),
        generation: 1,
        owner_version: owner.version,
        observed_commit: owner.base_commit,
        effective_tools: Default::default(),
        prompt: "PRIVATE_EXEC_PROMPT_中文 ; $(literal)".into(),
    };
    assert!(!format!("{write:?}").contains("PRIVATE_"));
    let admitted = chats
        .admit_execution_attempt(write.clone())
        .await
        .expect("body before admission");
    assert_eq!(
        chats
            .execution_prompt(write.request_id)
            .await
            .expect("verified prompt"),
        write.prompt
    );
    chats
        .claim_execution_attempt(write.request_id, 1, fence)
        .await
        .expect("claim");
    chats
        .bind_execution_attempt(write.request_id, 1, fence, "vendor_turn".into())
        .await
        .expect("acknowledgement");
    let reply = "PRIVATE_EXEC_REPLY_中文";
    let record_id = Uuid::new_v4();
    chats
        .append_execution_record(ChatRecordWrite {
            request_id: write.request_id,
            generation: 1,
            fence,
            record_id,
            kind: ChatRecordKind::Assistant,
            original_byte_count: reply.len() as u64,
            text: reply.into(),
        })
        .await
        .expect("private final record");
    let state = chats
        .execution_owner(owner.chat_id)
        .await
        .expect("owner")
        .expect("known");
    chats
        .finish_execution_attempt(ChatTurnFinish {
            request_id: write.request_id,
            generation: 1,
            fence,
            outcome: ChatTurnOutcome::Completed,
            correlated_terminal: true,
            vendor_session_id: state.session.vendor_session_id,
            vendor_turn_id: Some("vendor_turn".into()),
            error_code: None,
            process_alive: true,
            timestamp: OffsetDateTime::now_utc(),
            output_complete: true,
        })
        .await
        .expect("attempt result");
    let history = chats
        .execution_records(owner.chat_id, 0, 16)
        .await
        .expect("explicit history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text.as_deref(), Some(write.prompt.as_str()));
    assert_eq!(history[1].text.as_deref(), Some(reply));
    assert!(!format!("{history:?}").contains("PRIVATE_"));
    let events = chats
        .execute(|store, _| Ok(store.events()?))
        .await
        .expect("events");
    assert!(
        !serde_json::to_string(&events)
            .expect("sanitized events")
            .contains("PRIVATE_EXEC")
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("join")
        .expect("shutdown");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let reopened = chats
        .execution_records(owner.chat_id, 0, 16)
        .await
        .expect("reopened private history");
    assert_eq!(reopened[1].text.as_deref(), Some(reply));
    let path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", write.request_id.simple()));
    std::fs::remove_file(&path).expect("owned missing-body fault");
    assert!(
        chats
            .admit_execution_attempt(write.clone())
            .await
            .expect("old receipt")
            .duplicate
    );
    assert!(
        !path.exists(),
        "receipt replay cannot republish private text"
    );
    assert!(chats.execution_prompt(write.request_id).await.is_err());
    let missing = chats
        .execution_records(owner.chat_id, 0, 16)
        .await
        .expect("metadata remains readable");
    assert!(missing[0].text.is_none() && missing[0].content_error.is_some());
    assert_eq!(admitted.owner.run_id, owner.run_id);
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("join")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_execution_admission_removes_only_its_own_published_body() {
    let temp = tempfile::tempdir().expect("private writer fixture");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (owner, _fence) = execution_ready(&chats).await;
    let write = ChatExecutionPrompt {
        chat_id: owner.chat_id,
        request_id: Uuid::new_v4(),
        generation: 1,
        owner_version: owner.version + 1,
        observed_commit: owner.base_commit,
        effective_tools: Default::default(),
        prompt: "PRIVATE_REJECTED_EXECUTION".into(),
    };
    assert_eq!(
        chats
            .admit_execution_attempt(write.clone())
            .await
            .expect_err("stale owner version")
            .code(),
        "chat_conflict"
    );
    assert!(
        !temp
            .path()
            .join("chat_private")
            .join(format!("{}.blob", write.request_id.simple()))
            .exists()
    );
    assert!(
        chats
            .execution_attempt(write.request_id)
            .await
            .expect("no metadata")
            .is_none()
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("join")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_replay_retries_released_title_cleanup() {
    let temp = tempfile::tempdir().expect("state directory");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let open = ChatOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Claude,
        profile_sha256: "a".repeat(64),
        resource_key: "b".repeat(64),
        base_commit: "c".repeat(40),
        title_ref: None,
        timestamp: OffsetDateTime::now_utc(),
    };
    let session = chats
        .open(open.clone(), Some(TITLE.to_owned()))
        .await
        .expect("open")
        .session;
    let old_path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", open.operation_id.simple()));
    // Simulate a file changed between the committed rename and its cleanup.
    std::fs::write(&old_path, b"changed").expect("change old title");
    let rename = mutation(session.chat_id, 1);
    assert!(
        chats
            .rename(rename.clone(), "New title".to_owned())
            .await
            .is_err()
    );
    assert_eq!(
        chats
            .title(session.chat_id)
            .await
            .expect("committed title")
            .as_deref(),
        Some("New title")
    );
    std::fs::write(&old_path, TITLE).expect("restore owned title");
    assert!(
        chats
            .rename(rename, "New title".to_owned())
            .await
            .expect("replay")
            .duplicate
    );
    assert!(
        !old_path.exists(),
        "replaying a committed rename must finish its released-file cleanup"
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_operation_retry_removes_its_matching_orphan() {
    let temp = tempfile::tempdir().expect("state directory");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let open = ChatOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Claude,
        profile_sha256: "a".repeat(64),
        resource_key: "b".repeat(64),
        base_commit: "c".repeat(40),
        title_ref: None,
        timestamp: OffsetDateTime::now_utc(),
    };
    let session = chats
        .open(open, Some(TITLE.to_owned()))
        .await
        .expect("open")
        .session;
    let rename = mutation(session.chat_id, 2);
    // Recovery fixture: a prior write published its body, canonical admission
    // refused the generation, and the cleanup failed before a caller retried.
    chats
        .files
        .write_blob(rename.operation_id, b"Orphan title")
        .expect("orphan fixture");
    let orphan_path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", rename.operation_id.simple()));
    assert!(
        chats
            .rename(rename, "Orphan title".to_owned())
            .await
            .is_err()
    );
    assert!(
        !orphan_path.exists(),
        "a matching unreferenced retry body must be reclaimed"
    );
    assert_eq!(
        chats
            .title(session.chat_id)
            .await
            .expect("original title")
            .as_deref(),
        Some(TITLE)
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn later_purge_cleans_titles_released_by_a_failed_rename_cleanup() {
    let temp = tempfile::tempdir().expect("state directory");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let open = ChatOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Claude,
        profile_sha256: "a".repeat(64),
        resource_key: "b".repeat(64),
        base_commit: "c".repeat(40),
        title_ref: None,
        timestamp: OffsetDateTime::now_utc(),
    };
    let session = chats
        .open(open.clone(), Some(TITLE.to_owned()))
        .await
        .expect("open")
        .session;
    let old_path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", open.operation_id.simple()));
    std::fs::write(&old_path, b"changed").expect("change old title");
    assert!(
        chats
            .rename(mutation(session.chat_id, 1), "New title".to_owned())
            .await
            .is_err()
    );
    std::fs::write(&old_path, TITLE).expect("restore owned title");
    chats
        .close(mutation(session.chat_id, 1))
        .await
        .expect("close unowned chat");
    chats
        .archive(mutation(session.chat_id, 1))
        .await
        .expect("archive");
    chats
        .purge(mutation(session.chat_id, 1))
        .await
        .expect("purge all private content");
    assert!(
        !old_path.exists(),
        "purge includes previously released title content"
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_history_survives_reopen_and_purged_replays_do_not_recreate_it() {
    let temp = tempfile::tempdir().expect("state directory");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let open = ChatOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Claude,
        profile_sha256: "a".repeat(64),
        resource_key: "b".repeat(64),
        base_commit: "c".repeat(40),
        title_ref: None,
        timestamp: OffsetDateTime::now_utc(),
    };
    let session = chats
        .open(open.clone(), Some(TITLE.to_owned()))
        .await
        .expect("open")
        .session;
    let owner = chats
        .claim_owner(session.chat_id, 1, Some("owned_vendor".to_owned()))
        .await
        .expect("owner");
    chats
        .transport_ready(session.chat_id, 1, owner.fence)
        .await
        .expect("transport");
    chats
        .bind_vendor(session.chat_id, 1, owner.fence, "owned_vendor".to_owned())
        .await
        .expect("root proof");
    let request_id = Uuid::new_v4();
    let turn = chats
        .admit_turn(
            session.chat_id,
            request_id,
            1,
            "c".repeat(40),
            PROMPT.to_owned(),
        )
        .await
        .expect("admit")
        .turn;
    chats
        .claim_turn(request_id, 1, owner.fence)
        .await
        .expect("send claim");
    chats
        .bind_turn(request_id, 1, owner.fence, "vendor_turn".to_owned())
        .await
        .expect("turn proof");
    chats
        .append(ChatRecordWrite {
            request_id,
            generation: 1,
            fence: owner.fence,
            record_id: Uuid::new_v4(),
            kind: ChatRecordKind::Assistant,
            text: REPLY.to_owned(),
            original_byte_count: REPLY.len() as u64,
        })
        .await
        .expect("private reply");
    chats
        .finish_turn(ChatTurnFinish {
            request_id,
            generation: 1,
            fence: owner.fence,
            outcome: ChatTurnOutcome::Completed,
            correlated_terminal: true,
            vendor_session_id: Some("owned_vendor".to_owned()),
            vendor_turn_id: Some("vendor_turn".to_owned()),
            error_code: None,
            process_alive: true,
            output_complete: true,
            timestamp: OffsetDateTime::now_utc(),
        })
        .await
        .expect("finish");
    assert_eq!(
        chats
            .session(session.chat_id)
            .await
            .expect("status")
            .expect("session")
            .state,
        ChatState::Ready
    );
    chats
        .release_owner(mutation(session.chat_id, 1), owner.fence, true, false)
        .await
        .expect("verified cleanup");
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
    let bytes = std::fs::read(temp.path().join("state.sqlite3")).expect("database");
    for marker in [PROMPT, REPLY, TITLE] {
        assert!(
            !bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()),
            "private text entered canonical database"
        );
    }
    let (worker, chats) = start(temp.path().to_owned()).await;
    assert_eq!(
        chats
            .title(session.chat_id)
            .await
            .expect("title")
            .as_deref(),
        Some(TITLE)
    );
    let records = chats
        .records(session.chat_id, 0, 32)
        .await
        .expect("history");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].text.as_deref(), Some(PROMPT));
    assert_eq!(records[1].text.as_deref(), Some(REPLY));
    assert!(records.iter().all(|record| record.content_error.is_none()));
    let replay = chats
        .admit_turn(
            session.chat_id,
            request_id,
            1,
            "d".repeat(40),
            PROMPT.to_owned(),
        )
        .await
        .expect("replay");
    assert!(replay.duplicate);
    assert_eq!(replay.turn.run_id, turn.run_id);
    assert_eq!(replay.turn.base_commit, "c".repeat(40));
    chats
        .archive(mutation(session.chat_id, 1))
        .await
        .expect("archive");
    let purge = mutation(session.chat_id, 1);
    chats.purge(purge.clone()).await.expect("purge");
    let blob_count = || {
        std::fs::read_dir(temp.path().join("chat_private"))
            .expect("private directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "blob")
            })
            .count()
    };
    assert_eq!(blob_count(), 0);
    assert!(
        chats
            .open(open, Some(TITLE.to_owned()))
            .await
            .expect("old open")
            .duplicate
    );
    assert!(
        chats
            .admit_turn(
                session.chat_id,
                request_id,
                1,
                "e".repeat(40),
                PROMPT.to_owned()
            )
            .await
            .expect("old send")
            .duplicate
    );
    chats.purge(purge).await.expect("same purge");
    assert_eq!(
        blob_count(),
        0,
        "old receipt lookup must not recreate purged content"
    );
    let records = chats
        .records(session.chat_id, 0, 32)
        .await
        .expect("purged history metadata");
    assert!(
        records
            .iter()
            .all(|record| record.text.is_none() && record.record.content_ref.is_none())
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}
