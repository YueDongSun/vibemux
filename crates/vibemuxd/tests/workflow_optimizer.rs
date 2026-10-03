#![cfg(feature = "test_helpers")]
//! The bounded prompt-policy optimizer through Control (ADR 031 §7): one
//! cycle over a fixed operator suite, where every case is a real workflow
//! the daemon runs, rejects a forbidden field, a harmful wording, and a
//! no-gain change, promotes the one wording that verifiably improves, and
//! can be rolled back (O02, O03). Admitted workflows keep the wording they
//! were prepared with. Synthetic workers whose behavior depends on the
//! rendered prompt: offline orchestration evidence, not live optimizer
//! evidence.

mod workflow_support;

use std::path::Path;

use serde_json::{Value, json};
use workflow_support::{
    FixtureOptions, WorkflowFixture, policy, remote, review_pass, session_id, store_request,
    store_task, taskboard_file, workflow_id,
};

const STORE_SUITE: [&str; 1] = ["store"];
const GOOD_STORE: &str = "calibration/good/src/store.mjs";
const BLANK_TITLE_MUTANT: &str = "calibration/mutants/blank_title_accepted/src/store.mjs";
const SUITE_ID: &str = "store_suite";
const BASE_ROLE: &str = "You are a VibeMux worker assigned one versioned task contract.";
/// The improving wording: the worker validates titles and writes a store
/// that passes the trusted suite.
const IMPROVE_MARKER: &str = "Check every title for blank input before writing the store.";
/// The harmful wording: the worker also rewrites a protected file.
const HARMFUL_MARKER: &str = "Also refresh the project manifest.";

fn store_file(source: &str) -> Value {
    json!({"src/store.mjs": taskboard_file(source)})
}

/// Without a marker the worker writes the blank-title mutant, and the
/// repair does not close it, so the baseline fails every case.
fn optimizer_script() -> Value {
    let mut harmful = store_file(GOOD_STORE);
    harmful["package.json"] = json!("{\"name\": \"rewritten\"}\n");
    json!({"steps": {
        "store/implement_1": {
            "write": store_file(BLANK_TITLE_MUTANT),
            "variants": [
                {"prompt_contains": IMPROVE_MARKER, "step": {"write": store_file(GOOD_STORE)}},
                {"prompt_contains": HARMFUL_MARKER, "step": {"write": harmful}},
            ],
        },
        "store/repair": {"write": store_file(BLANK_TITLE_MUTANT)},
        "store/review": review_pass(),
    }})
}

fn write_operator_file(fixture: &WorkflowFixture, directory: &str, id: &str, value: &Value) {
    let directory = fixture.state_dir.join(directory);
    std::fs::create_dir_all(&directory).expect("operator directory");
    let encoded = serde_json::to_vec_pretty(value).expect("encode");
    std::fs::write(Path::new(&directory).join(format!("{id}.json")), encoded)
        .expect("operator file");
}

/// A suite with one case per split; every case is the store task under the
/// trusted store suite.
fn write_suite(fixture: &WorkflowFixture, max_model_requests: u32) {
    let case = json!({
        "request": store_request("case", &fixture.head, "cooperate", &store_task(&STORE_SUITE),
            false),
        "policy": policy(&STORE_SUITE, 2, false),
    });
    let suite = json!({
        "schema_version": 1,
        "suite_id": SUITE_ID,
        "train": {"dataset_id": "store_train", "split": "train", "case_ids": ["train_case"]},
        "dev": {"dataset_id": "store_dev", "split": "dev", "case_ids": ["dev_case"]},
        "holdout": {"dataset_id": "store_holdout", "split": "holdout",
            "case_ids": ["holdout_case"]},
        "cases": {"train_case": case, "dev_case": case, "holdout_case": case},
        "limits": {"max_candidates": 4, "max_model_requests": max_model_requests},
        "case_deadline_ms": 120_000,
    });
    write_operator_file(fixture, "prompt_suites", SUITE_ID, &suite);
}

fn role_change(marker: &str) -> Value {
    json!({"changes": [{"field": "template_overrides.worker_v1.worker_role",
            "value": format!("{BASE_ROLE} {marker}")}],
        "expected_benefit": "The worker validates input first.",
        "possible_regressions": ["Longer worker prompts."]})
}

fn write_candidates(fixture: &WorkflowFixture, candidate_id: &str, diffs: &[Value]) {
    let candidates = json!({"schema_version": 1, "candidate_id": candidate_id, "diffs": diffs});
    write_operator_file(fixture, "prompt_candidates", candidate_id, &candidates);
}

