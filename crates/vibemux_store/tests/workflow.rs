//! Store schema 5 workflow transactions (ADR 031): admission, contract
//! generations, fenced leases, attempts, receipts and gates, messages,
//! content index, policy versions, and restart reconciliation.

use std::collections::BTreeMap;

use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{
        AttemptOutcome, DispatchError, NativeProtocol, OutcomeDecision, PromptDigest,
        capture_budget::{CaptureSummary, KindCounts},
        events::ProcessSummary,
    },
};
use vibemux_store::{
    AttemptPurpose, ClaimFence, ContentEntry, ContentKind, ContentState, HarnessDispatchAdmission,
    HarnessDispatchFinish, LeaseFence, LeaseRequest, MessageChange, SqliteStore, StoreError,
    WorkflowAttemptAdmission, WorkflowAttemptRecord,
};
use vibemux_types::{Run, RunSpec, Task, TaskSpec as CanonicalTaskSpec};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::{ContractArtifact, RouteTransformation, SemanticDiff},
    gates::{
        CandidateOrigin, CandidateRecord, IntegrationReceipt, RECEIPT_SCHEMA_VERSION,
        ReviewReceipt, ReviewVerdict, SuiteResult, SuiteStatus, VerifierReceipt, WorkflowPhase,
    },
    leases::{LeaseEvent, LeaseState},
    messages::{MessageEnvelope, MessageKind, MessageState, Participant},
    optimizer::OptimizablePolicy,
    receipts::{CandidateReceipt, WorkflowReceipt},
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    snapshot::{SnapshotChange, SnapshotEntry, SnapshotManifest},
    supervisor::LoopBudget,
    task_spec::WorkflowMode,
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";

fn id(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("identifier")
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + seconds).expect("timestamp")
}

fn open_store() -> (NamedTempFile, SqliteStore) {
    let file = NamedTempFile::new().expect("temporary database");
    let mut store = SqliteStore::open(file.path()).expect("open store");
    let detections: BTreeMap<String, HarnessDetection> = ["claude", "codex"]
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("1.0.0".into()),
                },
            )
        })
        .collect();
    store
        .commit_harness_snapshot(&detections, "2026-10-03T00:00:00Z", "probe_fixture")
        .expect("detect harnesses");
    (file, store)
}

fn contract(task: &str, version: u32, previous: Option<Sha256Digest>) -> ContractArtifact {
    ContractArtifact {
        schema_version: 1,
        contract_id: Sha256Digest::of(format!("{task}:{version}").as_bytes()),
        workflow_key: "taskboard_lite".into(),
        task_key: task.into(),
        contract_version: version,
        task_spec_digest: Sha256Digest::of(format!("spec:{task}:{version}").as_bytes()),
        policy_digest: Sha256Digest::of(b"policy"),
        template_digest: Sha256Digest::of(b"template"),
        template_version: "worker_v1".into(),
        harness_profile: "fixture_writable_v1".into(),
        route_transformation: RouteTransformation::None,
        context_bundle_digests: vec![],
        base_commit: BASE.into(),
        rendered_prompt_digest: Sha256Digest::of(format!("prompt:{task}:{version}").as_bytes()),
        rendered_prompt_bytes: 100,
        previous_contract_id: previous,
        semantic_diff: previous.map(|_| SemanticDiff {
            requirements_changed: vec!["r02".into()],
            ..SemanticDiff::default()
        }),
    }
}

fn task(key: &str) -> WorkflowTask {
    WorkflowTask {
        task_key: id(key),
        contract_id: contract(key, 1, None).contract_id,
        contract_version: 1,
        required_suites: vec![id("api")],
        progress: TaskProgress::Pending,
        turns_used: 0,
        repairs_used: 0,
        max_turns: 4,
        max_repairs: 1,
        accepted_candidate: None,
        failure_code: None,
    }
}

