#![cfg(feature = "test_helpers")]
//! Admission through Control (ADR 031 §3–§4): a slot whose harness route
//! cannot execute a writable turn is refused before admission (G02), a
//! TaskSpec that drops a prohibition or invents scope is rejected with its
//! violation codes (P02), an uncompiled request needs a compiler route, a
//! changed request cannot overwrite an admitted one (P04), and a workflow
//! with no free slot blocks instead of sharing one (S02, S03). Synthetic
//! workers: offline orchestration evidence.

mod workflow_support;

use serde_json::{Value, json};
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, TaskDraft, WorkflowFixture, cooperative_request, good_files, phase, policy,
    remote, review_pass, store_request, store_task, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
/// Long enough to hold both slots while the second workflow starts.
const HOLDING_TURN_MS: u64 = 20_000;

fn request_with(fixture: &WorkflowFixture, request_key: &str, a: &TaskDraft) -> Value {
    cooperative_request(request_key, &fixture.head, a, &track_b(&SYNTAX), &SYNTAX)
}

fn violation_codes(response: &Value) -> Vec<String> {
    response["violation_codes"]
        .as_array()
        .expect("violation codes")
        .iter()
        .map(|code| code.as_str().expect("code").to_string())
        .collect()
}

/// G02: an ACP route only probes, so a task assigned to it is refused at
/// prepare, before any contract is admitted or any turn runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_that_cannot_execute_is_refused_before_admission() {
    let fixture = WorkflowFixture::start(FixtureOptions::with_acp_slot()).await;
    let mut request = request_with(&fixture, "taskboard_acp", &track_a(&SYNTAX));
    request["assignments"]["track_b"] = json!(["slot_acp"]);
    assert_eq!(
        fixture
            .client
            .workflow_prepare(request, policy(&SYNTAX, 2, false))
            .await,
        Err(remote("workflow_slot_unavailable"))
    );
    // A slot without the reviewer role cannot stand in for the independent
    // reviewer, and a slot that also writes is refused by request shape.
    let mut request = request_with(&fixture, "taskboard_no_reviewer", &track_a(&SYNTAX));
    request["reviewer_slot"] = json!("slot_acp");
    assert_eq!(
        fixture
            .client
            .workflow_prepare(request, policy(&SYNTAX, 2, false))
            .await,
        Err(remote("workflow_slot_unavailable"))
    );
    let mut request = request_with(&fixture, "taskboard_self_review", &track_a(&SYNTAX));
    request["reviewer_slot"] = json!("slot_b");
    assert_eq!(
        fixture
            .client
            .workflow_prepare(request, policy(&SYNTAX, 2, false))
            .await,
        Err(remote("workflow_request_invalid"))
    );
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
}

/// P02 through Control: a dropped prohibition and a requirement that cites
/// no request span (invented scope) are rejected with their codes, and
/// nothing is admitted, so the corrected request under the same key is
/// prepared fresh. Owned paths are not source-traced; the operator
/// policy's writable roots bound them instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_contract_that_drops_a_prohibition_or_invents_scope_is_rejected() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut dropped = track_a(&SYNTAX);
    dropped
        .requirements
        .retain(|requirement| requirement.requirement_id != "r_manifest");
    let dropped = request_with(&fixture, "taskboard_rejected", &dropped);
    let mut invented = request_with(&fixture, "taskboard_rejected", &track_a(&SYNTAX));
    invented["task_specs"][0]["requirements"]
        .as_array_mut()
        .expect("requirements")
        .push(json!({
            "requirement_id": "r_admin",
            "statement": "Add an administration page.",
            "force": "must",
            "source_refs": [],
            "contract_excerpts": [],
            "literals": [],
            "acceptance_check_ids": ["c_syntax"],
        }));
    for (request, expected) in [
        (dropped, "dropped_prohibition"),
        (invented, "invented_scope"),
    ] {
        let rejected = fixture
            .client
            .workflow_prepare(request, policy(&SYNTAX, 2, false))
            .await
            .expect("prepare");
        assert_eq!(rejected["outcome"], "rejected", "{rejected}");
        assert_eq!(rejected["error_code"], "workflow_contract_rejected");
        let codes = violation_codes(&rejected);
        assert!(
            codes.iter().any(|code| code.contains(expected)),
            "{expected} missing from {codes:?}"
        );
        assert!(rejected.get("start_contract").is_none());
    }
    let prepared = fixture
        .prepare(
            request_with(&fixture, "taskboard_rejected", &track_a(&SYNTAX)),
            policy(&SYNTAX, 2, false),
        )
        .await;
    assert_eq!(prepared["duplicate"], false);
    fixture.shutdown().await;
}

