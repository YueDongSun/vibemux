//! Schema-7 controller and recovery persistence checks on a private SQLite DB.
//! Every test opens the normal production migration; no table DDL is injected.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{
    ContentEntry, ContentKind, ContentState, ControllerOutboxOutcome, ControllerOutboxState,
    SqliteStore, StoreError,
};
use vibemux_workflow::{
    PathPattern, Sha256Digest, SpecIdentifier,
    checkpoint::{ContextSettingsIdentity, RecoveryCheckpoint},
    contract::{ContractArtifact, RouteTransformation},
    gates::{AcceptanceLevel, VerifierReceipt, WorkflowPhase},
    native_controller::{
        ControllerActor, ControllerCommand, ControllerIdentity, ControllerObservation,
        ControllerOwner, DeliveryState, HandoffTarget, HandoffTransfer, InputReservation,
        NativeController, SubmissionOutcome,
    },
    policy::{OPERATOR_POLICY_SCHEMA_VERSION, OperatorPolicy, VerifierSuiteDecl},
    receipts::WorkflowReceipt,
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    supervisor::LoopBudget,
    task_spec::{
        AcceptanceCheck, CheckKind, CommunicationPolicy, CompletionDefinition, ContextPolicy,
        NormativeForce, OutputContract, Requirement, ResourceBudget, SourceLanguage, SourceSpan,
        TASK_SPEC_SCHEMA_VERSION, TaskSpec, ToolName, WorkerRole, WorkflowMode,
    },
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};

const BASE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + seconds).expect("timestamp")
}

fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("test identifier")
}

fn path(value: &str) -> PathPattern {
    PathPattern::new(value).expect("test path")
}

fn contract(key: &str) -> ContractArtifact {
    ContractArtifact {
        schema_version: 1,
        contract_id: Sha256Digest::of(format!("contract:{key}").as_bytes()),
        workflow_key: "taskboard_lite".to_string(),
        task_key: key.to_string(),
        contract_version: 1,
        task_spec_digest: Sha256Digest::of(format!("spec:{key}").as_bytes()),
        policy_digest: Sha256Digest::of(b"policy"),
        template_digest: Sha256Digest::of(b"template"),
        template_version: "worker_v1".to_string(),
        harness_profile: "fixture_writable_v1".to_string(),
        route_transformation: RouteTransformation::None,
        context_bundle_digests: vec![],
        base_commit: BASE_COMMIT.to_string(),
        rendered_prompt_digest: Sha256Digest::of(format!("rendered:{key}").as_bytes()),
        rendered_prompt_bytes: 32,
        previous_contract_id: None,
        semantic_diff: None,
    }
}

