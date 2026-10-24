#![cfg(feature = "test_helpers")]
//! Model decisions drive actual fixture work; transport/model usage is synthetic.
#[path = "workflow_supervisor_context/cases.rs"]
mod context_cases;
mod workflow_support;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{Notify, watch};
use uuid::Uuid;
use vibemux_harness::{AgentKind, dispatch::FixtureLaunchGuard};
use vibemux_workflow::{
    Sha256Digest,
    gateway::{
        CallOutcome, GATEWAY_CONFIG_SCHEMA_VERSION, GatewayConfig, GatewayProtocol,
        ModelCallReceipt, ProviderAttribution, ReportedUsage, RouteBinding, RouteRole,
    },
    model_call::ModelCallRequest,
    supervisor::SupervisorObservation,
};
use vibemuxd::{
    control::{ControlClient, DaemonControlServer, ExecutionSettings},
    harness_dispatch::{DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings},
    workflow::{
        WorkflowServiceSettings,
        error::WorkflowError,
        model_compiler::WorkflowModelGatewayFactory,
        model_runtime::{ModelGateway, ModelGatewayCompletion, ModelGatewayFailure},
        settings::WORKFLOW_CONFIG_FILE_NAME,
    },
};
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, phase, policy, rendezvous, review_pass,
    taskboard_file, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
/// Both model-assigned implementation turns wait here for each other, so
/// they overlap however far apart a slow host launches them.
const IMPLEMENT_OVERLAP_GROUP: &str = "model_implement_overlap";
const IMPLEMENT_PARTIES: usize = 2;
/// Held after the rendezvous so each turn finishes strictly after the other
/// one has started.
const TURN_DELAY_MS: u64 = 100;
const SUPERVISOR_ALIAS: &str = "supervisor_fixture";

#[derive(Clone, Copy)]
enum Strategy {
    Useful,
    WrongTaskSlot,
    UnknownArtifact,
    Malformed,
    HoldThenAssign,
    ContextActions,
    ContextLeakAttack,
}

struct FixtureModel {
    strategy: Strategy,
    calls: AtomicUsize,
    actions: Mutex<Vec<String>>,
    contexts: Mutex<Vec<Value>>,
    entered: Notify,
    release: Notify,
}

