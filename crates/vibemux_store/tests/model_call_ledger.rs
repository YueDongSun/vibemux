//! Durable model-call reservations, owner budgets, and restart fencing.

use std::path::Path;

use rusqlite::Connection;
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{STORE_SCHEMA_VERSION, SqliteStore};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::{ContractArtifact, RouteTransformation},
    gates::WorkflowPhase,
    gateway::{
        CallOutcome, GatewayProtocol, ModelCallReceipt, ProviderAttribution, ReportedUsage,
        RouteRole,
    },
    model_call::{ModelCallAdmission, ModelCallRequest, ModelCallState},
    slots::EvidenceClass,
    supervisor::LoopBudget,
    task_spec::WorkflowMode,
    workflow_record::{ContentStoreMode, TaskProgress, WorkflowRecord, WorkflowTask},
};

const CALL_OWNER: &str = "compiler_budget";
const CALL_ALIAS: &str = "compiler_alias";
const MAX_CALLS: u32 = 2;
const DEADLINE_WINDOW_MS: u64 = 60_000;

#[test]
fn restart_recovery_crosses_pages_without_refunding_or_replaying_calls() {
    let database = NamedTempFile::new().expect("owned ledger database");
    let mut store = open_store(database.path());
    const CALL_COUNT: u32 = 513;
    for index in 1..=CALL_COUNT {
        let call = admission(
            Uuid::from_u128(u128::from(index)),
            &format!("paged_recovery_{index}"),
            "bounded recovery fixture",
            1,
        );
        store.reserve_model_call(&call).expect("reservation");
        store
            .submit_model_call(call.call_id)
            .expect("submitted before effect");
    }
    assert_eq!(
        store.recover_model_calls().expect("all pages recovered"),
        CALL_COUNT
    );
    assert_eq!(store.recover_model_calls().expect("idempotent recovery"), 0);
    for index in [1, 512, 513] {
        let call_id = Uuid::from_u128(index);
        let record = store
            .model_call(call_id)
            .expect("read")
            .expect("retained charge");
        assert_eq!(record.state, ModelCallState::Unknown);
        assert_eq!(record.calls_used, 1);
        assert!(store.submit_model_call(call_id).is_err());
    }
}

fn owner_key(value: &str) -> SpecIdentifier {
    SpecIdentifier::new(value).expect("test owner key")
}

fn unix_time_ms() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("current time fits in u64 milliseconds")
}

