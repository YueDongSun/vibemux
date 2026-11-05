//! The desktop Ask boundary through public services and continuously drained
//! capacity-one worker channels. These fixtures never invoke a vendor CLI.

#[path = "support/native_ask_support.rs"]
mod support;

use std::{
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

use serde_json::json;
use uuid::Uuid;
use vibemux_desktop::{
    desktop_settings::SettingsPatch,
    ui_contract::{AskHarness, LocalAskPhase, ObserverState, OutputState, UiEvent},
};
use vibemux_harness::dispatch::DispatchPhase;

use support::{Fixture, Gate, PRIVATE_PROMPT, PRIVATE_REPLY, Program, WAIT};

#[test]
fn consent_and_available_routes_are_required_in_rust() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .submit(Uuid::new_v4())
            .expect_err("consent required")
            .code,
        "ask_consent_required"
    );
    assert_eq!(fixture.spawner.submissions(), 0);
    for forged in [
        json!({"ask_confirmed_projects":["forged"]}),
        json!({"project_folders":["forged"]}),
    ] {
        assert!(serde_json::from_value::<SettingsPatch>(forged).is_err());
    }
    let public = serde_json::to_value(fixture.state.public_settings()).expect("public settings");
    assert!(public.get("project_folders").is_none());
    assert!(public.get("ask_confirmed_projects").is_none());
    assert!(public.get("ask_consent_identities").is_none());

    fixture.consent();
    fixture.services.available.store(false, Ordering::Release);
    assert_eq!(
        fixture
            .submit(Uuid::new_v4())
            .expect_err("route unavailable")
            .code,
        "harness_not_available"
    );
    assert_eq!(fixture.spawner.submissions(), 0);
    fixture.services.available.store(true, Ordering::Release);
    fixture.completed(Uuid::new_v4());
    assert_eq!(fixture.spawner.submissions(), 1);
}

#[test]
fn listing_and_selection_never_perform_setup_or_cancel_another_ask() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture
        .spawner
        .enqueue(Program::WaitCancel(DispatchPhase::Cancelled));
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Running);
    let other = fixture.directory.path().join("another project 中文");
    std::fs::create_dir(&other).expect("another temporary project");
    let other_key = fixture.state.register_project(&other).expect("registered");
    fixture.state.select_project(&other_key).expect("selected");
    fixture.state.projects_list().expect("list");
    fixture.state.ask_targets(&other_key).expect("targets");
    assert_eq!(fixture.services.setups.load(Ordering::Acquire), 0);
    assert_eq!(fixture.services.lifecycle.load(Ordering::Acquire), 0);
    assert_eq!(fixture.spawner.cancellations.load(Ordering::Acquire), 0);
    assert!(fixture.state.asks.has_active(&fixture.project_key));
    assert_eq!(
        fixture
            .state
            .forget_project(&fixture.project_key)
            .expect_err("active project cannot be forgotten")
            .code,
        "ask_busy"
    );
    assert_eq!(
        fixture
            .state
            .ask_setup(&fixture.project_key)
            .expect_err("active ownership")
            .code,
        "ask_busy"
    );
    fixture.state.ask_setup(&other_key).expect("explicit setup");
    assert_eq!(fixture.services.setups.load(Ordering::Acquire), 1);
}

#[test]
fn concurrent_duplicate_during_route_preflight_returns_the_same_receipt() {
    let fixture = Fixture::new();
    fixture.consent();
    let gate = Arc::new(Gate::default());
    *fixture.services.target_gate.lock().expect("target gate") = Some(gate.clone());
    let key = Uuid::new_v4();
    let first_state = fixture.state.clone();
    let first_project = fixture.project_key.clone();
    let first = std::thread::spawn(move || {
        first_state.ask_submit(
            &first_project,
            AskHarness::Claude,
            PRIVATE_PROMPT.into(),
            key,
        )
    });
    gate.wait_entered();
    let second_state = fixture.state.clone();
    let second_project = fixture.project_key.clone();
    let (returned, second_result) = mpsc::channel();
    let second = std::thread::spawn(move || {
        let result = second_state.ask_submit(
            &second_project,
            AskHarness::Claude,
            PRIVATE_PROMPT.into(),
            key,
        );
        returned.send(result).expect("test waits");
    });
    // With the first invocation held inside route validation, the duplicate
    // must wait for or share its reservation, not fail as a conflicting job.
    let early = second_result.recv_timeout(Duration::from_millis(200)).ok();
    gate.release();
    let first = first
        .join()
        .expect("first invocation")
        .expect("first receipt");
    let second_receipt =
        early.unwrap_or_else(|| second_result.recv_timeout(WAIT).expect("second invocation"));
    second.join().expect("second thread");
    let second_receipt = second_receipt.expect("a duplicate is not a project-busy error");
    assert_eq!(first.ask_id, second_receipt.ask_id);
    assert_eq!(first.request_id, second_receipt.request_id);
    assert!(second_receipt.duplicate);
    fixture
        .sink
        .wait_phase(&first.ask_id, LocalAskPhase::Finished);
    assert_eq!(fixture.spawner.submissions(), 1);
    assert_eq!(fixture.spawner.observations(), 0);
}