/// Prepares the store task under `request_key` with the active policy and
/// returns its worker's exact first prompt.
async fn admitted_prompt(fixture: &WorkflowFixture, request_key: &str) -> (Value, String) {
    let request = store_request(
        request_key,
        &fixture.head,
        "cooperate",
        &store_task(&STORE_SUITE),
        false,
    );
    let prepared = fixture
        .prepare(request, policy(&STORE_SUITE, 2, false))
        .await;
    let session = session_id(workflow_id(&prepared), "store", "slot_a");
    let next = fixture
        .client
        .workflow_session_prompt(session, 0)
        .await
        .expect("next prompt");
    let text = next["text"].as_str().expect("next prompt text").to_string();
    (prepared, text)
}

fn cases_of<'a>(report: &'a Value, split: &str) -> Vec<&'a Value> {
    report["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["split"] == split)
        .collect()
}

/// O02 and O03: the cycle evaluates candidates on the fixed dev split
/// only, consults the protected holdout once for the best one, rejects a
/// forbidden field, a hard-gate failure, and a no-gain change, promotes
/// the improving wording for future admissions only, and rolls back to
/// the baseline, after which the same cycle replays without paying again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_verified_improvement_is_promoted_and_it_can_be_rolled_back() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&optimizer_script());
    write_suite(&fixture, 64);
    write_candidates(
        &fixture,
        "wording",
        &[
            json!({"changes": [{"field": "budget_caps.max_turns", "value": 12}],
                "expected_benefit": "More turns.", "possible_regressions": []}),
            role_change(HARMFUL_MARKER),
            json!({"changes": [{"field": "summary_budget_bytes", "value": 4096}],
                "expected_benefit": "Smaller bundles.", "possible_regressions": []}),
            role_change(IMPROVE_MARKER),
        ],
    );
    assert_eq!(
        fixture
            .client
            .workflow_policy_versions()
            .await
            .expect("versions"),
        json!([])
    );
    // Admitted before the cycle: it must keep the baseline wording.
    let (admitted, admitted_prompt_text) = admitted_prompt(&fixture, "store_before").await;
    assert!(!admitted_prompt_text.contains(IMPROVE_MARKER));

    let report = fixture
        .client
        .workflow_policy_evaluate("wording", SUITE_ID)
        .await
        .expect("evaluate");
    assert_eq!(report["duplicate"], false, "{report:#}");
    assert_eq!(report["baseline_dev"]["verified_successes"], 0);
    assert_eq!(report["baseline_holdout"]["verified_successes"], 0);
    let outcomes = report["outcomes"].as_array().expect("outcomes");
    let rejections: Vec<&Value> = outcomes
        .iter()
        .map(|outcome| &outcome["rejection"])
        .collect();
    assert_eq!(
        rejections,
        [
            &json!("forbidden_field"),
            &json!("hard_gate_failure"),
            &json!("no_improvement"),
            &Value::Null,
        ],
        "{report:#}"
    );
    let promoted_digest = &outcomes[3]["candidate_digest"];
    assert!(promoted_digest.is_string());
    assert_eq!(&report["promoted_policy_digest"], promoted_digest);
    assert_eq!(report["promoted_version"], 2);
    assert_eq!(report["optimization_improved"], true);
    assert_eq!(report["holdout_consultations"], 1);
    assert!(report["promotion_withheld"].is_null());
    let spent = report["model_requests_spent"].as_u64().expect("spent");
    assert!(spent > 0 && spent <= 64, "{report:#}");
    // Train cases are never run; the holdout ran for the baseline and the
    // promoted candidate only; every case result is a recorded workflow.
    assert!(cases_of(&report, "train").is_empty());
    let holdout = cases_of(&report, "holdout");
    assert_eq!(holdout.len(), 2);
    assert_eq!(holdout[0]["verified_success"], false);
    assert_eq!(holdout[1]["verified_success"], true);
    assert_eq!(&holdout[1]["policy_digest"], promoted_digest);
    let harmful = cases_of(&report, "dev")
        .into_iter()
        .find(|case| !case["hard_gates_passed"].as_bool().expect("gate"))
        .expect("the harmful candidate's case");
    let harmful_status = fixture
        .status(
            harmful["workflow_id"]
                .as_str()
                .expect("id")
                .parse()
                .expect("uuid"),
        )
        .await;
    assert_eq!(
        harmful_status["last_run"]["end"],
        json!({"end": "failed", "code": "snapshot_protected_path"})
    );

    let versions = fixture
        .client
        .workflow_policy_versions()
        .await
        .expect("versions");
    assert_eq!(
        versions,
        json!([
            {"version": 1, "parent": null, "policy_digest": report["base_policy_digest"],
                "state": "superseded"},
            {"version": 2, "parent": 1, "policy_digest": promoted_digest, "state": "active"},
        ])
    );
    // Future admissions use the promoted wording. The workflow admitted
    // before the cycle is unchanged: re-preparing it would render a
    // different contract prompt, so it is refused rather than re-worded.
    let (promoted, promoted_prompt) = admitted_prompt(&fixture, "store_after").await;
    assert!(promoted_prompt.contains(IMPROVE_MARKER));
    assert_ne!(
        promoted["contracts"][0]["rendered_prompt_digest"],
        admitted["contracts"][0]["rendered_prompt_digest"]
    );
    assert_eq!(
        fixture
            .client
            .workflow_prepare(
                store_request(
                    "store_before",
                    &fixture.head,
                    "cooperate",
                    &store_task(&STORE_SUITE),
                    false,
                ),
                policy(&STORE_SUITE, 2, false),
            )
            .await,
        Err(remote("workflow_request_conflict"))
    );
    let unchanged = fixture
        .client
        .workflow_session_prompt(session_id(workflow_id(&admitted), "store", "slot_a"), 0)
        .await
        .expect("admitted prompt");
    assert_eq!(
        unchanged["text"].as_str(),
        Some(admitted_prompt_text.as_str())
    );

    // Rollback restores the baseline for future admissions only.
    let restored = fixture
        .client
        .workflow_policy_rollback()
        .await
        .expect("rollback");
    assert_eq!(restored["version"], 1);
    assert_eq!(restored["state"], "active");
    let versions = fixture
        .client
        .workflow_policy_versions()
        .await
        .expect("versions");
    assert_eq!(versions[1]["state"], "rolled_back");
    let (_, rolled_back_prompt) = admitted_prompt(&fixture, "store_rolled_back").await;
    assert_eq!(rolled_back_prompt, admitted_prompt_text);
    assert_eq!(
        fixture.client.workflow_policy_rollback().await,
        Err(remote("optimizer_invalid_history"))
    );

    // The cycle is keyed by the suite, candidates, commit, and base policy.
    // With the baseline active again it is the same cycle: its holdout was
    // consulted, so the stored report comes back, nothing runs or is paid
    // for, and nothing is promoted again.
    let turns = fixture.started_keys().len();
    let replay = fixture
        .client
        .workflow_policy_evaluate("wording", SUITE_ID)
        .await
        .expect("replay");
    assert_eq!(replay["duplicate"], true);
    assert_eq!(replay["cycle_id"], report["cycle_id"]);
    assert_eq!(fixture.started_keys().len(), turns);
    let versions = fixture
        .client
        .workflow_policy_versions()
        .await
        .expect("versions");
    assert_eq!(versions[0]["state"], "active");
    assert_eq!(versions.as_array().expect("versions").len(), 2);
    assert_eq!(fixture.checkout_status(), "");
    fixture.record_value(
        "optimizer_cycle",
        &json!({"report": report, "restored": restored, "replay": replay, "versions": versions}),
    );
    fixture.shutdown().await;
}

