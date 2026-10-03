#![cfg(feature = "test_helpers")]
//! Operator context shares and retained content (ADR 031 §5): an operator
//! grants Track A's recorded bundle to Track B's session while the
//! workflow is paused; the grant survives restarts, reaches B's next turn
//! with the bundle text read back from the opt-in content store, and is
//! never forged once that content is purged. The fast syntax suite stands
//! in for the TaskBoard suites. Synthetic workers: offline orchestration
//! evidence.

mod workflow_support;

use serde_json::{Value, json};
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, good_files, phase, policy, remote,
    review_pass, session_id, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const QUESTION: &str = "Which error code does the store raise for a blank title?";
const ANSWER: &str = "The store raises invalid_title for a title that is blank after trimming.";
const STORE_FIRST_LINE: &str = "TaskBoard Lite store: calibration reference implementation";
/// Long enough to pause the workflow during B's follow-up turn.
const FOLLOW_UP_DELAY_MS: u64 = 2_000;

fn share_script() -> Value {
    json!({"steps": {
        "track_a/implement_1": {"write": good_files(&["src/server.mjs", "src/store.mjs"])},
        "track_b/implement_1": {
            "write": good_files(&["public/app.mjs"]),
            "checkpoint": {"messages": [{"kind": "question", "to": "track_a", "text": QUESTION}]},
        },
        "track_a/answer_2": {
            "checkpoint": {"messages": [{"kind": "answer", "to": "track_b", "text": ANSWER,
                "source_refs": [{"path": "src/store.mjs", "start_line": 1, "end_line": 12}]}]},
        },
        "track_b/implement_2": {"checkpoint": {"consume_context": true},
            "delay_ms": FOLLOW_UP_DELAY_MS},
        "track_b/implement_3": {"checkpoint": {"consume_context": true}},
        "track_a/review_1": review_pass(),
        "track_b/review_1": review_pass(),
    }})
}

/// Starts the share workflow and pauses it during B's follow-up turn, once
/// A's answer bundle is recorded. Returns the workflow, the prepared
/// response, and the bundle ID.
async fn paused_with_bundle(fixture: &WorkflowFixture, request_key: &str) -> (Uuid, Value, Uuid) {
    fixture.write_script(&share_script());
    let request = cooperative_request(
        request_key,
        &fixture.head,
        &track_a(&SYNTAX),
        &track_b(&SYNTAX),
        &SYNTAX,
    );
    let prepared = fixture.prepare(request, policy(&SYNTAX, 2, true)).await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    fixture.wait_for_turn("track_b/implement_2").await;
    let paused = fixture
        .client
        .workflow_pause(workflow)
        .await
        .expect("pause");
    assert_eq!(paused["signalled"], true);
    let status = fixture.wait_phase(workflow, &["paused"]).await;
    let bundles = status["receipts"]["bundles"].as_array().expect("bundles");
    assert_eq!(bundles.len(), 1, "status: {status:#}");
    assert_eq!(bundles[0]["source_task"], "track_a");
    let bundle_id = bundles[0]["bundle_id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .expect("bundle id");
    (workflow, prepared, bundle_id)
}

fn message<'a>(status: &'a Value, message_id: &Value) -> &'a Value {
    status["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| &message["message_id"] == message_id)
        .expect("message")
}

