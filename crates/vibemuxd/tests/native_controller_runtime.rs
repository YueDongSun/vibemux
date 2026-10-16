#![cfg(feature = "test_helpers")]
//! Real writer transactions and an owned byte-queue adapter. These fixtures
//! exercise the daemon effect boundary; they are not native/live terminal proof.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_store::{ControllerOutboxRecord, ControllerOutboxState};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::{ContractArtifact, RouteTransformation},
    gates::WorkflowPhase,
    native_controller::{
        ApprovalChallenge, ApprovalDecision, ControllerActor, ControllerCommand,
        ControllerIdentity, ControllerOwner, ControllerPhase, DeliveryState, HandoffTarget,
        HandoffTransfer, InputReservation, InterruptionState, NativeController, SubmissionOutcome,
    },
    slots::EvidenceClass,
    supervisor::LoopBudget,
    task_spec::WorkflowMode,
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};
use vibemuxd::{
    WriterHandle, WriterWorker,
    workflow::native_runtime::{
        AdapterDelivery, AdapterResourceState, ControllerAcknowledgement, ControllerAdapter,
        ControllerAdapterCapabilities, ControllerAdapterError, ControllerInventory,
        ControllerOperationKind, ControllerResource, LiteralInput, NativeControllerRuntime,
        NativeInputRequest, NativeInputStatus, NativeRuntimeError, NativeRuntimeLimits,
    },
};

const QUEUE_CAPACITY: usize = 16;
const INPUT_A: &str = "literal @src/a.rs; $(do_not_execute)\n任务 ✓";
const INPUT_B: &str = "second controller \"quoted\" input\n/compact is data";

fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("identifier")
}
fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

#[derive(Clone, Copy)]
enum DeliveryMode {
    Acknowledge,
    Unknown,
    WrongCorrelation,
}

struct FixtureResource {
    inventory: ControllerInventory,
    delivery: DeliveryMode,
    input: VecDeque<(Uuid, Vec<u8>)>,
    interruptions: usize,
    attachments: usize,
}

#[derive(Default)]
struct OwnedFixtureAdapter {
    resources: Mutex<BTreeMap<Uuid, FixtureResource>>,
}

impl OwnedFixtureAdapter {
    fn register(&self, inventory: ControllerInventory, delivery: DeliveryMode) {
        let mut resources = self.resources.lock().expect("resources");
        assert!(
            !resources
                .values()
                .any(|registered| registered.inventory.resource.controller
                    == inventory.resource.controller),
            "one resource per physical controller generation"
        );
        let previous = resources.insert(
            inventory.resource.resource_id,
            FixtureResource {
                inventory,
                delivery,
                input: VecDeque::new(),
                interruptions: 0,
                attachments: 0,
            },
        );
        assert!(previous.is_none(), "resource registration is unique");
    }

    fn input(&self, resource_id: Uuid) -> Vec<(Uuid, Vec<u8>)> {
        self.resources.lock().expect("resources")[&resource_id]
            .input
            .iter()
            .cloned()
            .collect()
    }

    fn set_state(&self, resource_id: Uuid, state: AdapterResourceState) {
        self.resources
            .lock()
            .expect("resources")
            .get_mut(&resource_id)
            .expect("owned resource")
            .inventory
            .state = state;
    }

    fn acknowledgement(
        resource: &ControllerResource,
        operation: ControllerOperationKind,
        operation_id: Uuid,
        payload_digest: Option<Sha256Digest>,
    ) -> AdapterDelivery {
        AdapterDelivery::Acknowledged(Box::new(ControllerAcknowledgement {
            resource: resource.clone(),
            operation,
            operation_id,
            payload_digest,
        }))
    }
}

#[async_trait]
impl ControllerAdapter for OwnedFixtureAdapter {
    fn evidence_class(&self) -> EvidenceClass {
        EvidenceClass::Fixture
    }