fn workflow() -> (WorkflowRecord, Vec<ContractArtifact>) {
    let contracts = vec![contract("track_a"), contract("track_b")];
    let tasks = contracts
        .iter()
        .map(|contract| WorkflowTask {
            task_key: id(&contract.task_key),
            contract_id: contract.contract_id,
            contract_version: 1,
            required_suites: vec![id("api")],
            progress: TaskProgress::Pending,
            turns_used: 0,
            repairs_used: 0,
            max_turns: 4,
            max_repairs: 1,
            accepted_candidate: None,
            failure_code: None,
        })
        .collect();
    let record = WorkflowRecord {
        schema_version: 1,
        workflow_id: Uuid::new_v4(),
        request_key: format!("runtime_{}", Uuid::new_v4().simple()),
        workflow_key: id("taskboard_lite"),
        mode: WorkflowMode::Cooperate,
        phase: WorkflowPhase::Prepared,
        version: 1,
        evidence_class: EvidenceClass::Fixture,
        base_commit: BASE_COMMIT.to_string(),
        policy_digest: Sha256Digest::of(b"policy"),
        verifier_digest: Sha256Digest::of(b"verifier"),
        integration_suites: vec![id("api")],
        tasks,
        budget: LoopBudget {
            decisions_remaining: 16,
            malformed_repairs_remaining: 2,
            model_requests_remaining: 40,
            deadline_unix_ms: u64::MAX,
        },
        content_store: ContentStoreMode::Disabled,
        blocked_reason: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    (record, contracts)
}

fn open_store() -> (NamedTempFile, SqliteStore) {
    let file = NamedTempFile::new().expect("private database");
    let mut store = SqliteStore::open(file.path()).expect("migrate through schema 7");
    let (record, contracts) = workflow();
    store
        .prepare_workflow(&record, &contracts, at(0))
        .expect("workflow");
    (file, store)
}

fn simple_spec(key: &str) -> TaskSpec {
    let check = id("check_api");
    TaskSpec {
        schema_version: TASK_SPEC_SCHEMA_VERSION,
        workflow_key: id("taskboard_lite"),
        task_key: id(key),
        contract_version: 1,
        original_request_ref: Sha256Digest::of(b"Build task"),
        source_language: SourceLanguage::En,
        requirements: vec![Requirement {
            requirement_id: id("requirement_api"),
            statement: "Implement the assigned TaskBoard file.".to_string(),
            force: NormativeForce::Must,
            source_refs: vec![SourceSpan {
                start: 0,
                end: 10,
                quoted: "Build task".to_string(),
            }],
            contract_excerpts: vec![],
            literals: vec![],
            acceptance_check_ids: vec![check.clone()],
        }],
        objective: "Implement the assigned TaskBoard file.".to_string(),
        non_goals: vec![],
        assumptions: vec![],
        unresolved_questions: vec![],
        source_exclusions: vec![],
        base_commit: BASE_COMMIT.to_string(),
        input_artifacts: vec![],
        shared_contract_refs: vec![],
        owned_paths: vec![path(&format!("src/{key}.mjs"))],
        read_scopes: vec![],
        forbidden_paths: vec![path("package.json")],
        dependencies: if key == "track_b" {
            vec![id("track_a")]
        } else {
            vec![]
        },
        required_capabilities: vec![SlotCapability::StructuredTurn],
        permitted_tools: vec![ToolName::ReadFiles, ToolName::WriteFiles],
        acceptance_checks: vec![AcceptanceCheck {
            check_id: check,
            kind: CheckKind::ExistingVerifierSuite,
            description: "Trusted API check passes.".to_string(),
            verifier_suite: Some(id("api")),
        }],
        output_contract: OutputContract {
            checkpoint_report_required: true,
            changes_within_owned_paths: true,
        },
        completion_definition: CompletionDefinition {
            required_levels: vec![
                AcceptanceLevel::CandidateArtifactCollected,
                AcceptanceLevel::IndependentReviewPassed,
                AcceptanceLevel::LocalVerifierPassed,
            ],
        },
        communication_policy: CommunicationPolicy {
            may_message: vec![],
            max_messages_per_turn: 0,
            auto_authorized_kinds: vec![],
        },
        context_policy: ContextPolicy {
            max_bundle_bytes: 1,
            retrieval_order: vec![],
            allow_public_excerpts: false,
        },
        resource_budget: ResourceBudget {
            max_turns: 4,
            max_repairs: 1,
            max_elapsed_seconds: 600,
            max_model_requests: 2,
        },
        role: WorkerRole::Generic,
        workflow_mode: WorkflowMode::Cooperate,
        template_version: id("worker_v1"),
        policy_version: id("policy_v1"),
    }
    .normalized()
}

fn simple_policy() -> OperatorPolicy {
    OperatorPolicy {
        schema_version: OPERATOR_POLICY_SCHEMA_VERSION,
        policy_version: id("policy_v1"),
        writable_roots: vec![path("src/**")],
        protected_paths: vec![path("package.json")],
        allowed_tools: vec![ToolName::ReadFiles, ToolName::WriteFiles],
        verifier_suites: vec![VerifierSuiteDecl {
            suite_id: id("api"),
            description: "Trusted API check".to_string(),
        }],
        budget_caps: ResourceBudget {
            max_turns: 4,
            max_repairs: 1,
            max_elapsed_seconds: 600,
            max_model_requests: 2,
        },
        max_parallel_slots: 2,
        max_supervisor_decisions: 8,
        max_optimizer_candidates: 1,
        content_store_opt_in: false,
        aag_only: true,
    }
}

fn slot(name: &str) -> SlotFacts {
    let evidence = CapabilityEvidence {
        level: EvidenceLevel::WriteModeCertified,
        class: EvidenceClass::Fixture,
        evidence_ref: "fixture".to_string(),
        checked_at_ms: 1,
        expires_at_ms: u64::MAX,
    };
    let mut capabilities = BTreeMap::new();
    capabilities.insert(SlotCapability::StructuredTurn, evidence);
    SlotFacts {
        slot_id: id(name),
        harness: vibemux_harness::AgentKind::Claude,
        interface_mode: InterfaceMode::Structured,
        capabilities,
        health: SlotHealth::Idle,
        route_role: None,
        cooldown_until_ms: None,
        leased_generation: None,
        budget: SlotBudget::Remaining(10),
    }
}

fn open_bound_store() -> (NamedTempFile, SqliteStore, WorkflowRecord, Value) {
    open_bound_store_with_content(false)
}

fn open_bound_store_with_content(
    content_enabled: bool,
) -> (NamedTempFile, SqliteStore, WorkflowRecord, Value) {
    let file = NamedTempFile::new().expect("private database");
    drop(SqliteStore::open(file.path()).expect("migrate through schema 7"));
    let mut policy = simple_policy();
    policy.content_store_opt_in = content_enabled;
    policy.validate().expect("valid policy");
    let mut specs = BTreeMap::new();
    for key in ["track_a", "track_b"] {
        let spec = simple_spec(key);
        spec.validate_structure().expect("valid spec");
        specs.insert(id(key), spec);
    }
    let (mut record, mut contracts) = workflow();
    record.policy_digest = policy.digest().expect("policy digest");
    record.content_store = if content_enabled {
        ContentStoreMode::Enabled { retention_days: 7 }
    } else {
        ContentStoreMode::Disabled
    };
    for contract in &mut contracts {
        contract.policy_digest = record.policy_digest;
        contract.task_spec_digest = specs[&id(&contract.task_key)]
            .digest()
            .expect("spec digest");
    }
    let mut store = SqliteStore::open(file.path()).expect("reopen private store");
    store
        .prepare_workflow(&record, &contracts, at(0))
        .expect("prepare");
    for name in ["slot_a", "slot_b", "slot_r"] {
        store
            .upsert_workflow_slot(&slot(name), at(1))
            .expect("slot");
    }
    let mut assignments = BTreeMap::new();
    assignments.insert(id("track_a"), vec![id("slot_a")]);
    assignments.insert(id("track_b"), vec![id("slot_b")]);
    let plan = json!({
        "schema_version": 1,
        "request_sha256": Sha256Digest::of(b"Build task"),
        "policy": policy,
        "specs": specs,
        "assignments": assignments,
        "reviewer_slot": id("slot_r"),
        "template_overrides": {},
        "negative_control": false,
    });
    (file, store, record, plan)
}

#[test]
fn controller_state_event_and_unknown_outbox_commit_once() {
    let (file, mut store) = open_store();
    let workflow = store
        .workflows(1)
        .expect("workflow list")
        .pop()
        .expect("workflow");
    let identity = ControllerIdentity {
        workflow_id: workflow.workflow_id,
        session_id: Uuid::new_v4(),
        controller_id: Uuid::new_v4(),
        contract_id: contract("track_a").contract_id,
        contract_version: 1,
    };
    let controller = NativeController::new(identity, ControllerOwner::Structured).expect("initial");
    let admitted = store
        .admit_controller(&controller, at(1))
        .expect("admit controller");
    assert!(!admitted.duplicate);
    assert!(admitted.event.is_some());
    assert_eq!(admitted.controller_key, identity.controller_id);
    assert!(admitted.effect.is_none());
    let repeated = store
        .admit_controller(&controller, at(2))
        .expect("duplicate admission");
    assert!(repeated.duplicate);
    assert!(repeated.event.is_none());

    store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::Running,
            None,
            at(2),
        )
        .expect("start workflow before controller input");

    let resumed = store
        .change_workflow_controller(
            identity.controller_id,
            controller.version,
            controller.fence(),
            ControllerActor::Automation,
            ControllerCommand::Resume,
            at(3),
        )
        .expect("resume");
    assert!(resumed.effect.is_none());
    let reservation = InputReservation {
        submission_id: Uuid::new_v4(),
        sequence: 1,
        contract_id: identity.contract_id,
        prompt_digest: Sha256Digest::of(b"prompt text not persisted"),
        prompt_bytes: 25,
    };
    let reserved = store
        .change_workflow_controller(
            identity.controller_id,
            resumed.controller.version,
            resumed.controller.fence(),
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(reservation),
            at(4),
        )
        .expect("reserve");
    let duplicate = store
        .change_workflow_controller(
            identity.controller_id,
            reserved.controller.version,
            reserved.controller.fence(),
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(reservation),
            at(5),
        )
        .expect("same reservation");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.controller.version, reserved.controller.version);
    assert!(duplicate.effect.is_none());
    let pending = reserved.effect.expect("one pending submission effect");
    assert_eq!(pending.state, ControllerOutboxState::Pending);
    assert_eq!(pending.controller_key, identity.controller_id);
    assert_eq!(pending.controller_id, identity.controller_id);
    assert_eq!(
        store
            .controller_outbox(pending.effect_id)
            .expect("read effect"),
        Some(pending.clone())
    );
    assert!(
        store
            .change_workflow_controller(
                identity.controller_id,
                reserved.controller.version,
                reserved.controller.fence(),
                ControllerActor::Automation,
                ControllerCommand::BeginSubmission {
                    submission_id: reservation.submission_id,
                },
                at(6),
            )
            .is_err()
    );
    let submitted = store
        .claim_controller_outbox(
            pending.effect_id,
            reserved.controller.version,
            reserved.controller.fence(),
            at(6),
        )
        .expect("atomic claim before effect");
    assert_eq!(
        submitted
            .controller
            .last_submission
            .expect("submission")
            .state,
        DeliveryState::Submitted
    );
    assert_eq!(submitted.effect.state, ControllerOutboxState::Submitted);
    assert!(
        store
            .claim_controller_outbox(
                pending.effect_id,
                submitted.controller.version,
                submitted.controller.fence(),
                at(7)
            )
            .is_err()
    );
    let unknown = store
        .finish_controller_outbox(
            pending.effect_id,
            submitted.controller.fence(),
            ControllerOutboxOutcome::DeliveryUnknown,
            at(8),
        )
        .expect("conservative unknown delivery");
    assert_eq!(unknown.effect.state, ControllerOutboxState::Unknown);
    assert!(
        store
            .claim_controller_outbox(
                pending.effect_id,
                unknown.controller.version,
                unknown.controller.fence(),
                at(9)
            )
            .is_err()
    );
    let acknowledged = store
        .finish_controller_outbox(
            pending.effect_id,
            unknown.controller.fence(),
            ControllerOutboxOutcome::Acknowledged,
            at(10),
        )
        .expect("late correlated acknowledgement");
    assert_eq!(
        acknowledged.effect.state,
        ControllerOutboxState::Acknowledged
    );
    let ack_duplicate = store
        .finish_controller_outbox(
            pending.effect_id,
            acknowledged.controller.fence(),
            ControllerOutboxOutcome::Acknowledged,
            at(11),
        )
        .expect("idempotent acknowledgement");
    assert!(ack_duplicate.duplicate);
    assert!(ack_duplicate.event.is_none());
    assert_eq!(
        store
            .workflow_controller(identity.controller_id)
            .expect("load controller"),
        Some(acknowledged.controller)
    );
    let connection = Connection::open(file.path()).expect("inspect private fixture DB");
    let (count, state, command_json): (u32, String, String) = connection
        .query_row(
            "SELECT COUNT(*), MAX(state), MAX(command_json) FROM workflow_controller_outbox",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("outbox row");
    assert_eq!(count, 1);
    assert_eq!(state, "acknowledged");
    assert!(!command_json.contains("prompt text"));
}

