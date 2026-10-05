//! Production writer tests for immutable successor admission and retirement.
use std::collections::BTreeMap;

use serde_json::{Value, json};
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ContractEditAdmission, SqliteStore};
use vibemux_workflow::{
    PathPattern, Sha256Digest, SpecIdentifier,
    checkpoint::ContextSettingsIdentity,
    contract::{ContractArtifact, RouteTransformation, semantic_diff},
    gates::WorkflowPhase,
    policy::{OperatorPolicy, VerifierSuiteDecl},
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    supervisor::LoopBudget,
    task_spec::{TaskSpec, WorkflowMode, fixtures::track_a_spec},
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};

fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("fixture identifier")
}
fn artifact(spec: &TaskSpec, policy: &OperatorPolicy) -> ContractArtifact {
    let digest = spec.digest().expect("spec digest");
    ContractArtifact {
        schema_version: 1,
        contract_id: Sha256Digest::of_fields("edit_test_contract", &[digest.as_bytes()]),
        workflow_key: spec.workflow_key.to_string(),
        task_key: spec.task_key.to_string(),
        contract_version: spec.contract_version,
        task_spec_digest: digest,
        policy_digest: policy.digest().expect("policy"),
        template_digest: Sha256Digest::of(b"template"),
        template_version: spec.template_version.to_string(),
        harness_profile: "fixture_writable_v1".into(),
        route_transformation: RouteTransformation::None,
        context_bundle_digests: vec![],
        base_commit: spec.base_commit.clone(),
        rendered_prompt_digest: digest,
        rendered_prompt_bytes: 32,
        previous_contract_id: None,
        semantic_diff: None,
    }
}