fn record(request_key: &str, tasks: &[&str]) -> WorkflowRecord {
    WorkflowRecord {
        schema_version: 1,
        workflow_id: Uuid::new_v4(),
        request_key: request_key.into(),
        workflow_key: id("taskboard_lite"),
        mode: WorkflowMode::Cooperate,
        phase: WorkflowPhase::Prepared,
        version: 1,
        evidence_class: EvidenceClass::Fixture,
        base_commit: BASE.into(),
        policy_digest: Sha256Digest::of(b"policy"),
        verifier_digest: Sha256Digest::of(b"verifier"),
        integration_suites: vec![id("api")],
        tasks: tasks.iter().map(|key| task(key)).collect(),
        budget: LoopBudget {
            decisions_remaining: 16,
            malformed_repairs_remaining: 2,
            model_requests_remaining: 40,
            deadline_unix_ms: u64::MAX,
        },
        content_store: ContentStoreMode::Enabled { retention_days: 7 },
        blocked_reason: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn slot(slot_id: &str) -> SlotFacts {
    let evidence = CapabilityEvidence {
        level: EvidenceLevel::WriteModeCertified,
        class: EvidenceClass::Fixture,
        evidence_ref: "fixture".into(),
        checked_at_ms: 0,
        expires_at_ms: u64::MAX,
    };
    let mut capabilities = BTreeMap::new();
    capabilities.insert(SlotCapability::StructuredTurn, evidence.clone());
    capabilities.insert(SlotCapability::WritableWorktree, evidence);
    SlotFacts {
        slot_id: id(slot_id),
        harness: AgentKind::Claude,
        interface_mode: InterfaceMode::Structured,
        capabilities,
        health: SlotHealth::Idle,
        route_role: None,
        cooldown_until_ms: None,
        leased_generation: None,
        budget: SlotBudget::Remaining(10),
    }
}

/// A prepared and running workflow with two registered slots.
fn running_workflow(store: &mut SqliteStore, tasks: &[&str]) -> WorkflowRecord {
    let record = record(&format!("request_{}", Uuid::new_v4().simple()), tasks);
    let contracts: Vec<ContractArtifact> = tasks.iter().map(|key| contract(key, 1, None)).collect();
    store
        .prepare_workflow(&record, &contracts, at(0))
        .expect("prepare");
    for slot_id in ["slot_a", "slot_b", "slot_r"] {
        store
            .upsert_workflow_slot(&slot(slot_id), at(0))
            .expect("slot");
    }
    store
        .transition_workflow(record.workflow_id, 1, WorkflowPhase::Running, None, at(1))
        .expect("start")
        .record
}

fn worktree(label: &str) -> Sha256Digest {
    vibemux_harness::dispatch::attempt::resource_key(label)
}

fn lease(
    store: &mut SqliteStore,
    workflow: &WorkflowRecord,
    task_key: &str,
    slot_id: &str,
    tree: &str,
) -> vibemux_store::LeaseRecord {
    store
        .acquire_workflow_lease(&LeaseRequest {
            lease_id: Uuid::new_v4(),
            workflow_id: workflow.workflow_id,
            task_key: id(task_key),
            slot_id: id(slot_id),
            worktree_key: worktree(tree),
            heartbeat_deadline_ms: 10_000,
            timestamp: at(2),
        })
        .expect("lease")
        .lease
}

fn current(store: &SqliteStore, workflow_id: Uuid) -> WorkflowRecord {
    store
        .workflow_record(workflow_id)
        .expect("query")
        .expect("workflow")
}

fn attempt_admission(
    store: &SqliteStore,
    workflow_id: Uuid,
    lease: &vibemux_store::LeaseRecord,
    tree: &str,
    purpose: AttemptPurpose,
) -> WorkflowAttemptAdmission {
    let workflow = current(store, workflow_id);
    let task = workflow.task(&lease.task_key).expect("task");
    let prompt = format!("rendered prompt for {}", lease.task_key);
    WorkflowAttemptAdmission {
        workflow_id,
        expected_workflow_version: workflow.version,
        lease_id: lease.lease_id,
        lease_generation: lease.generation,
        task_key: lease.task_key.clone(),
        contract_id: task.contract_id,
        purpose,
        session_id: Uuid::new_v4(),
        dispatch: HarnessDispatchAdmission {
            request_id: Uuid::new_v4(),
            harness: AgentKind::Claude,
            protocol: NativeProtocol::ClaudeStreamJson,
            prompt: PromptDigest {
                sha256: Sha256Digest::of(prompt.as_bytes()),
                byte_count: prompt.len() as u64,
            },
            config_sha256: Sha256Digest::of(b"workflow config"),
            resource_key: worktree(tree),
            base_commit: BASE.into(),
            timestamp: at(3),
        },
    }
}

fn finish_clean(store: &mut SqliteStore, request_id: Uuid) {
    let fence: ClaimFence = store
        .claim_harness_dispatch(request_id, at(4))
        .expect("claim")
        .fence;
    store
        .finish_harness_dispatch(&HarnessDispatchFinish {
            request_id,
            fence,
            decision: OutcomeDecision {
                outcome: AttemptOutcome::Completed,
                error_code: None,
            },
            process: ProcessSummary {
                exit_code: Some(0),
                forced_termination: false,
                stderr_bytes: 0,
            },
            capture: CaptureSummary {
                record_count: 2,
                record_bytes: 64,
                kinds: KindCounts {
                    started: 1,
                    completed: 1,
                    ..KindCounts::default()
                },
                transcript_sha256: Sha256Digest::of(b"transcript"),
            },
            timestamp: at(5),
        })
        .expect("finish");
}

fn run_id(store: &SqliteStore, request_id: Uuid) -> Uuid {
    *store
        .harness_dispatch(request_id)
        .expect("query")
        .expect("dispatch")
        .run_id
        .as_uuid()
}

fn manifest(content: &str) -> SnapshotManifest {
    SnapshotManifest {
        schema_version: 1,
        base_commit: BASE.into(),
        entries: vec![SnapshotEntry {
            path: "src/server.mjs".into(),
            change: SnapshotChange::Write {
                sha256: Sha256Digest::of(content.as_bytes()),
                byte_count: content.len() as u64,
            },
        }],
    }
}

fn candidate_receipt(
    store: &SqliteStore,
    workflow_id: Uuid,
    lease: &vibemux_store::LeaseRecord,
    attempt: &WorkflowAttemptRecord,
    content: &str,
) -> CandidateReceipt {
    let manifest = manifest(content);
    CandidateReceipt {
        schema_version: 1,
        receipt_id: Uuid::new_v4(),
        workflow_id,
        attempt_request_id: attempt.request_id,
        lease_id: lease.lease_id,
        candidate: CandidateRecord {
            candidate_digest: manifest.digest().expect("digest"),
            contract_id: attempt.contract_id,
            task_key: attempt.task_key.clone(),
            worker_run_id: run_id(store, attempt.request_id),
            worker_session_id: attempt.session_id,
            worker_route: "worker_a_alias".into(),
            worker_harness: AgentKind::Claude,
            lease_generation: lease.generation,
            origin: CandidateOrigin::FixtureWorker,
        },
        manifest: Some(manifest),
        admitted: true,
        violation_codes: vec![],
        mutated_after_checkpoint: false,
    }
}

fn fence(lease: &vibemux_store::LeaseRecord) -> Option<LeaseFence> {
    Some(LeaseFence {
        lease_id: lease.lease_id,
        generation: lease.generation,
    })
}

fn code(error: StoreError) -> String {
    error.code().to_string()
}

fn suite(status: SuiteStatus, failed: u32) -> SuiteResult {
    SuiteResult {
        suite_id: id("api"),
        status,
        tests_total: 20,
        tests_passed: 20 - failed,
        tests_failed: failed,
        failed_tests: vec![],
        command: vec!["node".into(), "run_verifier.mjs".into()],
        exit_code: Some(i32::from(failed > 0)),
        duration_ms: 10,
        output_sha256: Sha256Digest::of(b"output"),
        blocked_reason: None,
    }
}

fn verification(
    workflow_id: Uuid,
    task_key: Option<&str>,
    subject: Sha256Digest,
    contracts: Vec<Sha256Digest>,
    status: SuiteStatus,
    failed: u32,
) -> VerifierReceipt {
    VerifierReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id,
        task_key: task_key.map(id),
        contract_ids: contracts,
        subject_digest: subject,
        suites: vec![suite(status, failed)],
        verifier_files_digest: Sha256Digest::of(b"verifier"),
        tool_versions: BTreeMap::new(),
        subject_unchanged: true,
    }
}

/// Implements and collects one candidate for `task_key`, returning the
/// lease, the attempt, and the admitted candidate receipt.
fn collected(
    store: &mut SqliteStore,
    workflow_id: Uuid,
    task_key: &str,
    slot_id: &str,
    tree: &str,
    content: &str,
) -> (
    vibemux_store::LeaseRecord,
    WorkflowAttemptRecord,
    CandidateReceipt,
) {
    let workflow = current(store, workflow_id);
    let lease = lease(store, &workflow, task_key, slot_id, tree);
    let admission = attempt_admission(store, workflow_id, &lease, tree, AttemptPurpose::Implement);
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    finish_clean(store, attempt.request_id);
    let receipt = candidate_receipt(store, workflow_id, &lease, &attempt, content);
    store
        .record_workflow_receipt(
            workflow_id,
            &WorkflowReceipt::Candidate(receipt.clone()),
            fence(&lease),
            at(6),
        )
        .expect("candidate");
    store
        .change_workflow_lease(
            lease.lease_id,
            LeaseEvent::AttemptSettled {
                generation: lease.generation,
            },
            0,
            "candidate_collected",
            at(6),
        )
        .expect("release");
    (lease, attempt, receipt)
}

/// Runs an independent review attempt and records a passing review.
fn reviewed(
    store: &mut SqliteStore,
    workflow_id: Uuid,
    candidate: &CandidateReceipt,
    tree: &str,
) -> ReviewReceipt {
    let workflow = current(store, workflow_id);
    let task_key = candidate.candidate.task_key.as_str();
    let lease = lease(store, &workflow, task_key, "slot_r", tree);
    let admission = attempt_admission(store, workflow_id, &lease, tree, AttemptPurpose::Review);
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("review attempt")
        .attempt;
    finish_clean(store, attempt.request_id);
    store
        .change_workflow_lease(
            lease.lease_id,
            LeaseEvent::AttemptSettled {
                generation: lease.generation,
            },
            0,
            "review_done",
            at(7),
        )
        .expect("release reviewer");
    let review = ReviewReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id,
        task_key: candidate.candidate.task_key.clone(),
        contract_id: candidate.candidate.contract_id,
        candidate_digest: candidate.candidate.candidate_digest,
        reviewer_run_id: run_id(store, attempt.request_id),
        reviewer_session_id: attempt.session_id,
        reviewer_route: "reviewer_alias".into(),
        reviewer_harness: AgentKind::Claude,
        verdict: ReviewVerdict::Pass,
        findings_count: 0,
        findings_digest: Sha256Digest::of(b"[]"),
        candidate_unchanged: true,
    };
    store
        .record_workflow_receipt(
            workflow_id,
            &WorkflowReceipt::Review(review.clone()),
            None,
            at(7),
        )
        .expect("review");
    review
}

#[test]
fn workflow_admission_is_idempotent_and_conflicting_content_is_refused() {
    let (_file, mut store) = open_store();
    let first = record("taskboard_request_1", &["track_a", "track_b"]);
    let contracts = [contract("track_a", 1, None), contract("track_b", 1, None)];
    let commit = store
        .prepare_workflow(&first, &contracts, at(0))
        .expect("prepare");
    assert!(commit.event.is_some());
    let mut retried = first.clone();
    retried.workflow_id = Uuid::new_v4();
    retried.created_at_ms = 99;
    let repeat = store
        .prepare_workflow(&retried, &contracts, at(1))
        .expect("repeat");
    assert!(repeat.event.is_none(), "a retried admission writes nothing");
    assert_eq!(repeat.record.workflow_id, first.workflow_id);
    let mut changed = first.clone();
    changed.verifier_digest = Sha256Digest::of(b"another verifier");
    assert_eq!(
        code(
            store
                .prepare_workflow(&changed, &contracts, at(2))
                .expect_err("conflict")
        ),
        "store_workflow_request_conflict"
    );
    // A contract that does not match its task is refused.
    let wrong = [contract("track_a", 1, None), contract("track_c", 1, None)];
    let other = record("taskboard_request_2", &["track_a", "track_b"]);
    assert_eq!(
        code(
            store
                .prepare_workflow(&other, &wrong, at(3))
                .expect_err("mismatch")
        ),
        "store_workflow_contract_mismatch"
    );
}