/// No compiler route is wired: a request without compiled TaskSpecs stops
/// with an explicit code instead of guessing a decomposition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uncompiled_request_needs_a_compiler_route() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut request = request_with(&fixture, "taskboard_uncompiled", &track_a(&SYNTAX));
    request["task_specs"] = json!([]);
    assert_eq!(
        fixture
            .client
            .workflow_prepare(request, policy(&SYNTAX, 2, false))
            .await,
        Err(remote("workflow_compiler_unavailable"))
    );
    fixture.shutdown().await;
}

/// P04: an admitted request is immutable; a changed request under the same
/// key is refused and the admitted contracts stay byte-identical.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_request_cannot_overwrite_an_admitted_one() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let request = request_with(&fixture, "taskboard_admitted", &track_a(&SYNTAX));
    let prepared = fixture
        .prepare(request.clone(), policy(&SYNTAX, 2, false))
        .await;
    assert_eq!(
        fixture
            .client
            .workflow_prepare(request.clone(), policy(&SYNTAX, 1, false))
            .await,
        Err(remote("workflow_request_conflict"))
    );
    let mut fewer_turns = track_a(&SYNTAX);
    fewer_turns.max_turns = 2;
    assert_eq!(
        fixture
            .client
            .workflow_prepare(
                request_with(&fixture, "taskboard_admitted", &fewer_turns),
                policy(&SYNTAX, 2, false),
            )
            .await,
        Err(remote("workflow_request_conflict"))
    );
    let replay = fixture.prepare(request, policy(&SYNTAX, 2, false)).await;
    assert_eq!(replay["duplicate"], true);
    assert_eq!(replay["contracts"], prepared["contracts"]);
    fixture.shutdown().await;
}

/// S02 and S03: while one workflow's leases hold both worker slots, a
/// second workflow blocks with `slot_ineligible` and takes no lease; once
/// the first is cancelled and its leases end, the second runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_free_slot_a_workflow_blocks_until_one_is_released() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&json!({"steps": {
        "track_a/implement_1": {"write": good_files(&["src/server.mjs", "src/store.mjs"]),
            "delay_ms": HOLDING_TURN_MS},
        "track_b/implement_1": {"write": good_files(&["public/app.mjs"]),
            "delay_ms": HOLDING_TURN_MS},
        "store/implement_1": {"write": good_files(&["src/store.mjs"])},
        "store/review": review_pass(),
    }}));
    let holder = fixture
        .prepare(
            request_with(&fixture, "taskboard_holder", &track_a(&SYNTAX)),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let holder_id = workflow_id(&holder);
    fixture.start_workflow(&holder).await;
    fixture.wait_for_turn("track_a/implement_1").await;
    fixture.wait_for_turn("track_b/implement_1").await;

    let waiting = fixture
        .prepare(
            store_request(
                "store_waiting",
                &fixture.head,
                "cooperate",
                &store_task(&SYNTAX),
                false,
            ),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let waiting_id = workflow_id(&waiting);
    let contract = waiting["start_contract"].as_str().expect("start contract");
    let blocked = fixture
        .client
        .workflow_start(contract, Uuid::new_v4())
        .await
        .expect("start");
    assert_eq!(blocked["phase"], "blocked");
    assert_eq!(
        blocked["parallelism"],
        json!({"plan": "blocked", "reason": "slot_ineligible"})
    );
    let status = fixture.status(waiting_id).await;
    assert_eq!(status["blocked_reason"], "slot_ineligible");
    assert_eq!(status["leases"], json!([]));
    assert!(fixture.started("store/implement_1").is_empty());

    fixture
        .client
        .workflow_cancel(holder_id)
        .await
        .expect("cancel holder");
    let holder_status = fixture.wait_phase(holder_id, &["cancelled"]).await;
    for lease in holder_status["leases"].as_array().expect("leases") {
        assert!(
            lease["state"] == "released" || lease["state"] == "revoked",
            "lease left open: {lease}"
        );
    }
    let resumed = fixture
        .client
        .workflow_start(contract, Uuid::new_v4())
        .await
        .expect("restart");
    assert_ne!(resumed["phase"], "blocked", "{resumed}");
    let status = fixture
        .wait_phase(waiting_id, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}
