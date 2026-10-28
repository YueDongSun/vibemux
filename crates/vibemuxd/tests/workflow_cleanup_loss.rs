#![cfg(feature = "test_helpers")]
//! Losing cleanup confirmation must retain owners, artifacts and reservations.
//! The hook discards a real completion receipt; it does not claim an OS fault.

mod workflow_support;

use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{DispatchPhase, DispatchRequest},
};
use workflow_support::{
    FixtureOptions, WorkflowFixture, good_files, policy, review_pass, store_request, store_task,
    workflow_id,
};

fn retained_files(path: &Path, suffix: &str) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(suffix)
        })
        .collect()
}

async fn check_cleanup_receipt_loss(verifier: bool) {
    let fixture = WorkflowFixture::start(FixtureOptions {
        trace_verifier_suites: verifier,
        ..FixtureOptions::default()
    })
    .await;
    fixture.write_script(&json!({"steps": {
        "store/implement_1": {"write": good_files(&["src/store.mjs"])},
        "store/review_1": review_pass(),
    }}));
    let suites: &[&str] = if verifier {
        &["syntax", "syntax_second"]
    } else {
        &["syntax"]
    };
    let prepared = fixture
        .prepare(
            store_request(
                "cleanup_loss",
                &fixture.head,
                "cooperate",
                &store_task(suites),
                false,
            ),
            policy(suites, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.lose_next_cleanup(verifier);
    fixture.start_workflow(&prepared).await;
    let result = tokio::time::timeout(Duration::from_secs(45), fixture.wait_idle(workflow)).await;
    let owners = fixture.retained_process_count();
    let next = fixture
        .client
        .harness_dispatch_submit(&DispatchRequest {
            schema_version: 1,
            request_id: Uuid::new_v4(),
            harness: AgentKind::Codex,
            prompt: "must remain unlaunched after uncertain cleanup".to_string(),
        })
        .await;
    let workflow_dir = fixture
        .state_dir
        .join("workflow_state")
        .join(workflow.simple().to_string());
    let ledger: Value = serde_json::from_slice(
        &std::fs::read(workflow_dir.join("workspaces.json")).expect("ledger"),
    )
    .expect("ledger JSON");
    let workspaces_remain = ledger["workspaces"]
        .as_object()
        .expect("workspaces")
        .values()
        .all(|workspace| Path::new(workspace["path"].as_str().expect("owned path")).is_dir());
    let scratch = workflow_dir.join("scratch");
    let started = retained_files(&scratch, ".started");
    let outputs = retained_files(&scratch, ".json");
    let mut dispatch_phase = None;
    if !verifier && let Ok(status) = &result {
        let session = status["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .find(|session| session["slot_id"] == "slot_a")
            .expect("worker session");
        let session_id = Uuid::parse_str(session["session_id"].as_str().unwrap()).unwrap();
        let inspected = fixture
            .client
            .workflow_session_inspect(session_id)
            .await
            .expect("inspect");
        let request_id =
            Uuid::parse_str(inspected["attempts"][0]["request_id"].as_str().unwrap()).unwrap();
        dispatch_phase = Some(
            fixture
                .client
                .harness_dispatch_status(request_id)
                .await
                .expect("dispatch")
                .phase,
        );
    }
    fixture.shutdown().await;

    let status = result.expect("workflow did not report the unresolved process");
    assert_eq!(
        owners, 1,
        "exact process owner was not retained: {status:#}"
    );
    assert!(
        next.is_err(),
        "new admission continued after cleanup uncertainty"
    );
    assert!(
        workspaces_remain,
        "cleanup removed an unresolved owner's worktree"
    );
    let cleanup = status["last_run"]["cleanup"].as_array().expect("cleanup");
    assert!(!cleanup.is_empty());
    assert!(
        cleanup.iter().all(|entry| entry["removed"] == false
            && entry["reason_code"] == "workflow_process_cleanup_pending"),
        "{cleanup:#?}"
    );
    assert!(
        status["receipts"]["verifications"]
            .as_array()
            .expect("verifications")
            .is_empty()
    );
    assert!(status["receipts"]["selection"].is_null());
    assert!(status["receipts"]["integration"].is_null());
    if verifier {
        assert_eq!(
            status["last_run"]["end"]["code"],
            "workflow_process_cleanup_pending"
        );
        assert_eq!(
            started.len(),
            1,
            "a second verifier suite started after uncertain cleanup"
        );
        assert_eq!(
            outputs.len(),
            1,
            "the unresolved verifier result was consumed or removed"
        );
    } else {
        assert_eq!(
            dispatch_phase,
            Some(DispatchPhase::Running),
            "unproven cleanup released its durable reservation"
        );
        assert!(
            status["receipts"]["candidates"]
                .as_array()
                .expect("candidates")
                .is_empty()
        );
        assert!(
            status["leases"]
                .as_array()
                .expect("leases")
                .iter()
                .any(|lease| lease["state"] == "active" || lease["state"] == "quarantined"),
            "unsettled attempt released its lease"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_dispatch_cleanup_receipt_keeps_the_attempt_reserved() {
    check_cleanup_receipt_loss(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_verifier_cleanup_receipt_stops_suites_and_retains_output() {
    check_cleanup_receipt_loss(true).await;
}
