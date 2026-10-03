#![cfg(feature = "test_helpers")]
//! Operator control of a running workflow (ADR 031 §4): pause and prompt
//! inspection, resume on one slot, cancellation mid-turn, and a daemon
//! restart that must not repeat a completed turn. The fast syntax suite
//! stands in for the TaskBoard suites; these tests prove control flow, not
//! TaskBoard behavior. Synthetic workers: offline orchestration evidence.

mod workflow_support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, good_files, phase, policy, remote,
    review_pass, session_id, store_request, store_task, task_view, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const SLOW_TURN_MS: u64 = 2_000;
/// Longer than the service's shutdown grace, so a restart interrupts it.
const INTERRUPTED_TURN_MS: u64 = 15_000;
/// Longer than the whole cancellation test may take.
const CANCELLED_TURN_MS: u64 = 30_000;

fn track_a_files() -> Value {
    good_files(&["src/server.mjs", "src/store.mjs"])
}

fn track_b_files() -> Value {
    good_files(&["public/app.mjs"])
}

fn at_ms(record: &Value) -> u64 {
    record["at_ms"].as_u64().expect("at_ms")
}

fn finished(fixture: &WorkflowFixture, key: &str) -> Vec<Value> {
    fixture
        .log()
        .into_iter()
        .filter(|record| record["event"] == "finished" && record["key"] == key)
        .collect()
}

fn syntax_request(fixture: &WorkflowFixture, request_key: &str) -> Value {
    cooperative_request(
        request_key,
        &fixture.head,
        &track_a(&SYNTAX),
        &track_b(&SYNTAX),
        &SYNTAX,
    )
}

/// A prepared workflow remains addressable after more than one lookup page
/// of newer workflows has been admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oldest_contract_can_start_after_two_hundred_fifty_six_newer_workflows() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let first = fixture
        .prepare(
            store_request(
                "oldest_prepared",
                &fixture.head,
                "cooperate",
                &store_task(&SYNTAX),
                false,
            ),
            policy(&SYNTAX, 1, false),
        )
        .await;
    for index in 0..256 {
        let request_key = format!("newer_{index}");
        fixture
            .prepare(
                store_request(
                    &request_key,
                    &fixture.head,
                    "cooperate",
                    &store_task(&SYNTAX),
                    false,
                ),
                policy(&SYNTAX, 1, false),
            )
            .await;
    }
    let oldest = workflow_id(&first);
    fixture.restart_after_crash(oldest).await;
    let recovered = fixture.status(oldest).await;
    assert_eq!(phase(&recovered), "blocked", "{recovered:#}");
    assert_eq!(recovered["blocked_reason"], "daemon_restart");
    let contract = first["start_contract"].as_str().expect("contract");
    let started = fixture
        .client
        .workflow_start(contract, Uuid::new_v4())
        .await
        .expect("oldest contract remains discoverable");
    assert_eq!(started["workflow_id"], first["workflow_id"]);
    fixture.shutdown().await;
}

