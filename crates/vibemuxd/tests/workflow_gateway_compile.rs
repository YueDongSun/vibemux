#![cfg(feature = "test_helpers")]

mod workflow_support;

#[path = "gateway_fixture_support.rs"]
mod gateway_fixture_support;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gateway_fixture_support::{FakeAagServer, fixture_token};
use serde_json::{Value, json};
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
use workflow_support::{FixtureOptions, WorkflowFixture, policy, store_request, store_task};

const STORE_SUITE: [&str; 1] = ["store"];
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const COMPILER_ALIAS: &str = "compiler_alias";
const WORKER_A_ALIAS: &str = "worker_a_alias";
const WORKER_B_ALIAS: &str = "worker_b_alias";
const REVIEWER_ALIAS: &str = "reviewer_alias";
const REQUEST_KEY: &str = "gateway_compiler_request";
const STALE_REQUEST_KEY: &str = "gateway_compiler_stale_request";

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
    };
    let runtime_dir = fixture.scratch("gateway_compile_runtime");
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

fn violation_codes(response: &Value) -> Vec<String> {
    response["violation_codes"]
        .as_array()
        .expect("violation codes")
        .iter()
        .map(|code| code.as_str().expect("code").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_compiler_uses_only_permitted_fixture_gateway_and_revalidates_candidates() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.stop_server_for_test().await;

    let mut source_request = store_request(
        REQUEST_KEY,
        &fixture.head,
        "cooperate",
        &store_task(&STORE_SUITE),
        false,
    );
    let candidate = source_request["task_specs"][0].clone();
    source_request["task_specs"] = json!([]);
    let compiler_output = serde_json::to_string(&json!({"task_specs": [candidate]}))
        .expect("strict compiler response JSON");
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
    let config = gateway_config(gateway.origin());
    install_gateway_config(&fixture, &config);
    let fixture_guard = FixtureLaunchGuard::register_worker_copies(
        Path::new(workflow_support::WORKER_FIXTURE),
        &fixture.vendor_dir,
        &[AgentKind::Codex, AgentKind::Claude],
    )
    .expect("registered copies of the built synthetic workers");
    let factory: Arc<dyn WorkflowModelGatewayFactory> = Arc::new(FixtureGatewayFactory {
        permit: gateway.permit(),
    });
    let (server, client) = start_fixture_daemon(&fixture, fixture_guard, factory).await;

    let prepared = client
        .workflow_prepare(source_request.clone(), policy(&STORE_SUITE, 2, false))
        .await
        .expect("prepare through fixture compiler");
    assert!(prepared.get("start_contract").is_some(), "{prepared:#}");
    assert_eq!(prepared["duplicate"], false);

    let duplicate = client
        .workflow_prepare(source_request, policy(&STORE_SUITE, 2, false))
        .await
        .expect("same request key is cached");
    assert_eq!(duplicate["duplicate"], true, "{duplicate:#}");
    assert_eq!(duplicate["workflow_id"], prepared["workflow_id"]);

    let mut stale_request = store_request(
        STALE_REQUEST_KEY,
        &fixture.head,
        "cooperate",
        &store_task(&STORE_SUITE),
        false,
    );
    stale_request["task_specs"] = json!([]);
    let original_text = stale_request["request_text"]
        .as_str()
        .expect("request text")
        .to_string();
    stale_request["request_text"] = json!(format!(
        "{original_text} Also add an administration dashboard in `src/admin.mjs`."
    ));
    let rejected = client
        .workflow_prepare(stale_request, policy(&STORE_SUITE, 2, false))
        .await
        .expect("stale model candidate receives a typed source-validation result");
    assert_eq!(rejected["outcome"], "rejected", "{rejected:#}");
    assert_eq!(rejected["error_code"], "workflow_contract_rejected");
    assert!(
        violation_codes(&rejected)
            .iter()
            .any(|code| code == "request_digest_mismatch"),
        "stale candidate was not rejected by source validation: {rejected:#}"
    );
    assert!(rejected.get("start_contract").is_none());

    let calls = gateway.calls();
    assert_eq!(
        calls.len(),
        2,
        "same-key prepare must not call the compiler again"
    );
    assert!(calls.iter().all(|call| {
        call.alias == COMPILER_ALIAS && call.authorization_ok && !call.correlation.is_empty()
    }));
    assert_ne!(calls[0].correlation, calls[1].correlation);
    assert!(fixture.started_keys().is_empty());

    server.shutdown().await.expect("model daemon shutdown");
    fixture.shutdown().await;
}