#[test]
fn a_reused_submission_key_cannot_change_prompt_harness_or_project() {
    let fixture = Fixture::new();
    fixture.consent();
    let key = Uuid::new_v4();
    fixture.completed(key);
    let changed_prompt = fixture
        .state
        .ask_submit(
            &fixture.project_key,
            AskHarness::Claude,
            "changed".into(),
            key,
        )
        .expect_err("prompt conflict");
    let changed_harness = fixture
        .state
        .ask_submit(
            &fixture.project_key,
            AskHarness::Codex,
            PRIVATE_PROMPT.into(),
            key,
        )
        .expect_err("harness conflict");
    let other = fixture.directory.path().join("other");
    std::fs::create_dir(&other).expect("other project");
    let other_key = fixture.state.register_project(&other).expect("register");
    let changed_project = fixture
        .state
        .ask_submit(&other_key, AskHarness::Claude, PRIVATE_PROMPT.into(), key)
        .expect_err("project conflict");
    for error in [changed_prompt, changed_harness, changed_project] {
        assert_eq!(error.code, "ask_submission_conflict");
    }
    assert_eq!(fixture.spawner.submissions(), 1);
}

#[test]
fn a_lost_acknowledgement_reconciles_the_original_request_without_resubmitting() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::Uncertain);
    fixture.spawner.enqueue_observer(Program::Complete);
    let key = Uuid::new_v4();
    let original = fixture.submit(key).expect("local receipt");
    fixture
        .sink
        .wait_phase(&original.ask_id, LocalAskPhase::Uncertain);
    fixture.spawner.wait_finished(1);
    for _ in 0..3 {
        fixture
            .state
            .asks
            .status(&original.ask_id)
            .expect("cached status");
        fixture
            .state
            .asks
            .reply(&original.ask_id)
            .expect("cached reply");
        fixture.state.projects_list().expect("reload discovery");
    }
    assert_eq!(
        fixture.spawner.observations(),
        0,
        "cached reads do not retry or attach implicitly"
    );
    fixture.services.available.store(false, Ordering::Release);
    let repeated = fixture.reconcile(key);
    fixture
        .sink
        .wait_phase(&original.ask_id, LocalAskPhase::Finished);
    fixture.spawner.wait_finished(2);
    assert_eq!(repeated.ask_id, original.ask_id);
    assert_eq!(repeated.request_id, original.request_id);
    assert!(repeated.duplicate);
    assert_eq!(fixture.spawner.submissions(), 1);
    assert_eq!(fixture.spawner.observations(), 1);
    let request_id = Uuid::parse_str(&original.request_id).expect("request id");
    assert!(
        fixture
            .spawner
            .calls()
            .iter()
            .all(|call| call.request_id == request_id)
    );
}

#[test]
fn display_eviction_keeps_submission_replay_protection() {
    let fixture = Fixture::new();
    fixture.consent();
    let first_key = Uuid::new_v4();
    let first = fixture.completed(first_key);
    for _ in 0..32 {
        fixture.completed(Uuid::new_v4());
    }
    let displayed = fixture.state.asks.ids(&fixture.project_key);
    assert_eq!(displayed.len(), 32);
    assert!(!displayed.contains(&first.ask_id));
    assert_eq!(
        fixture
            .state
            .asks
            .status(&first.ask_id)
            .expect_err("evicted display")
            .code,
        "ask_not_available"
    );
    let repeated = fixture.submit(first_key).expect("retained binding");
    assert_eq!(repeated.ask_id, first.ask_id);
    assert_eq!(repeated.request_id, first.request_id);
    assert!(repeated.duplicate);
    assert_eq!(fixture.spawner.submissions(), 33);
    assert_eq!(fixture.spawner.observations(), 0);
    assert_eq!(
        fixture
            .state
            .ask_submit(
                &fixture.project_key,
                AskHarness::Claude,
                "changed after eviction".into(),
                first_key
            )
            .expect_err("tombstone conflict")
            .code,
        "ask_submission_conflict"
    );
}