struct Fixture {
    store: SqliteStore,
    parent: WorkflowRecord,
    spec: TaskSpec,
    policy: OperatorPolicy,
    plan: Value,
    _file: NamedTempFile,
}
impl Fixture {
    fn new() -> Self {
        let file = NamedTempFile::new().expect("owned database");
        let mut store = SqliteStore::open(file.path()).expect("current production schema");
        let spec = track_a_spec().normalized();
        let policy = OperatorPolicy {
            schema_version: 1,
            policy_version: spec.policy_version.clone(),
            writable_roots: spec.owned_paths.clone(),
            protected_paths: vec![PathPattern::new(".git/**").expect("protected")],
            allowed_tools: spec.permitted_tools.clone(),
            verifier_suites: vec![VerifierSuiteDecl {
                suite_id: id("api"),
                description: "API checks".into(),
            }],
            budget_caps: spec.resource_budget,
            max_parallel_slots: 1,
            max_supervisor_decisions: 16,
            max_optimizer_candidates: 1,
            content_store_opt_in: false,
            aag_only: true,
        };
        let contract = artifact(&spec, &policy);
        let parent = WorkflowRecord {
            schema_version: 1,
            workflow_id: Uuid::new_v4(),
            request_key: format!("edit_parent_{}", Uuid::new_v4().simple()),
            workflow_key: spec.workflow_key.clone(),
            mode: WorkflowMode::Cooperate,
            phase: WorkflowPhase::Prepared,
            version: 1,
            evidence_class: EvidenceClass::Fixture,
            base_commit: spec.base_commit.clone(),
            policy_digest: contract.policy_digest,
            verifier_digest: Sha256Digest::of(b"verifier"),
            integration_suites: vec![id("api")],
            tasks: vec![WorkflowTask {
                task_key: spec.task_key.clone(),
                contract_id: contract.contract_id,
                contract_version: 1,
                required_suites: vec![id("api")],
                progress: TaskProgress::Pending,
                turns_used: 1,
                repairs_used: 0,
                max_turns: spec.resource_budget.max_turns,
                max_repairs: spec.resource_budget.max_repairs,
                accepted_candidate: None,
                failure_code: None,
            }],
            budget: LoopBudget {
                decisions_remaining: 9,
                malformed_repairs_remaining: 1,
                model_requests_remaining: 7,
                deadline_unix_ms: 9_000_000,
            },
            content_store: ContentStoreMode::Disabled,
            blocked_reason: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        store
            .prepare_workflow(&parent, &[contract], OffsetDateTime::UNIX_EPOCH)
            .expect("parent admission");
        store
            .bind_workflow_contract_authority(
                parent.tasks[0].contract_id,
                &spec,
                OffsetDateTime::UNIX_EPOCH,
            )
            .expect("authority");
        for name in ["slot_a", "review"] {
            let evidence = CapabilityEvidence {
                level: EvidenceLevel::WriteModeCertified,
                class: EvidenceClass::Fixture,
                evidence_ref: "fixture".into(),
                checked_at_ms: 0,
                expires_at_ms: u64::MAX,
            };
            let slot = SlotFacts {
                slot_id: id(name),
                harness: vibemux_harness::AgentKind::Claude,
                interface_mode: InterfaceMode::Structured,
                capabilities: BTreeMap::from([
                    (SlotCapability::StructuredTurn, evidence.clone()),
                    (SlotCapability::WritableWorktree, evidence),
                ]),
                health: SlotHealth::Idle,
                route_role: None,
                cooldown_until_ms: None,
                leased_generation: None,
                budget: SlotBudget::Remaining(40),
            };
            store
                .upsert_workflow_slot(&slot, OffsetDateTime::UNIX_EPOCH)
                .expect("owned fixture slot");
        }
        let specs = BTreeMap::from([(spec.task_key.clone(), spec.clone())]);
        let assignments = BTreeMap::from([(spec.task_key.clone(), vec![id("slot_a")])]);
        let plan = json!({"schema_version":1,"request_sha256":spec.original_request_ref,"policy":policy,"specs":specs,
            "assignments":assignments,"reviewer_slot":"review","template_overrides":{},"negative_control":false});
        store
            .bind_recovery_plan(
                parent.workflow_id,
                &plan,
                1,
                ContextSettingsIdentity {
                    configured_context_tokens: None,
                    compaction_settings_digest: None,
                },
                OffsetDateTime::UNIX_EPOCH,
            )
            .expect("exact plan binding");
        Self {
            store,
            parent,
            spec,
            policy,
            plan,
            _file: file,
        }
    }
    fn edit(&self) -> ContractEditAdmission {
        let mut spec = self.spec.clone();
        spec.objective.push_str(" Keep the error messages precise.");
        spec.contract_version += 1;
        let mut contract = artifact(&spec, &self.policy);
        contract.previous_contract_id = Some(self.parent.tasks[0].contract_id);
        contract.semantic_diff = Some(semantic_diff(&self.spec, &spec));
        let mut successor = self.parent.clone();
        successor.workflow_id = Uuid::new_v4();
        successor.request_key = format!("edit_child_{}", Uuid::new_v4().simple());
        successor.tasks[0].contract_id = contract.contract_id;
        successor.tasks[0].contract_version = spec.contract_version;
        let mut plan = self.plan.clone();
        plan["specs"][spec.task_key.as_str()] =
            serde_json::to_value(&spec).expect("replacement spec");
        ContractEditAdmission {
            edit_id: Uuid::new_v4(),
            parent_workflow_id: self.parent.workflow_id,
            expected_parent_version: 1,
            request_digest: Sha256Digest::of(b"complete operator edit request"),
            parent_plan: self.plan.clone(),
            successor_plan: plan,
            successor,
            contracts: vec![contract],
            timestamp: OffsetDateTime::UNIX_EPOCH,
        }
    }
}

#[test]
fn successor_retirement_history_budget_and_replay_share_real_transaction() {
    let mut fixture = Fixture::new();
    let edit = fixture.edit();
    let committed = fixture
        .store
        .prepare_contract_edit(&edit)
        .expect("bounded semantic successor");
    assert!(!committed.duplicate);
    assert_eq!(committed.record.budget, fixture.parent.budget);
    assert_eq!(committed.record.tasks[0].turns_used, 1);
    assert_eq!(
        fixture
            .store
            .workflow_record(fixture.parent.workflow_id)
            .expect("parent")
            .expect("present")
            .phase,
        WorkflowPhase::Cancelled
    );
    let replay = fixture
        .store
        .prepare_contract_edit(&edit)
        .expect("exact replay after retirement");
    assert!(replay.duplicate);
    assert_eq!(replay.record.workflow_id, committed.record.workflow_id);
    let snapshot = fixture
        .store
        .workflow_snapshot(edit.successor.workflow_id)
        .expect("snapshot")
        .expect("successor");
    assert_eq!(snapshot.contracts.len(), 2);
    assert!(
        snapshot
            .contracts
            .iter()
            .any(|contract| contract.contract_id == fixture.parent.tasks[0].contract_id)
    );
    let mut conflict = edit.clone();
    conflict.request_digest = Sha256Digest::of(b"changed input");
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&conflict)
            .err()
            .expect("conflict")
            .code(),
        "workflow_edit_request_conflict"
    );
    let mut conflict = edit.clone();
    conflict.successor.budget.model_requests_remaining += 1;
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&conflict)
            .err()
            .expect("replay budget conflict")
            .code(),
        "workflow_edit_request_conflict"
    );
    let mut conflict = edit.clone();
    conflict.contracts[0].harness_profile = "changed_profile".into();
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&conflict)
            .err()
            .expect("replay profile conflict")
            .code(),
        "workflow_edit_request_conflict"
    );
    let mut conflict = edit;
    conflict.successor_plan["specs"][fixture.spec.task_key.as_str()]["objective"] =
        json!("Different content");
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&conflict)
            .err()
            .expect("plan conflict")
            .code(),
        "workflow_edit_request_conflict"
    );
}