    fn capabilities(&self) -> ControllerAdapterCapabilities {
        ControllerAdapterCapabilities {
            literal_input: true,
            correlated_acknowledgements: true,
            interruption: true,
            same_session_resume_evidence: None,
        }
    }

    async fn inventory(
        &self,
        resource_id: Uuid,
    ) -> Result<ControllerInventory, ControllerAdapterError> {
        self.resources
            .lock()
            .map_err(|_| ControllerAdapterError::Unavailable)?
            .get(&resource_id)
            .map(|value| value.inventory.clone())
            .ok_or(ControllerAdapterError::Unavailable)
    }

    async fn submit_literal(
        &self,
        expected: &ControllerResource,
        submission_id: Uuid,
        bytes: &[u8],
    ) -> Result<AdapterDelivery, ControllerAdapterError> {
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| ControllerAdapterError::Unavailable)?;
        let resource = resources
            .get_mut(&expected.resource_id)
            .ok_or(ControllerAdapterError::Unavailable)?;
        if resource.inventory.resource != *expected
            || resource.inventory.state != AdapterResourceState::Ready
        {
            return Err(ControllerAdapterError::OwnershipChanged);
        }
        if resource.input.len() == QUEUE_CAPACITY {
            return Err(ControllerAdapterError::Unavailable);
        }
        resource.input.push_back((submission_id, bytes.to_vec()));
        resource.inventory.state = AdapterResourceState::Running;
        Ok(match resource.delivery {
            DeliveryMode::Acknowledge => Self::acknowledgement(
                expected,
                ControllerOperationKind::Input,
                submission_id,
                Some(Sha256Digest::of(bytes)),
            ),
            DeliveryMode::Unknown => AdapterDelivery::Unknown,
            DeliveryMode::WrongCorrelation => Self::acknowledgement(
                expected,
                ControllerOperationKind::Input,
                Uuid::new_v4(),
                Some(Sha256Digest::of(bytes)),
            ),
        })
    }

    async fn interrupt(
        &self,
        expected: &ControllerResource,
        submission_id: Uuid,
    ) -> Result<AdapterDelivery, ControllerAdapterError> {
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| ControllerAdapterError::Unavailable)?;
        let resource = resources
            .get_mut(&expected.resource_id)
            .ok_or(ControllerAdapterError::Unavailable)?;
        if resource.inventory.resource != *expected
            || !matches!(
                resource.inventory.state,
                AdapterResourceState::Ready | AdapterResourceState::Running
            )
        {
            return Err(ControllerAdapterError::OwnershipChanged);
        }
        resource.interruptions += 1;
        Ok(Self::acknowledgement(
            expected,
            ControllerOperationKind::Interruption,
            submission_id,
            None,
        ))
    }

    async fn attach(
        &self,
        old: &ControllerResource,
        next: &ControllerResource,
        _transfer: HandoffTransfer,
    ) -> Result<AdapterDelivery, ControllerAdapterError> {
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| ControllerAdapterError::Unavailable)?;
        let predecessor = resources
            .get(&old.resource_id)
            .ok_or(ControllerAdapterError::Unavailable)?;
        if predecessor.inventory.resource != *old
            || predecessor.inventory.state != AdapterResourceState::Terminal
        {
            return Err(ControllerAdapterError::OwnershipChanged);
        }
        let successor = resources
            .get_mut(&next.resource_id)
            .ok_or(ControllerAdapterError::Unavailable)?;
        if successor.inventory.resource != *next
            || successor.inventory.state != AdapterResourceState::Detached
        {
            return Err(ControllerAdapterError::OwnershipChanged);
        }
        successor.attachments += 1;
        successor.inventory.state = AdapterResourceState::Ready;
        Ok(Self::acknowledgement(
            next,
            ControllerOperationKind::Attach,
            next.controller.controller_id,
            None,
        ))
    }
}

