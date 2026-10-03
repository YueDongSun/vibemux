#![cfg(feature = "test_helpers")]
//! Candidate admission and gating of a single store task (ADR 031 §6):
//! out-of-scope files are refused and repaired, protected and forbidden
//! paths fail the Run, a trusted-suite failure gets exactly the bounded
//! repairs the contract allows (A03), and exhausted repairs fail the task.
//! Synthetic workers: offline orchestration evidence.

mod workflow_support;

use serde_json::{Value, json};
use workflow_support::{
    FixtureOptions, WorkflowFixture, phase, policy, review_pass, store_request, store_task,
    taskboard_file, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const STORE_SUITE: [&str; 1] = ["store"];
const GOOD_STORE: &str = "calibration/good/src/store.mjs";
const BLANK_TITLE_MUTANT: &str = "calibration/mutants/blank_title_accepted/src/store.mjs";
const WORKER_TEST_PATH: &str = "tests/worker_store/store_smoke.test.mjs";
const WORKER_TEST: &str = "// development test owned by the store worker\nexport {};\n";

fn store_file(source: &str) -> Value {
    json!({"src/store.mjs": taskboard_file(source)})
}

fn candidates(status: &Value) -> &Vec<Value> {
    status["receipts"]["candidates"]
        .as_array()
        .expect("candidates")
}

fn verification_of<'a>(status: &'a Value, digest: &Value) -> &'a Value {
    status["receipts"]["verifications"]
        .as_array()
        .expect("verifications")
        .iter()
        .find(|verification| &verification["subject_digest"] == digest)
        .expect("verification")
}

/// Runs one cooperative store workflow to a terminal phase.
async fn run_store(
    fixture: &WorkflowFixture,
    request_key: &str,
    suites: &[&'static str],
    script: &Value,
) -> (Value, Value) {
    fixture.write_script(script);
    let request = store_request(
        request_key,
        &fixture.head,
        "cooperate",
        &store_task(suites),
        false,
    );
    let prepared = fixture.prepare(request, policy(suites, 2, false)).await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked", "cancelled"])
        .await;
    (prepared, status)
}

/// W01 and W02: a file outside the task's owned paths makes the candidate
/// inadmissible; the repair turn is told why, removes it, and the
/// repaired candidate (with a permitted untracked test file) is accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_out_of_scope_file_is_refused_and_repaired() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut first = store_file(GOOD_STORE);
    first["src/extra.mjs"] = json!("export const extra = 1;\n");
    first[WORKER_TEST_PATH] = json!(WORKER_TEST);
    let script = json!({"steps": {
        "store/implement_1": {"write": first},
        "store/repair": {"remove": ["src/extra.mjs"]},
        "store/review": review_pass(),
    }});
    let (_, status) = run_store(&fixture, "store_out_of_scope", &SYNTAX, &script).await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    let candidates = candidates(&status);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["admitted"], false);
    assert_eq!(
        candidates[0]["violation_codes"],
        json!(["snapshot_outside_owned_paths"])
    );
    assert_eq!(candidates[1]["admitted"], true);
    // The store file and the untracked development test; not the extra.
    assert_eq!(candidates[1]["files"], 2);
    assert_eq!(
        status["tasks"][0]["accepted_candidate"],
        candidates[1]["candidate_digest"]
    );
    assert_eq!(status["tasks"][0]["repairs_used"], 1);
    let repair = &fixture.started("store/repair_2");
    assert_eq!(repair.len(), 1);
    assert!(
        repair[0]["prompt"]
            .as_str()
            .expect("prompt")
            .contains("candidate refused: snapshot_outside_owned_paths")
    );
    // The refused candidate was never reviewed or verified.
    assert!(
        !status["receipts"]["reviews"]
            .as_array()
            .expect("reviews")
            .iter()
            .any(|review| review["candidate_digest"] == candidates[0]["candidate_digest"])
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

/// W01: touching a protected path fails the Run at once; no repair,
/// review, or integration follows. `package.json` is also forbidden by the
/// task contract, and the protected classification takes precedence; the
/// forbidden-only classification is pinned by the snapshot unit tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_paths_fail_the_run() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    for (request_key, path) in [
        ("store_protected_contract", "CONTRACT.md"),
        ("store_protected_manifest", "package.json"),
    ] {
        let code = "snapshot_protected_path";
        let mut write = store_file(GOOD_STORE);
        write[path] = json!("tampered\n");
        let script = json!({"steps": {
            "store/implement_1": {"write": write},
            "store/repair": {"write": store_file(GOOD_STORE)},
            "store/review": review_pass(),
        }});
        let (_, status) = run_store(&fixture, request_key, &SYNTAX, &script).await;
        assert_eq!(phase(&status), "failed", "status: {status:#}");
        assert_eq!(
            status["last_run"]["end"],
            json!({"end": "failed", "code": code}),
            "{path}"
        );
        let candidates = candidates(&status);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["admitted"], false);
        assert!(
            candidates[0]["violation_codes"]
                .as_array()
                .expect("codes")
                .contains(&json!(code))
        );
        assert!(status["receipts"]["integration"].is_null());
        assert!(status["tasks"][0]["accepted_candidate"].is_null());
    }
    assert!(fixture.started("store/repair_2").is_empty());
    assert!(
        fixture
            .started_keys()
            .iter()
            .all(|key| !key.contains("review"))
    );
    // The tampering happened only in the workers' worktrees.
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

