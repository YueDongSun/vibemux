#![cfg(feature = "test_helpers")]
//! Async goal acceptance exercises real owned fixture worktrees and verifier
//! processes. This is separate from, and does not replace, the IPC tests.

mod workflow_support;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest,
    gateway::{CallOutcome, GatewayConfig, ModelCallReceipt, ProviderAttribution, ReportedUsage},
    model_call::{ModelCallRequest, ModelCallState},
};
use vibemuxd::workflow::{
    error::WorkflowError,
    model_compiler::WorkflowModelGatewayFactory,
    model_runtime::{ModelGateway, ModelGatewayCompletion, ModelGatewayFailure},
    service::GoalSubmission,
};
use workflow_support::{
    InProcessWorkflowFixture, cooperative_request, good_files, policy, review_pass, store_request,
    store_task, track_a, track_b,
};

struct FixtureCompiler {
    calls: Mutex<Vec<Uuid>>,
}

struct FixtureCompilerFactory(Arc<FixtureCompiler>);

impl WorkflowModelGatewayFactory for FixtureCompilerFactory {
    fn for_workflow(
        &self,
        _workflow_id: Uuid,
        _config: &GatewayConfig,
        _digest: Sha256Digest,
    ) -> Result<Arc<dyn ModelGateway>, WorkflowError> {
        Ok(self.0.clone())
    }
}

#[async_trait]
impl ModelGateway for FixtureCompiler {
    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        _cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        self.calls.lock().unwrap().push(call_id);
        let input: Value = serde_json::from_str(&request.user_prompt).unwrap();
        assert!(input["task_specs"].as_array().unwrap().is_empty());
        assert_eq!(
            input["task_spec_format"]["example_candidate"]["schema_version"],
            1
        );
        assert!(!input["source_clauses"].as_array().unwrap().is_empty());
        let compiled = store_request(
            "fixture_output",
            input["base_commit"].as_str().unwrap(),
            "cooperate",
            &store_task(&["syntax"]),
            false,
        );
        let text = json!({"task_specs":compiled["task_specs"]}).to_string();
        let identity = request.identity().unwrap();
        Ok(ModelGatewayCompletion {
            receipt: ModelCallReceipt {
                call_id,
                role: identity.role,
                requested_alias: identity.requested_alias,
                protocol: identity.protocol,
                reported_model: Some("fixture_compiler".into()),
                effective_provider: ProviderAttribution::Unknown,
                usage: ReportedUsage::default(),
                http_status: None,
                outcome: CallOutcome::Completed,
                failure: None,
                elapsed_ms: 1,
                request_sha256: identity.request_sha256,
                response_sha256: Some(Sha256Digest::of(text.as_bytes())),
                route_generation: request.config_sha256,
                attempt,
            },
            text,
        })
    }
}

fn compiler_config() -> Value {
    let bindings: Vec<_> = ["compiler", "worker_a", "worker_b", "reviewer"].into_iter().map(|role| {
        json!({"role":role,"model_alias":format!("fixture_{role}"),"protocol":"openai_chat_completions","max_output_tokens":8192,"timeout_ms":10000})
    }).collect();
    json!({"schema_version":1,"endpoint":"http://127.0.0.1:1","credential_env":"AAG_FIXTURE_TOKEN","bindings":bindings,"strict_routing":true})
}

fn goal(fixture: &InProcessWorkflowFixture, event: Uuid, auto_start: bool) -> GoalSubmission {
    serde_json::from_value(json!({
        "schema_version":1,"event_id":event,"auto_start":auto_start,
        "request":cooperative_request("ignored_external_key", &fixture.head, &track_a(&["syntax"]), &track_b(&["syntax"]), &["syntax"]),
        "policy":policy(&["syntax"],2,true),
    })).unwrap()
}

