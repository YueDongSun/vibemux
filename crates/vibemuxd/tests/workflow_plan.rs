#![cfg(feature = "test_helpers")]
//! Planning entrypoint (ADR 031 planning contract): a goal is compiled
//! through a pinned profile over the fixture model-compiler route, admitted
//! through the normal prepare/start gates, and answered as a content-free
//! planning observation. Replaying the same request UUID reuses the
//! admitted plan without a second compiler call, a changed goal under the
//! same UUID is refused, and a stale profile digest is refused before any
//! prepare. Fixture evidence only: no live model call.

mod workflow_support;

#[path = "gateway_fixture_support.rs"]
mod gateway_fixture_support;

use std::{path::PathBuf, sync::Arc};

use gateway_fixture_support::{FakeAagServer, fixture_token};
use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_harness::{AgentKind, dispatch::FixtureLaunchGuard};
use vibemux_workflow::gateway::{
    GATEWAY_CONFIG_SCHEMA_VERSION, GatewayConfig, GatewayProtocol, RouteBinding, RouteRole,
};
use vibemuxd::{
    control::{ControlClient, DaemonControlServer, ExecutionSettings},
    harness_dispatch::{DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings},
    workflow::{
        WorkflowServiceSettings,
        aag_gateway::{Aag_Call_Class, Aag_Credential, Aag_Model_Gateway},
        error::WorkflowError,
        model_compiler::WorkflowModelGatewayFactory,
        model_runtime::{AagModelGateway, ModelGateway},
        settings::WORKFLOW_CONFIG_FILE_NAME,
    },
};
use workflow_support::{
    FixtureOptions, WorkflowFixture, good_files, policy, review_pass, store_request, store_task,
};

const STORE_SUITE: [&str; 1] = ["store"];
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const COMPILER_ALIAS: &str = "compiler_alias";
const WORKER_A_ALIAS: &str = "worker_a_alias";
const WORKER_B_ALIAS: &str = "worker_b_alias";
const REVIEWER_ALIAS: &str = "reviewer_alias";

struct FixtureGatewayFactory {
    permit: vibemuxd::workflow::aag_gateway::Aag_Fixture_Permit,
}

impl WorkflowModelGatewayFactory for FixtureGatewayFactory {
    fn for_workflow(
        &self,
        workflow_id: uuid::Uuid,
        config: &GatewayConfig,
        pinned_digest: vibemux_workflow::Sha256Digest,
    ) -> Result<Arc<dyn ModelGateway>, WorkflowError> {
        let credential =
            Aag_Credential::explicit(fixture_token()).map_err(|_| WorkflowError::Gateway)?;
        let gateway = Aag_Model_Gateway::new(config.clone(), pinned_digest, credential)
            .map_err(|_| WorkflowError::Gateway)?
            .with_fixture_permit(self.permit.clone())
            .map_err(|_| WorkflowError::Gateway)?;
        Ok(Arc::new(AagModelGateway::new(
            gateway,
            workflow_id,
            Aag_Call_Class::Fixture,
        )))
    }
}

fn gateway_config(endpoint: &str) -> GatewayConfig {
    let binding = |role, model_alias: &str| RouteBinding {
        role,
        model_alias: model_alias.to_string(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 2048,
        timeout_ms: 60_000,
    };
    GatewayConfig {
        schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
        endpoint: endpoint.to_string(),
        credential_env: "AAG_FIXTURE_TOKEN".to_string(),
        bindings: vec![
            binding(RouteRole::Compiler, COMPILER_ALIAS),
            binding(RouteRole::WorkerA, WORKER_A_ALIAS),
            binding(RouteRole::WorkerB, WORKER_B_ALIAS),
            binding(RouteRole::Reviewer, REVIEWER_ALIAS),
        ],
        strict_routing: true,
    }
}

fn install_gateway_config(fixture: &WorkflowFixture, config: &GatewayConfig) {
    let path = fixture.state_dir.join(WORKFLOW_CONFIG_FILE_NAME);
    let mut workflow_config: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("workflow config"))
            .expect("workflow config JSON");
    workflow_config["gateway"] = serde_json::to_value(config).expect("serialize gateway config");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&workflow_config).expect("encode workflow config"),
    )
    .expect("write gateway-enabled workflow config");
}