#[test]
fn acceptance_is_unreachable_without_the_gate_and_terminal_phases_hold() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    assert_eq!(
        code(
            store
                .transition_workflow(
                    workflow.workflow_id,
                    workflow.version,
                    WorkflowPhase::Accepted,
                    None,
                    at(2)
                )
                .expect_err("gate")
        ),
        "store_workflow_acceptance_requires_gate"
    );
    assert_eq!(
        code(
            store
                .transition_workflow(
                    workflow.workflow_id,
                    workflow.version + 7,
                    WorkflowPhase::Paused,
                    None,
                    at(2)
                )
                .expect_err("stale")
        ),
        "store_workflow_version_conflict"
    );
    let cancel = store
        .transition_workflow(
            workflow.workflow_id,
            workflow.version,
            WorkflowPhase::CancelRequested,
            Some("operator"),
            at(2),
        )
        .expect("cancel request")
        .record;
    let cancelled = store
        .transition_workflow(
            workflow.workflow_id,
            cancel.version,
            WorkflowPhase::Cancelled,
            None,
            at(3),
        )
        .expect("cancelled")
        .record;
    assert_eq!(
        code(
            store
                .transition_workflow(
                    workflow.workflow_id,
                    cancelled.version,
                    WorkflowPhase::Running,
                    None,
                    at(4)
                )
                .expect_err("terminal")
        ),
        "workflow_invalid_transition"
    );
}

#[test]
fn leases_reserve_one_slot_and_one_worktree_under_increasing_generations() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let first = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    assert_eq!(first.generation, 1);
    let busy_slot = store.acquire_workflow_lease(&LeaseRequest {
        lease_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: id("track_b"),
        slot_id: id("slot_a"),
        worktree_key: worktree("tree_b"),
        heartbeat_deadline_ms: 10_000,
        timestamp: at(2),
    });
    assert_eq!(
        code(busy_slot.expect_err("slot busy")),
        "store_workflow_slot_busy"
    );
    let busy_tree = store.acquire_workflow_lease(&LeaseRequest {
        lease_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: id("track_b"),
        slot_id: id("slot_b"),
        worktree_key: worktree("tree_a"),
        heartbeat_deadline_ms: 10_000,
        timestamp: at(2),
    });
    assert_eq!(
        code(busy_tree.expect_err("worktree reserved")),
        "store_workflow_worktree_reserved"
    );
    let second = lease(&mut store, &workflow, "track_b", "slot_b", "tree_b");
    assert_eq!(second.generation, 1, "generations are per slot");
    let slots = store.workflow_slots().expect("slots");
    assert!(
        slots
            .iter()
            .all(|slot| slot.slot_id.as_str() == "slot_r" || slot.leased_generation == Some(1))
    );

    // Release, then re-lease the same slot: the generation increases.
    store
        .change_workflow_lease(
            first.lease_id,
            LeaseEvent::AttemptSettled { generation: 1 },
            0,
            "idle_release",
            at(3),
        )
        .expect("release without attempt");
    let again = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a2");
    assert_eq!(again.generation, 2);
}

#[test]
fn a_missed_heartbeat_quarantines_and_keeps_the_worktree_reserved() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    store
        .claim_harness_dispatch(attempt.request_id, at(4))
        .expect("claim");
    let quarantined = store
        .change_workflow_lease(
            held.lease_id,
            LeaseEvent::HeartbeatMissed { now_ms: 10_001 },
            0,
            "heartbeat_missed",
            at(5),
        )
        .expect("quarantine")
        .lease;
    assert_eq!(quarantined.state, LeaseState::Quarantined);
    // No second writer may enter the same worktree, even on another slot.
    let second = store.acquire_workflow_lease(&LeaseRequest {
        lease_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: id("track_a"),
        slot_id: id("slot_b"),
        worktree_key: worktree("tree_a"),
        heartbeat_deadline_ms: 10_000,
        timestamp: at(6),
    });
    assert_eq!(
        code(second.expect_err("reserved")),
        "store_workflow_worktree_reserved"
    );
    // Settlement cannot be claimed while the attempt still runs.
    assert_eq!(
        code(
            store
                .change_workflow_lease(
                    held.lease_id,
                    LeaseEvent::AttemptSettled { generation: 1 },
                    0,
                    "x",
                    at(6)
                )
                .expect_err("unsettled")
        ),
        "workflow_lease_attempt_unsettled"
    );
    // A candidate under a quarantined lease is refused.
    let mut late = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    late.expected_workflow_version = current(&store, workflow.workflow_id).version;
    assert_eq!(
        code(
            store
                .admit_workflow_attempt(&late)
                .expect_err("quarantined")
        ),
        "workflow_lease_not_active"
    );
}

#[test]
fn attempts_need_a_running_workflow_an_active_lease_and_its_worktree() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let wrong_tree = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_other",
        AttemptPurpose::Implement,
    );
    assert_eq!(
        code(store.admit_workflow_attempt(&wrong_tree).expect_err("tree")),
        "store_workflow_lease_mismatch"
    );
    let mut stale = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    stale.lease_generation = 7;
    assert_eq!(
        code(store.admit_workflow_attempt(&stale).expect_err("stale")),
        "workflow_lease_stale_generation"
    );
    let mut wrong_base = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    wrong_base.dispatch.base_commit = "ffffffffffffffffffffffffffffffffffffffff".into();
    assert_eq!(
        code(
            store
                .admit_workflow_attempt(&wrong_base)
                .expect_err("wrong base")
        ),
        "store_workflow_base_mismatch"
    );
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let commit = store.admit_workflow_attempt(&admission).expect("attempt");
    assert!(!commit.duplicate);
    assert_eq!(
        commit
            .workflow
            .task(&id("track_a"))
            .expect("task")
            .turns_used,
        1
    );
    let events = store.events().expect("events").len();
    // The same admission again: no duplicated dispatch, turn, or event.
    let repeat = store.admit_workflow_attempt(&admission).expect("repeat");
    assert!(repeat.duplicate);
    assert_eq!(store.events().expect("events").len(), events);
    assert_eq!(
        current(&store, workflow.workflow_id)
            .task(&id("track_a"))
            .expect("task")
            .turns_used,
        1
    );
    // The dispatch-owned Task/Run created for the attempt keep the
    // dispatch guard: generic projection writes are refused.
    let dispatch = store
        .harness_dispatch(admission.dispatch.request_id)
        .expect("query")
        .expect("dispatch");
    let foreign = Run::new(RunSpec {
        project_id: dispatch.project_id,
        task_id: dispatch.task_id,
        harness: "codex".into(),
        role: "worker".into(),
        protocol: "codex_exec".into(),
        base_commit: BASE.into(),
    })
    .expect("run");
    let draft = vibemux_events::EventDraft {
        event_id: vibemux_types::EventId::new(),
        event_type: vibemux_events::EventType::new("run_created").expect("type"),
        project_id: dispatch.project_id,
        task_id: Some(dispatch.task_id),
        run_id: Some(foreign.run_id()),
        causation_id: None,
        actor: vibemux_events::ActorName::new("system").expect("actor"),
        timestamp: at(9),
        idempotency_key: Some("foreign_run".into()),
        payload: vibemux_events::EventPayload::new(serde_json::json!({})).expect("payload"),
    };
    assert_eq!(
        code(store.commit_run(&foreign, draft).expect_err("guard")),
        "store_harness_dispatch_bound_projection"
    );
    // Pausing the workflow stops new admissions.
    let paused = store
        .transition_workflow(
            workflow.workflow_id,
            current(&store, workflow.workflow_id).version,
            WorkflowPhase::Paused,
            None,
            at(9),
        )
        .expect("pause")
        .record;
    let mut next = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    next.expected_workflow_version = paused.version;
    assert_eq!(
        code(store.admit_workflow_attempt(&next).expect_err("paused")),
        "store_workflow_not_running"
    );
    let _ = Task::new(CanonicalTaskSpec {
        project_id: dispatch.project_id,
        title: "unused".into(),
        description: String::new(),
    });
}