#[test]
fn stale_parent_and_budget_reset_never_create_successor_or_retire_parent() {
    let mut fixture = Fixture::new();
    let mut edit = fixture.edit();
    edit.expected_parent_version += 1;
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&edit)
            .err()
            .expect("stale")
            .code(),
        "store_workflow_version_conflict"
    );
    edit.expected_parent_version = 1;
    edit.successor.budget.model_requests_remaining += 1;
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&edit)
            .err()
            .expect("budget widening")
            .code(),
        "workflow_edit_immutable_authority"
    );
    assert!(
        fixture
            .store
            .workflow_record(edit.successor.workflow_id)
            .expect("lookup")
            .is_none()
    );
    assert_eq!(
        fixture
            .store
            .workflow_record(fixture.parent.workflow_id)
            .expect("lookup")
            .expect("parent")
            .phase,
        WorkflowPhase::Prepared
    );
}

#[test]
fn a_second_successor_cannot_fork_a_retired_parent() {
    let mut fixture = Fixture::new();
    let first = fixture.edit();
    let second = fixture.edit();
    fixture.store.prepare_contract_edit(&first).expect("first");
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&second)
            .err()
            .expect("no competing successor")
            .code(),
        "workflow_edit_parent_already_retired"
    );
    assert!(
        fixture
            .store
            .workflow_record(second.successor.workflow_id)
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn a_running_parent_is_not_implicitly_cancelled_by_edit() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .transition_workflow(
            fixture.parent.workflow_id,
            1,
            WorkflowPhase::Running,
            None,
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("start");
    let mut edit = fixture.edit();
    edit.expected_parent_version = 2;
    assert_eq!(
        fixture
            .store
            .prepare_contract_edit(&edit)
            .err()
            .expect("running refused")
            .code(),
        "workflow_edit_parent_not_quiescent"
    );
    assert_eq!(
        fixture
            .store
            .workflow_record(fixture.parent.workflow_id)
            .expect("lookup")
            .expect("parent")
            .phase,
        WorkflowPhase::Running
    );
}