#[test]
fn bound_fixture_checkpoint_is_writer_derived_and_becomes_stale() {
    let (_file, mut store, workflow, plan) = open_bound_store();
    let unknown_context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    assert!(store.workflow_recovery_state(workflow.workflow_id).is_err());
    let bound = store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, unknown_context, at(2))
        .expect("bind exact full plan");
    assert!(!bound.duplicate);
    assert!(bound.event.is_some());
    let duplicate = store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, unknown_context, at(3))
        .expect("same plan is idempotent");
    assert!(duplicate.duplicate);
    assert!(duplicate.event.is_none());
    assert!(
        store
            .bind_recovery_plan(workflow.workflow_id, &plan, 2, unknown_context, at(4))
            .is_err()
    );
    let state = store
        .workflow_recovery_state(workflow.workflow_id)
        .expect("complete prepared fixture projection");
    assert_eq!(state.workflow_version, workflow.version);
    assert_eq!(state.contracts.len(), 2);
    assert_eq!(state.task_budgets.len(), 2);
    assert!(
        state
            .task_budgets
            .iter()
            .all(|budget| budget.model_requests_used == 0)
    );
    assert_eq!(state.context_settings, unknown_context);
    assert!(state.attempts.is_empty());
    let checkpoint =
        RecoveryCheckpoint::new(Uuid::new_v4(), state.clone()).expect("normalized checkpoint");
    let saved = store
        .save_recovery_checkpoint(&checkpoint, at(5))
        .expect("save current");
    assert!(!saved.duplicate);
    assert!(saved.event.is_some());
    assert_eq!(
        store
            .load_recovery_checkpoint(checkpoint.checkpoint_id)
            .expect("load current"),
        Some(checkpoint.clone())
    );
    let again = store
        .save_recovery_checkpoint(&checkpoint, at(6))
        .expect("same checkpoint");
    assert!(again.duplicate);
    assert!(again.event.is_none());
    let mut invented = state;
    invented
        .artifact_digests
        .push(Sha256Digest::of(b"invented artifact"));
    let invented = RecoveryCheckpoint::new(Uuid::new_v4(), invented)
        .expect("internally valid but uncommitted reference");
    assert!(store.save_recovery_checkpoint(&invented, at(7)).is_err());
    store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::Running,
            None,
            at(8),
        )
        .expect("source advances");
    assert!(
        store
            .load_recovery_checkpoint(checkpoint.checkpoint_id)
            .is_err()
    );
}