#[test]
fn deduplication_budget_exhaustion_refuses_new_work_without_forgetting_old_keys() {
    let fixture = Fixture::new();
    fixture.consent();
    let first_key = Uuid::new_v4();
    let first = fixture.completed(first_key);
    for _ in 1..1024 {
        fixture.completed(Uuid::new_v4());
    }
    let error = fixture
        .submit(Uuid::new_v4())
        .expect_err("finite replay protection budget");
    assert_eq!(error.code, "ask_dedup_capacity");
    assert_eq!(fixture.spawner.submissions(), 1024);
    let old = fixture
        .submit(first_key)
        .expect("old binding survives a full budget");
    assert_eq!(old.ask_id, first.ask_id);
    assert_eq!(old.request_id, first.request_id);
    assert!(old.duplicate);
    assert_eq!(fixture.spawner.submissions(), 1024);
    assert_eq!(fixture.state.asks.ids(&fixture.project_key).len(), 32);
}

#[test]
fn uncertain_ownership_is_neither_evicted_nor_forgotten() {
    let fixture = Fixture::new();
    fixture.consent();
    for _ in 0..31 {
        fixture.completed(Uuid::new_v4());
    }
    fixture.spawner.enqueue(Program::Uncertain);
    let uncertain = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&uncertain.ask_id, LocalAskPhase::Uncertain);
    assert_eq!(
        fixture
            .submit(Uuid::new_v4())
            .expect_err("unresolved ownership")
            .code,
        "ask_busy"
    );
    assert_eq!(
        fixture
            .state
            .forget_project(&fixture.project_key)
            .expect_err("cannot forget unresolved work")
            .code,
        "ask_busy"
    );
    assert_eq!(
        fixture
            .state
            .asks
            .forget_display(&fixture.project_key)
            .expect_err("cannot discard unresolved display")
            .code,
        "ask_busy"
    );
    assert_eq!(fixture.state.asks.ids(&fixture.project_key).len(), 32);
    assert!(
        fixture
            .state
            .asks
            .ids(&fixture.project_key)
            .contains(&uncertain.ask_id)
    );
}

#[test]
fn collector_drains_a_capacity_one_burst_without_any_frontend_polling() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::Burst(256));
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    // Waiting on fixture worker completion and invalidations does not read
    // status/reply. Every send would deadlock if drainage required polling.
    fixture.spawner.wait_finished(1);
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
    let reply = fixture.state.asks.reply(&receipt.ask_id).expect("reply");
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert_eq!(reply.reply, PRIVATE_REPLY);
    assert_eq!(reply.prompt, PRIVATE_PROMPT);
    assert!(status.terminal_observed && status.delivery_complete);
    assert_eq!(status.revision, reply.revision);
    let projects = fixture
        .state
        .projects_list()
        .expect("webview reload discovery");
    assert!(
        projects
            .iter()
            .any(|project| project.ask_ids.contains(&receipt.ask_id))
    );
    assert_eq!(fixture.spawner.submissions(), 1);
}

#[test]
fn cancellation_before_admission_is_not_fabricated_canonical_cancellation() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::CancelBeforeAdmission);
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture.spawner.wait_calls(1);
    fixture
        .state
        .asks
        .cancel(&receipt.ask_id)
        .expect("cancel local submission");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Rejected);
    fixture.spawner.wait_finished(1);
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert!(!status.terminal_observed);
    assert!(status.canonical.is_none());
    assert!(status.cancel_requested);
    assert!(!fixture.state.asks.has_active(&fixture.project_key));
    assert_eq!(fixture.spawner.submissions(), 1);
    assert_eq!(fixture.spawner.cancellations.load(Ordering::Acquire), 1);
}

#[test]
fn completion_racing_cancellation_keeps_the_daemon_completed_outcome() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture
        .spawner
        .enqueue(Program::WaitCancel(DispatchPhase::Completed));
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Running);
    fixture
        .state
        .asks
        .cancel(&receipt.ask_id)
        .expect("cancel request");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Cancelling);
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert_eq!(
        status.canonical.expect("canonical").phase,
        DispatchPhase::Completed
    );
    assert!(status.terminal_observed && status.cancel_requested);
    assert_eq!(fixture.spawner.submissions(), 1);
}