struct Rig {
    root: TempDir,
    worker: Option<WriterWorker>,
    writer: WriterHandle,
    workflow: WorkflowRecord,
    contracts: Vec<ContractArtifact>,
    adapter: Arc<OwnedFixtureAdapter>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown().expect("writer shutdown");
        }
    }
}

impl Rig {
    fn new() -> Self {
        use vibemux_workflow::{
            checkpoint::ContextSettingsIdentity,
            contract::ContractIdentityInputs,
            policy::{OperatorPolicy, intersect},
            renderer::{PromptCapabilities, WorkerRenderInputs, render_worker_contract},
            slots::{InterfaceMode, SlotBudget, SlotFacts, SlotHealth},
            task_spec::{CheckKind, TaskSpec, ToolName, fixtures::track_a_spec},
            templates::ResolvedTemplate,
        };

        let root = tempfile::tempdir().expect("owned fixture root");
        let worker = WriterWorker::start(&root.path().join("controller.sqlite3")).expect("writer");
        let writer = worker.handle().expect("handle");
        let policy: OperatorPolicy = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "policy_version": "policy_v1",
            "writable_roots": ["src/**", "tests/worker_a/**", "tests/worker_b/**"],
            "protected_paths": ["package.json"],
            "allowed_tools": ["read_files", "search_files", "edit_files", "write_files", "run_dev_tests"],
            "verifier_suites": [{"suite_id":"fixture_suite", "description":"Owned controller fixture checks"}],
            "budget_caps": {"max_turns":4, "max_repairs":1, "max_elapsed_seconds":1800, "max_model_requests":16},
            "max_parallel_slots": 2,
            "max_supervisor_decisions": 16,
            "max_optimizer_candidates": 3,
            "content_store_opt_in": false,
            "aag_only": true
        }))
        .expect("operator policy shape");
        policy.validate().expect("operator policy");
        let policy_digest = policy.digest().expect("policy digest");
        let specs: BTreeMap<SpecIdentifier, TaskSpec> = ["task_a", "task_b"]
            .into_iter()
            .map(|task_key| {
                let mut spec = track_a_spec();
                spec.workflow_key = id("native_fixture");
                spec.task_key = id(task_key);
                spec.base_commit = "a".repeat(40);
                spec.communication_policy.may_message.clear();
                spec.communication_policy.auto_authorized_kinds.clear();
                spec.permitted_tools = vec![
                    ToolName::ReadFiles,
                    ToolName::SearchFiles,
                    ToolName::EditFiles,
                    ToolName::WriteFiles,
                    ToolName::RunDevTests,
                ];
                spec.resource_budget.max_turns = 4;
                spec.resource_budget.max_repairs = 1;
                spec.resource_budget.max_model_requests = 16;
                for check in &mut spec.acceptance_checks {
                    if check.kind == CheckKind::ExistingVerifierSuite {
                        check.verifier_suite = Some(id("fixture_suite"));
                        check.description = "Owned controller fixture checks pass.".to_string();
                    }
                }
                let spec = intersect(spec.normalized(), &policy)
                    .expect("narrowed fixture specification")
                    .spec;
                spec.validate_structure()
                    .expect("full fixture specification");
                (spec.task_key.clone(), spec)
            })
            .collect();
        let contracts: Vec<_> = specs
            .values()
            .map(|spec| {
                let template = ResolvedTemplate::resolve(&spec.template_version, &BTreeMap::new())
                    .expect("fixture template");
                let spec_digest = spec.digest().expect("actual TaskSpec digest");
                let rendered = render_worker_contract(WorkerRenderInputs {
                    spec,
                    task_spec_digest: spec_digest,
                    policy_digest,
                    template: &template,
                    capabilities: PromptCapabilities::UNCERTIFIED,
                    context_blocks: &[],
                    turn: None,
                })
                .expect("fixture contract rendering");
                let identity = ContractIdentityInputs {
                    task_spec_digest: spec_digest,
                    policy_digest,
                    template_digest: template.digest,
                    harness_profile: "fixture_controller_v1",
                    prompt_capabilities: PromptCapabilities::UNCERTIFIED,
                    route_transformation: &RouteTransformation::None,
                    context_bundle_digests: &[],
                    base_commit: &spec.base_commit,
                };
                ContractArtifact::new(spec, &identity, &rendered, None)
            })
            .collect();
        let workflow = WorkflowRecord {
            schema_version: 1,
            workflow_id: Uuid::new_v4(),
            request_key: format!("native_fixture_{}", Uuid::new_v4().simple()),
            workflow_key: id("native_fixture"),
            mode: WorkflowMode::Cooperate,
            phase: WorkflowPhase::Prepared,
            version: 1,
            evidence_class: EvidenceClass::Fixture,
            base_commit: "a".repeat(40),
            policy_digest,
            verifier_digest: Sha256Digest::of(b"fixture_verifier"),
            integration_suites: vec![id("fixture_suite")],
            tasks: contracts
                .iter()
                .map(|contract| WorkflowTask {
                    task_key: id(&contract.task_key),
                    contract_id: contract.contract_id,
                    contract_version: 1,
                    required_suites: vec![id("fixture_suite")],
                    progress: TaskProgress::Pending,
                    turns_used: 0,
                    repairs_used: 0,
                    max_turns: 4,
                    max_repairs: 1,
                    accepted_candidate: None,
                    failure_code: None,
                })
                .collect(),
            budget: LoopBudget {
                decisions_remaining: 16,
                malformed_repairs_remaining: 2,
                model_requests_remaining: 16,
                deadline_unix_ms: u64::MAX,
            },
            content_store: ContentStoreMode::Disabled,
            blocked_reason: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        writer
            .prepare_workflow(workflow.clone(), contracts.clone(), now())
            .expect("prepare workflow");
        for contract in &contracts {
            writer
                .bind_workflow_contract_authority(
                    contract.contract_id,
                    specs[&id(&contract.task_key)].clone(),
                    now(),
                )
                .expect("bind actual contract authority");
        }
        // The recovery plan references declared fixture slots. No capability
        // evidence or real native/vendor certification is invented for them.
        for slot_id in ["slot_a", "slot_b", "slot_r"] {
            writer
                .upsert_workflow_slot(
                    SlotFacts {
                        slot_id: id(slot_id),
                        harness: vibemux_harness::AgentKind::Claude,
                        interface_mode: InterfaceMode::Structured,
                        capabilities: BTreeMap::new(),
                        health: SlotHealth::Idle,
                        route_role: None,
                        cooldown_until_ms: None,
                        leased_generation: None,
                        budget: SlotBudget::Unknown,
                    },
                    now(),
                )
                .expect("declare plan slot");
        }
        let plan = serde_json::json!({
            "schema_version": 1,
            "request_sha256": specs[&id("task_a")].original_request_ref,
            "policy": policy,
            "specs": specs,
            "assignments": {"task_a":["slot_a"], "task_b":["slot_b"]},
            "reviewer_slot": "slot_r",
            "template_overrides": {},
            "negative_control": false
        });
        writer
            .bind_recovery_plan(
                workflow.workflow_id,
                plan,
                1,
                ContextSettingsIdentity {
                    configured_context_tokens: None,
                    compaction_settings_digest: None,
                },
                now(),
            )
            .expect("bind immutable full plan");
        let workflow = writer
            .transition_workflow(workflow.workflow_id, 1, WorkflowPhase::Running, None, now())
            .expect("running")
            .record;
        Self {
            root,
            worker: Some(worker),
            writer,
            workflow,
            contracts,
            adapter: Arc::new(OwnedFixtureAdapter::default()),
        }
    }

    fn runtime(&self) -> NativeControllerRuntime {
        NativeControllerRuntime::new(
            self.writer.clone(),
            self.adapter.clone(),
            NativeRuntimeLimits::default(),
        )
        .expect("runtime")
    }

    fn reserve(
        &self,
        task_index: usize,
        text: &str,
        mode: DeliveryMode,
        owner: ControllerOwner,
    ) -> (
        Uuid,
        NativeController,
        ControllerResource,
        ControllerOutboxRecord,
    ) {
        let contract = &self.contracts[task_index];
        let controller_id = Uuid::new_v4();
        let controller = NativeController::new(
            ControllerIdentity {
                workflow_id: self.workflow.workflow_id,
                session_id: Uuid::new_v4(),
                controller_id,
                contract_id: contract.contract_id,
                contract_version: 1,
            },
            owner,
        )
        .expect("controller");
        let admitted = self
            .writer
            .admit_controller(controller.clone(), now())
            .expect("admit controller");
        let controller_key = admitted.controller_key;
        let actor = match owner {
            ControllerOwner::Human { operator_id } => ControllerActor::Human { operator_id },
            _ => ControllerActor::Automation,
        };
        let controller = self
            .writer
            .change_workflow_controller(
                controller_key,
                controller.version,
                controller.fence(),
                actor,
                ControllerCommand::Resume,
                now(),
            )
            .expect("resume")
            .controller;
        let input = LiteralInput::new(text.to_string()).expect("literal input");
        let reservation = InputReservation {
            submission_id: Uuid::new_v4(),
            sequence: 1,
            contract_id: contract.contract_id,
            prompt_digest: input.digest(),
            prompt_bytes: input.byte_count(),
        };
        let reserved = self
            .writer
            .change_workflow_controller(
                controller_key,
                controller.version,
                controller.fence(),
                actor,
                ControllerCommand::ReserveSubmission(reservation),
                now(),
            )
            .expect("reserve");
        let controller = reserved.controller;
        let effect = reserved.effect.expect("pending outbox");
        let resource = ControllerResource {
            project_id: self
                .writer
                .project_id()
                .expect("project")
                .expect("project admitted"),
            workflow_id: self.workflow.workflow_id,
            session_id: controller.identity.session_id,
            controller: controller.fence(),
            backend_id: id("owned_fixture"),
            resource_id: Uuid::new_v4(),
            ownership_token: Uuid::new_v4(),
            workspace_identity: Sha256Digest::of(contract.task_key.as_bytes()),
        };
        self.adapter.register(
            ControllerInventory {
                resource: resource.clone(),
                owner,
                state: AdapterResourceState::Ready,
                pending_approval: None,
            },
            mode,
        );
        (controller_key, controller, resource, effect)
    }

    fn restart_writer(&mut self) {
        self.worker
            .take()
            .expect("writer")
            .shutdown()
            .expect("shutdown");
        let worker = WriterWorker::start(&self.root.path().join("controller.sqlite3"))
            .expect("restart writer");
        self.writer = worker.handle().expect("restarted handle");
        self.worker = Some(worker);
    }
}