fn ready_script() -> Value {
    json!({"steps": {
        "track_a/implement_1":{"write":good_files(&["src/server.mjs","src/store.mjs"])},
        "track_b/implement_1":{"write":good_files(&["public/app.mjs"])},
        "track_a/review_1":review_pass(),"track_b/review_1":review_pass()
    }})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_goal_runs_to_verified_result_and_exact_event_deduplicates() {
    let fixture = InProcessWorkflowFixture::start().await;
    fixture.write_script(&ready_script());
    let event = Uuid::new_v4();
    let first = fixture
        .service
        .submit_goal(goal(&fixture, event, true))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(first).unwrap()["state"], "accepted");
    let duplicate = fixture
        .service
        .submit_goal(goal(&fixture, event, true))
        .await
        .unwrap();
    assert!(duplicate.duplicate);
    let receipt = fixture.wait_operation(event).await;
    assert_eq!(receipt["state"], "running", "{receipt}");
    let id = Uuid::parse_str(receipt["workflow_id"].as_str().unwrap()).unwrap();
    assert!(
        fixture
            .service
            .wait_idle(id, std::time::Duration::from_secs(90))
            .await
    );
    let status = serde_json::to_value(fixture.service.status(id).await.unwrap()).unwrap();
    assert_eq!(status["phase"], "accepted", "{status}");
    assert!(
        !status["receipts"]["verifications"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let index = fixture.service.index(None, 32).await.unwrap();
    assert_eq!(index["workflows"].as_array().unwrap().len(), 1);
    assert_eq!(index["workflows"][0]["phase"], "accepted");
    assert!(
        !index
            .to_string()
            .contains(fixture.project_root.to_str().unwrap())
    );
    let duplicate = fixture
        .service
        .submit_goal(goal(&fixture, event, true))
        .await
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.workflow_id, Some(id));
    assert_eq!(
        fixture.service.index(None, 32).await.unwrap()["workflows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let conflict = fixture
        .service
        .submit_goal(goal(&fixture, event, false))
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), "workflow_operation_conflict");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prepare_only_is_explicit_and_index_pages_without_hiding_workflows() {
    let fixture = InProcessWorkflowFixture::start().await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let event = Uuid::new_v4();
        fixture
            .service
            .submit_goal(goal(&fixture, event, false))
            .await
            .unwrap();
        let receipt = fixture.wait_operation(event).await;
        assert_eq!(receipt["state"], "prepared");
        ids.push(receipt["workflow_id"].clone());
    }
    let first = fixture.service.index(None, 2).await.unwrap();
    assert_eq!(first["more"], true);
    let cursor = Uuid::parse_str(first["next"].as_str().unwrap()).unwrap();
    let second = fixture.service.index(Some(cursor), 2).await.unwrap();
    assert_eq!(second["more"], false);
    assert_eq!(second["workflows"].as_array().unwrap().len(), 1);
    let all: Vec<_> = first["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["workflows"].as_array().unwrap())
        .map(|value| value["workflow_id"].clone())
        .collect();
    assert!(ids.iter().all(|id| all.contains(id)));
    assert!(fixture.service.index(None, 0).await.is_err());
    assert!(fixture.service.index(None, 33).await.is_err());
    let readiness = fixture.service.readiness().await.unwrap();
    assert_eq!(readiness["configured"], true);
    assert_eq!(readiness["compiler_available"], false);
    assert_eq!(readiness["start_ready"], true);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_goal_compiles_once_through_the_model_ledger_and_reaches_verified_acceptance() {
    let compiler = Arc::new(FixtureCompiler {
        calls: Mutex::new(Vec::new()),
    });
    let fixture = InProcessWorkflowFixture::start_with_compiler(
        Arc::new(FixtureCompilerFactory(compiler.clone())),
        compiler_config(),
    )
    .await;
    fixture.write_script(&json!({"steps": {
        "store/implement_1":{"write":good_files(&["src/store.mjs"])},
        "store/review_1":review_pass(),
    }}));
    let event = Uuid::new_v4();
    let mut request = store_request(
        "ignored",
        &fixture.head,
        "cooperate",
        &store_task(&["syntax"]),
        false,
    );
    request["task_specs"] = json!([]);
    let submission = json!({"schema_version":1,"event_id":event,"request":request,"policy":policy(&["syntax"],2,true),"auto_start":true});
    fixture
        .service
        .submit_goal(serde_json::from_value(submission.clone()).unwrap())
        .await
        .unwrap();
    let receipt = fixture.wait_operation(event).await;
    assert_eq!(receipt["state"], "running", "{receipt}");
    let workflow_id = Uuid::parse_str(receipt["workflow_id"].as_str().unwrap()).unwrap();
    assert!(
        fixture
            .service
            .wait_idle(workflow_id, std::time::Duration::from_secs(90))
            .await
    );
    let status = serde_json::to_value(fixture.service.status(workflow_id).await.unwrap()).unwrap();
    assert_eq!(status["phase"], "accepted", "{status}");
    assert!(
        !status["receipts"]["verifications"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let duplicate = fixture
        .service
        .submit_goal(serde_json::from_value(submission).unwrap())
        .await
        .unwrap();
    assert!(duplicate.duplicate);
    let calls = compiler.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "the same goal cannot ask the compiler again"
    );
    let call = fixture.model_call(calls[0]);
    assert_eq!(call.state, ModelCallState::Completed);
    assert_eq!(call.calls_used, 1);
    assert_eq!(
        call.receipt.unwrap().reported_model.as_deref(),
        Some("fixture_compiler")
    );
    fixture.shutdown().await;
}