#[test]
fn shutdown_closes_a_full_terminal_channel_within_the_owned_budget() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::FinalOnStop);
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Running);
    let started = Instant::now();
    let unresolved = fixture
        .state
        .asks
        .shutdown_with_limits(Duration::from_millis(20), Duration::from_secs(1));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown exceeded its bounded reconciliation/join budgets"
    );
    fixture.spawner.wait_finished(1);
    assert_eq!(unresolved, 1);
    let status = fixture
        .state
        .asks
        .status(&receipt.ask_id)
        .expect("retained status");
    assert!(!status.terminal_observed);
    assert_eq!(status.phase, LocalAskPhase::Uncertain);
    assert_eq!(
        status.observer_error.as_deref(),
        Some("ask_shutdown_unreconciled")
    );
}

#[test]
fn completed_unavailable_output_preserves_prior_text_identity_and_receipt_metadata() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::RunningThenLost);
    fixture
        .spawner
        .enqueue_observer(Program::CompletedUnavailable);
    let key = Uuid::new_v4();
    let receipt = fixture.submit(key).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Uncertain);
    let before = fixture.state.asks.status(&receipt.ask_id).expect("before");
    let canonical_before = before.canonical.expect("canonical before loss");
    fixture.spawner.wait_finished(1);
    fixture.reconcile(key);
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    let reply = fixture.state.asks.reply(&receipt.ask_id).expect("reply");
    let canonical = status.canonical.as_ref().expect("canonical completed");
    assert_eq!(canonical.task_id, canonical_before.task_id);
    assert_eq!(canonical.run_id, canonical_before.run_id);
    assert_eq!(canonical.request_id, receipt.request_id);
    assert_eq!(
        canonical.sequence,
        Some(17),
        "re-observation must retain the confirmed admission receipt metadata"
    );
    assert_eq!(canonical.phase, DispatchPhase::Completed);
    assert!(status.terminal_observed);
    assert_eq!(status.output_state, OutputState::Unavailable);
    assert!(!status.delivery_complete);
    assert_eq!(reply.reply, PRIVATE_REPLY);
    assert_eq!(reply.prompt, PRIVATE_PROMPT);
    assert!(status.reply_available);
}

#[test]
fn public_metadata_and_events_never_contain_prompt_or_reply_text() {
    let fixture = Fixture::new();
    fixture.consent();
    let receipt = fixture.completed(Uuid::new_v4());
    let metadata = json!({
        "receipt": receipt,
        "status": fixture.state.asks.status(&receipt.ask_id).expect("status"),
        "projects": fixture.state.projects_list().expect("projects"),
        "events": fixture.sink.all(),
    })
    .to_string();
    assert!(!metadata.contains(PRIVATE_PROMPT));
    assert!(!metadata.contains(PRIVATE_REPLY));
    let text = fixture
        .state
        .asks
        .reply(&receipt.ask_id)
        .expect("explicit text accessor");
    assert_eq!(text.prompt, PRIVATE_PROMPT);
    assert_eq!(text.reply, PRIVATE_REPLY);
}

#[test]
fn stale_canonical_snapshots_do_not_overwrite_text_or_publish_new_revisions() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::StaleSnapshot);
    let receipt = fixture.completed(Uuid::new_v4());
    let revisions = fixture
        .sink
        .all()
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::AskChanged {
                ask_id, revision, ..
            } if ask_id == receipt.ask_id => Some(revision),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        revisions.len(),
        2,
        "only the two accepted snapshots should invalidate the view"
    );
    assert!(revisions.windows(2).all(|pair| pair[0] < pair[1]));
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    let reply = fixture.state.asks.reply(&receipt.ask_id).expect("reply");
    assert_eq!(reply.reply, PRIVATE_REPLY);
    assert_eq!(reply.revision, status.revision);
    assert_eq!(Some(&status.revision), revisions.last());
}

#[test]
fn a_newer_shorter_partial_snapshot_replaces_the_previous_draft() {
    let fixture = Fixture::new();
    fixture.consent();
    let gate = Arc::new(Gate::default());
    fixture
        .spawner
        .enqueue(Program::ReplacingPartial(gate.clone()));
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    gate.wait_entered();
    fixture.sink.wait_for(|events| events.iter().filter(|event| matches!(event,
        UiEvent::AskChanged { ask_id, phase: LocalAskPhase::Running, .. } if ask_id == &receipt.ask_id)).count() >= 2);
    let partial = fixture
        .state
        .asks
        .reply(&receipt.ask_id)
        .expect("newer partial");
    gate.release();
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
    assert_eq!(
        partial.reply, "short",
        "full snapshots replace text even when the correction is shorter"
    );
}