async fn start_fixture_daemon(
    fixture: &WorkflowFixture,
    fixture_guard: FixtureLaunchGuard,
    gateway_factory: Arc<dyn WorkflowModelGatewayFactory>,
) -> (DaemonControlServer, ControlClient) {
    let settings = ExecutionSettings {
        dispatch: DispatchServiceSettings {
            project_root: fixture.project_root.clone(),
            config_path: fixture.state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            trampoline: Some(PathBuf::from(workflow_support::TRAMPOLINE)),
        },
        workflow: Some(WorkflowServiceSettings::for_project(
            &fixture.project_root,
            &fixture.state_dir,
        )),
        decision: None,
    };
    let runtime_dir = fixture.scratch("plan_runtime");
    let database_path = fixture.state_dir.join(DATABASE_FILE_NAME);
    let server = DaemonControlServer::start_with_model_fixture_execution(
        &database_path,
        &runtime_dir,
        settings,
        fixture_guard,
        gateway_factory,
    )
    .await
    .expect("fixture model daemon");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("control client");
    (server, client)
}

/// The pinned profile: one store task on `slot_a`, the store suite, and the
/// operator policy every planned request is admitted under.
fn planning_profile() -> Value {
    json!({
        "profile_id": "fixture_store",
        "workflow_key": "taskboard_lite",
        "mode": "cooperate",
        "worker_slots": ["slot_a"],
        "reviewer_slot": "review",
        "integration_suites": ["store"],
        "task_assignments": [{"task_key": "store", "slots": ["slot_a"]}],
        "policy": policy(&STORE_SUITE, 2, false),
        "max_tasks": 1,
        "allow_execution": true,
        "fixture": true,
    })
}

fn planning_request(digest: &str, request_id: Uuid, goal: &str, execute: bool) -> Value {
    json!({
        "schema_version": 1,
        "profile_id": "fixture_store",
        "profile_digest": digest,
        "request_id": request_id,
        "request_text": goal,
        "execute": execute,
    })
}

/// The digest-bound recovery query the Hub sends after a lost submit reply;
/// it never carries the goal text.
fn planning_query(digest: &str, request_id: Uuid, goal: &str) -> Value {
    json!({
        "schema_version": 1,
        "profile_id": "fixture_store",
        "profile_digest": digest,
        "request_id": request_id,
        "request_sha256": vibemux_workflow::planning_contract::request_sha256(goal),
        "execute": true,
    })
}

