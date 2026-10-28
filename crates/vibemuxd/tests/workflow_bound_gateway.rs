#![cfg(feature = "test_helpers")]
#[path = "aag_bound_fixture_support.rs"]
mod aag_bound_fixture_support;
mod workflow_support;

use aag_bound_fixture_support::{BoundFixtureServer, FIXTURE_TOKEN, FixtureMode};
use serde_json::{Value, json};
use vibemux_workflow::gateway::{
    GatewayConfig, GatewayProtocol, LiveAdmission, RouteBinding, RouteRole,
};
use vibemuxd::workflow::settings::WORKFLOW_CONFIG_FILE_NAME;
use workflow_support::{FixtureOptions, WorkflowFixture, policy, store_request, store_task};

const SUITES: [&str; 1] = ["store"];
const CREDENTIAL_ENV: &str = "AAG_BOUND_FACTORY_TOKEN";
const ALIASES: [&str; 4] = [
    "compiler_alias",
    "worker_a_alias",
    "worker_b_alias",
    "reviewer_alias",
];

fn configure(fixture: &WorkflowFixture, origin: &str, admission: LiveAdmission) {
    let binding = |role, alias: &str| RouteBinding {
        role,
        model_alias: alias.into(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 4096,
        timeout_ms: 60_000,
    };
    let config = GatewayConfig {
        schema_version: 1,
        endpoint: origin.into(),
        credential_env: CREDENTIAL_ENV.into(),
        strict_routing: true,
        live_admission: admission,
        bindings: vec![
            binding(RouteRole::Compiler, ALIASES[0]),
            binding(RouteRole::WorkerA, ALIASES[1]),
            binding(RouteRole::WorkerB, ALIASES[2]),
            binding(RouteRole::Reviewer, ALIASES[3]),
        ],
    };
    let path = fixture.state_dir.join(WORKFLOW_CONFIG_FILE_NAME);
    let mut current: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    current["gateway"] = serde_json::to_value(config).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&current).unwrap()).unwrap();
}

/// The task runner supplies only this test's synthetic token. No ambient real provider is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit source fixture with synthetic AAG_BOUND_FACTORY_TOKEN"]
async fn startup_owned_bound_factory_compiles_once_and_keeps_native_gates() {
    assert_eq!(
        std::env::var(CREDENTIAL_ENV).ok().as_deref(),
        Some(FIXTURE_TOKEN),
        "synthetic fixture token required"
    );
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.stop_server_for_test().await;
    let mut request = store_request(
        "bound_compiler_success",
        &fixture.head,
        "cooperate",
        &store_task(&SUITES),
        false,
    );
    let candidate = request["task_specs"][0].clone();
    request["task_specs"] = json!([]);
    let output = serde_json::to_string(&json!({"task_specs":[candidate]})).unwrap();
    let gateway = BoundFixtureServer::start(&ALIASES, &output).await;
    configure(&fixture, gateway.origin(), LiveAdmission::BoundReceiptV2);
    // This restart uses ordinary WorkflowService startup, with no injected model factory.
    fixture.restart().await;
    assert_eq!(gateway.counts().requests, 0, "startup is passive");
    let prepared = fixture
        .client
        .workflow_prepare(request.clone(), policy(&SUITES, 2, false))
        .await
        .expect("bound factory compiler");
    assert!(prepared.get("start_contract").is_some(), "{prepared:#}");
    let duplicate = fixture
        .client
        .workflow_prepare(request, policy(&SUITES, 2, false))
        .await
        .unwrap();
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(prepared["workflow_id"], duplicate["workflow_id"]);
    assert_eq!(gateway.counts().inference_requests, 1);
    assert_eq!(gateway.counts().receipt_reads, 1);
    assert!(
        fixture.started_keys().is_empty(),
        "model planning must not launch native work"
    );

    let mut uncertain = store_request(
        "bound_compiler_uncertain",
        &fixture.head,
        "cooperate",
        &store_task(&SUITES),
        false,
    );
    uncertain["task_specs"] = json!([]);
    gateway.set_mode(FixtureMode::ReceiptMissing);
    assert!(
        fixture
            .client
            .workflow_prepare(uncertain.clone(), policy(&SUITES, 2, false))
            .await
            .is_err()
    );
    let attempted = gateway.counts().inference_requests;
    gateway.set_mode(FixtureMode::Normal);
    assert!(
        fixture
            .client
            .workflow_prepare(uncertain, policy(&SUITES, 2, false))
            .await
            .is_err()
    );
    assert_eq!(
        gateway.counts().inference_requests,
        attempted,
        "uncertain call must not replay"
    );

    fixture.stop_server_for_test().await;
    configure(&fixture, gateway.origin(), LiveAdmission::Disabled);
    fixture.restart().await;
    let prior = gateway.counts().requests;
    let mut disabled = store_request(
        "bound_compiler_disabled",
        &fixture.head,
        "cooperate",
        &store_task(&SUITES),
        false,
    );
    disabled["task_specs"] = json!([]);
    assert!(
        fixture
            .client
            .workflow_prepare(disabled, policy(&SUITES, 2, false))
            .await
            .is_err()
    );
    assert_eq!(
        gateway.counts().requests,
        prior,
        "disabled policy cannot contact a model"
    );
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
    gateway.close().await;
}