#[test]
fn mismatched_worker_identity_cannot_complete_or_release_the_owned_ask() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::WrongIdentity);
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Uncertain);
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert_eq!(status.observer_state, ObserverState::Failed);
    assert!(!status.terminal_observed);
    assert!(status.canonical.is_none());
    assert!(fixture.state.asks.has_active(&fixture.project_key));
    assert_eq!(fixture.spawner.submissions(), 1);
}

#[test]
fn recovery_pending_stays_owned_and_shutdown_never_acknowledges_it() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::RecoveryPending);
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Uncertain);
    fixture.spawner.wait_finished(1);
    assert_eq!(
        fixture
            .state
            .asks
            .cancel(&receipt.ask_id)
            .expect_err("recovery needs its separate decision")
            .code,
        "ask_recovery_pending"
    );
    let unresolved = fixture
        .state
        .asks
        .shutdown_with_limits(Duration::ZERO, Duration::from_secs(1));
    assert_eq!(unresolved, 1);
    assert_eq!(fixture.spawner.observations(), 0);
    assert_eq!(fixture.spawner.cancellations.load(Ordering::Acquire), 0);
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert!(!status.terminal_observed);
    assert_eq!(
        status.canonical.expect("canonical").phase,
        DispatchPhase::RecoveryPending
    );
}

#[test]
fn explicit_reconciliation_uses_the_owned_request_without_resubmitting_a_prompt() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::Uncertain);
    fixture.spawner.enqueue_observer(Program::Complete);
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Uncertain);
    fixture.spawner.wait_finished(1);
    let deadline = Instant::now() + WAIT;
    while fixture.spawner.observations() == 0 {
        fixture
            .state
            .asks
            .reconcile(&receipt.ask_id)
            .expect("explicit known-ID refresh");
        assert!(
            Instant::now() < deadline,
            "known-ID reconciliation did not attach"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
    assert_eq!(fixture.spawner.submissions(), 1);
    assert_eq!(fixture.spawner.observations(), 1);
    let request_id = Uuid::parse_str(&receipt.request_id).expect("request id");
    assert!(
        fixture
            .spawner
            .calls()
            .iter()
            .all(|call| call.request_id == request_id)
    );
    assert_eq!(
        fixture
            .state
            .asks
            .reconcile(&Uuid::new_v4().to_string())
            .expect_err("unknown IDs are not authority")
            .code,
        "ask_not_available"
    );
}

#[test]
fn shutdown_reconciles_an_owned_ask_after_its_previous_worker_exited() {
    let fixture = Fixture::new();
    fixture.consent();
    fixture.spawner.enqueue(Program::Uncertain);
    fixture
        .spawner
        .enqueue_observer(Program::WaitCancel(DispatchPhase::Cancelled));
    let receipt = fixture.submit(Uuid::new_v4()).expect("receipt");
    fixture
        .sink
        .wait_phase(&receipt.ask_id, LocalAskPhase::Uncertain);
    fixture.spawner.wait_finished(1);
    let unresolved = fixture
        .state
        .asks
        .shutdown_with_limits(Duration::from_secs(1), Duration::from_secs(1));
    assert_eq!(
        unresolved,
        0,
        "app shutdown must reconcile cancellation even after the original observer exits; calls={:?}, cancellations={}, status={:?}",
        fixture.spawner.calls(),
        fixture.spawner.cancellations.load(Ordering::Acquire),
        fixture
            .state
            .asks
            .status(&receipt.ask_id)
            .expect("failure status")
    );
    fixture.spawner.wait_finished(2);
    assert_eq!(fixture.spawner.submissions(), 1);
    assert_eq!(fixture.spawner.observations(), 1);
    assert_eq!(fixture.spawner.cancellations.load(Ordering::Acquire), 1);
    let request_id = Uuid::parse_str(&receipt.request_id).expect("request id");
    assert!(
        fixture
            .spawner
            .calls()
            .iter()
            .all(|call| call.request_id == request_id)
    );
    let status = fixture.state.asks.status(&receipt.ask_id).expect("status");
    assert!(status.terminal_observed);
    assert_eq!(
        status.canonical.expect("canonical").phase,
        DispatchPhase::Cancelled
    );
}