fn input_request(
    controller_key: Uuid,
    controller: &NativeController,
    resource: ControllerResource,
    effect: &ControllerOutboxRecord,
) -> NativeInputRequest {
    NativeInputRequest {
        controller_key,
        effect_id: effect.effect_id,
        expected_version: controller.version,
        actor: ControllerActor::Automation,
        resource,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_claims_deliver_exact_literal_bytes_once() {
    let rig = Rig::new();
    let runtime = rig.runtime();
    let (key, controller, resource, effect) = rig.reserve(
        0,
        INPUT_A,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let request = input_request(key, &controller, resource.clone(), &effect);
    let (_cancel_sender, cancel) = watch::channel(false);
    let (first, second) = tokio::join!(
        runtime.submit(
            request.clone(),
            LiteralInput::new(INPUT_A.to_string()).expect("input"),
            cancel.clone()
        ),
        runtime.submit(
            request,
            LiteralInput::new(INPUT_A.to_string()).expect("input"),
            cancel
        ),
    );
    assert!(
        first
            .as_ref()
            .is_ok_and(|result| result.status == NativeInputStatus::Acknowledged)
            || second
                .as_ref()
                .is_ok_and(|result| result.status == NativeInputStatus::Acknowledged)
    );
    assert_eq!(
        rig.adapter.input(resource.resource_id),
        vec![(effect.submission_id, INPUT_A.as_bytes().to_vec())]
    );
    let persisted = rig
        .writer
        .workflow_controller(key)
        .expect("read")
        .expect("controller");
    assert_eq!(
        persisted.last_submission.expect("submission").state,
        DeliveryState::Acknowledged
    );
    assert_eq!(
        rig.writer
            .workflow_record(rig.workflow.workflow_id)
            .expect("workflow")
            .expect("record")
            .phase,
        WorkflowPhase::Running
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_owned_controllers_do_not_cross_deliver() {
    let rig = Rig::new();
    let runtime = rig.runtime();
    let (key_a, controller_a, resource_a, effect_a) = rig.reserve(
        0,
        INPUT_A,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let (key_b, controller_b, resource_b, effect_b) = rig.reserve(
        1,
        INPUT_B,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let (_cancel_sender, cancel) = watch::channel(false);
    let (first, second) = tokio::join!(
        runtime.submit(
            input_request(key_a, &controller_a, resource_a.clone(), &effect_a),
            LiteralInput::new(INPUT_A.to_string()).expect("input"),
            cancel.clone()
        ),
        runtime.submit(
            input_request(key_b, &controller_b, resource_b.clone(), &effect_b),
            LiteralInput::new(INPUT_B.to_string()).expect("input"),
            cancel
        ),
    );
    assert_eq!(
        first.expect("first").status,
        NativeInputStatus::Acknowledged
    );
    assert_eq!(
        second.expect("second").status,
        NativeInputStatus::Acknowledged
    );
    assert_eq!(
        rig.adapter.input(resource_a.resource_id)[0].1,
        INPUT_A.as_bytes()
    );
    assert_eq!(
        rig.adapter.input(resource_b.resource_id)[0].1,
        INPUT_B.as_bytes()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatched_payload_inventory_and_human_owner_refuse_before_effect() {
    let rig = Rig::new();
    let runtime = rig.runtime();
    let (key, controller, resource, effect) = rig.reserve(
        0,
        INPUT_A,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let request = input_request(key, &controller, resource.clone(), &effect);
    let (_cancel_sender, cancel) = watch::channel(false);
    assert!(matches!(
        runtime
            .submit(
                request.clone(),
                LiteralInput::new(INPUT_B.to_string()).expect("input"),
                cancel.clone()
            )
            .await,
        Err(NativeRuntimeError::InvalidInput)
    ));
    let mut mismatched = request;
    mismatched.resource.ownership_token = Uuid::new_v4();
    assert!(matches!(
        runtime
            .submit(
                mismatched,
                LiteralInput::new(INPUT_A.to_string()).expect("input"),
                cancel.clone()
            )
            .await,
        Err(NativeRuntimeError::ResourceMismatch)
    ));
    assert!(rig.adapter.input(resource.resource_id).is_empty());
    assert_eq!(
        rig.writer
            .controller_outbox(effect.effect_id)
            .expect("outbox")
            .expect("record")
            .state,
        ControllerOutboxState::Pending
    );
    let (human_key, human_controller, human_resource, human_effect) = rig.reserve(
        1,
        INPUT_B,
        DeliveryMode::Acknowledge,
        ControllerOwner::Human {
            operator_id: Uuid::new_v4(),
        },
    );
    assert!(matches!(
        runtime
            .submit(
                input_request(
                    human_key,
                    &human_controller,
                    human_resource.clone(),
                    &human_effect
                ),
                LiteralInput::new(INPUT_B.to_string()).expect("input"),
                cancel
            )
            .await,
        Err(NativeRuntimeError::StateRefused)
    ));
    assert!(rig.adapter.input(human_resource.resource_id).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_or_mismatched_acknowledgement_is_durable_and_never_resent() {
    for mode in [DeliveryMode::Unknown, DeliveryMode::WrongCorrelation] {
        let mut rig = Rig::new();
        let (key, controller, resource, effect) =
            rig.reserve(0, INPUT_A, mode, ControllerOwner::Native);
        let request = input_request(key, &controller, resource.clone(), &effect);
        let (_cancel_sender, cancel) = watch::channel(false);
        let first = rig
            .runtime()
            .submit(
                request.clone(),
                LiteralInput::new(INPUT_A.to_string()).expect("input"),
                cancel.clone(),
            )
            .await
            .expect("unknown result");
        assert_eq!(first.status, NativeInputStatus::DeliveryUnknown);
        assert_eq!(first.controller.phase, ControllerPhase::Paused);
        assert_eq!(first.effect.state, ControllerOutboxState::Unknown);
        rig.restart_writer();
        let restored = rig
            .writer
            .workflow_controller(key)
            .expect("restored")
            .expect("controller");
        assert_eq!(
            restored.last_submission.expect("submission").state,
            DeliveryState::DeliveryUnknown
        );
        let result = rig
            .runtime()
            .submit(
                request,
                LiteralInput::new(INPUT_A.to_string()).expect("input"),
                cancel,
            )
            .await
            .expect("replay suppressed");
        assert_eq!(result.status, NativeInputStatus::ReplaySuppressed);
        assert_eq!(rig.adapter.input(resource.resource_id).len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interruption_ack_is_not_completion_and_handoff_waits_for_terminal_owner() {
    let rig = Rig::new();
    let runtime = rig.runtime();
    let (key, controller, resource, effect) = rig.reserve(
        0,
        INPUT_A,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let (_cancel_sender, cancel) = watch::channel(false);
    let submitted = runtime
        .submit(
            input_request(key, &controller, resource.clone(), &effect),
            LiteralInput::new(INPUT_A.to_string()).expect("input"),
            cancel.clone(),
        )
        .await
        .expect("submit");
    let interrupted = runtime
        .interrupt(
            key,
            submitted.controller.version,
            resource.clone(),
            ControllerActor::Automation,
            cancel.clone(),
        )
        .await
        .expect("interrupt");
    assert!(interrupted.acknowledged);
    assert_eq!(
        interrupted.controller.interruption,
        Some(InterruptionState::Acknowledged)
    );
    assert!(interrupted.controller.has_unsettled_input());
    let target = HandoffTarget {
        controller_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        owner: ControllerOwner::Native,
        transfer: HandoffTransfer::ContextTransfer {
            bundle_id: Uuid::new_v4(),
        },
    };
    let pending = runtime
        .begin_handoff(
            key,
            interrupted.controller.version,
            resource.clone(),
            target,
            ControllerActor::Automation,
        )
        .await
        .expect("quiesce");
    let next = ControllerResource {
        session_id: target.session_id,
        controller: vibemux_workflow::native_controller::ControllerFence {
            controller_id: target.controller_id,
            generation: resource.controller.generation + 1,
        },
        resource_id: Uuid::new_v4(),
        ownership_token: Uuid::new_v4(),
        ..resource.clone()
    };
    rig.adapter.register(
        ControllerInventory {
            resource: next.clone(),
            owner: target.owner,
            state: AdapterResourceState::Detached,
            pending_approval: None,
        },
        DeliveryMode::Acknowledge,
    );
    assert!(
        runtime
            .finish_handoff(
                key,
                pending.version,
                resource.clone(),
                next.clone(),
                cancel.clone()
            )
            .await
            .is_err()
    );
    let settled = rig
        .writer
        .change_workflow_controller(
            key,
            pending.version,
            resource.controller,
            ControllerActor::Adapter,
            ControllerCommand::SettleSubmission {
                submission_id: effect.submission_id,
                outcome: SubmissionOutcome::Cancelled,
            },
            now(),
        )
        .expect("correlated terminal fixture")
        .controller;
    rig.adapter
        .set_state(resource.resource_id, AdapterResourceState::Terminal);
    let handoff = runtime
        .finish_handoff(
            key,
            settled.version,
            resource.clone(),
            next.clone(),
            cancel.clone(),
        )
        .await
        .expect("handoff");
    assert!(handoff.acknowledged);
    assert_eq!(handoff.transfer, target.transfer);
    assert_eq!(handoff.controller.identity.session_id, target.session_id);
    assert_eq!(handoff.controller.phase, ControllerPhase::Paused);
    assert_eq!(
        rig.writer
            .workflow_controller(key)
            .expect("stable key")
            .expect("controller")
            .identity
            .controller_id,
        target.controller_id
    );
    assert!(
        runtime
            .finish_handoff(key, settled.version, resource, next.clone(), cancel)
            .await
            .is_err()
    );
    assert_eq!(
        rig.adapter.resources.lock().expect("resources")[&next.resource_id].attachments,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_same_session_resume_and_wrong_approval_scope_are_refused() {
    let rig = Rig::new();
    let runtime = rig.runtime();
    let (key, controller, resource, effect) = rig.reserve(
        0,
        INPUT_A,
        DeliveryMode::Acknowledge,
        ControllerOwner::Native,
    );
    let target = HandoffTarget {
        controller_id: Uuid::new_v4(),
        session_id: controller.identity.session_id,
        owner: ControllerOwner::Native,
        transfer: HandoffTransfer::SameSession {
            capability_evidence: Sha256Digest::of(b"unavailable certification"),
        },
    };
    assert!(matches!(
        runtime
            .begin_handoff(
                key,
                controller.version,
                resource.clone(),
                target,
                ControllerActor::Automation
            )
            .await,
        Err(NativeRuntimeError::Unsupported)
    ));
    let (_cancel_sender, cancel) = watch::channel(false);
    let submitted = runtime
        .submit(
            input_request(key, &controller, resource.clone(), &effect),
            LiteralInput::new(INPUT_A.to_string()).expect("input"),
            cancel,
        )
        .await
        .expect("submitted");
    let challenge = ApprovalChallenge {
        challenge_id: Uuid::new_v4(),
        requester_id: Uuid::new_v4(),
        session_id: controller.identity.session_id,
        controller: resource.controller,
        submission_id: effect.submission_id,
        operation_digest: Sha256Digest::of(b"operation"),
        scope_digest: Sha256Digest::of(b"exact scope"),
    };
    let pending = rig
        .writer
        .change_workflow_controller(
            key,
            submitted.controller.version,
            resource.controller,
            ControllerActor::Adapter,
            ControllerCommand::RaiseApproval(challenge),
            now(),
        )
        .expect("challenge")
        .controller;
    let mut wrong = challenge;
    wrong.scope_digest = Sha256Digest::of(b"broader scope");
    assert!(
        runtime
            .record_approval_decision(
                key,
                pending.version,
                resource.clone(),
                Uuid::new_v4(),
                wrong,
                ApprovalDecision::Allow
            )
            .await
            .is_err()
    );
    let recorded = runtime
        .record_approval_decision(
            key,
            pending.version,
            resource.clone(),
            Uuid::new_v4(),
            challenge,
            ApprovalDecision::Allow,
        )
        .await
        .expect("exact scoped decision");
    assert!(recorded.delivery_unknown);
    assert_eq!(
        recorded
            .controller
            .last_approval_resolution
            .expect("decision")
            .challenge,
        challenge
    );
    assert_eq!(
        rig.adapter.input(resource.resource_id).len(),
        1,
        "approval decision sends no automatic response or new input"
    );
}
