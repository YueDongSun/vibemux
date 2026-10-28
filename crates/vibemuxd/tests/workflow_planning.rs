#![cfg(feature = "test_helpers")]
//! Source-only fixture coverage: production bound compiler, synthetic workers.
#[path = "aag_bound_fixture_support.rs"]
mod aag_bound_fixture_support;
mod workflow_support;

use aag_bound_fixture_support::{BoundFixtureServer, FIXTURE_TOKEN, FixtureMode};
use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest,
    gateway::{GatewayConfig, GatewayProtocol, LiveAdmission, RouteBinding, RouteRole},
};
use vibemuxd::workflow::{
    planning_profile::{PlanningQuery, PlanningRequest},
    settings::WORKFLOW_CONFIG_FILE_NAME,
};
use workflow_support::{
    FixtureOptions, STORE_REQUEST, WORKFLOW_KEY, WorkflowFixture, good_files, phase, policy,
    remote, review_pass, store_request, store_task,
};

const SUITES: [&str; 1] = ["syntax"];
const CREDENTIAL_ENV: &str = "AAG_BOUND_FACTORY_TOKEN";
const ALIASES: [&str; 4] = [
    "compiler_alias",
    "worker_a_alias",
    "worker_b_alias",
    "reviewer_alias",
];

fn configure(fixture: &WorkflowFixture, origin: &str, allow_execution: bool) {
    let binding = |role, alias: &str| RouteBinding {
        role,
        model_alias: alias.into(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 4096,
        timeout_ms: 60_000,
    };
    let gateway = GatewayConfig {
        schema_version: 1,
        endpoint: origin.into(),
        credential_env: CREDENTIAL_ENV.into(),
        strict_routing: true,
        live_admission: LiveAdmission::BoundReceiptV2,
        bindings: vec![
            binding(RouteRole::Compiler, ALIASES[0]),
            binding(RouteRole::WorkerA, ALIASES[1]),
            binding(RouteRole::WorkerB, ALIASES[2]),
            binding(RouteRole::Reviewer, ALIASES[3]),
        ],
    };
    let profile = |profile_id: &str| {
        json!({
            "profile_id": profile_id,
            "workflow_key": WORKFLOW_KEY,
            "mode": "cooperate",
            "worker_slots": ["slot_b", "slot_a"],
            "reviewer_slot": "review",
            "integration_suites": SUITES,
            "shared_contract_paths": {},
            "policy": policy(&SUITES, 2, false),
            "max_tasks": 2,
            "allow_execution": allow_execution,
        })
    };
    let path = fixture.state_dir.join(WORKFLOW_CONFIG_FILE_NAME);
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["gateway"] = serde_json::to_value(gateway).unwrap();
    config["planning_profiles"] = json!([profile("development"), profile("alternate")]);
    std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

async fn fixture(allow_execution: bool, malformed: bool) -> (WorkflowFixture, BoundFixtureServer) {
    assert_eq!(
        std::env::var(CREDENTIAL_ENV).ok().as_deref(),
        Some(FIXTURE_TOKEN),
        "only the explicit synthetic fixture credential is accepted"
    );
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.stop_server_for_test().await;
    let source = store_request(
        "candidate_source",
        &fixture.head,
        "cooperate",
        &store_task(&SUITES),
        false,
    );
    let output = if malformed {
        "This is not a TaskSpec response.".into()
    } else {
        serde_json::to_string(&json!({"task_specs": source["task_specs"]})).unwrap()
    };
    let gateway = BoundFixtureServer::start(&ALIASES, &output).await;
    configure(&fixture, gateway.origin(), allow_execution);
    fixture.restart().await;
    assert_eq!(
        gateway.counts().requests,
        0,
        "startup must not contact the compiler"
    );
    (fixture, gateway)
}

async fn planning_request(
    fixture: &WorkflowFixture,
    request_id: Uuid,
    execute: bool,
) -> PlanningRequest {
    let profiles = fixture.client.workflow_planning_profiles().await.unwrap();
    serde_json::from_value(json!({
        "schema_version": 1,
        "profile_id": "development",
        "profile_digest": profiles["profiles"][0]["profile_digest"],
        "request_id": request_id,
        "request_text": STORE_REQUEST,
        "execute": execute,
    }))
    .unwrap()
}

fn wire(value: &Value) {
    if std::env::var("VIBEMUX_PLANNING_WIRE_CAPTURE")
        .ok()
        .as_deref()
        == Some("1")
    {
        println!("PLANNING_WIRE:{}", serde_json::to_string(value).unwrap());
    }
}

fn planning_path(fixture: &WorkflowFixture, request_id: Uuid, kind: &str) -> std::path::PathBuf {
    fixture
        .state_dir
        .join("workflow_state")
        .join("planning")
        .join(format!("{}.{kind}.json", request_id.simple()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn prepare_only_is_one_shot_and_recovers_after_restart_and_lost_positive_publication() {
    let (mut fixture, gateway) = fixture(false, false).await;
    let request = planning_request(&fixture, Uuid::new_v4(), false).await;
    let query = request.query().unwrap();
    let missing = fixture
        .client
        .workflow_planning_observation(query.clone())
        .await
        .unwrap();
    wire(&missing);
    assert_eq!(missing["status"], "not_observed");
    assert_eq!(missing["retry_allowed"], false);
    assert!(!fixture.state_dir.join("workflow_state/planning").exists());

    let prepared = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    wire(&prepared);
    assert_eq!(prepared["status"], "prepared", "{prepared:#}");
    assert_eq!(prepared["execute"], false);
    assert!(prepared["execution"].is_null());
    assert_eq!(prepared["workflow"]["phase"], "prepared");
    let workflow_id =
        Uuid::parse_str(prepared["workflow"]["workflow_id"].as_str().unwrap()).unwrap();
    let before = fixture.status(workflow_id).await;
    let duplicate = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    assert_eq!(duplicate, prepared);
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(fixture.started_keys().is_empty());

    fixture.restart().await;
    assert_eq!(
        fixture
            .client
            .workflow_planning_observation(query.clone())
            .await
            .unwrap(),
        prepared
    );
    assert_eq!(
        fixture
            .client
            .workflow_plan_request(request.clone())
            .await
            .unwrap(),
        prepared
    );
    // Simulate a process crash after existing prepare published its plan, before
    // the content-free positive publication reached the caller.
    std::fs::remove_file(planning_path(&fixture, request.request_id, "prepared")).unwrap();
    assert_eq!(
        fixture
            .client
            .workflow_planning_observation(query)
            .await
            .unwrap(),
        prepared
    );
    assert_eq!(fixture.status(workflow_id).await, before);
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(fixture.started_keys().is_empty());
    let intent =
        std::fs::read_to_string(planning_path(&fixture, request.request_id, "intent")).unwrap();
    assert!(!intent.contains(STORE_REQUEST));
    assert!(!intent.contains("request_text"));
    fixture.shutdown().await;
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn one_uuid_cannot_rebind_goal_profile_digest_or_execution_intent() {
    let (fixture, gateway) = fixture(true, false).await;
    let request = planning_request(&fixture, Uuid::new_v4(), false).await;
    fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    let mut changed = request.clone();
    changed.request_text.push(' ');
    assert_eq!(
        fixture.client.workflow_plan_request(changed).await,
        Err(remote("workflow_request_conflict"))
    );
    changed = request.clone();
    changed.execute = true;
    assert_eq!(
        fixture.client.workflow_plan_request(changed).await,
        Err(remote("workflow_request_conflict"))
    );
    let profiles = fixture.client.workflow_planning_profiles().await.unwrap();
    changed = request.clone();
    changed.profile_id = serde_json::from_value(json!("alternate")).unwrap();
    changed.profile_digest =
        serde_json::from_value(profiles["profiles"][1]["profile_digest"].clone()).unwrap();
    assert_eq!(
        fixture.client.workflow_plan_request(changed).await,
        Err(remote("workflow_request_conflict"))
    );
    let mut query = request.query().unwrap();
    query.profile_digest = Sha256Digest::of(b"wrong profile");
    assert_eq!(
        fixture.client.workflow_planning_observation(query).await,
        Err(remote("workflow_contract_mismatch"))
    );
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn disabled_execution_and_generic_profile_namespace_fail_before_compiler_or_workers() {
    let (fixture, gateway) = fixture(false, false).await;
    let request = planning_request(&fixture, Uuid::new_v4(), true).await;
    assert_eq!(
        fixture.client.workflow_plan_request(request.clone()).await,
        Err(remote("workflow_request_invalid"))
    );
    for uncompiled in [false, true] {
        let mut generic = store_request(
            &format!("profile_{}", request.request_id.simple()),
            &fixture.head,
            "cooperate",
            &store_task(&SUITES),
            false,
        );
        if uncompiled {
            generic["task_specs"] = json!([]);
        }
        assert_eq!(
            fixture
                .client
                .workflow_prepare(generic, policy(&SUITES, 2, false))
                .await,
            Err(remote("workflow_request_invalid")),
        );
    }
    assert_eq!(gateway.counts().requests, 0);
    assert!(fixture.started_keys().is_empty());
    assert!(!fixture.state_dir.join("workflow_state/planning").exists());
    fixture.shutdown().await;
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn invalid_model_output_is_a_retained_rejection_without_preparation_or_replay() {
    let (mut fixture, gateway) = fixture(false, true).await;
    let request = planning_request(&fixture, Uuid::new_v4(), false).await;
    let rejected = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    wire(&rejected);
    assert_eq!(rejected["status"], "rejected", "{rejected:#}");
    assert!(rejected["workflow"].is_null());
    assert_eq!(rejected["retry_allowed"], false);
    fixture.restart().await;
    assert_eq!(
        fixture.client.workflow_plan_request(request).await.unwrap(),
        rejected
    );
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn unknown_compiler_receipt_and_claim_without_output_never_replay() {
    let (mut fixture, gateway) = fixture(false, false).await;
    let request = planning_request(&fixture, Uuid::new_v4(), false).await;
    gateway.set_mode(FixtureMode::ReceiptMissing);
    let unknown = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    wire(&unknown);
    assert_eq!(unknown["status"], "outcome_unknown", "{unknown:#}");
    gateway.set_mode(FixtureMode::Normal);
    fixture.restart().await;
    assert_eq!(
        fixture
            .client
            .workflow_plan_request(request.clone())
            .await
            .unwrap(),
        unknown
    );
    assert_eq!(gateway.counts().inference_requests, 1);

    let abandoned = planning_request(&fixture, Uuid::new_v4(), false).await;
    let query: PlanningQuery = abandoned.query().unwrap();
    let bytes = std::fs::read(fixture.state_dir.join(WORKFLOW_CONFIG_FILE_NAME)).unwrap();
    let config_digest = Sha256Digest::of_fields("vibemux.workflow.config.v1", &[&bytes]);
    let path = planning_path(&fixture, abandoned.request_id, "intent");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version":1,"profile_id":query.profile_id,"profile_digest":query.profile_digest,
            "request_id":query.request_id,"request_sha256":query.request_sha256,"execute":false,
            "config_digest":config_digest,
        }))
        .unwrap(),
    )
    .unwrap();
    let observed = fixture
        .client
        .workflow_plan_request(abandoned.clone())
        .await
        .unwrap();
    assert_eq!(observed["status"], "outcome_unknown");
    assert_eq!(observed["retry_allowed"], false);
    assert!(observed["workflow"].is_null());
    assert_eq!(gateway.counts().inference_requests, 1);
    std::fs::write(&path, b"{").unwrap();
    assert!(
        fixture
            .client
            .workflow_plan_request(abandoned)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"{");
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn execute_uses_original_start_uuid_once_and_retains_fixture_verifier_gates() {
    let (fixture, gateway) = fixture(true, false).await;
    fixture.write_script(&json!({"steps":{
        "store/implement_1":{"write":good_files(&["src/store.mjs"])},
        "store/review":review_pass(),
    }}));
    let request = planning_request(&fixture, Uuid::new_v4(), true).await;
    let started = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    wire(&started);
    assert_eq!(started["status"], "prepared", "{started:#}");
    assert_eq!(started["execution"]["status"], "observed");
    assert_eq!(
        started["execution"]["request_id"],
        json!(request.request_id)
    );
    assert_eq!(started["execution"]["workflow"]["generation"], 1);
    assert_eq!(started["execution"]["workflow"]["mode"], "deterministic");
    let workflow_id =
        Uuid::parse_str(started["workflow"]["workflow_id"].as_str().unwrap()).unwrap();
    let status = fixture
        .wait_phase(workflow_id, &["accepted", "failed", "blocked", "cancelled"])
        .await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    assert_eq!(status["evidence_class"], "fixture");
    let terminal = fixture
        .client
        .workflow_planning_observation(request.query().unwrap())
        .await
        .unwrap();
    wire(&terminal);
    let worker_log = fixture.log();
    let duplicate = fixture
        .client
        .workflow_plan_request(request.clone())
        .await
        .unwrap();
    assert_eq!(duplicate["execution"]["workflow"]["generation"], 1);
    assert_eq!(
        duplicate["execution"]["request_id"],
        json!(request.request_id)
    );
    assert_eq!(fixture.log(), worker_log);
    assert_eq!(gateway.counts().inference_requests, 1);
    assert!(planning_path(&fixture, request.request_id, "start").is_file());
    fixture.shutdown().await;
    gateway.close().await;
}