#[test]
fn candidates_are_fenced_and_bound_to_their_settled_attempt() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    let receipt = candidate_receipt(&store, workflow.workflow_id, &held, &attempt, "server v1");
    // Not settled yet.
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Candidate(receipt.clone()),
                    fence(&held),
                    at(5)
                )
                .expect_err("unsettled")
        ),
        "workflow_lease_attempt_unsettled"
    );
    finish_clean(&mut store, attempt.request_id);
    let mut wrong_base = receipt.clone();
    wrong_base.receipt_id = Uuid::new_v4();
    let manifest = wrong_base.manifest.as_mut().expect("manifest");
    manifest.base_commit = "ffffffffffffffffffffffffffffffffffffffff".into();
    wrong_base.candidate.candidate_digest = manifest.digest().expect("digest");
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Candidate(wrong_base),
                    fence(&held),
                    at(6),
                )
                .expect_err("wrong candidate base")
        ),
        "store_workflow_base_mismatch"
    );
    // A stale generation is refused.
    let stale = Some(LeaseFence {
        lease_id: held.lease_id,
        generation: held.generation + 1,
    });
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Candidate(receipt.clone()),
                    stale,
                    at(6)
                )
                .expect_err("stale")
        ),
        "workflow_lease_stale_generation"
    );
    // A digest that does not match its manifest is refused.
    let mut forged = receipt.clone();
    forged.receipt_id = Uuid::new_v4();
    forged.candidate.candidate_digest = Sha256Digest::of(b"forged");
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Candidate(forged),
                    fence(&held),
                    at(6)
                )
                .expect_err("forged")
        ),
        "store_workflow_candidate_invalid"
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Candidate(receipt.clone()),
            fence(&held),
            at(6),
        )
        .expect("candidate");
    assert_eq!(
        current(&store, workflow.workflow_id)
            .task(&id("track_a"))
            .expect("task")
            .progress,
        TaskProgress::CandidateCollected
    );
    let repeat = store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Candidate(receipt),
            fence(&held),
            at(7),
        )
        .expect("repeat");
    assert!(repeat.duplicate);
    // After release the lease can no longer report a completion.
    store
        .change_workflow_lease(
            held.lease_id,
            LeaseEvent::AttemptSettled { generation: 1 },
            0,
            "collected",
            at(7),
        )
        .expect("release");
    let late = candidate_receipt(&store, workflow.workflow_id, &held, &attempt, "server v2");
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Candidate(late),
                    fence(&held),
                    at(8)
                )
                .expect_err("released")
        ),
        "workflow_lease_ended"
    );
}

#[test]
fn a_protected_path_violation_fails_the_task_and_is_kept_as_evidence() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    finish_clean(&mut store, attempt.request_id);
    let mut rejected = candidate_receipt(&store, workflow.workflow_id, &held, &attempt, "x");
    rejected.manifest = None;
    rejected.admitted = false;
    rejected.violation_codes = vec!["snapshot_protected_path".into()];
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Candidate(rejected.clone()),
            fence(&held),
            at(6),
        )
        .expect("rejected candidate recorded");
    let task = current(&store, workflow.workflow_id)
        .task(&id("track_a"))
        .expect("task")
        .clone();
    assert_eq!(task.progress, TaskProgress::Failed);
    assert_eq!(
        task.failure_code.as_deref(),
        Some("snapshot_protected_path")
    );
    // A rejected candidate can never be accepted.
    assert_eq!(
        code(
            store
                .accept_workflow_candidate(
                    workflow.workflow_id,
                    current(&store, workflow.workflow_id).version,
                    &id("track_a"),
                    rejected.candidate.candidate_digest,
                    at(7)
                )
                .expect_err("rejected")
        ),
        "store_workflow_gate_failed"
    );
}

#[test]
fn the_gate_needs_an_independent_review_and_a_passing_verifier_on_the_same_candidate() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let (_, _, candidate) = collected(
        &mut store,
        workflow.workflow_id,
        "track_a",
        "slot_a",
        "tree_a",
        "server v1",
    );
    let digest = candidate.candidate.candidate_digest;
    let contract_id = candidate.candidate.contract_id;
    let accept = |store: &mut SqliteStore| {
        let version = current(store, workflow.workflow_id).version;
        store.accept_workflow_candidate(
            workflow.workflow_id,
            version,
            &id("track_a"),
            digest,
            at(9),
        )
    };
    assert_eq!(
        code(accept(&mut store).expect_err("no review")),
        "store_workflow_gate_failed"
    );
    // A review not bound to a review attempt of this workflow is refused.
    let mut unbound = ReviewReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        task_key: id("track_a"),
        contract_id,
        candidate_digest: digest,
        reviewer_run_id: Uuid::new_v4(),
        reviewer_session_id: Uuid::new_v4(),
        reviewer_route: "reviewer_alias".into(),
        reviewer_harness: AgentKind::Codex,
        verdict: ReviewVerdict::Pass,
        findings_count: 0,
        findings_digest: Sha256Digest::of(b"[]"),
        candidate_unchanged: true,
    };
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Review(unbound.clone()),
                    None,
                    at(8)
                )
                .expect_err("unbound")
        ),
        "store_workflow_review_unbound"
    );
    unbound.candidate_digest = Sha256Digest::of(b"unknown");
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Review(unbound),
                    None,
                    at(8)
                )
                .expect_err("unknown")
        ),
        "store_workflow_review_unknown_candidate"
    );
    reviewed(&mut store, workflow.workflow_id, &candidate, "tree_review");
    assert_eq!(
        code(accept(&mut store).expect_err("no verifier")),
        "store_workflow_gate_failed"
    );
    // A protocol-completed, reviewed, but functionally failing candidate.
    let failing = verification(
        workflow.workflow_id,
        Some("track_a"),
        digest,
        vec![contract_id],
        SuiteStatus::Failed,
        2,
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Verification(failing),
            None,
            at(9),
        )
        .expect("failing verification recorded");
    assert_eq!(
        code(accept(&mut store).expect_err("failing")),
        "store_workflow_gate_failed"
    );
    let passing = verification(
        workflow.workflow_id,
        Some("track_a"),
        digest,
        vec![contract_id],
        SuiteStatus::Passed,
        0,
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Verification(passing),
            None,
            at(10),
        )
        .expect("passing verification");
    let accepted = accept(&mut store).expect("accepted");
    let task = accepted.record.task(&id("track_a")).expect("task");
    assert_eq!(task.progress, TaskProgress::Accepted);
    assert_eq!(task.accepted_candidate, Some(digest));
}

#[test]
fn review_attempt_must_match_the_candidate_task_and_contract() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let (_, _, candidate_a) = collected(
        &mut store,
        workflow.workflow_id,
        "track_a",
        "slot_a",
        "tree_a",
        "a code",
    );
    let (_, _, candidate_b) = collected(
        &mut store,
        workflow.workflow_id,
        "track_b",
        "slot_b",
        "tree_b",
        "b code",
    );
    let review_a = reviewed(&mut store, workflow.workflow_id, &candidate_a, "review_a");
    let mut forged = review_a.clone();
    forged.receipt_id = Uuid::new_v4();
    forged.task_key = candidate_b.candidate.task_key.clone();
    forged.contract_id = candidate_b.candidate.contract_id;
    forged.candidate_digest = candidate_b.candidate.candidate_digest;
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Review(forged),
                    None,
                    at(8),
                )
                .expect_err("reviewer of another task")
        ),
        "store_workflow_review_unbound"
    );
    let mut wrong_harness = review_a.clone();
    wrong_harness.receipt_id = Uuid::new_v4();
    wrong_harness.reviewer_harness = AgentKind::Codex;
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Review(wrong_harness),
                    None,
                    at(8),
                )
                .expect_err("reviewer harness mismatch")
        ),
        "store_workflow_review_unbound"
    );
    let mut wrong_contract = review_a;
    wrong_contract.receipt_id = Uuid::new_v4();
    wrong_contract.contract_id = candidate_b.candidate.contract_id;
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Review(wrong_contract),
                    None,
                    at(8),
                )
                .expect_err("review candidate contract mismatch")
        ),
        "store_workflow_review_unknown_candidate"
    );
}