#[test]
fn identical_recovery_binding_replays_after_workflow_cancellation() {
    let (_file, mut store, workflow, plan) = open_bound_store();
    let context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(2))
        .expect("initial immutable binding");
    let running = store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::Running,
            None,
            at(3),
        )
        .expect("start workflow before cancellation");
    let stopping = store
        .transition_workflow(
            workflow.workflow_id,
            running.record.version,
            WorkflowPhase::CancelRequested,
            None,
            at(3),
        )
        .expect("request cancellation");
    store
        .transition_workflow(
            workflow.workflow_id,
            stopping.record.version,
            WorkflowPhase::Cancelled,
            None,
            at(4),
        )
        .expect("no active lease prevents cancellation");
    let duplicate = store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(5))
        .expect("terminal workflow can return its identical historical binding");
    assert!(duplicate.duplicate);
    assert!(duplicate.event.is_none());
    assert!(
        store
            .bind_recovery_plan(workflow.workflow_id, &plan, 2, context, at(6))
            .is_err()
    );
}

#[test]
fn native_input_charges_budget_once_and_unknown_delivery_never_refunds() {
    let (_file, mut store, workflow, plan) = open_bound_store();
    let context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(2))
        .expect("bound plan before native input");
    let identity = ControllerIdentity {
        workflow_id: workflow.workflow_id,
        session_id: Uuid::new_v4(),
        controller_id: Uuid::new_v4(),
        contract_id: workflow.tasks[0].contract_id,
        contract_version: workflow.tasks[0].contract_version,
    };
    let controller =
        NativeController::new(identity, ControllerOwner::Native).expect("native controller");
    store.admit_controller(&controller, at(3)).expect("admit");
    store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::Running,
            None,
            at(4),
        )
        .expect("start");
    let active = store
        .change_workflow_controller(
            identity.controller_id,
            controller.version,
            controller.fence(),
            ControllerActor::Automation,
            ControllerCommand::Resume,
            at(5),
        )
        .expect("resume native")
        .controller;
    let reservation = InputReservation {
        submission_id: Uuid::new_v4(),
        sequence: 1,
        contract_id: identity.contract_id,
        prompt_digest: Sha256Digest::of(b"native prompt bytes"),
        prompt_bytes: 19,
    };
    let reserved = store
        .change_workflow_controller(
            identity.controller_id,
            active.version,
            active.fence(),
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(reservation),
            at(6),
        )
        .expect("reserve charges native budget");
    let effect = reserved.effect.expect("pending effect");
    assert!(effect.charged_model_request);
    let charged = store
        .workflow_record(workflow.workflow_id)
        .expect("workflow")
        .expect("record");
    assert_eq!(charged.task(&id("track_a")).expect("task").turns_used, 1);
    assert_eq!(charged.budget.model_requests_remaining, 39);
    let duplicate = store
        .change_workflow_controller(
            identity.controller_id,
            reserved.controller.version,
            reserved.controller.fence(),
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(reservation),
            at(7),
        )
        .expect("same reservation");
    assert!(duplicate.duplicate);
    assert_eq!(
        store
            .workflow_record(workflow.workflow_id)
            .expect("workflow")
            .expect("record"),
        charged
    );
    let claimed = store
        .claim_controller_outbox(
            effect.effect_id,
            reserved.controller.version,
            reserved.controller.fence(),
            at(8),
        )
        .expect("claim once");
    let unknown = store
        .finish_controller_outbox(
            effect.effect_id,
            claimed.controller.fence(),
            ControllerOutboxOutcome::DeliveryUnknown,
            at(9),
        )
        .expect("unknown outcome");
    let settled = store
        .change_workflow_controller(
            identity.controller_id,
            unknown.controller.version,
            unknown.controller.fence(),
            ControllerActor::Adapter,
            ControllerCommand::SettleSubmission {
                submission_id: reservation.submission_id,
                outcome: SubmissionOutcome::NotDelivered,
            },
            at(10),
        )
        .expect("conservative settlement");
    assert_eq!(
        settled
            .controller
            .last_submission
            .expect("submission")
            .state,
        DeliveryState::Settled
    );
    let state = store
        .workflow_recovery_state(workflow.workflow_id)
        .expect("known native fixture budget projection");
    let budget = state
        .task_budgets
        .iter()
        .find(|budget| budget.task_key == id("track_a"))
        .expect("track budget");
    assert_eq!(budget.model_requests_used, 1);
    assert_eq!(state.loop_budget.model_requests_remaining, 39);
    assert_eq!(state.unresolved_effects.len(), 1);
    assert_eq!(state.unresolved_effects[0].effect_id, effect.effect_id);
    let checkpoint = RecoveryCheckpoint::new(Uuid::new_v4(), state).expect("checkpoint");
    store
        .save_recovery_checkpoint(&checkpoint, at(11))
        .expect("save actual budget");
}