async fn catalog_digest(client: &ControlClient) -> String {
    let catalog = client.workflow_planning_profiles().await.expect("catalog");
    catalog["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .find(|entry| entry["profile_id"] == "fixture_store")
        .and_then(|entry| entry["profile_digest"].as_str())
        .expect("profile digest")
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_compiles_replays_and_never_repeats_the_compiler_call() {
    let mut fixture = WorkflowFixture::start(FixtureOptions {
        planning_profiles: vec![planning_profile()],
        ..FixtureOptions::default()
    })
    .await;
    fixture.stop_server_for_test().await;

    // The pinned compiler output is the store task spec compiled for the
    // exact goal text; the deterministic admission revalidates it.
    let seed = store_request(
        "plan_seed",
        &fixture.head,
        "cooperate",
        &store_task(&STORE_SUITE),
        false,
    );
    let goal = seed["request_text"]
        .as_str()
        .expect("goal text")
        .to_string();
    let candidate = seed["task_specs"][0].clone();
    let compiler_output =
        serde_json::to_string(&json!({"task_specs": [candidate]})).expect("compiler JSON");

    let gateway = FakeAagServer::start(
        vec![
            COMPILER_ALIAS.to_string(),
            WORKER_A_ALIAS.to_string(),
            WORKER_B_ALIAS.to_string(),
            REVIEWER_ALIAS.to_string(),
        ],
        compiler_output,
    )
    .await;
    install_gateway_config(&fixture, &gateway_config(gateway.origin()));
    let fixture_guard = FixtureLaunchGuard::register_worker_copies(
        &std::fs::canonicalize(workflow_support::WORKER_FIXTURE).expect("canonical built worker"),
        &fixture.vendor_dir,
        &[AgentKind::Codex, AgentKind::Claude],
    )
    .expect("registered copies of the built synthetic workers");
    let factory: Arc<dyn WorkflowModelGatewayFactory> = Arc::new(FixtureGatewayFactory {
        permit: gateway.permit(),
    });
    let (server, client) = start_fixture_daemon(&fixture, fixture_guard, factory).await;

    fixture.write_script(&json!({"steps": {
        "store/implement_1": {"write": good_files(&["src/store.mjs"])},
        "store/review": review_pass(),
    }}));

    let digest = catalog_digest(&client).await;
    let request_id = Uuid::new_v4();

    let planned = client
        .workflow_plan_request(planning_request(&digest, request_id, &goal, true))
        .await
        .expect("plan through the pinned profile");
    assert_eq!(planned["schema_version"], json!(1));
    assert_eq!(planned["status"], json!("prepared"));
    assert_eq!(planned["retry_allowed"], json!(false));
    assert_eq!(planned["request_id"], json!(request_id));
    assert_eq!(planned["profile_id"], json!("fixture_store"));
    assert_eq!(planned["profile_digest"], json!(digest));
    assert_eq!(
        planned["request_sha256"],
        json!(vibemux_workflow::planning_contract::request_sha256(&goal))
    );
    let workflow_id: Uuid = planned["workflow"]["workflow_id"]
        .as_str()
        .and_then(|text| text.parse().ok())
        .expect("workflow id");
    let start_contract = planned["workflow"]["start_contract"]
        .as_str()
        .expect("contract");
    assert_eq!(start_contract.len(), 64);
    assert!(planned["workflow"]["version"].as_u64().is_some());
    assert!(planned["execution"].is_null());
    // The observation never carries the goal text.
    assert!(!planned.to_string().contains(&goal));

    // The lost-submit replay: the same request returns the same workflow
    // and never calls the compiler again.
    let replay = client
        .workflow_plan_request(planning_request(&digest, request_id, &goal, true))
        .await
        .expect("replay");
    assert_eq!(
        replay["workflow"]["workflow_id"],
        json!(workflow_id.to_string())
    );
    assert_eq!(replay["workflow"]["start_contract"], json!(start_contract));
    let calls = gateway.calls();
    assert_eq!(calls.len(), 1, "replay must not call the compiler again");

    // The planned workflow runs through the normal gates to acceptance.
    let mut accepted = false;
    for _ in 0..600 {
        let status = client.workflow_status(workflow_id).await.expect("status");
        if matches!(
            status["phase"].as_str(),
            Some("accepted" | "failed" | "blocked" | "cancelled")
        ) {
            assert_eq!(status["phase"], json!("accepted"), "{status:#}");
            accepted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(accepted, "workflow did not settle in time");

    // The digest-bound recovery query returns the same observation without
    // the goal text, and an unknown request answers not_observed.
    let recovered = client
        .workflow_planning_observation(planning_query(&digest, request_id, &goal))
        .await
        .expect("recovery query");
    assert_eq!(recovered["status"], json!("prepared"));
    assert_eq!(
        recovered["workflow"]["workflow_id"],
        json!(workflow_id.to_string())
    );
    assert_eq!(
        recovered["workflow"]["start_contract"],
        json!(start_contract)
    );
    assert!(recovered["execution"].is_null());
    assert!(!recovered.to_string().contains(&goal));
    let unknown = client
        .workflow_planning_observation(planning_query(&digest, Uuid::new_v4(), &goal))
        .await
        .expect("unknown query");
    assert_eq!(unknown["status"], json!("not_observed"));
    assert!(unknown["workflow"].is_null());
    let mismatched = client
        .workflow_planning_observation(json!({
            "schema_version": 1,
            "profile_id": "fixture_store",
            "profile_digest": digest,
            "request_id": request_id,
            "request_sha256": "d".repeat(64),
            "execute": true,
        }))
        .await
        .expect_err("a changed content digest must not observe");
    assert_eq!(mismatched.code(), "workflow_request_conflict");
    assert_eq!(gateway.calls().len(), 1, "no further compiler calls");

    // A changed goal under the same request UUID is refused.
    let changed = client
        .workflow_plan_request(planning_request(
            &digest,
            request_id,
            &format!("{goal} Also add an export button."),
            true,
        ))
        .await
        .expect_err("a changed goal must not replay");
    assert_eq!(changed.code(), "workflow_request_conflict");

    // A stale profile digest is refused before any prepare.
    let stale = client
        .workflow_plan_request(planning_request(
            &"a".repeat(64),
            Uuid::new_v4(),
            &goal,
            true,
        ))
        .await
        .expect_err("a stale digest must not plan");
    assert_eq!(stale.code(), "workflow_planning_profile_mismatch");
    assert_eq!(gateway.calls().len(), 1, "no further compiler calls");

    server.shutdown().await.expect("model daemon shutdown");
    fixture.shutdown().await;
}