#[test]
fn review_verdicts_require_appropriate_dispatch_phase() {
    for outcome in [AttemptOutcome::Failed, AttemptOutcome::Cancelled] {
        let (_file, mut store) = open_store();
        let workflow = running_workflow(&mut store, &["track_a"]);
        let (_, _, candidate) = collected(
            &mut store,
            workflow.workflow_id,
            "track_a",
            "slot_a",
            "tree_a",
            "a code",
        );
        let latest = current(&store, workflow.workflow_id);
        let held = lease(&mut store, &latest, "track_a", "slot_r", "review_tree");
        let admission = attempt_admission(
            &store,
            workflow.workflow_id,
            &held,
            "review_tree",
            AttemptPurpose::Review,
        );
        let attempt = store
            .admit_workflow_attempt(&admission)
            .expect("review attempt")
            .attempt;
        let pass = ReviewReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            receipt_id: Uuid::new_v4(),
            workflow_id: workflow.workflow_id,
            task_key: candidate.candidate.task_key.clone(),
            contract_id: candidate.candidate.contract_id,
            candidate_digest: candidate.candidate.candidate_digest,
            reviewer_run_id: run_id(&store, attempt.request_id),
            reviewer_session_id: attempt.session_id,
            reviewer_route: "reviewer_alias".into(),
            reviewer_harness: AgentKind::Claude,
            verdict: ReviewVerdict::Pass,
            findings_count: 0,
            findings_digest: Sha256Digest::of(b"[]"),
            candidate_unchanged: true,
        };
        assert_eq!(
            code(
                store
                    .record_workflow_receipt(
                        workflow.workflow_id,
                        &WorkflowReceipt::Review(pass.clone()),
                        None,
                        at(7),
                    )
                    .expect_err("admitted review cannot pass")
            ),
            "store_workflow_review_unsettled"
        );
        let mut diagnostic = pass.clone();
        diagnostic.receipt_id = Uuid::new_v4();
        diagnostic.verdict = ReviewVerdict::ChangesRequested;
        assert_eq!(
            code(
                store
                    .record_workflow_receipt(
                        workflow.workflow_id,
                        &WorkflowReceipt::Review(diagnostic.clone()),
                        None,
                        at(7),
                    )
                    .expect_err("substantive review before dispatch completion")
            ),
            "store_workflow_review_unsettled"
        );
        let mut blocked = pass.clone();
        blocked.receipt_id = Uuid::new_v4();
        blocked.verdict = ReviewVerdict::Blocked;
        assert_eq!(
            code(
                store
                    .record_workflow_receipt(
                        workflow.workflow_id,
                        &WorkflowReceipt::Review(blocked.clone()),
                        None,
                        at(7),
                    )
                    .expect_err("blocked claim before terminal dispatch")
            ),
            "store_workflow_review_unsettled"
        );
        if outcome == AttemptOutcome::Failed {
            let claim = store
                .claim_harness_dispatch(attempt.request_id, at(8))
                .expect("claim");
            assert_eq!(
                code(
                    store
                        .record_workflow_receipt(
                            workflow.workflow_id,
                            &WorkflowReceipt::Review(pass.clone()),
                            None,
                            at(8),
                        )
                        .expect_err("running review cannot pass")
                ),
                "store_workflow_review_unsettled"
            );
            store
                .finish_harness_dispatch(&HarnessDispatchFinish {
                    request_id: attempt.request_id,
                    fence: claim.fence,
                    decision: OutcomeDecision {
                        outcome: AttemptOutcome::Failed,
                        error_code: Some(DispatchError::ProcessFailed),
                    },
                    process: ProcessSummary {
                        exit_code: Some(1),
                        forced_termination: false,
                        stderr_bytes: 0,
                    },
                    capture: CaptureSummary {
                        record_count: 1,
                        record_bytes: 32,
                        kinds: KindCounts {
                            started: 1,
                            ..KindCounts::default()
                        },
                        transcript_sha256: Sha256Digest::of(b"failed review"),
                    },
                    timestamp: at(9),
                })
                .expect("failed review");
        } else {
            store
                .cancel_harness_dispatch(attempt.request_id, at(9))
                .expect("cancel review");
        }
        assert_eq!(
            code(
                store
                    .record_workflow_receipt(
                        workflow.workflow_id,
                        &WorkflowReceipt::Review(pass),
                        None,
                        at(10),
                    )
                    .expect_err("terminal non-completed review cannot pass")
            ),
            "store_workflow_review_unsettled"
        );
        assert_eq!(
            code(
                store
                    .record_workflow_receipt(
                        workflow.workflow_id,
                        &WorkflowReceipt::Review(diagnostic),
                        None,
                        at(10),
                    )
                    .expect_err("failed or cancelled dispatch cannot request changes")
            ),
            "store_workflow_review_unsettled"
        );
        store
            .record_workflow_receipt(
                workflow.workflow_id,
                &WorkflowReceipt::Review(blocked),
                None,
                at(10),
            )
            .expect("terminal blocked diagnostic remains recordable");
    }
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let (_, _, candidate) = collected(
        &mut store,
        workflow.workflow_id,
        "track_a",
        "slot_a",
        "tree_a",
        "a code",
    );
    let mut substantive = reviewed(&mut store, workflow.workflow_id, &candidate, "review_tree");
    substantive.receipt_id = Uuid::new_v4();
    substantive.verdict = ReviewVerdict::ChangesRequested;
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Review(substantive),
            None,
            at(10),
        )
        .expect("completed substantive review remains recordable");
}

