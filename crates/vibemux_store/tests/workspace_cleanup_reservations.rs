//! Public store reservation contracts with in-memory SQLite and synthetic metadata.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vibemux_harness::{AgentKind, dispatch::attempt::resource_key};
use vibemux_store::{
    ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation, LeaseRecord, LeaseRequest,
    SqliteStore, StoreError, WorkspaceCleanupConfirmation, WorkspaceCleanupPlan,
};
use vibemux_types::{
    ProjectId, Run, RunSpec, Task, TaskSpec,
    a2a::{A2aRunStart, RunWorkspace},
    chat::ChatHarness,
    workspace_cleanup::{CleanupObjectIdentity, WorkspaceCleanupBinding, WorkspaceCleanupPhase},
};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::{ContractArtifact, RouteTransformation},
    gates::WorkflowPhase,
    leases::{LeaseEvent, LeaseState},
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    supervisor::LoopBudget,
    task_spec::WorkflowMode,
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};

const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_793_433_600 + seconds).expect("fixture time")
}

fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("fixture identifier")
}

fn store() -> SqliteStore {
    SqliteStore::open(Path::new(":memory:")).expect("in-memory store")
}

fn cleanup_plan(store: &mut SqliteStore) -> WorkspaceCleanupPlan {
    let mut owner = store
        .open_chat_execution_owner(&ChatExecutionOwnerOpen {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Codex,
            profile_sha256: "a".repeat(64),
            base_commit: BASE.into(),
            admitted_tools: BTreeSet::new(),
            title_ref: None,
            timestamp: at(0),
        })
        .expect("owner");
    let workspace = RunWorkspace {
        path: format!("C:/owned/project/.vibemux/worktrees/{}", owner.owner.run_id),
        branch: format!("codex/run_{}", owner.owner.run_id.as_uuid().simple()),
        base_commit: BASE.into(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    let key = resource_key(&workspace.path).to_hex();
    for change in [
        ChatOwnerChange::Plan {
            workspace: workspace.clone(),
            resource_key: key.clone(),
        },
        ChatOwnerChange::ClaimCreation,
        ChatOwnerChange::VerifyWorkspace,
        ChatOwnerChange::Stop,
    ] {
        owner = store
            .change_chat_execution_owner(
                &ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.owner.chat_id,
                    expected_version: owner.owner.version,
                    timestamp: at(0),
                },
                &change,
            )
            .expect("finalized verified owner");
    }
    let identity = CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: u64::MAX,
        file_id: [13; 16],
    };
    WorkspaceCleanupPlan {
        operation_id: Uuid::new_v4(),
        daemon_incarnation: Uuid::new_v4(),
        binding: WorkspaceCleanupBinding {
            project_id: owner.owner.project_id,
            chat_id: owner.owner.chat_id,
            task_id: owner.owner.task_id,
            run_id: owner.owner.run_id,
            owner_version: owner.owner.version,
            session_generation: owner.session.generation,
            session_version: owner.session.version,
            workspace,
            resource_key: key,
            repository_root: "C:/owned/project".into(),
            common_git_dir: "C:/owned/project/.git".into(),
            repository_identity: identity.clone(),
            common_directory_identity: identity.clone(),
            worktree_identity: identity,
            head_commit: "d".repeat(40),
            terminal_plugin_id: "observer".into(),
            session_id: "session".into(),
            instance_id: "instance".into(),
        },
        timestamp: at(1),
    }
}

fn confirmation(
    plan: &WorkspaceCleanupPlan,
    preview: &vibemux_store::WorkspaceCleanupCommit,
) -> WorkspaceCleanupConfirmation {
    WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: preview.cleanup.id,
        expected_version: preview.cleanup.version,
        daemon_incarnation: plan.daemon_incarnation,
        timestamp: at(4),
    }
}

fn claim_cleanup(store: &mut SqliteStore, plan: &WorkspaceCleanupPlan) {
    let preview = store.plan_workspace_cleanup(plan).expect("cleanup preview");
    let claimed = store
        .confirm_workspace_cleanup(&confirmation(plan, &preview))
        .expect("cleanup claim");
    assert_eq!(claimed.cleanup.phase, WorkspaceCleanupPhase::Claimed);
    assert!(claimed.execution_claim.is_some());
}