fn request(user_prompt: &str) -> ModelCallRequest {
    ModelCallRequest {
        role: RouteRole::Compiler,
        requested_alias: CALL_ALIAS.to_string(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 1024,
        timeout_ms: 10_000,
        config_sha256: Sha256Digest::of(b"model call ledger test config"),
        observation_digest: None,
        system_prompt: "Compile the admitted request into typed task candidates.".to_string(),
        user_prompt: user_prompt.to_string(),
    }
}

fn admission(call_id: Uuid, owner: &str, user_prompt: &str, max_calls: u32) -> ModelCallAdmission {
    admission_with_deadline(
        call_id,
        owner,
        user_prompt,
        max_calls,
        unix_time_ms().saturating_add(DEADLINE_WINDOW_MS),
    )
}

fn admission_with_deadline(
    call_id: Uuid,
    owner: &str,
    user_prompt: &str,
    max_calls: u32,
    deadline_unix_ms: u64,
) -> ModelCallAdmission {
    let request = request(user_prompt);
    ModelCallAdmission {
        call_id,
        owner_key: owner_key(owner),
        workflow_id: None,
        identity: request.identity().expect("content-free request identity"),
        deadline_unix_ms,
        max_calls,
    }
}

fn receipt(admission: &ModelCallAdmission) -> ModelCallReceipt {
    ModelCallReceipt {
        call_id: admission.call_id,
        role: admission.identity.role,
        requested_alias: admission.identity.requested_alias.clone(),
        protocol: admission.identity.protocol,
        reported_model: None,
        effective_provider: ProviderAttribution::Unknown,
        usage: ReportedUsage::default(),
        http_status: Some(200),
        outcome: CallOutcome::Completed,
        failure: None,
        elapsed_ms: 25,
        request_sha256: admission.identity.request_sha256,
        response_sha256: Some(Sha256Digest::of(b"synthetic response bytes")),
        // The request had no prior route observation, so the receipt stores
        // the actual observation made at call time without retroactively
        // substituting it into admission identity.
        route_generation: Sha256Digest::of(b"observed route at call time"),
        attempt: 1,
    }
}

fn open_store(path: &Path) -> SqliteStore {
    SqliteStore::open(path).expect("open migrated store")
}

fn schema_version(path: &Path) -> u32 {
    Connection::open(path)
        .expect("open owned closed database")
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("schema version")
}

#[test]
fn reservation_identity_and_owner_budgets_are_idempotent_and_isolated() {
    let database = NamedTempFile::new().expect("owned ledger database");
    let mut store = open_store(database.path());
    let deadline = unix_time_ms().saturating_add(DEADLINE_WINDOW_MS);
    let first = admission_with_deadline(
        Uuid::from_u128(0x101),
        CALL_OWNER,
        "request one",
        MAX_CALLS,
        deadline,
    );

    let first_record = store
        .reserve_model_call(&first)
        .expect("reserve first call");
    assert_eq!(first_record.state, ModelCallState::Reserved);
    assert_eq!(first_record.calls_used, 1);
    assert!(
        !serde_json::to_string(&first_record)
            .expect("serialize content-free model-call record")
            .contains("request one")
    );
    assert_eq!(
        store
            .reserve_model_call(&first)
            .expect("idempotent reserve"),
        first_record,
        "same call ID and full admission must return the stored reservation without a second charge"
    );

    let mut changed_call = first.clone();
    changed_call.identity.request_fingerprint = Sha256Digest::of(b"different semantic payload");
    assert_eq!(
        store
            .reserve_model_call(&changed_call)
            .expect_err("same call ID cannot change payload identity")
            .code(),
        "model_call_identity_mismatch"
    );

    for changed_budget in [
        admission_with_deadline(
            Uuid::from_u128(0x102),
            CALL_OWNER,
            "budget cap change",
            MAX_CALLS + 1,
            deadline,
        ),
        admission_with_deadline(
            Uuid::from_u128(0x103),
            CALL_OWNER,
            "deadline change",
            MAX_CALLS,
            deadline + 1,
        ),
    ] {
        assert_eq!(
            store
                .reserve_model_call(&changed_budget)
                .expect_err("owner budget identity is fixed on first reservation")
                .code(),
            "model_call_budget_identity_mismatch"
        );
    }

    let second = admission_with_deadline(
        Uuid::from_u128(0x104),
        CALL_OWNER,
        "request two with a separate stable call ID",
        MAX_CALLS,
        deadline,
    );
    let second_record = store
        .reserve_model_call(&second)
        .expect("reserve second call");
    assert_eq!(second_record.calls_used, 2);
    assert_eq!(
        store
            .reserve_model_call(&admission_with_deadline(
                Uuid::from_u128(0x105),
                CALL_OWNER,
                "request three exceeds the owner cap",
                MAX_CALLS,
                deadline,
            ))
            .expect_err("owner maximum is enforced")
            .code(),
        "model_call_limit_exceeded"
    );

    let independent = admission_with_deadline(
        Uuid::from_u128(0x106),
        "independent_owner",
        "independent owner request",
        1,
        deadline,
    );
    let independent_record = store
        .reserve_model_call(&independent)
        .expect("independent owner has its own budget");
    assert_eq!(independent_record.calls_used, 1);
    drop(store);

    let store = open_store(database.path());
    assert_eq!(
        store
            .model_call(first.call_id)
            .expect("load first reservation")
            .expect("first persisted call"),
        first_record
    );
    assert_eq!(schema_version(database.path()), STORE_SCHEMA_VERSION);
}

#[test]
fn submitted_calls_recover_to_unknown_and_cannot_be_resubmitted() {
    let database = NamedTempFile::new().expect("owned ledger database");
    let call = admission(
        Uuid::from_u128(0x201),
        "recovery_owner",
        "submitted request with no terminal evidence",
        2,
    );
    {
        let mut store = open_store(database.path());
        let reserved = store.reserve_model_call(&call).expect("reserve call");
        assert_eq!(reserved.state, ModelCallState::Reserved);
        let submitted = store
            .submit_model_call(call.call_id)
            .expect("persist submitted before an external effect");
        assert_eq!(submitted.state, ModelCallState::Submitted);
        assert_eq!(submitted.calls_used, 1);
    }

    let mut reopened = open_store(database.path());
    assert_eq!(
        reopened
            .recover_model_calls()
            .expect("recover pending calls"),
        1
    );
    let recovered = reopened
        .model_call(call.call_id)
        .expect("load recovered call")
        .expect("call remains recorded");
    assert_eq!(recovered.state, ModelCallState::Unknown);
    assert_eq!(
        recovered.calls_used, 1,
        "recovery does not refund the owner budget"
    );
    assert_eq!(
        reopened
            .submit_model_call(call.call_id)
            .expect_err("unknown submitted work must never be replayed")
            .code(),
        "model_call_not_reservable"
    );
    assert_eq!(
        reopened.recover_model_calls().expect("idempotent recovery"),
        0
    );
}

#[test]
fn preflight_block_stays_blocked_and_completed_receipt_persists_without_usage_claims() {
    let database = NamedTempFile::new().expect("owned ledger database");
    let mut store = open_store(database.path());
    let blocked = admission(
        Uuid::from_u128(0x301),
        "blocked_owner",
        "refused before submission",
        1,
    );
    store
        .reserve_model_call(&blocked)
        .expect("reserve preflight call");
    let blocked_record = store
        .finish_model_call(
            blocked.call_id,
            None,
            Some("gateway_live_attestation_unavailable".to_string()),
        )
        .expect("record preflight block");
    assert_eq!(blocked_record.state, ModelCallState::Blocked);
    assert_eq!(
        store
            .submit_model_call(blocked.call_id)
            .expect_err("blocked reservation is not submitted")
            .code(),
        "model_call_not_reservable"
    );

    let completed = admission(
        Uuid::from_u128(0x302),
        "completed_owner",
        "call completed with unknown usage",
        1,
    );
    store
        .reserve_model_call(&completed)
        .expect("reserve completed call");
    store
        .submit_model_call(completed.call_id)
        .expect("record submitted state");
    let completed_record = store
        .finish_model_call(completed.call_id, Some(receipt(&completed)), None)
        .expect("persist completed receipt");
    assert_eq!(completed_record.state, ModelCallState::Completed);
    assert!(
        completed_record
            .receipt
            .as_ref()
            .expect("receipt")
            .usage
            .is_unknown()
    );
    drop(store);

    let reopened = open_store(database.path());
    let blocked_after_reopen = reopened
        .model_call(blocked.call_id)
        .expect("load blocked call")
        .expect("blocked call persisted");
    assert_eq!(blocked_after_reopen.state, ModelCallState::Blocked);
    let completed_after_reopen = reopened
        .model_call(completed.call_id)
        .expect("load completed call")
        .expect("completed call persisted");
    assert_eq!(completed_after_reopen, completed_record);
    assert!(
        completed_after_reopen
            .receipt
            .expect("receipt")
            .usage
            .is_unknown()
    );
}

#[test]
fn a_v5_marker_without_ledger_tables_migrates_forward_when_reopened() {
    let database = NamedTempFile::new().expect("owned ledger database");
    drop(open_store(database.path()));

    // Model the old schema only while the store is closed. The v6 objects are
    // removed with no rows, then the v6 marker is deleted to represent v5.
    let connection = Connection::open(database.path()).expect("open closed v5 fixture");
    connection
        .execute_batch(
            "DROP TABLE workflow_supervisor_actions; \
             DROP TABLE workflow_supervisor_rounds; \
             DROP TABLE workflow_supervisors; \
             DROP TABLE workflow_controller_outbox; \
             DROP TABLE workflow_controllers; \
             DROP TABLE workflow_checkpoints; \
             DROP TABLE workflow_recovery_bindings; \
             DROP TABLE workflow_contract_authorities; \
             DROP TABLE workflow_model_calls; \
             DROP TABLE workflow_model_call_owners; \
             DELETE FROM schema_migrations WHERE version >= 6;",
        )
        .expect("restore the owned fixture to the v5 schema marker");
    drop(connection);
    assert_eq!(schema_version(database.path()), 5);

    let store = open_store(database.path());
    drop(store);
    assert_eq!(schema_version(database.path()), STORE_SCHEMA_VERSION);
    let connection = Connection::open(database.path()).expect("inspect migrated ledger tables");
    for table in ["workflow_model_calls", "workflow_model_call_owners"] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
                [table],
                |row| row.get(0),
            )
            .expect("ledger table query");
        assert!(exists, "schema v6 table missing: {table}");
    }
}

