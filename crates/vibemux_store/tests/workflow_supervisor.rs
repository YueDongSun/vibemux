//! Real schema-8 transactions: exact observations, durable effects and recovery fences.
use serde_json::json;
use std::collections::BTreeMap;
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{
    SqliteStore, SupervisorActionFinish, SupervisorRoundAdmission, SupervisorStart,
};
use vibemux_workflow::{
    PathPattern, Sha256Digest, SpecIdentifier,
    checkpoint::ContextSettingsIdentity,
    contract::{ContractArtifact, RouteTransformation},
    gates::{AcceptanceLevel, WorkflowPhase},
    gateway::{
        CallOutcome, GatewayProtocol, ModelCallReceipt, ProviderAttribution, ReportedUsage,
        RouteRole,
    },
    model_call::{ModelCallAdmission, ModelCallRequest},
    policy::{OPERATOR_POLICY_SCHEMA_VERSION, OperatorPolicy, VerifierSuiteDecl},
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    supervisor::{
        LoopBudget, SupervisorActionState, SupervisorDecision, SupervisorExecution,
        SupervisorExecutionState, SupervisorMode, SupervisorObservation, SupervisorProposal,
        SupervisorRound, SupervisorRoundState, SupervisorTool,
    },
    task_spec::{
        AcceptanceCheck, CheckKind, CommunicationPolicy, CompletionDefinition, ContextPolicy,
        NormativeForce, OutputContract, Requirement, ResourceBudget, SourceLanguage, SourceSpan,
        TASK_SPEC_SCHEMA_VERSION, TaskSpec, ToolName, WorkerRole, WorkflowMode,
    },
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};
const BASE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
#[path = "workflow_supervisor_context/cases.rs"]
mod context_cases;
#[path = "workflow_supervisor_context/read_privacy_cases.rs"]
mod read_privacy_cases;
fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("test identifier")
}
fn path(value: &str) -> PathPattern {
    PathPattern::new(value).expect("test path")
}
fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
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
    capabilities.insert(SlotCapability::StructuredTurn, evidence.clone());
    capabilities.insert(SlotCapability::WritableWorktree, evidence);
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

struct Fixture {
    store: SqliteStore,
    // Close SQLite before NamedTempFile removes the owned file on Windows.
    file: NamedTempFile,
    start: SupervisorStart,
    execution: SupervisorExecution,
}

impl Fixture {
    fn new() -> Self {
        Self::with_decisions(16)
    }

    fn with_decisions(decisions: u32) -> Self {
        Self::configured(decisions, false)
    }

