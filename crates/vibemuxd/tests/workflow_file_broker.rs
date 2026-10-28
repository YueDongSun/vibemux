#![cfg(all(feature = "test_helpers", windows))]
//! Actual file-broker subprocesses inside synthetic workflow controllers.
//! This qualifies fixture integration only, not a native model or OS sandbox.

mod workflow_support;

use serde_json::{Value, json};
use workflow_support::{
    FixtureOptions, WorkflowFixture, phase, policy, review_pass, store_request, store_task,
    taskboard_file, workflow_id,
};

const GOOD_STORE: &str = "calibration/good/src/store.mjs";
const BAD_STORE: &str = "calibration/mutants/blank_title_accepted/src/store.mjs";

fn file_policy(repairs: u32) -> Value {
    let mut value = policy(&["store"], 2, false);
    value["allowed_tools"]
        .as_array_mut()
        .expect("tools")
        .retain(|tool| tool != "run_dev_tests");
    value["budget_caps"]["max_repairs"] = json!(repairs);
    value
}

async fn fixture() -> WorkflowFixture {
    WorkflowFixture::start(FixtureOptions {
        file_broker: true,
        ..FixtureOptions::default()
    })
    .await
}

async fn run(fixture: &WorkflowFixture, key: &str, repairs: u32) -> (uuid::Uuid, Value) {
    let prepared = fixture
        .prepare(
            store_request(
                key,
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            file_policy(repairs),
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(id).await;
    if phase(&status) != "accepted" {
        for attempt in attempts(fixture, id).await {
            let request = uuid::Uuid::parse_str(attempt["request_id"].as_str().unwrap()).unwrap();
            let record = fixture
                .client
                .harness_dispatch_status(request)
                .await
                .expect("dispatch");
            eprintln!(
                "fixture dispatch: phase={:?}, error={:?}, process={:?}",
                record.phase, record.error_code, record.process
            );
        }
        eprintln!(
            "fixture started count: {}",
            fixture.started("store/implement_1").len()
        );
    }
    (id, status)
}

async fn attempts(fixture: &WorkflowFixture, id: uuid::Uuid) -> Vec<Value> {
    let status = fixture.status(id).await;
    let mut attempts = vec![];
    for session in status["sessions"].as_array().expect("sessions") {
        let session_id =
            uuid::Uuid::parse_str(session["session_id"].as_str().expect("id")).expect("UUID");
        let view = fixture
            .client
            .workflow_session_inspect(session_id)
            .await
            .expect("session");
        attempts.extend(
            view["attempts"]
                .as_array()
                .expect("attempts")
                .iter()
                .cloned(),
        );
    }
    attempts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_file_broker_candidates_reach_review_and_the_trusted_verifier() {
    let fixture = fixture().await;
    let mut review = review_pass();
    review["read"] = json!(["src/store.mjs", "tests/worker_store/retained.mjs"]);
    fixture.write_script(&json!({"steps":{
        "store/implement_1":{
            "read":["src/store.mjs"], "search":["src"],
            "write":{"src/store.mjs":taskboard_file(GOOD_STORE),
                "tests/worker_store/retained.mjs":"export const retained = 1;\n"}
        },
        "store/review":review
    }}));
    let (id, status) = run(&fixture, "broker_happy", 0).await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    let attempts = attempts(&fixture, id).await;
    assert_eq!(attempts.len(), 2);
    for attempt in &attempts {
        assert!(matches!(
            attempt["execution"]["harness_profile"].as_str(),
            Some("codex_fixture_broker_v1" | "claude_fixture_broker_v1")
        ));
        assert!(
            !attempt["execution"]["effective_tools"]
                .as_array()
                .expect("tools")
                .contains(&json!("run_dev_tests"))
        );
    }
    assert_eq!(status["receipts"]["reviews"][0]["verdict"], "pass");
    assert!(
        status["receipts"]["verifications"]
            .as_array()
            .expect("verifications")
            .iter()
            .all(|value| value["suites"]
                .as_array()
                .expect("suites")
                .iter()
                .all(|suite| suite["status"] == "passed"))
    );
    assert_eq!(fixture.checkout_status(), "");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), fixture.head);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_partial_and_stale_owner_outputs_never_admit_candidates() {
    let fixture = fixture().await;
    for fault in ["missing", "partial", "stale_attempt", "stale_snapshot"] {
        fixture.write_script(&json!({"steps":{"store/implement_1":{
            "write":{"src/store.mjs":taskboard_file(GOOD_STORE)},
            "broker_output":fault
        }}}));
        let (id, status) = run(&fixture, &format!("broker_{fault}"), 0).await;
        assert_eq!(phase(&status), "failed", "{fault}: {status:#}");
        assert!(status["tasks"][0]["accepted_candidate"].is_null());
        assert!(
            status["receipts"]["candidates"]
                .as_array()
                .expect("candidates")
                .iter()
                .all(|candidate| candidate["admitted"] == false)
        );
        let attempts = attempts(&fixture, id).await;
        assert_eq!(attempts.len(), 1, "no review after invalid publication");
        let request = uuid::Uuid::parse_str(attempts[0]["request_id"].as_str().unwrap()).unwrap();
        let record = fixture
            .client
            .harness_dispatch_status(request)
            .await
            .expect("dispatch");
        assert_eq!(
            record.error_code,
            Some(vibemux_harness::dispatch::DispatchError::BrokerCandidateInvalid)
        );
        assert_eq!(fixture.checkout_status(), "");
    }
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_input_contains_prior_turn_files_and_keeps_the_cumulative_candidate() {
    let fixture = fixture().await;
    let mut review = review_pass();
    review["read"] = json!(["tests/worker_store/retained.mjs"]);
    fixture.write_script(&json!({"steps":{
        "store/implement_1":{"write":{
            "src/store.mjs":taskboard_file(BAD_STORE),
            "tests/worker_store/retained.mjs":"export const retained = 1;\n"
        }},
        "store/repair":{"read":["tests/worker_store/retained.mjs"],
            "write":{"src/store.mjs":taskboard_file(GOOD_STORE)},
            "checkpoint":{"changed_files":["src/store.mjs","tests/worker_store/retained.mjs"]}},
        "store/review":review
    }}));
    let (_, status) = run(&fixture, "broker_repair", 1).await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    assert_eq!(fixture.started("store/repair_2").len(), 1);
    let candidates = status["receipts"]["candidates"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate["admitted"] == true)
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_broker_publication_still_prevents_candidate_admission() {
    let fixture = fixture().await;
    fixture.write_script(&json!({"steps":{"store/implement_1":{
        "write":{"src/store.mjs":taskboard_file(GOOD_STORE)}, "delay_ms":30_000
    }}}));
    let prepared = fixture
        .prepare(
            store_request(
                "broker_cancel_published",
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            file_policy(0),
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if fixture
                .log()
                .iter()
                .any(|entry| entry["event"] == "broker_published")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual broker publication");
    fixture.client.workflow_cancel(id).await.expect("cancel");
    let status = fixture.wait_idle(id).await;
    assert_eq!(phase(&status), "cancelled", "{status:#}");
    assert!(
        status["receipts"]["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|candidate| candidate["admitted"] == false)
    );
    assert!(status["tasks"][0]["accepted_candidate"].is_null());
    assert!(
        status["receipts"]["reviews"]
            .as_array()
            .expect("reviews")
            .is_empty()
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_only_profile_refuses_command_grants_before_worker_launch() {
    let fixture = fixture().await;
    fixture.write_script(&json!({"steps":{"store/implement_1":{
        "write":{"src/store.mjs":taskboard_file(GOOD_STORE)}
    }}}));
    let mut full_policy = policy(&["store"], 2, false);
    full_policy["budget_caps"]["max_repairs"] = json!(0);
    let prepared = fixture
        .prepare(
            store_request(
                "broker_refuse_command",
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            full_policy,
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(id).await;
    assert_eq!(phase(&status), "failed", "{status:#}");
    assert!(fixture.started("store/implement_1").is_empty());
    assert!(
        status["receipts"]["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty()
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}