#[test]
fn terminal_handoff_keeps_logical_key_and_fences_new_physical_identity() {
    let (file, mut store) = open_store();
    let workflow = store
        .workflows(1)
        .expect("workflow list")
        .pop()
        .expect("workflow");
    let identity = ControllerIdentity {
        workflow_id: workflow.workflow_id,
        session_id: Uuid::new_v4(),
        controller_id: Uuid::new_v4(),
        contract_id: contract("track_a").contract_id,
        contract_version: 1,
    };
    let initial = NativeController::new(identity, ControllerOwner::Native).expect("controller");
    store.admit_controller(&initial, at(1)).expect("admit");
    store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::Running,
            None,
            at(2),
        )
        .expect("start");
    let target = HandoffTarget {
        controller_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        owner: ControllerOwner::Structured,
        transfer: HandoffTransfer::ContextTransfer {
            bundle_id: Uuid::new_v4(),
        },
    };
    let quiescing = store
        .change_workflow_controller(
            identity.controller_id,
            initial.version,
            initial.fence(),
            ControllerActor::Automation,
            ControllerCommand::BeginHandoff(target),
            at(3),
        )
        .expect("begin handoff");
    assert!(
        store
            .change_workflow_controller(
                identity.controller_id,
                quiescing.controller.version,
                quiescing.controller.fence(),
                ControllerActor::Adapter,
                ControllerCommand::Attach {
                    controller_id: target.controller_id,
                    session_id: target.session_id
                },
                at(4)
            )
            .is_err()
    );
    let terminal = store
        .change_workflow_controller(
            identity.controller_id,
            quiescing.controller.version,
            quiescing.controller.fence(),
            ControllerActor::Adapter,
            ControllerCommand::ObserveOldController(ControllerObservation::Terminal),
            at(5),
        )
        .expect("old owner terminal");
    let attached = store
        .change_workflow_controller(
            identity.controller_id,
            terminal.controller.version,
            terminal.controller.fence(),
            ControllerActor::Adapter,
            ControllerCommand::Attach {
                controller_id: target.controller_id,
                session_id: target.session_id,
            },
            at(6),
        )
        .expect("attach new physical identity");
    assert_eq!(attached.controller_key, identity.controller_id);
    assert_eq!(
        attached.controller.identity.controller_id,
        target.controller_id
    );
    assert_eq!(attached.controller.identity.session_id, target.session_id);
    assert_eq!(attached.controller.generation, initial.generation + 1);
    assert_eq!(
        store
            .workflow_controller(identity.controller_id)
            .expect("logical lookup"),
        Some(attached.controller.clone())
    );
    assert!(
        store
            .workflow_controller(target.controller_id)
            .expect("physical is not row key")
            .is_none()
    );
    assert!(store.admit_controller(&initial, at(7)).is_err());
    let connection = Connection::open(file.path()).expect("inspect private fixture DB");
    let (rows, key, session): (u32, String, String) = connection
        .query_row(
            "SELECT COUNT(*),MAX(controller_key),MAX(session_id) FROM workflow_controllers",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("stable row");
    assert_eq!(rows, 1);
    assert_eq!(key, identity.controller_id.to_string());
    assert_eq!(session, target.session_id.to_string());
}