#[test]
fn workflow_acceptance_requires_integration_and_a_cancel_blocks_late_results() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let mut accepted = Vec::new();
    for (task, slot, tree) in [
        ("track_a", "slot_a", "tree_a"),
        ("track_b", "slot_b", "tree_b"),
    ] {
        let (_, _, candidate) = collected(
            &mut store,
            workflow.workflow_id,
            task,
            slot,
            tree,
            &format!("{task} code"),
        );
        reviewed(
            &mut store,
            workflow.workflow_id,
            &candidate,
            &format!("review_{task}"),
        );
        let digest = candidate.candidate.candidate_digest;
        let verification = verification(
            workflow.workflow_id,
            Some(task),
            digest,
            vec![candidate.candidate.contract_id],
            SuiteStatus::Passed,
            0,
        );
        store
            .record_workflow_receipt(
                workflow.workflow_id,
                &WorkflowReceipt::Verification(verification),
                None,
                at(10),
            )
            .expect("verify");
        let version = current(&store, workflow.workflow_id).version;
        store
            .accept_workflow_candidate(workflow.workflow_id, version, &id(task), digest, at(11))
            .expect("accept");
        accepted.push(candidate.candidate);
    }
    let version = current(&store, workflow.workflow_id).version;
    assert_eq!(
        code(
            store
                .accept_workflow(workflow.workflow_id, version, at(12))
                .expect_err("running")
        ),
        "store_workflow_gate_failed"
    );
    let integrating = store
        .transition_workflow(
            workflow.workflow_id,
            version,
            WorkflowPhase::Integrating,
            None,
            at(12),
        )
        .expect("integrate")
        .record;
    assert_eq!(
        code(
            store
                .accept_workflow(workflow.workflow_id, integrating.version, at(12))
                .expect_err("no integration")
        ),
        "store_workflow_gate_failed"
    );
    // An integration naming an unapproved candidate is refused.
    let mut integration = IntegrationReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        base_commit: BASE.into(),
        applied_candidates: vec![
            accepted[0].candidate_digest,
            Sha256Digest::of(b"unapproved"),
        ],
        contract_ids: accepted
            .iter()
            .map(|candidate| candidate.contract_id)
            .collect(),
        integration_tree_digest: Sha256Digest::of(b"integration tree"),
        patch_sha256: Sha256Digest::of(b"patch"),
        conflicts: vec![],
    };
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Integration(integration.clone()),
                    None,
                    at(13)
                )
                .expect_err("unapproved")
        ),
        "store_workflow_integration_not_approved"
    );
    integration.applied_candidates = accepted
        .iter()
        .map(|candidate| candidate.candidate_digest)
        .collect();
    let mut wrong_base = integration.clone();
    wrong_base.base_commit = "ffffffffffffffffffffffffffffffffffffffff".into();
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Integration(wrong_base),
                    None,
                    at(13),
                )
                .expect_err("wrong integration base")
        ),
        "store_workflow_base_mismatch"
    );
    let mut missing_contract = integration.clone();
    missing_contract.contract_ids.pop();
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Integration(missing_contract),
                    None,
                    at(13),
                )
                .expect_err("missing accepted contract")
        ),
        "store_workflow_integration_not_approved"
    );
    let mut extra_contract = integration.clone();
    extra_contract
        .contract_ids
        .push(Sha256Digest::of(b"foreign contract"));
    assert_eq!(
        code(
            store
                .record_workflow_receipt(
                    workflow.workflow_id,
                    &WorkflowReceipt::Integration(extra_contract),
                    None,
                    at(13),
                )
                .expect_err("extra contract")
        ),
        "store_workflow_integration_not_approved"
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Integration(integration.clone()),
            None,
            at(13),
        )
        .expect("integration");
    let tree_verification = verification(
        workflow.workflow_id,
        None,
        integration.integration_tree_digest,
        accepted
            .iter()
            .map(|candidate| candidate.contract_id)
            .collect(),
        SuiteStatus::Passed,
        0,
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Verification(tree_verification),
            None,
            at(14),
        )
        .expect("integration verified");

    // A cancel requested now must block the otherwise valid acceptance.
    let cancel = store
        .transition_workflow(
            workflow.workflow_id,
            integrating.version,
            WorkflowPhase::CancelRequested,
            Some("operator"),
            at(15),
        )
        .expect("cancel");
    assert_eq!(
        code(
            store
                .accept_workflow(workflow.workflow_id, cancel.record.version, at(16))
                .expect_err("cancelled")
        ),
        "store_workflow_gate_failed"
    );
    let cancelled = store
        .transition_workflow(
            workflow.workflow_id,
            cancel.record.version,
            WorkflowPhase::Cancelled,
            None,
            at(16),
        )
        .expect("cancelled");
    assert_eq!(cancelled.record.phase, WorkflowPhase::Cancelled);
}

#[test]
fn a_fully_verified_cooperative_workflow_is_accepted() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let mut accepted = Vec::new();
    for (task, slot, tree) in [
        ("track_a", "slot_a", "tree_a"),
        ("track_b", "slot_b", "tree_b"),
    ] {
        let (_, _, candidate) = collected(
            &mut store,
            workflow.workflow_id,
            task,
            slot,
            tree,
            &format!("{task} code"),
        );
        reviewed(
            &mut store,
            workflow.workflow_id,
            &candidate,
            &format!("review_{task}"),
        );
        let digest = candidate.candidate.candidate_digest;
        let verification = verification(
            workflow.workflow_id,
            Some(task),
            digest,
            vec![candidate.candidate.contract_id],
            SuiteStatus::Passed,
            0,
        );
        store
            .record_workflow_receipt(
                workflow.workflow_id,
                &WorkflowReceipt::Verification(verification),
                None,
                at(10),
            )
            .expect("verify");
        let version = current(&store, workflow.workflow_id).version;
        store
            .accept_workflow_candidate(workflow.workflow_id, version, &id(task), digest, at(11))
            .expect("accept");
        accepted.push(candidate.candidate);
    }
    let version = current(&store, workflow.workflow_id).version;
    let integrating = store
        .transition_workflow(
            workflow.workflow_id,
            version,
            WorkflowPhase::Integrating,
            None,
            at(12),
        )
        .expect("integrate")
        .record;
    let integration = IntegrationReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: workflow.workflow_id,
        base_commit: BASE.into(),
        applied_candidates: accepted
            .iter()
            .map(|candidate| candidate.candidate_digest)
            .collect(),
        contract_ids: accepted
            .iter()
            .map(|candidate| candidate.contract_id)
            .collect(),
        integration_tree_digest: Sha256Digest::of(b"integration tree"),
        patch_sha256: Sha256Digest::of(b"patch"),
        conflicts: vec![],
    };
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Integration(integration.clone()),
            None,
            at(13),
        )
        .expect("integration");
    let contracts: Vec<Sha256Digest> = accepted
        .iter()
        .map(|candidate| candidate.contract_id)
        .collect();
    let tree = verification(
        workflow.workflow_id,
        None,
        integration.integration_tree_digest,
        contracts,
        SuiteStatus::Passed,
        0,
    );
    store
        .record_workflow_receipt(
            workflow.workflow_id,
            &WorkflowReceipt::Verification(tree),
            None,
            at(14),
        )
        .expect("verified");
    let done = store
        .accept_workflow(workflow.workflow_id, integrating.version, at(15))
        .expect("accepted");
    assert_eq!(done.record.phase, WorkflowPhase::Accepted);
    let snapshot = store
        .workflow_snapshot(workflow.workflow_id)
        .expect("snapshot")
        .expect("workflow");
    assert_eq!(
        snapshot.attempts.len(),
        4,
        "two implementation and two review attempts"
    );
    assert!(
        snapshot
            .leases
            .iter()
            .all(|lease| !lease.state.holds_reservation())
    );
}

#[test]
fn contract_generations_never_rebind_admitted_attempts() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let old_contract = admission.contract_id;
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    let version = current(&store, workflow.workflow_id).version;
    // Not a successor: same version.
    let mut bogus = contract("track_a", 1, Some(old_contract));
    bogus.contract_id = Sha256Digest::of(b"bogus");
    assert_eq!(
        code(
            store
                .add_contract_generation(workflow.workflow_id, version, &bogus, at(5))
                .expect_err("bogus")
        ),
        "store_workflow_invalid_generation"
    );
    let next = contract("track_a", 2, Some(old_contract));
    let updated = store
        .add_contract_generation(workflow.workflow_id, version, &next, at(5))
        .expect("generation")
        .record;
    let task = updated.task(&id("track_a")).expect("task");
    assert_eq!(task.contract_id, next.contract_id);
    assert_eq!(task.contract_version, 2);
    // The admitted attempt keeps its original contract.
    assert_eq!(
        store
            .workflow_attempt(attempt.request_id)
            .expect("query")
            .expect("attempt")
            .contract_id,
        old_contract
    );
    // A new attempt under the old contract is refused.
    finish_clean(&mut store, attempt.request_id);
    store
        .change_workflow_lease(
            held.lease_id,
            LeaseEvent::AttemptSettled { generation: 1 },
            0,
            "superseded",
            at(6),
        )
        .expect("release");
    let again = lease(&mut store, &updated, "track_a", "slot_a", "tree_a2");
    let mut old = attempt_admission(
        &store,
        workflow.workflow_id,
        &again,
        "tree_a2",
        AttemptPurpose::Implement,
    );
    old.contract_id = old_contract;
    assert_eq!(
        code(
            store
                .admit_workflow_attempt(&old)
                .expect_err("stale contract")
        ),
        "store_workflow_stale_contract"
    );
    let snapshot = store
        .workflow_snapshot(workflow.workflow_id)
        .expect("snapshot")
        .expect("workflow");
    assert_eq!(snapshot.contracts.len(), 2, "both generations stay visible");
}