/// A03: the trusted store suite fails a protocol-complete, review-passed
/// candidate; one bounded repair, told which suite failed, closes the
/// defect. The contract and the failed candidate's receipts are kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_bounded_repair_closes_an_injected_defect() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let script = json!({"steps": {
        "store/implement_1": {"write": store_file(BLANK_TITLE_MUTANT)},
        "store/repair": {"write": store_file(GOOD_STORE)},
        "store/review": review_pass(),
    }});
    let (prepared, status) = run_store(&fixture, "store_repair", &STORE_SUITE, &script).await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    let candidates = candidates(&status);
    assert_eq!(candidates.len(), 2);
    let defective = verification_of(&status, &candidates[0]["candidate_digest"]);
    assert_eq!(defective["suites"][0]["status"], "failed");
    let repaired = verification_of(&status, &candidates[1]["candidate_digest"]);
    assert_eq!(repaired["suites"][0]["status"], "passed");
    let task = &status["tasks"][0];
    assert_eq!(
        task["accepted_candidate"],
        candidates[1]["candidate_digest"]
    );
    assert_eq!(task["repairs_used"], 1);
    // The contract the work was admitted under is unchanged.
    assert_eq!(task["contract_id"], prepared["contracts"][0]["contract_id"]);
    assert_eq!(task["contract_version"], 1);
    let repair_prompt = fixture.started("store/repair_2")[0]["prompt"]
        .as_str()
        .expect("prompt")
        .to_string();
    assert!(repair_prompt.contains("gate_suite_failed(store)"));
    assert!(repair_prompt.contains("verifier suite store failed tests:"));
    assert_eq!(fixture.checkout_status(), "");
    fixture
        .record_evidence("bounded_repair", workflow_id(&prepared))
        .await;
    fixture.shutdown().await;
}

/// A defect the repair does not close fails the task once the contract's
/// repair budget is spent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhausted_repairs_fail_the_task() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let script = json!({"steps": {
        "store/implement_1": {"write": store_file(BLANK_TITLE_MUTANT)},
        "store/repair": {"write": store_file(BLANK_TITLE_MUTANT)},
        "store/review": review_pass(),
    }});
    let (_, status) = run_store(&fixture, "store_exhausted", &STORE_SUITE, &script).await;
    assert_eq!(phase(&status), "failed", "status: {status:#}");
    assert_eq!(
        status["last_run"]["end"],
        json!({"end": "failed", "code": "task_repairs_exhausted"})
    );
    assert_eq!(status["tasks"][0]["repairs_used"], 1);
    assert!(status["tasks"][0]["accepted_candidate"].is_null());
    assert!(status["receipts"]["integration"].is_null());
    let repairs: Vec<String> = fixture
        .started_keys()
        .into_iter()
        .filter(|key| key.contains("repair"))
        .collect();
    assert_eq!(repairs, ["store/repair_2"]);
    fixture.shutdown().await;
}