#[test]
fn an_unexplained_content_reference_blocks_checkpoint_projection() {
    let (file, mut store, workflow, plan) = open_bound_store();
    let context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(2))
        .expect("bound plan");
    drop(store);
    let connection = Connection::open(file.path()).expect("private fixture DB");
    let sequence: i64 = connection
        .query_row("SELECT MIN(sequence) FROM events", [], |row| row.get(0))
        .expect("existing event");
    connection.execute(
        "INSERT INTO content_store_entries(content_sha256,workflow_id,kind,byte_count,state,record_json,updated_sequence) VALUES (?,?,?,?,?,?,?)",
        rusqlite::params![
            Sha256Digest::of(b"unexplained artifact").to_hex(),
            workflow.workflow_id.to_string(), "artifact", 1_u64, "present", "{}", sequence,
        ],
    ).expect("inject one unbacked fixture content row");
    drop(connection);
    let store = SqliteStore::open(file.path()).expect("reopen private store");
    assert!(store.workflow_recovery_state(workflow.workflow_id).is_err());
}

#[test]
fn a_verifier_subject_without_an_admitted_artifact_is_not_attested() {
    let (file, mut store, workflow, plan) = open_bound_store();
    let context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(2))
        .expect("bound plan");
    drop(store);
    let verifier = VerifierReceipt {
        schema_version: 1,
        receipt_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: Some(id("track_a")),
        contract_ids: vec![workflow.tasks[0].contract_id],
        subject_digest: Sha256Digest::of(b"derived negative-control subject"),
        derived_subject: None,
        suites: vec![],
        verifier_files_digest: workflow.verifier_digest,
        tool_versions: BTreeMap::new(),
        subject_unchanged: true,
    };
    let receipt = WorkflowReceipt::Verification(verifier.clone());
    let connection = Connection::open(file.path()).expect("private fixture DB");
    let sequence: i64 = connection
        .query_row("SELECT MIN(sequence) FROM events", [], |row| row.get(0))
        .expect("existing event");
    connection.execute(
        "INSERT INTO workflow_receipts(receipt_id,workflow_id,kind,task_key,subject_digest,receipt_digest,record_json,created_sequence) VALUES (?,?,?,?,?,?,?,?)",
        rusqlite::params![
            verifier.receipt_id.to_string(), workflow.workflow_id.to_string(),
            "verification", "track_a", verifier.subject_digest.to_hex(),
            receipt.digest().expect("envelope receipt digest").to_hex(),
            serde_json::to_string(&receipt).expect("receipt JSON"), sequence,
        ],
    ).expect("inject known receipt without derived-subject authority");
    drop(connection);
    let store = SqliteStore::open(file.path()).expect("reopen private store");
    assert!(matches!(
        store.workflow_recovery_state(workflow.workflow_id),
        Err(StoreError::Workflow(
            "store_recovery_derived_subject_unverified"
        ))
    ));
}