fn envelope(workflow_id: Uuid, message_id: Uuid, to: Participant) -> MessageEnvelope {
    MessageEnvelope {
        message_id,
        workflow_id,
        sender: Participant::Worker {
            task_key: id("track_b"),
            session_id: Uuid::from_u128(2),
        },
        recipient: to.clone(),
        kind: MessageKind::Question,
        correlation_id: message_id,
        reply_to: None,
        depth: 0,
        contract_id: contract("track_a", 1, None).contract_id,
        contract_version: 1,
        source_refs: vec![],
        body_sha256: Sha256Digest::of(b"body"),
        body_bytes: 4,
        created_at_ms: at(1).unix_timestamp() as u64 * 1000,
        expires_at_ms: u64::MAX,
        audience: vec![to, Participant::Supervisor],
        escalated: false,
    }
}

#[test]
fn messages_are_sequenced_deduplicated_and_survive_reopen() {
    let (file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "message_tree");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "message_tree",
        AttemptPurpose::Implement,
    );
    let attempt = admission.dispatch.request_id;
    let recipient_session = admission.session_id;
    store
        .admit_workflow_attempt(&admission)
        .expect("recipient attempt");
    let recipient = Participant::Worker {
        task_key: id("track_a"),
        session_id: recipient_session,
    };
    let first = store
        .admit_workflow_message(
            &envelope(workflow.workflow_id, Uuid::from_u128(11), recipient.clone()),
            at(2),
        )
        .expect("first");
    let mut expiring = envelope(workflow.workflow_id, Uuid::from_u128(12), recipient.clone());
    expiring.expires_at_ms = at(5).unix_timestamp() as u64 * 1000;
    let second = store
        .admit_workflow_message(&expiring, at(3))
        .expect("second");
    assert_eq!(
        (
            first.message.recipient_sequence,
            second.message.recipient_sequence
        ),
        (1, 2)
    );
    let repeat = store
        .admit_workflow_message(
            &envelope(workflow.workflow_id, Uuid::from_u128(11), recipient.clone()),
            at(9),
        )
        .expect("repeat");
    assert!(repeat.event.is_none(), "a redelivered admission is a no-op");
    let mut changed = envelope(workflow.workflow_id, Uuid::from_u128(11), recipient.clone());
    changed.body_sha256 = Sha256Digest::of(b"other body");
    assert_eq!(
        code(
            store
                .admit_workflow_message(&changed, at(4))
                .expect_err("conflict")
        ),
        "store_workflow_message_conflict"
    );
    let intruder = Participant::Worker {
        task_key: id("track_b"),
        session_id: Uuid::from_u128(2),
    };
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Acknowledge {
                        by: recipient.clone(),
                        attempt
                    },
                    at(5)
                )
                .expect_err("undelivered")
        ),
        "message_not_delivered"
    );
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Deliver {
                        attempt: Uuid::new_v4()
                    },
                    at(5),
                )
                .expect_err("unknown attempt")
        ),
        "store_workflow_unknown_attempt"
    );
    let latest = current(&store, workflow.workflow_id);
    let other_lease = lease(&mut store, &latest, "track_b", "slot_b", "message_tree_b");
    let other_admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &other_lease,
        "message_tree_b",
        AttemptPurpose::Implement,
    );
    store
        .admit_workflow_attempt(&other_admission)
        .expect("other task attempt");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Deliver {
                        attempt: other_admission.dispatch.request_id
                    },
                    at(5),
                )
                .expect_err("other task")
        ),
        "store_workflow_message_attempt_mismatch"
    );
    let events_before = store.events().expect("events").len();
    assert_eq!(
        code(
            store
                .deliver_workflow_messages(
                    workflow.workflow_id,
                    &[Uuid::from_u128(11), Uuid::from_u128(12)],
                    attempt,
                    at(5),
                )
                .expect_err("second message expired")
        ),
        "message_expired"
    );
    assert_eq!(store.events().expect("events").len(), events_before);
    assert_eq!(
        store
            .workflow_message(Uuid::from_u128(11))
            .expect("query")
            .expect("first")
            .state,
        MessageState::Admitted,
    );
    assert_eq!(
        store.deliver_workflow_messages(
            workflow.workflow_id,
            &[Uuid::from_u128(11)],
            attempt,
            at(5),
        ).expect("deliver batch").len(),
        1,
    );
    assert!(
        store
            .change_workflow_message(
                Uuid::from_u128(11),
                &MessageChange::Deliver { attempt },
                at(6)
            )
            .expect("redeliver")
            .event
            .is_none()
    );
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Deliver {
                        attempt: Uuid::from_u128(501)
                    },
                    at(6)
                )
                .expect_err("other attempt")
        ),
        "store_workflow_unknown_attempt"
    );
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Acknowledge {
                        by: intruder,
                        attempt
                    },
                    at(6)
                )
                .expect_err("intruder")
        ),
        "message_ack_not_recipient"
    );
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Acknowledge {
                        by: recipient.clone(),
                        attempt
                    },
                    at(6),
                )
                .expect_err("unsettled attempt")
        ),
        "store_workflow_message_attempt_unsettled"
    );
    finish_clean(&mut store, attempt);
    let mut second_attempt = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "message_tree",
        AttemptPurpose::Answer,
    );
    second_attempt.session_id = recipient_session;
    let other_attempt = second_attempt.dispatch.request_id;
    store
        .admit_workflow_attempt(&second_attempt)
        .expect("second attempt");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Deliver {
                        attempt: other_attempt
                    },
                    at(6),
                )
                .expect_err("real other attempt")
        ),
        "store_workflow_message_redelivery"
    );
    finish_clean(&mut store, other_attempt);
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    Uuid::from_u128(11),
                    &MessageChange::Acknowledge {
                        by: recipient.clone(),
                        attempt: other_attempt
                    },
                    at(6),
                )
                .expect_err("other delivered attempt")
        ),
        "store_workflow_message_attempt_mismatch"
    );
    store
        .change_workflow_message(
            Uuid::from_u128(11),
            &MessageChange::Acknowledge {
                by: recipient,
                attempt,
            },
            at(7),
        )
        .expect("ack");
    store
        .record_workflow_message_rejection(
            workflow.workflow_id,
            Uuid::from_u128(13),
            &Participant::Supervisor,
            "message_forged_sender",
            at(8),
        )
        .expect("rejection evidence");
    drop(store);
    let reopened = SqliteStore::open(file.path()).expect("reopen");
    let message = reopened
        .workflow_message(Uuid::from_u128(11))
        .expect("query")
        .expect("message");
    assert_eq!(message.state, MessageState::Acknowledged);
    assert_eq!(message.acknowledged_in_attempt, Some(attempt));
    let pending = reopened
        .workflow_message(Uuid::from_u128(12))
        .expect("query")
        .expect("message");
    assert_eq!(
        pending.state,
        MessageState::Admitted,
        "undelivered messages persist across restart"
    );
}