fn compiler_admission(workflow_id: Uuid, prompt: &str) -> ModelCallAdmission {
    let identity = request(prompt)
        .identity()
        .expect("compiler request identity");
    let digest = Sha256Digest::of_fields(
        "vibemux.workflow.compiler_request.v1",
        &[
            workflow_id.as_bytes(),
            identity.request_fingerprint.as_bytes(),
        ],
    );
    ModelCallAdmission {
        call_id: Uuid::from_bytes(digest.as_bytes()[..16].try_into().expect("sixteen bytes")),
        owner_key: owner_key(&format!("compiler_{}", workflow_id.simple())),
        workflow_id: None,
        identity,
        max_calls: 1,
        deadline_unix_ms: unix_time_ms() + DEADLINE_WINDOW_MS,
    }
}

fn preparation(workflow_id: Uuid, remaining: u32) -> (WorkflowRecord, ContractArtifact) {
    let policy_digest = Sha256Digest::of(b"compiler accounting fixture policy");
    let contract = ContractArtifact {
        schema_version: 1,
        contract_id: Sha256Digest::of(workflow_id.as_bytes()),
        workflow_key: "compiler_accounting".to_string(),
        task_key: "task_a".to_string(),
        contract_version: 1,
        task_spec_digest: Sha256Digest::of(b"compiler accounting fixture specification"),
        policy_digest,
        template_digest: Sha256Digest::of(b"compiler accounting fixture template"),
        template_version: "worker_v1".to_string(),
        harness_profile: "fixture_controller_v1".to_string(),
        route_transformation: RouteTransformation::None,
        context_bundle_digests: vec![],
        base_commit: "a".repeat(40),
        rendered_prompt_digest: Sha256Digest::of(b"compiler accounting fixture contract"),
        rendered_prompt_bytes: 36,
        previous_contract_id: None,
        semantic_diff: None,
    };
    let record = WorkflowRecord {
        schema_version: 1,
        workflow_id,
        request_key: format!("compiler_fixture_{}", workflow_id.simple()),
        workflow_key: owner_key("compiler_accounting"),
        mode: WorkflowMode::Cooperate,
        phase: WorkflowPhase::Prepared,
        version: 1,
        evidence_class: EvidenceClass::Fixture,
        base_commit: contract.base_commit.clone(),
        policy_digest,
        verifier_digest: Sha256Digest::of(b"fixture verifier"),
        integration_suites: vec![owner_key("fixture_suite")],
        tasks: vec![WorkflowTask {
            task_key: owner_key("task_a"),
            contract_id: contract.contract_id,
            contract_version: 1,
            required_suites: vec![owner_key("fixture_suite")],
            progress: TaskProgress::Pending,
            turns_used: 0,
            repairs_used: 0,
            max_turns: 4,
            max_repairs: 1,
            accepted_candidate: None,
            failure_code: None,
        }],
        budget: LoopBudget {
            decisions_remaining: 8,
            malformed_repairs_remaining: 1,
            model_requests_remaining: remaining,
            deadline_unix_ms: unix_time_ms() + 600_000,
        },
        content_store: ContentStoreMode::Disabled,
        blocked_reason: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    (record, contract)
}

fn complete_compiler(store: &mut SqliteStore, admission: &ModelCallAdmission) {
    store
        .reserve_model_call(admission)
        .expect("reserve compiler");
    store
        .submit_model_call(admission.call_id)
        .expect("submit compiler");
    store
        .finish_model_call(admission.call_id, Some(receipt(admission)), None)
        .expect("complete compiler");
}

#[test]
fn compiler_prepare_charges_once_links_owner_and_preserves_original_admission() {
    let database = NamedTempFile::new().expect("owned database");
    let mut store = open_store(database.path());
    let workflow_id = Uuid::new_v4();
    let compiler = compiler_admission(workflow_id, "compile this workflow");
    complete_compiler(&mut store, &compiler);
    let original_call = store
        .model_call(compiler.call_id)
        .expect("call")
        .expect("record");
    let (original, contract) = preparation(workflow_id, 4);
    let fingerprint = original
        .admission_fingerprint()
        .expect("admission identity");
    let first = store
        .prepare_workflow(
            &original,
            std::slice::from_ref(&contract),
            OffsetDateTime::now_utc(),
        )
        .expect("prepare and charge");
    assert_eq!(first.record.budget.model_requests_remaining, 3);
    assert_eq!(
        first.record.budget.deadline_unix_ms,
        original.budget.deadline_unix_ms
    );
    assert_eq!(
        first.record.budget.decisions_remaining,
        original.budget.decisions_remaining
    );
    assert_eq!(first.record.version, 1);
    assert_eq!(
        first.record.admission_fingerprint().expect("same identity"),
        fingerprint
    );
    assert_eq!(
        store
            .reserve_model_call(&compiler)
            .expect("original admission replay"),
        original_call
    );
    assert!(original_call.admission.workflow_id.is_none());
    assert_eq!(
        store
            .model_call(compiler.call_id)
            .expect("original history"),
        Some(original_call.clone())
    );
    let replay = store
        .prepare_workflow(
            &original,
            std::slice::from_ref(&contract),
            OffsetDateTime::now_utc(),
        )
        .expect("duplicate prepare");
    assert_eq!(replay.record, first.record);
    assert_eq!(replay.sequence, first.sequence);
    assert!(replay.event.is_none());
    let events = store.events().expect("canonical accounting events");
    let associations: Vec<_> = events
        .iter()
        .filter(|event| event.event_type().as_str() == "workflow_compiler_call_associated")
        .collect();
    assert_eq!(associations.len(), 1);
    assert_eq!(
        associations[0].payload().value()["charged_model_requests"],
        serde_json::json!(1)
    );
    assert_eq!(
        associations[0].payload().value()["model_requests_remaining"],
        serde_json::json!(3)
    );
    assert_eq!(
        associations[0].payload().value()["workflow_id"],
        serde_json::json!(workflow_id)
    );
    assert!(
        !serde_json::to_string(associations[0])
            .expect("content-free event")
            .contains("compile this workflow")
    );
    drop(store);

    let connection = Connection::open(database.path()).expect("inspect closed owned database");
    let associated: Option<String> = connection
        .query_row(
            "SELECT workflow_id FROM workflow_model_call_owners WHERE owner_key=?",
            [compiler.owner_key.as_str()],
            |row| row.get(0),
        )
        .expect("associated owner");
    assert_eq!(
        associated.as_deref(),
        Some(workflow_id.to_string().as_str())
    );
    drop(connection);

    let mut reopened = open_store(database.path());
    assert_eq!(
        reopened
            .prepare_workflow(
                &original,
                std::slice::from_ref(&contract),
                OffsetDateTime::now_utc()
            )
            .expect("replay after restart")
            .record
            .budget
            .model_requests_remaining,
        3
    );
    assert_eq!(
        reopened
            .model_call(compiler.call_id)
            .expect("call after restart"),
        Some(original_call)
    );
    let second = compiler_admission(workflow_id, "changed interpretation cannot be a fresh call");
    assert_eq!(
        reopened
            .reserve_model_call(&second)
            .expect_err("already prepared owner")
            .code(),
        "model_call_compiler_workflow_prepared"
    );
    reopened
        .transition_workflow(
            workflow_id,
            1,
            WorkflowPhase::Running,
            None,
            OffsetDateTime::now_utc(),
        )
        .expect("start prepared workflow");
    let mut followup_request = request("subsequent supervisor call");
    followup_request.role = RouteRole::Supervisor;
    let followup = ModelCallAdmission {
        call_id: Uuid::new_v4(),
        owner_key: owner_key("supervisor_after_prepare"),
        workflow_id: Some(workflow_id),
        identity: followup_request.identity().expect("supervisor identity"),
        deadline_unix_ms: compiler.deadline_unix_ms,
        max_calls: 1,
    };
    reopened
        .reserve_model_call(&followup)
        .expect("charge later model call");
    let replay_after_usage = reopened
        .prepare_workflow(&original, &[contract], OffsetDateTime::now_utc())
        .expect("replay cannot restore original allowance");
    assert_eq!(replay_after_usage.record.budget.model_requests_remaining, 2);
    assert_eq!(replay_after_usage.record.phase, WorkflowPhase::Running);
    assert!(replay_after_usage.event.is_none());
}

#[test]
fn ordinary_prepare_does_not_associate_unrelated_compiler_owners() {
    let database = NamedTempFile::new().expect("owned database");
    let mut store = open_store(database.path());
    let unrelated = admission(Uuid::new_v4(), CALL_OWNER, "unrelated interpretation", 2);
    store
        .reserve_model_call(&unrelated)
        .expect("unrelated owner");
    let (record, contract) = preparation(Uuid::new_v4(), 4);
    let prepared = store
        .prepare_workflow(&record, &[contract], OffsetDateTime::now_utc())
        .expect("operator-supplied workflow");
    assert_eq!(prepared.record, record);
    assert_eq!(
        store
            .model_call(unrelated.call_id)
            .expect("unrelated call")
            .expect("record")
            .state,
        ModelCallState::Reserved
    );
}

#[test]
fn settled_failed_or_unknown_compiler_reservations_are_never_refunded() {
    for terminal in [
        ModelCallState::Blocked,
        ModelCallState::Rejected,
        ModelCallState::Unknown,
    ] {
        let database = NamedTempFile::new().expect("owned database");
        let mut store = open_store(database.path());
        let workflow_id = Uuid::new_v4();
        let compiler = compiler_admission(workflow_id, "compiler accounting after failure");
        store.reserve_model_call(&compiler).expect("reserve");
        let (result, reason) = match terminal {
            ModelCallState::Blocked => (None, "gateway_live_attestation_unavailable"),
            ModelCallState::Rejected => {
                store.submit_model_call(compiler.call_id).expect("submit");
                (Some(receipt(&compiler)), "gateway_result_invalid")
            }
            ModelCallState::Unknown => {
                store.submit_model_call(compiler.call_id).expect("submit");
                (None, "model_call_outcome_unknown")
            }
            _ => unreachable!("test terminal states"),
        };
        let call = store
            .finish_model_call(compiler.call_id, result, Some(reason.to_string()))
            .expect("settled ledger state");
        assert_eq!(call.state, terminal);
        let (record, contract) = preparation(workflow_id, 2);
        let prepared = store
            .prepare_workflow(
                &record,
                std::slice::from_ref(&contract),
                OffsetDateTime::now_utc(),
            )
            .expect("manual candidate retains compiler charge");
        assert_eq!(prepared.record.budget.model_requests_remaining, 1);
        assert_eq!(
            store.model_call(compiler.call_id).expect("history"),
            Some(call)
        );
        assert_eq!(
            store
                .prepare_workflow(&record, &[contract], OffsetDateTime::now_utc())
                .expect("duplicate")
                .record
                .budget
                .model_requests_remaining,
            1
        );
    }
}

#[test]
fn pending_compilers_block_prepare_without_spending_or_creating_new_state() {
    for submitted in [false, true] {
        let database = NamedTempFile::new().expect("owned database");
        let mut store = open_store(database.path());
        let workflow_id = Uuid::new_v4();
        let compiler = compiler_admission(workflow_id, "compiler still pending");
        store.reserve_model_call(&compiler).expect("reserve");
        if submitted {
            store
                .submit_model_call(compiler.call_id)
                .expect("submitted");
        }
        let before = store
            .model_call(compiler.call_id)
            .expect("call")
            .expect("record");
        let (record, contract) = preparation(workflow_id, 4);
        assert_eq!(
            store
                .prepare_workflow(&record, &[contract], OffsetDateTime::now_utc())
                .expect_err("pending compilation")
                .code(),
            "model_call_compiler_pending"
        );
        assert!(
            store
                .workflow_record(workflow_id)
                .expect("no workflow")
                .is_none()
        );
        assert_eq!(
            store.model_call(compiler.call_id).expect("unchanged call"),
            Some(before)
        );
    }
}

#[test]
fn exhausted_prepare_budget_leaves_compiler_charge_and_owner_unassociated() {
    let database = NamedTempFile::new().expect("owned database");
    let mut store = open_store(database.path());
    let workflow_id = Uuid::new_v4();
    let compiler = compiler_admission(workflow_id, "completed compiler exhausted global allowance");
    complete_compiler(&mut store, &compiler);
    let (record, contract) = preparation(workflow_id, 0);
    assert_eq!(
        store
            .prepare_workflow(
                &record,
                std::slice::from_ref(&contract),
                OffsetDateTime::now_utc()
            )
            .expect_err("no allowance for prior compiler")
            .code(),
        "model_call_limit_exceeded"
    );
    assert!(
        store
            .workflow_record(workflow_id)
            .expect("workflow absent")
            .is_none()
    );
    assert!(
        store
            .workflow_contract(contract.contract_id)
            .expect("contract absent")
            .is_none()
    );
    assert_eq!(
        store
            .model_call(compiler.call_id)
            .expect("call")
            .expect("record")
            .calls_used,
        1
    );
    drop(store);
    let connection = Connection::open(database.path()).expect("inspect closed database");
    let (association, used): (Option<String>, u32) = connection
        .query_row(
            "SELECT workflow_id, calls_used FROM workflow_model_call_owners WHERE owner_key=?",
            [compiler.owner_key.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("owner");
    assert_eq!(association, None);
    assert_eq!(used, 1);
}

#[test]
fn exact_compiler_namespace_requires_role_call_identity_and_single_call_fence() {
    let database = NamedTempFile::new().expect("owned database");
    let mut store = open_store(database.path());
    let original = compiler_admission(
        Uuid::from_u128(0x0123456789abcdef0123456789abcdef),
        "identity fence",
    );
    let suffix = original
        .owner_key
        .as_str()
        .strip_prefix("compiler_")
        .expect("compiler namespace");
    assert!(SpecIdentifier::new(format!("compiler_{}", suffix.to_ascii_uppercase())).is_err());
    for change in 0..4 {
        let mut invalid = original.clone();
        match change {
            0 => invalid.identity.role = RouteRole::WorkerA,
            1 => invalid.max_calls = 2,
            2 => invalid.call_id = Uuid::new_v4(),
            _ => invalid.workflow_id = Some(Uuid::new_v4()),
        }
        assert_eq!(
            store
                .reserve_model_call(&invalid)
                .expect_err("invalid compiler association")
                .code(),
            "model_call_compiler_association_invalid"
        );
    }
    assert_eq!(
        store
            .reserve_model_call(&original)
            .expect("valid exact owner")
            .calls_used,
        1
    );
}

#[test]
fn corrupted_owner_count_cannot_be_associated_or_silently_repaired() {
    let database = NamedTempFile::new().expect("owned database");
    let workflow_id = Uuid::new_v4();
    let compiler = compiler_admission(workflow_id, "projection mismatch fixture");
    {
        let mut store = open_store(database.path());
        complete_compiler(&mut store, &compiler);
    }
    // Deliberate corruption of an owned, closed test database, never an alternate
    // runtime writer or a way to grant authority to an otherwise invalid fixture.
    let connection = Connection::open(database.path()).expect("closed fixture database");
    connection
        .execute(
            "UPDATE workflow_model_call_owners SET calls_used=2 WHERE owner_key=?",
            [compiler.owner_key.as_str()],
        )
        .expect("corrupt counter");
    drop(connection);
    let mut store = open_store(database.path());
    let (record, contract) = preparation(workflow_id, 4);
    assert_eq!(
        store
            .prepare_workflow(&record, &[contract], OffsetDateTime::now_utc())
            .expect_err("inconsistent ledger rejected")
            .code(),
        "model_call_compiler_association_invalid"
    );
    assert!(
        store
            .workflow_record(workflow_id)
            .expect("no partial workflow")
            .is_none()
    );
    assert_eq!(
        store
            .model_call(compiler.call_id)
            .expect("immutable historical call")
            .expect("record")
            .calls_used,
        1
    );
}

#[test]
fn a_mismatched_existing_compiler_association_cannot_move_between_workflows() {
    let database = NamedTempFile::new().expect("owned database");
    let workflow_id = Uuid::new_v4();
    let other_id = Uuid::new_v4();
    let compiler = compiler_admission(workflow_id, "cross-workflow association fixture");
    {
        let mut store = open_store(database.path());
        complete_compiler(&mut store, &compiler);
        let (other, contract) = preparation(other_id, 4);
        store
            .prepare_workflow(&other, &[contract], OffsetDateTime::now_utc())
            .expect("other workflow");
    }
    let connection = Connection::open(database.path()).expect("closed owned fixture database");
    connection
        .execute(
            "UPDATE workflow_model_call_owners SET workflow_id=? WHERE owner_key=?",
            rusqlite::params![other_id.to_string(), compiler.owner_key.as_str()],
        )
        .expect("inject mismatched association");
    drop(connection);
    let mut store = open_store(database.path());
    let (record, contract) = preparation(workflow_id, 4);
    assert_eq!(
        store
            .prepare_workflow(&record, &[contract], OffsetDateTime::now_utc())
            .expect_err("cannot move association")
            .code(),
        "model_call_compiler_association_invalid"
    );
    assert!(
        store
            .workflow_record(workflow_id)
            .expect("no new workflow")
            .is_none()
    );
    assert_eq!(
        store
            .workflow_record(other_id)
            .expect("other workflow")
            .expect("record")
            .budget
            .model_requests_remaining,
        4
    );
}