    fn configured(decisions: u32, context_enabled: bool) -> Self {
        let file = NamedTempFile::new().expect("owned database");
        let mut store = SqliteStore::open(file.path()).expect("migrate production schema");
        let mut policy = simple_policy();
        policy.content_store_opt_in = context_enabled;
        let mut specs: BTreeMap<_, _> = ["track_a", "track_b"]
            .into_iter()
            .map(|key| (id(key), simple_spec(key)))
            .collect();
        let (mut record, mut contracts) = workflow();
        if context_enabled {
            record.content_store = ContentStoreMode::Enabled { retention_days: 1 };
            for (key, spec) in &mut specs {
                let other = if key.as_str() == "track_a" {
                    "track_b"
                } else {
                    "track_a"
                };
                spec.communication_policy.may_message = vec![id(other)];
                spec.communication_policy.max_messages_per_turn = 1;
                spec.context_policy.max_bundle_bytes = 8192;
                spec.context_policy.allow_public_excerpts = true;
                spec.context_policy.retrieval_order = vec![
                    vibemux_workflow::task_spec::ContextSourceKind::CandidateSnapshot,
                    vibemux_workflow::task_spec::ContextSourceKind::PublicMessages,
                ];
                spec.read_scopes = vec![path(&format!("src/{other}.mjs"))];
            }
        }
        record.budget.decisions_remaining = decisions;
        record.policy_digest = policy.digest().expect("policy");
        record.budget.deadline_unix_ms =
            u64::try_from(now().unix_timestamp_nanos() / 1_000_000).expect("time") + 120_000;
        for contract in &mut contracts {
            contract.policy_digest = record.policy_digest;
            contract.task_spec_digest = specs[&id(&contract.task_key)]
                .digest()
                .expect("spec digest");
        }
        store
            .prepare_workflow(&record, &contracts, now())
            .expect("prepare");
        for name in ["slot_a", "slot_b", "slot_r"] {
            store
                .upsert_workflow_slot(&slot(name), now())
                .expect("slot");
        }
        let plan = json!({"schema_version":1,"request_sha256":Sha256Digest::of(b"Build task"),"policy":policy,"specs":specs,
            "assignments":{"track_a":["slot_a"],"track_b":["slot_b"]},"reviewer_slot":"slot_r","template_overrides":{},"negative_control":false});
        store
            .bind_recovery_plan(
                record.workflow_id,
                &plan,
                1,
                ContextSettingsIdentity {
                    configured_context_tokens: None,
                    compaction_settings_digest: None,
                },
                now(),
            )
            .expect("bind exact full plan");
        let record = store
            .transition_workflow(
                record.workflow_id,
                record.version,
                WorkflowPhase::Running,
                None,
                now(),
            )
            .expect("start workflow")
            .record;
        let start = SupervisorStart {
            workflow_id: record.workflow_id,
            start_request_id: Uuid::new_v4(),
            expected_version: record.version,
            mode: SupervisorMode::Model,
            full_plan: plan,
            config_digest: Sha256Digest::of(b"supervisor config"),
            slot_harnesses: ["slot_a", "slot_b", "slot_r"]
                .into_iter()
                .map(|name| (id(name), vibemux_harness::AgentKind::Claude))
                .collect(),
            timestamp: now(),
        };
        let execution = store
            .begin_supervisor_execution(&start)
            .expect("begin supervisor");
        Self {
            file,
            store,
            start,
            execution,
        }
    }

    fn observe(&mut self) -> SupervisorObservation {
        self.store
            .supervisor_observation(self.start.workflow_id)
            .expect("observe")
    }

    fn admission(&self, observation: &SupervisorObservation) -> SupervisorRoundAdmission {
        let request = ModelCallRequest {
            role: RouteRole::Supervisor,
            requested_alias: "supervisor_fixture".into(),
            protocol: GatewayProtocol::OpenaiChatCompletions,
            max_output_tokens: 1024,
            timeout_ms: 10_000,
            config_sha256: self.start.config_digest,
            observation_digest: None,
            system_prompt: "Typed proposal fixture".into(),
            user_prompt: serde_json::to_string(
                &json!({"schema_version":1,"observation":observation}),
            )
            .expect("context"),
        };
        SupervisorRoundAdmission {
            workflow_id: self.start.workflow_id,
            generation: self.execution.generation,
            observed_version: observation.workflow.version,
            observation_digest: observation.digest().expect("observation digest"),
            context_digest: Sha256Digest::of(request.user_prompt.as_bytes()),
            model_call: ModelCallAdmission {
                call_id: Uuid::new_v4(),
                owner_key: id(&format!("supervisor_{}", self.start.workflow_id.simple())),
                workflow_id: Some(self.start.workflow_id),
                identity: request.identity().expect("identity"),
                deadline_unix_ms: self.execution.deadline_unix_ms,
                max_calls: self.execution.max_model_calls,
            },
            timestamp: now(),
        }
    }

    fn reserve(&mut self, observation: &SupervisorObservation) -> SupervisorRound {
        let admission = self.admission(observation);
        self.store
            .reserve_supervisor_round(&admission)
            .expect("atomic reserve")
    }

    fn complete(&mut self, round: &SupervisorRound) {
        let call = self
            .store
            .submit_model_call(round.call_id)
            .expect("submit once");
        let receipt = ModelCallReceipt {
            call_id: call.call_id,
            role: RouteRole::Supervisor,
            requested_alias: call.admission.identity.requested_alias.clone(),
            protocol: call.admission.identity.protocol,
            reported_model: None,
            effective_provider: ProviderAttribution::Unknown,
            usage: ReportedUsage::default(),
            http_status: Some(200),
            outcome: CallOutcome::Completed,
            failure: None,
            elapsed_ms: 1,
            request_sha256: call.admission.identity.request_sha256,
            response_sha256: Some(Sha256Digest::of(b"fixture response")),
            route_generation: Sha256Digest::of(b"observed gateway route"),
            attempt: call.calls_used,
        };
        self.store
            .finish_model_call(round.call_id, Some(receipt), None)
            .expect("completed model evidence");
    }