/// O03: the model-request budget bounds a cycle before any case runs past
/// it, and a malformed suite is refused before anything runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_budget_and_the_suite_shape_bound_a_cycle() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&optimizer_script());
    write_candidates(&fixture, "wording", &[role_change(IMPROVE_MARKER)]);
    // One case may cost 2 × max_turns = 8 requests; 7 cannot cover it.
    write_suite(&fixture, 7);
    assert_eq!(
        fixture
            .client
            .workflow_policy_evaluate("wording", SUITE_ID)
            .await,
        Err(remote("optimizer_budget_exhausted"))
    );
    assert!(fixture.started_keys().is_empty());

    // The same case in two splits is not a protected holdout.
    let mut suite: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .state_dir
                .join("prompt_suites")
                .join(format!("{SUITE_ID}.json")),
        )
        .expect("suite"),
    )
    .expect("suite json");
    suite["holdout"]["case_ids"] = json!(["dev_case"]);
    suite["limits"]["max_model_requests"] = json!(64);
    write_operator_file(&fixture, "prompt_suites", SUITE_ID, &suite);
    assert_eq!(
        fixture
            .client
            .workflow_policy_evaluate("wording", SUITE_ID)
            .await,
        Err(remote("workflow_evaluation_invalid"))
    );
    assert!(fixture.started_keys().is_empty());
    assert_eq!(
        fixture
            .client
            .workflow_policy_versions()
            .await
            .expect("versions"),
        json!([])
    );
    fixture.shutdown().await;
}