fn a2a_start(project_id: ProjectId) -> A2aRunStart {
    let task = Task::new(TaskSpec {
        project_id,
        title: "reservation fixture".into(),
        description: String::new(),
    })
    .expect("A2A task");
    let run = Run::new(RunSpec {
        project_id,
        task_id: task.task_id(),
        harness: "mock".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: BASE.into(),
    })
    .expect("A2A run");
    let run_id = run.run_id();
    A2aRunStart {
        task,
        run,
        peer_id: "fixture_peer".into(),
        external_task_id: format!("external_{run_id}"),
        transport: "http_json".into(),
        protocol_version: "1.0".into(),
        workspace: RunWorkspace {
            path: format!("C:/unrelated/{run_id}"),
            branch: format!("codex/a2a_{}", run_id.as_uuid().simple()),
            base_commit: BASE.into(),
            ownership_token: Uuid::new_v4().simple().to_string(),
        },
        timestamp: at(5),
        idempotency_key: format!("start_{run_id}"),
    }
}

fn running_workflow(store: &mut SqliteStore) -> WorkflowRecord {
    let contract = ContractArtifact {
        schema_version: 1,
        contract_id: Sha256Digest::of(b"reservation contract"),
        workflow_key: "reservation_fixture".into(),
        task_key: "task_a".into(),
        contract_version: 1,
        task_spec_digest: Sha256Digest::of(b"reservation spec"),
        policy_digest: Sha256Digest::of(b"policy"),
        template_digest: Sha256Digest::of(b"template"),
        template_version: "worker_v1".into(),
        harness_profile: "fixture_writable_v1".into(),
        route_transformation: RouteTransformation::None,
        context_bundle_digests: vec![],
        base_commit: BASE.into(),
        rendered_prompt_digest: Sha256Digest::of(b"reservation prompt"),
        rendered_prompt_bytes: 18,
        previous_contract_id: None,
        semantic_diff: None,
    };
    let workflow = WorkflowRecord {
        schema_version: 1,
        workflow_id: Uuid::new_v4(),
        request_key: format!("reservation_{}", Uuid::new_v4().simple()),
        workflow_key: id("reservation_fixture"),
        mode: WorkflowMode::Cooperate,
        phase: WorkflowPhase::Prepared,
        version: 1,
        evidence_class: EvidenceClass::Fixture,
        base_commit: BASE.into(),
        policy_digest: contract.policy_digest,
        verifier_digest: Sha256Digest::of(b"verifier"),
        integration_suites: vec![],
        tasks: vec![WorkflowTask {
            task_key: id("task_a"),
            contract_id: contract.contract_id,
            contract_version: 1,
            required_suites: vec![],
            progress: TaskProgress::Pending,
            turns_used: 0,
            repairs_used: 0,
            max_turns: 1,
            max_repairs: 0,
            accepted_candidate: None,
            failure_code: None,
        }],
        budget: LoopBudget {
            decisions_remaining: 1,
            malformed_repairs_remaining: 0,
            model_requests_remaining: 1,
            deadline_unix_ms: u64::MAX,
        },
        content_store: ContentStoreMode::Disabled,
        blocked_reason: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store
        .prepare_workflow(&workflow, &[contract], at(0))
        .expect("prepare workflow");
    let evidence = CapabilityEvidence {
        level: EvidenceLevel::WriteModeCertified,
        class: EvidenceClass::Fixture,
        evidence_ref: "synthetic_metadata".into(),
        checked_at_ms: 0,
        expires_at_ms: u64::MAX,
    };
    store
        .upsert_workflow_slot(
            &SlotFacts {
                slot_id: id("slot_a"),
                harness: AgentKind::Codex,
                interface_mode: InterfaceMode::Structured,
                capabilities: BTreeMap::from([
                    (SlotCapability::StructuredTurn, evidence.clone()),
                    (SlotCapability::WritableWorktree, evidence),
                ]),
                health: SlotHealth::Idle,
                route_role: None,
                cooldown_until_ms: None,
                leased_generation: None,
                budget: SlotBudget::Remaining(1),
            },
            at(0),
        )
        .expect("fixture slot");
    store
        .transition_workflow(workflow.workflow_id, 1, WorkflowPhase::Running, None, at(1))
        .expect("running workflow")
        .record
}

fn lease_request(workflow: &WorkflowRecord, plan: &WorkspaceCleanupPlan) -> LeaseRequest {
    LeaseRequest {
        lease_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: id("task_a"),
        slot_id: id("slot_a"),
        worktree_key: resource_key(&plan.binding.workspace.path),
        heartbeat_deadline_ms: u64::try_from(at(2).unix_timestamp() * 1000 + 500)
            .expect("deadline"),
        timestamp: at(2),
    }
}

fn acquire_lease(
    store: &mut SqliteStore,
    workflow: &WorkflowRecord,
    plan: &WorkspaceCleanupPlan,
    quarantined: bool,
) -> LeaseRecord {
    let lease = store
        .acquire_workflow_lease(&lease_request(workflow, plan))
        .expect("workflow lease")
        .lease;
    if quarantined {
        let lease = store
            .change_workflow_lease(
                lease.lease_id,
                LeaseEvent::HeartbeatMissed {
                    now_ms: lease.heartbeat_deadline_ms + 1,
                },
                0,
                "heartbeat_missed",
                at(3),
            )
            .expect("quarantine lease")
            .lease;
        assert_eq!(lease.state, LeaseState::Quarantined);
        lease
    } else {
        assert_eq!(lease.state, LeaseState::Active);
        lease
    }
}

#[test]
fn claimed_cleanup_blocks_a2a_normalized_path_and_branch_but_unrelated_start_replays() {
    let mut store = store();
    let plan = cleanup_plan(&mut store);
    claim_cleanup(&mut store, &plan);
    let before = store.events().expect("audit snapshot");
    for same_path in [true, false] {
        let mut request = a2a_start(plan.binding.project_id);
        if same_path {
            request.workspace.path = format!(
                "{}\\",
                plan.binding
                    .workspace
                    .path
                    .to_ascii_uppercase()
                    .replace('/', "\\")
            );
            assert_eq!(
                request.workspace.path_key(),
                plan.binding.workspace.path_key()
            );
        } else {
            request.workspace.branch = plan.binding.workspace.branch.to_ascii_uppercase();
            assert_ne!(
                request.workspace.path_key(),
                plan.binding.workspace.path_key()
            );
        }
        let task_id = request.task.task_id();
        assert!(matches!(
            store.start_a2a_run(request),
            Err(StoreError::A2aBindingConflict)
        ));
        assert!(store.a2a_runs(task_id).expect("A2A records").is_empty());
        assert_eq!(store.events().expect("unchanged audit"), before);
    }
    let request = a2a_start(plan.binding.project_id);
    let admitted = store
        .start_a2a_run(request.clone())
        .expect("unrelated A2A start");
    assert!(!admitted.duplicate);
    let before_replay = store.events().expect("audit snapshot");
    let replay = store.start_a2a_run(request).expect("A2A replay");
    assert!(replay.duplicate);
    assert_eq!(replay.record, admitted.record);
    assert_eq!(replay.event, admitted.event);
    assert_eq!(
        store.events().expect("unchanged replay audit"),
        before_replay
    );
}

#[test]
fn claimed_cleanup_blocks_fresh_workflow_lease_but_unrelated_lease_replays() {
    let mut store = store();
    let plan = cleanup_plan(&mut store);
    claim_cleanup(&mut store, &plan);
    let workflow = running_workflow(&mut store);
    let mut request = lease_request(&workflow, &plan);
    let before = store.events().expect("audit snapshot");
    assert_eq!(
        store
            .acquire_workflow_lease(&request)
            .expect_err("cleanup reserves worktree")
            .code(),
        "store_workflow_worktree_reserved"
    );
    assert!(
        store
            .workflow_lease(request.lease_id)
            .expect("lease query")
            .is_none()
    );
    assert_eq!(
        store.workflow_slots().expect("slot query")[0].leased_generation,
        None
    );
    assert_eq!(store.events().expect("unchanged audit"), before);

    request.worktree_key = resource_key("C:/unrelated/lease");
    let admitted = store
        .acquire_workflow_lease(&request)
        .expect("unrelated worktree lease");
    let before_replay = store.events().expect("audit snapshot");
    let replay = store
        .acquire_workflow_lease(&request)
        .expect("lease replay");
    assert_eq!(replay.lease, admitted.lease);
    assert!(replay.event.is_none());
    assert_eq!(
        store.events().expect("unchanged replay audit"),
        before_replay
    );
}

#[test]
fn cleanup_preview_refuses_active_and_quarantined_workflow_leases_until_release() {
    for quarantined in [false, true] {
        let mut store = store();
        let mut plan = cleanup_plan(&mut store);
        let workflow = running_workflow(&mut store);
        let lease = acquire_lease(&mut store, &workflow, &plan, quarantined);
        let before = store.events().expect("audit snapshot");
        assert_eq!(
            store
                .plan_workspace_cleanup(&plan)
                .expect_err("holding lease blocks preview")
                .code(),
            "workspace_cleanup_busy"
        );
        assert_eq!(store.events().expect("unchanged audit"), before);
        assert_eq!(
            store
                .workflow_lease(lease.lease_id)
                .expect("lease query")
                .expect("lease"),
            lease
        );

        store
            .change_workflow_lease(
                lease.lease_id,
                LeaseEvent::AttemptSettled {
                    generation: lease.generation,
                },
                0,
                "fixture_settled",
                at(4),
            )
            .expect("settle lease with no dispatched attempt");
        plan.timestamp = at(5);
        let preview = store
            .plan_workspace_cleanup(&plan)
            .expect("same request succeeds after release");
        assert_eq!(preview.cleanup.phase, WorkspaceCleanupPhase::Planned);
        assert!(!preview.duplicate);
    }
}

#[test]
fn confirmation_rechecks_workflow_leases_arriving_after_preview_and_replay_stays_inert() {
    for quarantined in [false, true] {
        let mut store = store();
        let plan = cleanup_plan(&mut store);
        let preview = store.plan_workspace_cleanup(&plan).expect("preview");
        let workflow = running_workflow(&mut store);
        let lease = acquire_lease(&mut store, &workflow, &plan, quarantined);
        let input = confirmation(&plan, &preview);
        let before = store.events().expect("audit snapshot");
        assert_eq!(
            store
                .confirm_workspace_cleanup(&input)
                .expect_err("late holding lease blocks confirmation")
                .code(),
            "workspace_cleanup_busy"
        );
        assert_eq!(store.events().expect("unchanged audit"), before);
        let stored = store
            .workspace_cleanup(preview.cleanup.id)
            .expect("cleanup query")
            .expect("preview remains");
        assert_eq!(stored.cleanup, preview.cleanup);
        assert!(stored.execution_claim.is_none());
        let replay = store
            .plan_workspace_cleanup(&plan)
            .expect("historical preview replay");
        assert!(replay.duplicate);
        assert_eq!(replay.cleanup, preview.cleanup);
        assert!(replay.execution_claim.is_none());
        assert_eq!(store.events().expect("unchanged replay audit"), before);

        store
            .change_workflow_lease(
                lease.lease_id,
                LeaseEvent::AttemptSettled {
                    generation: lease.generation,
                },
                0,
                "fixture_settled",
                at(5),
            )
            .expect("release workflow lease");
        let mut retry = input;
        retry.timestamp += Duration::seconds(2);
        let claimed = store
            .confirm_workspace_cleanup(&retry)
            .expect("same confirmation succeeds after release");
        assert_eq!(claimed.cleanup.phase, WorkspaceCleanupPhase::Claimed);
        assert!(!claimed.duplicate);
        assert!(claimed.execution_claim.is_some());
        let after_claim = store.events().expect("audit snapshot");
        let replay = store
            .confirm_workspace_cleanup(&retry)
            .expect("confirmation replay");
        assert!(replay.duplicate);
        assert!(replay.execution_claim.is_none());
        assert_eq!(store.events().expect("unchanged replay audit"), after_claim);
    }
}