    fn proposal(
        &mut self,
        key: &str,
        action: SupervisorTool,
    ) -> (SupervisorRound, SupervisorProposal) {
        let observation = self.observe();
        let round = self.reserve(&observation);
        self.complete(&round);
        (
            round,
            SupervisorProposal {
                decisions: vec![SupervisorDecision {
                    idempotency_key: key.into(),
                    expected_workflow_version: observation.workflow.version,
                    action,
                }],
                rationale: "Fixture decision".into(),
                evidence_refs: vec![],
            },
        )
    }
}

#[test]
fn own_reservation_does_not_invalidate_original_version_and_exact_replay_is_free() {
    let mut fixture = Fixture::new();
    let before = fixture.observe();
    let (round, proposal) = fixture.proposal("inspect_once", SupervisorTool::InspectWorkflow {});
    assert_eq!(round.observed_version, before.workflow.version);
    assert_eq!(round.reservation_version, before.workflow.version + 1);
    let commit = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("commit observed version");
    assert!(commit.accepted, "{:?}", commit.round.rejection_codes);
    let after = fixture.observe();
    assert_eq!(
        after.workflow.budget.model_requests_remaining,
        before.workflow.budget.model_requests_remaining - 1
    );
    assert_eq!(
        after.workflow.budget.decisions_remaining,
        before.workflow.budget.decisions_remaining - 1
    );
    let replay = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("identical round replay");
    assert!(replay.accepted && replay.duplicate);
    assert_eq!(fixture.observe(), after);
}

#[test]
fn cross_round_same_key_is_conflict_and_cannot_avoid_decision_charge() {
    let mut fixture = Fixture::new();
    let (round, proposal) = fixture.proposal("inspect_once", SupervisorTool::InspectWorkflow {});
    let commit = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("first proposal");
    let action = fixture
        .store
        .claim_supervisor_action(
            commit.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim");
    fixture
        .store
        .finish_supervisor_action(&SupervisorActionFinish {
            action_id: action.action_id,
            generation: action.generation,
            state: SupervisorActionState::Applied,
            result_refs: vec![],
            result_code: Some(id("observed")),
            timestamp: now(),
        })
        .expect("finish read");
    let before = fixture.observe();
    let (second, proposal) = fixture.proposal("inspect_once", SupervisorTool::InspectWorkflow {});
    let rejected = fixture
        .store
        .commit_supervisor_proposal(second.round_id, &proposal, now())
        .expect("durable rejection");
    assert!(!rejected.accepted && rejected.actions.is_empty());
    assert!(
        rejected
            .round
            .rejection_codes
            .contains(&"supervisor_idempotency_conflict".into())
    );
    assert_eq!(
        fixture.observe().workflow.budget.decisions_remaining,
        before.workflow.budget.decisions_remaining - 1
    );
}

#[test]
fn operator_phase_change_during_model_call_rejects_late_proposal() {
    let mut fixture = Fixture::new();
    let observation = fixture.observe();
    let round = fixture.reserve(&observation);
    let current = fixture.observe().workflow;
    fixture
        .store
        .transition_workflow(
            current.workflow_id,
            current.version,
            WorkflowPhase::Paused,
            None,
            now(),
        )
        .expect("operator pause");
    fixture.complete(&round);
    let proposal = SupervisorProposal {
        decisions: vec![SupervisorDecision {
            idempotency_key: "late".into(),
            expected_workflow_version: observation.workflow.version,
            action: SupervisorTool::AssignTask {
                task_key: id("track_a"),
                slot_id: id("slot_a"),
            },
        }],
        rationale: String::new(),
        evidence_refs: vec![],
    };
    let result = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("record rejection");
    assert!(!result.accepted && result.actions.is_empty());
    assert!(
        result
            .round
            .rejection_codes
            .contains(&"supervisor_stale_version".into())
    );
    assert_eq!(fixture.observe().workflow.phase, WorkflowPhase::Paused);
}

#[test]
fn completed_model_without_committed_proposal_fences_restart_and_repeated_end() {
    let mut fixture = Fixture::new();
    let observed = fixture.observe();
    let round = fixture.reserve(&observed);
    fixture.complete(&round);
    fixture
        .store
        .end_supervisor_execution(
            fixture.start.workflow_id,
            fixture.execution.generation,
            now(),
        )
        .expect("end unknown response");
    let first = fixture
        .store
        .supervisor_status(fixture.start.workflow_id)
        .expect("status")
        .expect("exists");
    assert_eq!(
        first.execution.state,
        SupervisorExecutionState::RecoveryRequired
    );
    assert_eq!(first.rounds[0].state, SupervisorRoundState::Unknown);
    fixture
        .store
        .end_supervisor_execution(
            fixture.start.workflow_id,
            fixture.execution.generation,
            now(),
        )
        .expect("idempotent second end");
    assert_eq!(
        fixture
            .store
            .supervisor_status(fixture.start.workflow_id)
            .expect("status"),
        Some(first)
    );
    let mut start = fixture.start.clone();
    start.start_request_id = Uuid::new_v4();
    start.expected_version = fixture.observe().workflow.version;
    assert_eq!(
        fixture
            .store
            .begin_supervisor_execution(&start)
            .expect_err("never replay lost output")
            .code(),
        "store_supervisor_effect_unresolved"
    );
    assert_eq!(
        fixture.observe().workflow.budget.model_requests_remaining,
        observed.workflow.budget.model_requests_remaining - 1
    );
}

#[test]
fn immutable_plan_rejects_assignment_reviewer_and_template_changes() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .end_supervisor_execution(
            fixture.start.workflow_id,
            fixture.execution.generation,
            now(),
        )
        .expect("clean end");
    let changes = [
        (
            "assignments",
            json!({"track_a":["slot_b"],"track_b":["slot_a"]}),
        ),
        ("reviewer_slot", json!("slot_b")),
        (
            "template_overrides",
            json!({"worker_v1":{"objective":"Changed wording"}}),
        ),
    ];
    for (field, value) in changes {
        let mut start = fixture.start.clone();
        start.start_request_id = Uuid::new_v4();
        start.full_plan[field] = value;
        assert_eq!(
            fixture
                .store
                .begin_supervisor_execution(&start)
                .expect_err("plan identity is immutable")
                .code(),
            "store_supervisor_plan_changed"
        );
    }
}