/// S02 and the pause walkthrough: with one parallel slot the tracks run
/// one after the other; a pause lets the running turn finish, keeps the
/// other track unstarted, and exposes that track's exact next prompt,
/// which is byte-identical to what the worker receives after resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_session_shows_its_exact_next_prompt_and_one_slot_runs_sequentially() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&json!({"steps": {
        "track_a/implement_1": {"write": track_a_files(), "delay_ms": SLOW_TURN_MS},
        "track_b/implement_1": {"write": track_b_files()},
        "track_a/review_1": review_pass(),
        "track_b/review_1": review_pass(),
    }}));
    let prepared = fixture
        .prepare(
            syntax_request(&fixture, "taskboard_paused"),
            policy(&SYNTAX, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    let contract = prepared["start_contract"].as_str().expect("start contract");
    let start_request = Uuid::new_v4();
    let started = fixture
        .client
        .workflow_start(contract, start_request)
        .await
        .expect("start");
    assert_eq!(started["parallelism"], json!({"plan": "sequential"}));
    // A repeated start request is answered from the running coordinator.
    let repeated = fixture
        .client
        .workflow_start(contract, start_request)
        .await
        .expect("repeated start");
    assert_eq!(repeated["duplicate"], true);
    assert_eq!(
        fixture
            .client
            .workflow_start(contract, Uuid::new_v4())
            .await,
        Err(remote("workflow_already_running"))
    );

    fixture.wait_for_turn("track_a/implement_1").await;
    let paused = fixture
        .client
        .workflow_pause(workflow)
        .await
        .expect("pause");
    assert_eq!(paused["signalled"], true);
    let status = fixture.wait_phase(workflow, &["paused"]).await;
    assert_eq!(status["last_run"]["end"], json!({"end": "paused"}));
    // The running turn finished; the next one never started.
    assert_eq!(finished(&fixture, "track_a/implement_1").len(), 1);
    assert!(fixture.started("track_b/implement_1").is_empty());

    let session_a = session_id(workflow, "track_a", "slot_a");
    let session_b = session_id(workflow, "track_b", "slot_b");
    let inspect_a = fixture
        .client
        .workflow_session_inspect(session_a)
        .await
        .expect("inspect a");
    assert_eq!(inspect_a["session"]["attempts"], 1);
    // Prompt text is not retained by default: the sent prompt is known by
    // its digest only.
    let sent_a = fixture
        .client
        .workflow_session_prompt(session_a, 0)
        .await
        .expect("sent prompt");
    assert_eq!(sent_a["source"], "sent");
    assert!(sent_a["text"].is_null());
    assert_eq!(sent_a["unavailable_reason"], "prompt_not_retained");
    assert_eq!(
        sent_a["rendered_prompt_sha256"],
        inspect_a["attempts"][0]["rendered_prompt_sha256"]
    );
    // A's next prompt depends on the gate outcome, so none is promised.
    let next_a = fixture
        .client
        .workflow_session_prompt(session_a, 1)
        .await
        .expect("next prompt of a");
    assert_eq!(next_a["source"], "next");
    assert!(next_a["text"].is_null());
    assert_eq!(
        next_a["unavailable_reason"],
        "next_prompt_depends_on_gate_outcome"
    );
    let inspect_b = fixture
        .client
        .workflow_session_inspect(session_b)
        .await
        .expect("inspect b");
    assert_eq!(inspect_b["session"]["attempts"], 0);
    let next_b = fixture
        .client
        .workflow_session_prompt(session_b, 0)
        .await
        .expect("next prompt of b");
    assert_eq!(next_b["source"], "next");
    assert_eq!(next_b["total"], 1);
    let next_text = next_b["text"].as_str().expect("next text").to_string();
    assert!(next_text.contains("public/app.mjs"));
    assert_eq!(
        fixture.client.workflow_session_prompt(session_b, 1).await,
        Err(remote("workflow_request_invalid"))
    );

    // Resume: B now runs, after A, with exactly the inspected prompt.
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    let b_started = &fixture.started("track_b/implement_1");
    assert_eq!(b_started.len(), 1);
    assert_eq!(b_started[0]["prompt"].as_str(), Some(next_text.as_str()));
    assert_eq!(fixture.started("track_a/implement_1").len(), 1);
    let a_finished = at_ms(&finished(&fixture, "track_a/implement_1")[0]);
    assert!(at_ms(&b_started[0]) >= a_finished);
    assert_eq!(fixture.checkout_status(), "");
    fixture.record_evidence("pause_inspect", workflow).await;
    fixture.shutdown().await;
}

/// V05: a cancellation stops the in-flight dispatch; nothing the worker
/// produced is collected, reviewed, or accepted, and the workflow cannot
/// be restarted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_turn_is_stopped_and_nothing_is_promoted() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&json!({"steps": {
        "store/implement_1": {"write": good_files(&["src/store.mjs"]),
            "delay_ms": CANCELLED_TURN_MS},
        "store/review_1": review_pass(),
    }}));
    let request = store_request(
        "store_cancelled",
        &fixture.head,
        "cooperate",
        &store_task(&SYNTAX),
        false,
    );
    let prepared = fixture.prepare(request, policy(&SYNTAX, 2, false)).await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    fixture.wait_for_turn("store/implement_1").await;
    let began = Instant::now();
    let cancelled = fixture
        .client
        .workflow_cancel(workflow)
        .await
        .expect("cancel");
    assert_eq!(cancelled["signalled"], true);
    let status = fixture.wait_phase(workflow, &["cancelled"]).await;
    assert!(
        began.elapsed() < Duration::from_millis(CANCELLED_TURN_MS),
        "the cancellation waited for the turn"
    );
    assert_eq!(status["last_run"]["end"], json!({"end": "cancelled"}));
    assert!(finished(&fixture, "store/implement_1").is_empty());
    assert!(fixture.started("store/review_1").is_empty());
    assert!(task_view(&status, "store")["accepted_candidate"].is_null());
    assert!(status["receipts"]["selection"].is_null());
    assert!(status["receipts"]["integration"].is_null());
    assert!(
        status["receipts"]["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|candidate| candidate["admitted"] == false),
        "a cancelled turn produced an admitted candidate: {status:#}"
    );
    for lease in status["leases"].as_array().expect("leases") {
        assert!(
            lease["state"] == "released" || lease["state"] == "revoked",
            "lease left open: {lease}"
        );
    }
    // Cancelling again is idempotent; a cancelled workflow never restarts.
    let again = fixture
        .client
        .workflow_cancel(workflow)
        .await
        .expect("repeated cancel");
    assert_eq!(again["phase"], "cancelled");
    assert_eq!(again["signalled"], false);
    let contract = prepared["start_contract"].as_str().expect("start contract");
    assert_eq!(
        fixture
            .client
            .workflow_start(contract, Uuid::new_v4())
            .await,
        Err(remote("workflow_invalid_phase"))
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

/// S04: a daemon restart interrupts one track mid-turn. A graceful
/// shutdown cancels the turn and pauses the workflow; a crash leaves it
/// `running`, which the next daemon blocks with `daemon_restart`. Either
/// way nothing resumes until the operator starts it again, and the resumed
/// run reuses the other track's recorded candidate instead of repeating
/// its turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_stops_the_workflow_and_the_resumed_run_repeats_no_completed_turn() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let quick_a = json!({"write": track_a_files()});
    fixture.write_script(&json!({"steps": {
        "track_a/implement_1": {"sequence": [
            {"write": track_a_files(), "delay_ms": INTERRUPTED_TURN_MS},
            quick_a.clone(),
        ]},
        "track_a/implement_2": quick_a,
        "track_b/implement_1": {"write": track_b_files()},
        "track_a/review_1": review_pass(),
        "track_b/review_1": review_pass(),
    }}));
    let prepared = fixture
        .prepare(
            syntax_request(&fixture, "taskboard_restart"),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    fixture.wait_for_turn("track_a/implement_1").await;
    fixture
        .wait_until(workflow, |status| {
            status["receipts"]["candidates"]
                .as_array()
                .is_some_and(|candidates| {
                    candidates
                        .iter()
                        .any(|candidate| candidate["task_key"] == "track_b")
                })
        })
        .await;

    let began = Instant::now();
    fixture.restart().await;
    assert!(
        began.elapsed() < Duration::from_millis(INTERRUPTED_TURN_MS),
        "the shutdown waited for the turn"
    );
    let status = fixture.status(workflow).await;
    assert_eq!(phase(&status), "paused", "status: {status:#}");
    assert_eq!(status["running_here"], false);
    assert!(finished(&fixture, "track_a/implement_1").is_empty());

    fixture.restart_after_crash(workflow).await;
    let status = fixture.status(workflow).await;
    assert_eq!(phase(&status), "blocked", "status: {status:#}");
    assert_eq!(status["blocked_reason"], "daemon_restart");
    assert_eq!(status["running_here"], false);

    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    // Track B's completed turn and its candidate exist exactly once.
    let b_keys: Vec<String> = fixture
        .started_keys()
        .into_iter()
        .filter(|key| key.starts_with("track_b/implement"))
        .collect();
    assert_eq!(b_keys, ["track_b/implement_1"]);
    let b_candidates = status["receipts"]["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|candidate| candidate["task_key"] == "track_b")
        .count();
    assert_eq!(b_candidates, 1);
    // The interrupted lease was settled, never left reserved.
    for lease in status["leases"].as_array().expect("leases") {
        assert_ne!(lease["state"], "quarantined", "lease {lease}");
        assert_ne!(lease["state"], "active", "lease {lease}");
    }
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

/// `slots list`: the configured slots, their routes, and which are
/// eligible for writable fixture work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slots_list_reports_routes_and_writable_eligibility() {
    let fixture = WorkflowFixture::start(FixtureOptions::with_acp_slot()).await;
    let slots = fixture.client.workflow_slots().await.expect("slots");
    assert_eq!(slots["evidence_class"], "fixture");
    let routes = slots["routes"].as_array().expect("routes");
    let executes = |slot: &str| {
        routes
            .iter()
            .find(|route| route["slot_id"] == slot)
            .map(|route| route["executes"].clone())
    };
    assert_eq!(executes("slot_a"), Some(json!(true)));
    assert_eq!(executes("slot_b"), Some(json!(true)));
    assert_eq!(executes("slot_acp"), Some(json!(false)));
    let eligible = slots["writable"]["eligible"].as_array().expect("eligible");
    assert!(eligible.contains(&json!("slot_a")));
    assert!(eligible.contains(&json!("slot_b")));
    assert!(!eligible.contains(&json!("slot_acp")));
    assert!(
        slots["writable"]["ineligible"]
            .as_array()
            .expect("ineligible")
            .iter()
            .any(|entry| entry[0] == "slot_acp")
    );
    fixture.shutdown().await;
}