impl FixtureModel {
    fn new(strategy: Strategy) -> Self {
        Self {
            strategy,
            calls: AtomicUsize::new(0),
            actions: Mutex::new(Vec::new()),
            contexts: Mutex::new(Vec::new()),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }

    fn action_sequence(&self, index: usize, observation: &SupervisorObservation) -> Vec<Value> {
        if matches!(self.strategy, Strategy::WrongTaskSlot) {
            return vec![
                json!({"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"slot_b"}}),
            ];
        }
        if matches!(self.strategy, Strategy::UnknownArtifact) {
            return vec![
                json!({"tool":"read_artifact","arguments":{"artifact_digest":Sha256Digest::of(b"unowned artifact")}}),
            ];
        }
        if matches!(self.strategy, Strategy::HoldThenAssign) {
            return vec![
                json!({"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"slot_a"}}),
            ];
        }
        if index == 0 {
            return vec![json!({"tool":"inspect_workflow","arguments":{}})];
        }
        if index == 1 {
            return vec![
                json!({"tool":"propose_plan","arguments":{"tasks":["track_a","track_b"]}}),
            ];
        }
        let mut pending = Vec::new();
        for task in &observation.workflow.tasks {
            let candidate = observation
                .candidates
                .iter()
                .rev()
                .find(|candidate| candidate.candidate.task_key == task.task_key);
            let Some(candidate) = candidate else {
                let slots = observation
                    .eligible_slots
                    .get(&task.task_key)
                    .expect("fixture task eligibility");
                let slot = slots.first().expect("owned fixture slot");
                pending.push(json!({"tool":"assign_task","arguments":{"task_key":task.task_key,"slot_id":slot}}));
                continue;
            };
            if !candidate.reviewed {
                pending.push(json!({"tool":"request_review","arguments":{"task_key":task.task_key,"candidate_digest":candidate.candidate.candidate_digest}}));
            } else if !candidate.verified {
                pending.push(json!({"tool":"request_verification","arguments":{"task_key":task.task_key,"candidate_digest":candidate.candidate.candidate_digest}}));
            }
        }
        if pending.is_empty() {
            let accepted: Vec<_> = observation
                .workflow
                .tasks
                .iter()
                .map(|task| {
                    task.accepted_candidate
                        .expect("writer accepted each independently gated candidate")
                })
                .collect();
            pending.push(
                json!({"tool":"propose_integration","arguments":{"candidate_digests":accepted}}),
            );
        }
        pending
    }
}

#[async_trait]
impl ModelGateway for FixtureModel {
    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        _cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        assert_eq!(request.role, RouteRole::Supervisor);
        assert_eq!(request.requested_alias, SUPERVISOR_ALIAS);
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let context: Value =
            serde_json::from_str(&request.user_prompt).expect("bounded supervisor context");
        assert_eq!(context["schema_version"], 1);
        let observation: SupervisorObservation =
            serde_json::from_value(context["observation"].clone())
                .expect("committed typed observation");
        self.contexts
            .lock()
            .expect("captured model contexts")
            .push(context.clone());
        if matches!(self.strategy, Strategy::HoldThenAssign) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        let text = if matches!(self.strategy, Strategy::Malformed) {
            "{not a valid proposal".to_owned()
        } else {
            let actions = if matches!(self.strategy, Strategy::ContextActions) {
                context_cases::actions(index, &observation, &context)
            } else if matches!(self.strategy, Strategy::ContextLeakAttack) {
                context_cases::leak_actions(index, &observation, &context)
            } else {
                self.action_sequence(index, &observation)
            };
            let decisions: Vec<_> = actions.into_iter().enumerate().map(|(ordinal, action)| {
                self.actions.lock().expect("action log").push(action["tool"].as_str().expect("tool").to_owned());
                json!({"idempotency_key":format!("fixture_{index}_{ordinal}"),"expected_workflow_version":observation.workflow.version,"action":action})
            }).collect();
            serde_json::to_string(&json!({"decisions":decisions,"rationale":"Use only committed fixture facts and existing gates.","evidence_refs":[]})).expect("strict proposal")
        };
        let identity = request.identity().expect("request identity");
        let receipt = ModelCallReceipt {
            call_id,
            role: request.role,
            requested_alias: request.requested_alias.clone(),
            protocol: request.protocol,
            reported_model: Some("synthetic_supervisor".into()),
            effective_provider: ProviderAttribution::Unknown,
            usage: ReportedUsage::default(),
            http_status: Some(200),
            outcome: CallOutcome::Completed,
            failure: None,
            elapsed_ms: 1,
            request_sha256: identity.request_sha256,
            response_sha256: Some(Sha256Digest::of(text.as_bytes())),
            route_generation: Sha256Digest::of(b"fixture observation, not upstream attestation"),
            attempt,
        };
        Ok(ModelGatewayCompletion { text, receipt })
    }
}

struct FixtureFactory {
    model: Arc<FixtureModel>,
}
impl WorkflowModelGatewayFactory for FixtureFactory {
    fn for_workflow(
        &self,
        _workflow_id: Uuid,
        _config: &GatewayConfig,
        _digest: Sha256Digest,
    ) -> Result<Arc<dyn ModelGateway>, WorkflowError> {
        Ok(self.model.clone())
    }
}

async fn start_model_fixture(
    strategy: Strategy,
) -> (
    WorkflowFixture,
    DaemonControlServer,
    ControlClient,
    Arc<FixtureModel>,
) {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.stop_server_for_test().await;
    let path = fixture.state_dir.join(WORKFLOW_CONFIG_FILE_NAME);
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("config")).expect("config JSON");
    let binding = |role, alias: &str| RouteBinding {
        role,
        model_alias: alias.into(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 4096,
        timeout_ms: 60_000,
    };
    let gateway = GatewayConfig {
        schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
        endpoint: "http://127.0.0.1:9".into(),
        credential_env: "UNUSED_FIXTURE_CREDENTIAL".into(),
        strict_routing: true,
        bindings: vec![
            binding(RouteRole::Supervisor, SUPERVISOR_ALIAS),
            binding(RouteRole::WorkerA, "worker_a_alias"),
            binding(RouteRole::WorkerB, "worker_b_alias"),
            binding(RouteRole::Reviewer, "reviewer_alias"),
        ],
    };
    config["gateway"] = serde_json::to_value(gateway).expect("gateway config");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&config).expect("config bytes"),
    )
    .expect("write config");
    let guard = FixtureLaunchGuard::register_worker_copies(
        Path::new(workflow_support::WORKER_FIXTURE),
        &fixture.vendor_dir,
        &[AgentKind::Codex, AgentKind::Claude],
    )
    .expect("owned built fixture copies");
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
    let model = Arc::new(FixtureModel::new(strategy));
    let server = DaemonControlServer::start_with_model_fixture_execution(
        &fixture.state_dir.join("vibemux_rust.sqlite3"),
        &fixture.scratch("model_supervisor_runtime"),
        settings,
        guard,
        Arc::new(FixtureFactory {
            model: model.clone(),
        }),
    )
    .await
    .expect("model fixture daemon");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("control client");
    fixture.client =
        ControlClient::from_descriptor(server.descriptor_path()).expect("fixture status client");
    (fixture, server, client, model)
}

fn write_model_worker_script(fixture: &WorkflowFixture) {
    let mut store = taskboard_file("calibration/good/src/store.mjs");
    store.push_str("\n// model supervisor fixture candidate\n");
    let mut page = taskboard_file("calibration/good/public/app.mjs");
    page.push_str("\n// model supervisor fixture candidate\n");
    let overlap = rendezvous(IMPLEMENT_OVERLAP_GROUP, IMPLEMENT_PARTIES);
    fixture.write_script(&json!({"steps":{
        "track_a/implement_1":{"write":{"src/store.mjs":store},
            "rendezvous":overlap,"delay_ms":TURN_DELAY_MS},
        "track_b/implement_1":{"write":{"public/app.mjs":page},
            "rendezvous":overlap,"delay_ms":TURN_DELAY_MS},
        "track_a/review_1":review_pass(),"track_b/review_1":review_pass()
    }}));
}

async fn prepare_model(client: &ControlClient, fixture: &WorkflowFixture, key: &str) -> Value {
    client
        .workflow_prepare(
            cooperative_request(
                key,
                &fixture.head,
                &track_a(&SYNTAX),
                &track_b(&SYNTAX),
                &SYNTAX,
            ),
            policy(&SYNTAX, 2, false),
        )
        .await
        .expect("prepare admitted fixture contracts")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_supervisor_runs_owned_workers_and_accepts_only_after_real_gates() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::Useful).await;
    write_model_worker_script(&fixture);
    let prepared = prepare_model(&client, &fixture, "model_supervisor_useful").await;
    let workflow = workflow_id(&prepared);
    let started = client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("contract"),
            Uuid::new_v4(),
        )
        .await
        .expect("explicit model driver start");
    assert_ne!(started["phase"], "blocked", "{started:#}");
    // A failed rendezvous also ends the wait: the group will never run
    // together, so report that instead of waiting out the model's retries.
    let status = fixture
        .wait_until(workflow, |status| {
            fixture.rendezvous_failed()
                || (status["running_here"] == false
                    && ["accepted", "failed", "blocked"]
                        .iter()
                        .any(|expected| status["phase"] == *expected))
        })
        .await;
    assert!(
        !fixture.rendezvous_failed(),
        "a model-assigned implementation turn never saw the other one running: {status:#}"
    );
    fixture.assert_phase(&status, "accepted").await;
    {
        let actions = model.actions.lock().expect("action log");
        for expected in [
            "inspect_workflow",
            "propose_plan",
            "assign_task",
            "request_review",
            "request_verification",
            "propose_integration",
        ] {
            assert!(
                actions.iter().any(|value| value == expected),
                "missing genuine model decision {expected}: {actions:?}"
            );
        }
    }
    assert_eq!(fixture.started("track_a/implement_1").len(), 1);
    assert_eq!(fixture.started("track_b/implement_1").len(), 1);
    // Each turn held itself open until the other had started, so the
    // ordering below follows from the rendezvous and not from launch timing.
    let log = fixture.log();
    let turn_time = |key: &str, event: &str| {
        log.iter()
            .find(|record| record["key"] == key && record["event"] == event)
            .and_then(|record| record["at_ms"].as_u64())
            .expect("owned fixture turn timestamp")
    };
    assert!(
        turn_time("track_a/implement_1", "started") < turn_time("track_b/implement_1", "finished")
            && turn_time("track_b/implement_1", "started")
                < turn_time("track_a/implement_1", "finished"),
        "the two model-assigned worker turns must overlap"
    );
    assert!(model.calls.load(Ordering::SeqCst) >= 6);
    assert_eq!(fixture.checkout_status(), "");
    server.shutdown().await.expect("owned server shutdown");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_proposals_cannot_assign_another_tasks_slot_or_read_unknown_artifacts() {
    for (strategy, key) in [
        (Strategy::WrongTaskSlot, "model_wrong_slot"),
        (Strategy::UnknownArtifact, "model_unknown_artifact"),
    ] {
        let (fixture, server, client, model) = start_model_fixture(strategy).await;
        let prepared = prepare_model(&client, &fixture, key).await;
        let workflow = workflow_id(&prepared);
        client
            .workflow_supervisor_start(
                prepared["start_contract"].as_str().expect("contract"),
                Uuid::new_v4(),
            )
            .await
            .expect("model driver start");
        let status = fixture
            .wait_phase(workflow, &["accepted", "failed", "blocked"])
            .await;
        assert_ne!(phase(&status), "accepted", "{status:#}");
        assert!(fixture.started_keys().is_empty());
        assert!(
            model.calls.load(Ordering::SeqCst) <= 3,
            "proposal repairs must be bounded"
        );
        server.shutdown().await.expect("owned server shutdown");
        fixture.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_model_proposals_stop_at_the_persisted_repair_ceiling() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::Malformed).await;
    let prepared = prepare_model(&client, &fixture, "model_malformed").await;
    client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("contract"),
            Uuid::new_v4(),
        )
        .await
        .expect("model driver start");
    let status = fixture
        .wait_phase(workflow_id(&prepared), &["accepted", "failed", "blocked"])
        .await;
    assert_ne!(phase(&status), "accepted", "{status:#}");
    assert!(fixture.started_keys().is_empty());
    assert!(model.calls.load(Ordering::SeqCst) <= 3);
    server.shutdown().await.expect("owned server shutdown");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_during_model_inference_prevents_a_delayed_assignment() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::HoldThenAssign).await;
    let prepared = prepare_model(&client, &fixture, "model_cancel_inference").await;
    let workflow = workflow_id(&prepared);
    client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("contract"),
            Uuid::new_v4(),
        )
        .await
        .expect("model driver start");
    tokio::time::timeout(std::time::Duration::from_secs(10), model.entered.notified())
        .await
        .expect("owned model inference entered");
    client
        .workflow_cancel(workflow)
        .await
        .expect("operator cancellation");
    model.release.notify_one();
    let status = fixture
        .wait_phase(workflow, &["cancelled", "failed", "blocked"])
        .await;
    assert_ne!(phase(&status), "accepted", "{status:#}");
    assert!(fixture.started_keys().is_empty());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    server.shutdown().await.expect("owned server shutdown");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_during_uncommitted_inference_reports_recovery_required() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::HoldThenAssign).await;
    let prepared = prepare_model(&client, &fixture, "model_pause_inference").await;
    let workflow = workflow_id(&prepared);
    client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("contract"),
            Uuid::new_v4(),
        )
        .await
        .expect("model driver start");
    tokio::time::timeout(std::time::Duration::from_secs(10), model.entered.notified())
        .await
        .expect("owned inference entered");
    client
        .workflow_pause(workflow)
        .await
        .expect("operator pause");
    model.release.notify_one();
    let status = fixture
        .wait_phase(workflow, &["blocked", "paused", "failed"])
        .await;
    assert_eq!(
        phase(&status),
        "blocked",
        "uncommitted response cannot appear safely resumable: {status:#}"
    );
    assert_eq!(status["blocked_reason"], "supervisor_recovery_required");
    assert!(fixture.started_keys().is_empty());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let ledger = client
        .workflow_supervisor_status(workflow)
        .await
        .expect("durable supervisor status");
    assert_eq!(
        ledger["execution"]["state"], "recovery_required",
        "{ledger:#}"
    );
    server.shutdown().await.expect("owned server shutdown");
    fixture.shutdown().await;
}
