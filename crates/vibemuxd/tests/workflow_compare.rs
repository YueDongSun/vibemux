#![cfg(feature = "test_helpers")]
//! Compare mode end to end (ADR 031 §6): two fixture workers implement the
//! same store task in separate worktrees; the trusted store suite, not the
//! workers' reports or the review, decides the winner, and a daemon-made
//! negative control proves the suite can fail. Synthetic workers: this is
//! offline orchestration evidence.

mod workflow_support;

use serde_json::{Value, json};
use workflow_support::{
    FixtureOptions, WorkflowFixture, phase, policy, review_pass, store_request, store_task,
    taskboard_file, workflow_id,
};

const GOOD_STORE: &str = "calibration/good/src/store.mjs";
const BLANK_TITLE_MUTANT: &str = "calibration/mutants/blank_title_accepted/src/store.mjs";

fn store_write(source: &str) -> Value {
    json!({"write": {"src/store.mjs": taskboard_file(source)}})
}

fn script(first: &str, second: &str) -> Value {
    json!({"steps": {
        "store/implement_1": {"sequence": [store_write(first), store_write(second)]},
        "store/review_1": review_pass(),
    }})
}

fn verification_of<'a>(status: &'a Value, digest: &Value) -> &'a Value {
    status["receipts"]["verifications"]
        .as_array()
        .expect("verifications")
        .iter()
        .find(|verification| &verification["subject_digest"] == digest)
        .expect("verification")
}

fn suite_passed(verification: &Value) -> bool {
    verification["suites"]
        .as_array()
        .expect("suites")
        .iter()
        .all(|suite| suite["status"] == "passed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trusted_suite_selects_the_correct_candidate_and_refuses_the_defect() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&script(GOOD_STORE, BLANK_TITLE_MUTANT));
    let request = store_request(
        "store_compare",
        &fixture.head,
        "compare",
        &store_task(&["store"]),
        true,
    );
    let prepared = fixture.prepare(request, policy(&["store"], 2, false)).await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(workflow).await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");

    let candidates = status["receipts"]["candidates"]
        .as_array()
        .expect("candidates");
    let workers: Vec<&Value> = candidates
        .iter()
        .filter(|candidate| candidate["origin"] == "fixture_worker")
        .collect();
    assert_eq!(workers.len(), 2);
    assert_ne!(
        workers[0]["worker_session_id"],
        workers[1]["worker_session_id"]
    );
    // V01: both candidates completed the protocol and passed review; only
    // the trusted suite separates them.
    let (passed, failed): (Vec<&Value>, Vec<&Value>) = workers.iter().partition(|candidate| {
        suite_passed(verification_of(&status, &candidate["candidate_digest"]))
    });
    assert_eq!((passed.len(), failed.len()), (1, 1));
    let defective = verification_of(&status, &failed[0]["candidate_digest"]);
    assert!(defective["suites"][0]["tests_failed"].as_u64() > Some(0));
    for candidate in &workers {
        assert!(
            status["receipts"]["reviews"]
                .as_array()
                .expect("reviews")
                .iter()
                .any(
                    |review| review["candidate_digest"] == candidate["candidate_digest"]
                        && review["verdict"] == "pass"
                )
        );
    }
    // The negative control is a daemon-made mutation of an admitted
    // candidate: it is never a worker candidate, the trusted suite must fail
    // it, and the selector excludes it as a mutation artifact. Its verifier
    // result survives only as the exclusion reasons of the selection
    // receipt; the store refuses a verification of a non-candidate subject.
    let selection = &status["receipts"]["selection"];
    let worker_digests: Vec<&Value> = workers
        .iter()
        .map(|candidate| &candidate["candidate_digest"])
        .collect();
    let control = selection["excluded"]
        .as_array()
        .expect("excluded")
        .iter()
        .find(|entry| !worker_digests.contains(&&entry["candidate_digest"]))
        .expect("negative control");
    let reasons = control["reasons"].as_array().expect("reasons");
    assert!(reasons.contains(&json!("selection_mutation_artifact")));
    assert!(reasons.contains(&json!("gate_suite_failed")));
    let defective_exclusion = selection["excluded"]
        .as_array()
        .expect("excluded")
        .iter()
        .find(|entry| entry["candidate_digest"] == failed[0]["candidate_digest"])
        .expect("defective candidate excluded");
    assert_eq!(defective_exclusion["reasons"], json!(["gate_suite_failed"]));
    // V04: the selector picked the verified candidate and nothing else.
    assert_eq!(selection["winner"], passed[0]["candidate_digest"]);
    assert_eq!(
        status["receipts"]["integration"]["applied_candidates"],
        json!([passed[0]["candidate_digest"]])
    );
    assert_eq!(
        status["tasks"][0]["accepted_candidate"],
        passed[0]["candidate_digest"]
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.record_evidence("compare_selection", workflow).await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_defective_candidates_select_no_winner() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&script(BLANK_TITLE_MUTANT, BLANK_TITLE_MUTANT));
    let request = store_request(
        "store_compare_both_fail",
        &fixture.head,
        "compare",
        &store_task(&["store"]),
        false,
    );
    let prepared = fixture.prepare(request, policy(&["store"], 2, false)).await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(workflow).await;
    assert_eq!(phase(&status), "failed", "status: {status:#}");
    assert_eq!(
        status["last_run"]["end"],
        json!({"end": "failed", "code": "no_valid_candidate"})
    );
    assert!(status["receipts"]["selection"]["winner"].is_null());
    assert!(status["receipts"]["integration"].is_null());
    assert!(status["tasks"][0]["accepted_candidate"].is_null());
    fixture.record_evidence("compare_both_fail", workflow).await;
    fixture.shutdown().await;
}
