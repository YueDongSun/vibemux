#![cfg(feature = "test_helpers")]
//! Positive-only start correlation. Reads never admit, resume, or cancel work.
mod workflow_support;

use serde_json::json;
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, WorkflowFixture, good_files, policy, remote, review_pass, store_request,
    store_task, workflow_id,
};

const SUITES: [&str; 1] = ["syntax"];

fn contract_probe(
    kind: &str,
    task: serde_json::Value,
    response: serde_json::Value,
    expected: serde_json::Value,
) {
    if std::env::var_os("VIBEMUX_RECORD_START_OBSERVATION_CONTRACT").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        println!(
            "HUB_WORKFLOW_CONTRACT {}",
            json!({"kind":kind,"task":task,"response":response,"expected":expected})
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_observation_before_admission_is_not_retry_authority_or_an_effect() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let prepared = fixture
        .prepare(
            store_request(
                "start_observation_empty",
                &fixture.head,
                "cooperate",
                &store_task(&SUITES),
                false,
            ),
            policy(&SUITES, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    let contract = prepared["start_contract"].as_str().expect("contract");
    let request_id = Uuid::new_v4();
    let before = fixture.status(workflow).await;
    let before_log = fixture.log();
    for _ in 0..2 {
        let observed = fixture
            .client
            .workflow_start_observation(contract, request_id)
            .await
            .expect("read observation");
        assert_eq!(
            observed,
            json!({"schema_version":1,"request_id":request_id,"start_contract":contract,"status":"not_observed","retry_allowed":false,"workflow":null})
        );
        contract_probe(
            "observation",
            json!({"request_id":request_id,"workflow_contract":contract}),
            json!({"ok":true,"observation":observed}),
            json!({"status":"outcome_unknown"}),
        );
    }
    assert_eq!(
        fixture
            .client
            .workflow_cancel_start(contract, request_id, 1)
            .await,
        Err(remote("workflow_start_identity_mismatch"))
    );
    assert_eq!(
        fixture
            .client
            .workflow_cancel_start(contract, request_id, 0)
            .await,
        Err(remote("workflow_request_invalid"))
    );
    assert_eq!(fixture.status(workflow).await, before);
    assert_eq!(fixture.log(), before_log);
    assert_eq!(
        fixture
            .client
            .workflow_start_observation(&"0".repeat(64), request_id)
            .await,
        Err(remote("workflow_contract_mismatch"))
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discarded_start_reply_recovers_latest_admission_without_another_start() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&json!({"steps":{"store/implement_1":{"delay_ms":30000}}}));
    let prepared = fixture
        .prepare(
            store_request(
                "start_observation_running",
                &fixture.head,
                "cooperate",
                &store_task(&SUITES),
                false,
            ),
            policy(&SUITES, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    let contract = prepared["start_contract"]
        .as_str()
        .expect("contract")
        .to_owned();
    let request_id = Uuid::new_v4();
    // Deliberately discard the start acknowledgment. Recovery uses only the exact read API.
    let _ = fixture
        .client
        .workflow_start(&contract, request_id)
        .await
        .expect("start");
    let observed = fixture
        .client
        .workflow_start_observation(&contract, request_id)
        .await
        .expect("correlated observation");
    assert_eq!(observed["status"], "observed");
    assert_eq!(observed["retry_allowed"], false);
    assert_eq!(observed["workflow"]["workflow_id"], json!(workflow));
    assert_eq!(observed["workflow"]["generation"], 1);
    assert_eq!(observed["workflow"]["mode"], "deterministic");
    assert_eq!(observed["workflow"]["execution_state"], "running");
    contract_probe(
        "observation",
        json!({"request_id":request_id,"workflow_contract":contract}),
        json!({"ok":true,"observation":observed}),
        json!({"status":"running","workflow_id":workflow,"workflow_generation":1}),
    );
    let mismatch = fixture
        .client
        .workflow_start_observation(&contract, Uuid::new_v4())
        .await
        .expect("unobserved other request");
    assert_eq!(mismatch["status"], "not_observed");
    assert!(mismatch["workflow"].is_null());
    fixture.restart_after_crash(workflow).await;
    let before = fixture.status(workflow).await;
    let recovered = fixture
        .client
        .workflow_start_observation(&contract, request_id)
        .await
        .expect("durable observation after restart");
    assert_eq!(recovered["status"], "observed");
    assert_eq!(recovered["workflow"]["workflow_id"], json!(workflow));
    assert_eq!(recovered["workflow"]["generation"], 1);
    assert!(matches!(
        recovered["workflow"]["execution_state"].as_str(),
        Some("stopped" | "recovery_required")
    ));
    assert_eq!(fixture.status(workflow).await, before);
    assert_eq!(recovered["retry_allowed"], false);
    contract_probe(
        "observation",
        json!({"request_id":request_id,"workflow_contract":contract,"workflow_id":workflow,"workflow_generation":1}),
        json!({"ok":true,"observation":recovered}),
        json!({"status":"outcome_unknown","workflow_id":workflow,"workflow_generation":1}),
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conditional_cancel_cannot_signal_a_newer_generation() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut review = review_pass();
    review["delay_ms"] = json!(30000);
    fixture.write_script(&json!({"steps":{
        "store/implement_1":{"write":good_files(&["src/store.mjs"]),"delay_ms":1000},
        "store/review_1":review
    }}));
    let prepared = fixture
        .prepare(
            store_request(
                "conditional_start_cancel",
                &fixture.head,
                "cooperate",
                &store_task(&SUITES),
                false,
            ),
            policy(&SUITES, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    let contract = prepared["start_contract"].as_str().expect("contract");
    let first = Uuid::new_v4();
    fixture
        .client
        .workflow_start(contract, first)
        .await
        .expect("first start");
    fixture
        .client
        .workflow_pause(workflow)
        .await
        .expect("pause first generation");
    fixture.wait_phase(workflow, &["paused"]).await;
    let previous = fixture
        .client
        .workflow_start_observation(contract, first)
        .await
        .expect("first admission");
    assert_eq!(previous["workflow"]["generation"], 1);
    let second = Uuid::new_v4();
    fixture
        .client
        .workflow_start(contract, second)
        .await
        .expect("second start");
    fixture.wait_for_turn("store/review_1").await;
    let before = fixture.status(workflow).await;
    let old = fixture
        .client
        .workflow_start_observation(contract, first)
        .await
        .expect("latest-only mismatch");
    assert_eq!(old["status"], "not_observed");
    assert_eq!(old["retry_allowed"], false);
    for (request_id, generation) in [(first, 1), (second, 1), (first, 2)] {
        assert_eq!(
            fixture
                .client
                .workflow_cancel_start(contract, request_id, generation)
                .await,
            Err(remote("workflow_start_identity_mismatch"))
        );
    }
    assert_eq!(fixture.status(workflow).await, before);
    let cancelled = fixture
        .client
        .workflow_cancel_start(contract, second, 2)
        .await
        .expect("cancel exact latest generation");
    assert_eq!(cancelled["request_id"], json!(second));
    assert_eq!(cancelled["generation"], 2);
    assert_eq!(cancelled["workflow_id"], json!(workflow));
    contract_probe(
        "cancel",
        json!({"request_id":second,"workflow_contract":contract,"workflow_id":workflow,"workflow_generation":2}),
        json!({"ok":true,"workflow":cancelled}),
        json!({"workflow_id":workflow,"generation":2,"request_id":second}),
    );
    fixture.wait_phase(workflow, &["cancelled"]).await;
    fixture.shutdown().await;
}