#[test]
fn message_delivery_rejects_foreign_workflow_session_and_contract() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let held = lease(&mut store, &workflow, "track_a", "slot_a", "message_tree");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &held,
        "message_tree",
        AttemptPurpose::Implement,
    );
    store.admit_workflow_attempt(&admission).expect("attempt");
    let recipient = Participant::Worker {
        task_key: id("track_a"),
        session_id: admission.session_id,
    };
    let foreign = running_workflow(&mut store, &["track_a"]);
    let foreign_lease = lease(&mut store, &foreign, "track_a", "slot_b", "foreign_tree");
    let foreign_admission = attempt_admission(
        &store,
        foreign.workflow_id,
        &foreign_lease,
        "foreign_tree",
        AttemptPurpose::Implement,
    );
    store
        .admit_workflow_attempt(&foreign_admission)
        .expect("foreign attempt");
    let message_id = Uuid::new_v4();
    store
        .admit_workflow_message(
            &envelope(workflow.workflow_id, message_id, recipient.clone()),
            at(3),
        )
        .expect("message");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    message_id,
                    &MessageChange::Deliver {
                        attempt: foreign_admission.dispatch.request_id
                    },
                    at(4),
                )
                .expect_err("foreign workflow")
        ),
        "store_workflow_message_attempt_mismatch"
    );
    let mut wrong_session = envelope(workflow.workflow_id, Uuid::new_v4(), recipient.clone());
    wrong_session.recipient = Participant::Worker {
        task_key: id("track_a"),
        session_id: Uuid::new_v4(),
    };
    store
        .admit_workflow_message(&wrong_session, at(3))
        .expect("message");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    wrong_session.message_id,
                    &MessageChange::Deliver {
                        attempt: admission.dispatch.request_id
                    },
                    at(4),
                )
                .expect_err("wrong session")
        ),
        "store_workflow_message_attempt_mismatch"
    );
    let mut wrong_contract = envelope(workflow.workflow_id, Uuid::new_v4(), recipient);
    wrong_contract.contract_id = Sha256Digest::of(b"foreign contract");
    store
        .admit_workflow_message(&wrong_contract, at(3))
        .expect("message");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    wrong_contract.message_id,
                    &MessageChange::Deliver {
                        attempt: admission.dispatch.request_id
                    },
                    at(4),
                )
                .expect_err("wrong contract")
        ),
        "store_workflow_message_attempt_mismatch"
    );
}

#[test]
fn supervisor_inbox_uses_only_its_deterministic_message_identity() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let message_id = Uuid::new_v4();
    store
        .admit_workflow_message(
            &envelope(workflow.workflow_id, message_id, Participant::Supervisor),
            at(2),
        )
        .expect("supervisor message");
    assert_eq!(
        code(
            store
                .change_workflow_message(
                    message_id,
                    &MessageChange::Deliver {
                        attempt: Uuid::new_v4()
                    },
                    at(3),
                )
                .expect_err("arbitrary synthetic id")
        ),
        "store_workflow_message_attempt_mismatch"
    );
    let digest = Sha256Digest::of_fields(
        "vibemux.workflow.supervisor_inbox.v1",
        &[message_id.as_bytes()],
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    let inbox_id = uuid::Builder::from_random_bytes(bytes).into_uuid();
    store
        .change_workflow_message(
            message_id,
            &MessageChange::Deliver { attempt: inbox_id },
            at(3),
        )
        .expect("supervisor delivery");
    store
        .change_workflow_message(
            message_id,
            &MessageChange::Acknowledge {
                by: Participant::Supervisor,
                attempt: inbox_id,
            },
            at(3),
        )
        .expect("supervisor acknowledgement");
}

#[test]
fn workflow_keyset_pages_reach_records_beyond_the_latest_snapshot() {
    let (_file, mut store) = open_store();
    let contracts = [contract("track_a", 1, None)];
    let mut expected = Vec::new();
    for index in 0..270 {
        let workflow = record(&format!("paged_request_{index}"), &["track_a"]);
        expected.push(workflow.workflow_id);
        store
            .prepare_workflow(&workflow, &contracts, at(0))
            .expect("prepare");
    }
    assert_eq!(store.workflows(256).expect("latest page").len(), 256);
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = store.list_workflows_after(after, 37).expect("keyset page");
        if page.is_empty() {
            break;
        }
        after = page.last().map(|record| record.workflow_id);
        seen.extend(page.into_iter().map(|record| record.workflow_id));
    }
    expected.sort();
    assert_eq!(seen, expected);
    assert!(
        store
            .list_workflows_after(after, 37)
            .expect("end page")
            .is_empty()
    );
}

#[test]
fn restart_quarantines_running_attempts_and_releases_idle_leases() {
    let (file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a", "track_b"]);
    let running = lease(&mut store, &workflow, "track_a", "slot_a", "tree_a");
    let admission = attempt_admission(
        &store,
        workflow.workflow_id,
        &running,
        "tree_a",
        AttemptPurpose::Implement,
    );
    let attempt = store
        .admit_workflow_attempt(&admission)
        .expect("attempt")
        .attempt;
    store
        .claim_harness_dispatch(attempt.request_id, at(4))
        .expect("claim");
    let workflow_now = current(&store, workflow.workflow_id);
    let idle = lease(&mut store, &workflow_now, "track_b", "slot_b", "tree_b");
    drop(store);
    let mut reopened = SqliteStore::open(file.path()).expect("reopen");
    reopened
        .recover_harness_dispatches(at(20))
        .expect("dispatch recovery");
    let recovery = reopened
        .recover_workflows(at(20))
        .expect("workflow recovery");
    assert_eq!(recovery.quarantined_leases, vec![running.lease_id]);
    assert_eq!(recovery.released_leases, vec![idle.lease_id]);
    assert_eq!(
        reopened
            .workflow_lease(running.lease_id)
            .expect("query")
            .expect("lease")
            .state,
        LeaseState::Quarantined
    );
    // Recovery is idempotent.
    let again = reopened.recover_workflows(at(21)).expect("again");
    assert!(again.quarantined_leases.is_empty() && again.released_leases.is_empty());
    // Reconciliation cannot release the worktree while the interrupted
    // attempt is unacknowledged: its process may still be running.
    let reconcile = |store: &mut SqliteStore| {
        store.change_workflow_lease(
            running.lease_id,
            LeaseEvent::Reconciled {
                generation: 1,
                process_gone: true,
            },
            0,
            "operator_reconciled",
            at(22),
        )
    };
    assert_eq!(
        code(reconcile(&mut reopened).expect_err("unsettled")),
        "workflow_lease_attempt_unsettled"
    );
    reopened
        .cancel_harness_dispatch(attempt.request_id, at(23))
        .expect("acknowledge recovery");
    let released = reconcile(&mut reopened).expect("reconciled").lease;
    assert_eq!(released.state, LeaseState::Released);
    assert_eq!(released.end_reason.as_deref(), Some("operator_reconciled"));
}

#[test]
fn content_index_records_and_tombstones_blobs_without_content() {
    let (_file, mut store) = open_store();
    let workflow = running_workflow(&mut store, &["track_a"]);
    let digest = Sha256Digest::of(b"rendered prompt bytes");
    let entry = ContentEntry {
        content_sha256: digest,
        workflow_id: Some(workflow.workflow_id),
        kind: ContentKind::RenderedPrompt,
        byte_count: 21,
        state: ContentState::Present,
        retain_until_ms: 1_000,
        updated_at_ms: 1,
    };
    assert!(
        store
            .record_workflow_content(&entry, at(2))
            .expect("record")
            .is_some()
    );
    assert!(
        store
            .record_workflow_content(&entry, at(3))
            .expect("repeat")
            .is_none()
    );
    let deleted = store
        .delete_workflow_content(digest, "retention_expired", at(4))
        .expect("delete")
        .expect("entry");
    assert_eq!(deleted.state, ContentState::Deleted);
    for event in store.events().expect("events") {
        let text = serde_json::to_string(&event).expect("json");
        assert!(!text.contains("rendered prompt bytes"));
    }
}

#[test]
fn policy_versions_promote_for_future_admissions_and_roll_back() {
    let (_file, mut store) = open_store();
    let baseline = OptimizablePolicy::baseline();
    let first = store
        .promote_prompt_policy(&baseline, at(1))
        .expect("baseline");
    assert_eq!(first.version.version, 1);
    let mut candidate = OptimizablePolicy::baseline();
    candidate.summary_budget_bytes = 4096;
    let second = store
        .promote_prompt_policy(&candidate, at(2))
        .expect("candidate");
    assert_eq!(second.version.parent, Some(1));
    let restored = store.rollback_prompt_policy(at(3)).expect("rollback");
    assert_eq!(restored.version.version, 1);
    assert_eq!(restored.policy, baseline);
    let versions = store.prompt_policy_versions().expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(
        versions
            .iter()
            .filter(
                |record| record.version.state == vibemux_workflow::optimizer::PolicyState::Active
            )
            .count(),
        1
    );
}