#[test]
fn action_finish_changes_full_observation_and_stale_reservation_is_rejected() {
    let mut fixture = Fixture::new();
    let (round, proposal) = fixture.proposal("inspect", SupervisorTool::InspectWorkflow {});
    let committed = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("proposal");
    let action = fixture
        .store
        .claim_supervisor_action(
            committed.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim");
    let before = fixture.observe();
    let admission = fixture.admission(&before);
    fixture
        .store
        .finish_supervisor_action(&SupervisorActionFinish {
            action_id: action.action_id,
            generation: action.generation,
            state: SupervisorActionState::Applied,
            result_refs: vec![],
            result_code: Some(id("observed")),
            timestamp: now(),
        })
        .expect("finish");
    let after = fixture.observe();
    assert_ne!(
        before.digest().expect("digest"),
        after.digest().expect("digest")
    );
    assert_eq!(
        fixture
            .store
            .reserve_supervisor_round(&admission)
            .expect_err("stale terminal action observation")
            .code(),
        "store_supervisor_stale_observation"
    );
}

#[test]
fn assignments_are_task_scoped_and_cannot_be_completed_without_dispatch_evidence() {
    let mut fixture = Fixture::new();
    let (round, wrong) = fixture.proposal(
        "wrong_slot",
        SupervisorTool::AssignTask {
            task_key: id("track_a"),
            slot_id: id("slot_b"),
        },
    );
    let rejected = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &wrong, now())
        .expect("reject wrong task slot");
    assert!(!rejected.accepted && rejected.actions.is_empty());
    assert!(
        rejected
            .round
            .rejection_codes
            .contains(&"supervisor_ineligible_slot".into())
    );
    let (round, valid) = fixture.proposal(
        "valid_slot",
        SupervisorTool::AssignTask {
            task_key: id("track_a"),
            slot_id: id("slot_a"),
        },
    );
    let accepted = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &valid, now())
        .expect("valid proposal");
    assert!(accepted.accepted, "{:?}", accepted.round.rejection_codes);
    let action = fixture
        .store
        .claim_supervisor_action(
            accepted.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim");
    assert_eq!(
        fixture
            .store
            .finish_supervisor_action(&SupervisorActionFinish {
                action_id: action.action_id,
                generation: action.generation,
                state: SupervisorActionState::Applied,
                result_refs: vec![],
                result_code: Some(id("fake_success")),
                timestamp: now()
            })
            .expect_err("no invented launch receipt")
            .code(),
        "store_supervisor_result_mismatch"
    );
}