#[test]
fn retained_prompt_metadata_is_not_promoted_to_an_artifact_reference() {
    let (_file, mut store, workflow, plan) = open_bound_store_with_content(true);
    let context = ContextSettingsIdentity {
        configured_context_tokens: None,
        compaction_settings_digest: None,
    };
    store
        .bind_recovery_plan(workflow.workflow_id, &plan, 1, context, at(2))
        .expect("bound plan");
    let prompt = ContentEntry {
        content_sha256: Sha256Digest::of(b"private rendered prompt"),
        workflow_id: Some(workflow.workflow_id),
        kind: ContentKind::RenderedPrompt,
        byte_count: 23,
        state: ContentState::Present,
        retain_until_ms: u64::MAX,
        updated_at_ms: 3,
    };
    store
        .record_workflow_content(&prompt, at(3))
        .expect("record metadata only");
    let state = store
        .workflow_recovery_state(workflow.workflow_id)
        .expect("metadata does not become artifact authority");
    assert!(state.artifact_digests.is_empty());
    let checkpoint = RecoveryCheckpoint::new(Uuid::new_v4(), state).expect("checkpoint");
    store
        .save_recovery_checkpoint(&checkpoint, at(4))
        .expect("save");
    assert_eq!(
        store
            .load_recovery_checkpoint(checkpoint.checkpoint_id)
            .expect("load"),
        Some(checkpoint)
    );
}