/// The A→B share walkthrough and C04: the grant is admitted while paused,
/// kept across a restart, delivered with the bundle at B's next turn
/// boundary, and still acknowledged after another restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_share_reaches_the_other_track_and_survives_restarts() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let (workflow, prepared, bundle_id) = paused_with_bundle(&fixture, "taskboard_share").await;
    let session_a = session_id(workflow, "track_a", "slot_a");
    let session_b = session_id(workflow, "track_b", "slot_b");

    let shared = fixture
        .client
        .workflow_share(bundle_id, session_b)
        .await
        .expect("share");
    assert_eq!(shared["recipient"], "worker:track_b");
    assert_eq!(shared["duplicate"], false);
    let repeated = fixture
        .client
        .workflow_share(bundle_id, session_b)
        .await
        .expect("repeated share");
    assert_eq!(repeated["duplicate"], true);
    assert_eq!(repeated["message_id"], shared["message_id"]);
    // A bundle never goes back to its own source, to an unknown session,
    // or from an unknown bundle.
    assert_eq!(
        fixture.client.workflow_share(bundle_id, session_a).await,
        Err(remote("workflow_share_invalid"))
    );
    assert_eq!(
        fixture
            .client
            .workflow_share(bundle_id, Uuid::new_v4())
            .await,
        Err(remote("workflow_session_not_found"))
    );
    assert_eq!(
        fixture
            .client
            .workflow_share(Uuid::new_v4(), session_b)
            .await,
        Err(remote("workflow_share_invalid"))
    );
    let status = fixture.status(workflow).await;
    let grant = message(&status, &shared["message_id"]);
    assert_eq!(grant["kind"], "context_grant");
    assert_eq!(grant["sender"], "operator");
    assert_eq!(grant["recipient"], "worker:track_b");
    assert_eq!(grant["state"], "admitted");

    // C04: a restart loses neither the grant nor the bundle.
    fixture.restart().await;
    let reopened = fixture.status(workflow).await;
    assert_eq!(phase(&reopened), "paused");
    assert_eq!(reopened["messages"], status["messages"]);
    assert_eq!(
        reopened["receipts"]["bundles"],
        status["receipts"]["bundles"]
    );

    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    let delivered = fixture.started("track_b/implement_3");
    assert_eq!(delivered.len(), 1);
    let delivered_prompt = delivered[0]["prompt"].as_str().expect("prompt");
    assert!(delivered_prompt.contains(&format!("bundle {bundle_id}")));
    assert!(delivered_prompt.contains(STORE_FIRST_LINE));
    assert_eq!(
        message(&status, &shared["message_id"])["state"],
        "acknowledged"
    );
    let sent = fixture
        .client
        .workflow_session_prompt(session_b, 2)
        .await
        .expect("sent prompt");
    assert_eq!(sent["text"].as_str(), Some(delivered_prompt));

    // C04: the acknowledged share is still there after another restart.
    fixture.restart().await;
    let reopened = fixture.status(workflow).await;
    assert_eq!(reopened["messages"], status["messages"]);

    // C03 on Windows: the retained texts sit under workflow state that is
    // restricted to the daemon's user, and every retained file inherits it.
    #[cfg(windows)]
    {
        let state_root = fixture.state_dir.join("workflow_state");
        let content = state_root.join("content");
        let mut paths = vec![state_root.clone(), content.clone()];
        paths.extend(
            std::fs::read_dir(&content)
                .expect("content store")
                .map(|entry| entry.expect("entry").path())
                .take(8),
        );
        assert!(paths.len() > 2, "no retained content");
        let borrowed: Vec<&std::path::Path> =
            paths.iter().map(std::path::PathBuf::as_path).collect();
        vibemux_platform::verify_restricted_path_acls(&borrowed)
            .expect("the private workflow state is restricted to the daemon user");
    }

    // C03: purging removes the retained texts; the evidence stays
    // content-free and complete.
    let purged = fixture
        .client
        .workflow_purge_content(workflow)
        .await
        .expect("purge");
    assert!(purged["deleted"].as_u64() > Some(0), "purge: {purged}");
    let after = fixture
        .client
        .workflow_session_prompt(session_b, 2)
        .await
        .expect("purged prompt");
    assert!(after["text"].is_null());
    assert_eq!(after["unavailable_reason"], "prompt_not_retained");
    assert_eq!(
        after["rendered_prompt_sha256"],
        sent["rendered_prompt_sha256"]
    );
    let items = fixture.export_all(workflow).await;
    let encoded = serde_json::to_string(&items).expect("encode");
    for secret in [QUESTION, ANSWER, STORE_FIRST_LINE] {
        assert!(!encoded.contains(secret), "the export leaked {secret}");
    }
    assert_eq!(fixture.checkout_status(), "");
    fixture.record_evidence("share_walkthrough", workflow).await;
    fixture.shutdown().await;
}

/// C03/C04: once the retained bundle text is purged, a new share is
/// refused and an admitted grant reaches the recipient without any bundle
/// text; nothing is reconstructed from elsewhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purged_bundle_content_is_never_forged_into_a_later_turn() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let (workflow, prepared, bundle_id) = paused_with_bundle(&fixture, "taskboard_purge").await;
    let session_b = session_id(workflow, "track_b", "slot_b");
    let shared = fixture
        .client
        .workflow_share(bundle_id, session_b)
        .await
        .expect("share");
    let purged = fixture
        .client
        .workflow_purge_content(workflow)
        .await
        .expect("purge");
    assert!(purged["deleted"].as_u64() > Some(0), "purge: {purged}");
    assert_eq!(
        fixture.client.workflow_share(bundle_id, session_b).await,
        Err(remote("workflow_share_invalid"))
    );

    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    let delivered = fixture.started("track_b/implement_3");
    assert_eq!(delivered.len(), 1);
    let delivered_prompt = delivered[0]["prompt"].as_str().expect("prompt");
    assert!(delivered_prompt.contains(&format!("bundle:{bundle_id}")));
    assert!(!delivered_prompt.contains(&format!("bundle {bundle_id}")));
    assert!(!delivered_prompt.contains(STORE_FIRST_LINE));
    assert_eq!(
        message(&status, &shared["message_id"])["state"],
        "acknowledged"
    );
    fixture.shutdown().await;
}