#[test]
fn schema_eight_adds_owned_tables_and_survives_reopen() {
    let fixture = Fixture::new();
    let workflow_id = fixture.start.workflow_id;
    let path = fixture.file.path().to_path_buf();
    drop(fixture.store);
    let store = SqliteStore::open(&path).expect("reopen schema eight");
    assert_eq!(vibemux_store::STORE_SCHEMA_VERSION, 9);
    assert_eq!(
        store
            .supervisor_status(workflow_id)
            .expect("durable status")
            .expect("exists")
            .execution
            .generation,
        1
    );
}

#[test]
fn no_new_round_or_idempotency_replay_bypasses_zero_decision_budget() {
    let mut fixture = Fixture::with_decisions(1);
    let (round, proposal) = fixture.proposal("last_inspect", SupervisorTool::InspectWorkflow {});
    let committed = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("last decision");
    assert!(committed.accepted);
    let action = fixture
        .store
        .claim_supervisor_action(
            committed.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim");
    fixture
        .store
        .finish_supervisor_action(&SupervisorActionFinish {
            action_id: action.action_id,
            generation: action.generation,
            state: SupervisorActionState::Applied,
            result_refs: vec![],
            result_code: Some(id("observed")),
            timestamp: now(),
        })
        .expect("read");
    let exhausted = fixture.observe();
    assert_eq!(exhausted.workflow.budget.decisions_remaining, 0);
    let admission = fixture.admission(&exhausted);
    assert!(fixture.store.reserve_supervisor_round(&admission).is_err());
    let replay = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("same recorded round replay");
    assert!(replay.duplicate);
    assert_eq!(fixture.observe(), exhausted);
}

#[test]
fn a_same_workflow_unrelated_receipt_cannot_complete_a_claimed_action() {
    let mut fixture = Fixture::new();
    let (round, proposal) = fixture.proposal("inspect", SupervisorTool::InspectWorkflow {});
    let committed = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("proposal");
    let action = fixture
        .store
        .claim_supervisor_action(
            committed.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim");
    let receipt_id = Uuid::new_v4();
    let unrelated = vibemux_workflow::receipts::WorkflowReceipt::Selection(
        vibemux_workflow::receipts::SelectionRecord {
            schema_version: 1,
            receipt_id,
            workflow_id: fixture.start.workflow_id,
            contract_id: contract("track_a").contract_id,
            outcome: vibemux_workflow::selection::SelectionOutcome {
                winner: None,
                decided_by: vibemux_workflow::selection::DecidedBy::NoValidCandidate,
                ranking: vec![],
                excluded: vec![],
            },
        },
    );
    fixture
        .store
        .record_workflow_receipt(fixture.start.workflow_id, &unrelated, None, now())
        .expect("real unrelated receipt");
    assert_eq!(
        fixture
            .store
            .finish_supervisor_action(&SupervisorActionFinish {
                action_id: action.action_id,
                generation: action.generation,
                state: SupervisorActionState::Applied,
                result_refs: vec![receipt_id],
                result_code: Some(id("invented_effect")),
                timestamp: now()
            })
            .expect_err("same workflow is insufficient")
            .code(),
        "store_supervisor_result_mismatch"
    );
}

#[test]
fn blocked_report_claim_uses_valid_phase_edges() {
    let mut fixture = Fixture::new();
    let (round, proposal) = fixture.proposal(
        "block",
        SupervisorTool::ReportBlocked {
            reason_code: id("needs_operator"),
        },
    );
    let committed = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("proposal");
    assert!(committed.accepted);
    let current = fixture.observe().workflow;
    fixture
        .store
        .transition_workflow(
            current.workflow_id,
            current.version,
            WorkflowPhase::Integrating,
            None,
            now(),
        )
        .expect("unrelated valid phase change");
    assert_eq!(
        fixture
            .store
            .claim_supervisor_action(
                committed.actions[0].action_id,
                fixture.execution.generation,
                now()
            )
            .expect_err("integrating to blocked is forbidden")
            .code(),
        "workflow_invalid_transition"
    );
    assert_eq!(fixture.observe().workflow.phase, WorkflowPhase::Integrating);
}

#[test]
fn recovery_walks_more_than_512_supervisors_without_skipping_or_replaying() {
    let mut fixture = Fixture::new();
    let template = fixture.observe().workflow;
    let specs: BTreeMap<SpecIdentifier, TaskSpec> =
        serde_json::from_value(fixture.start.full_plan["specs"].clone()).expect("specs");
    let contracts: Vec<_> = ["track_a", "track_b"]
        .into_iter()
        .map(|key| {
            let mut contract = contract(key);
            contract.policy_digest = template.policy_digest;
            contract.task_spec_digest = specs[&id(key)].digest().expect("spec digest");
            contract
        })
        .collect();
    let mut ids = vec![template.workflow_id];
    for index in 1..=512_u128 {
        let mut record = template.clone();
        record.workflow_id = Uuid::from_u128(index);
        record.request_key = format!("paged_{index}");
        record.phase = WorkflowPhase::Prepared;
        record.version = 1;
        fixture
            .store
            .prepare_workflow(&record, &contracts, now())
            .expect("prepare paged workflow");
        fixture
            .store
            .bind_recovery_plan(
                record.workflow_id,
                &fixture.start.full_plan,
                1,
                ContextSettingsIdentity {
                    configured_context_tokens: None,
                    compaction_settings_digest: None,
                },
                now(),
            )
            .expect("immutable binding");
        let record = fixture
            .store
            .transition_workflow(
                record.workflow_id,
                record.version,
                WorkflowPhase::Running,
                None,
                now(),
            )
            .expect("running")
            .record;
        let mut start = fixture.start.clone();
        start.workflow_id = record.workflow_id;
        start.start_request_id = Uuid::new_v4();
        start.expected_version = record.version;
        fixture
            .store
            .begin_supervisor_execution(&start)
            .expect("supervisor row");
        ids.push(record.workflow_id);
    }
    assert_eq!(
        fixture.store.recover_supervisors(now()).expect("all pages"),
        513
    );
    assert_eq!(
        fixture
            .store
            .recover_supervisors(now())
            .expect("idempotent recovery"),
        0
    );
    for workflow_id in ids {
        let status = fixture
            .store
            .supervisor_status(workflow_id)
            .expect("status")
            .expect("durable owner");
        assert_eq!(status.execution.state, SupervisorExecutionState::Stopped);
        assert!(status.rounds.is_empty() && status.actions.is_empty());
    }
}

#[test]
fn schema_valid_authority_denials_consume_the_same_bounded_response_repair_allowance() {
    let mut fixture = Fixture::new();
    let initial = fixture.observe().workflow.budget;
    assert_eq!(initial.malformed_repairs_remaining, 2);
    for index in 0..3 {
        let (round, proposal) = fixture.proposal(
            &format!("invalid_{index}"),
            SupervisorTool::RequestHandoff {
                task_key: id("track_a"),
            },
        );
        let result = fixture
            .store
            .commit_supervisor_proposal(round.round_id, &proposal, now())
            .expect("record typed refusal");
        assert!(!result.accepted && result.actions.is_empty());
        assert!(
            result
                .round
                .rejection_codes
                .contains(&"supervisor_tool_unavailable".into())
        );
        let observed = fixture.observe();
        assert_eq!(
            observed.workflow.budget.malformed_repairs_remaining,
            initial
                .malformed_repairs_remaining
                .saturating_sub(index + 1)
        );
        assert_eq!(
            observed.workflow.budget.model_requests_remaining,
            initial.model_requests_remaining - index - 1
        );
    }
}
