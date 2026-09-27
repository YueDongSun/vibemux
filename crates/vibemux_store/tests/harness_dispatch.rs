//! Store schema 4 dispatch transactions (ADR 029 §2 and §3, Stage 3 gates).

use std::{collections::BTreeMap, path::Path};

use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchPhase, DispatchRequest, NativeProtocol,
        OutcomeDecision, PromptDigest, Sha256Digest,
        attempt::resource_key,
        capture_budget::{CaptureSummary, KindCounts},
        events::ProcessSummary,
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
    },
};
use vibemux_store::{
    ClaimFence, HarnessDispatchAdmission, HarnessDispatchCommit, HarnessDispatchFinish,
    HarnessDispatchRecord, HarnessDispatchRecovery, QuarantinedDispatch, SqliteStore, StoreError,
};
use vibemux_types::{EventId, Run, RunSpec, RunStatus, Task, TaskStatus};

const FIXTURE_PROMPT: &str = "fixture prompt";

fn open_store(detected: &[&str]) -> (NamedTempFile, SqliteStore) {
    let file = NamedTempFile::new().expect("temporary database");
    let mut store = SqliteStore::open(file.path()).expect("open store");
    let detections: BTreeMap<String, HarnessDetection> = detected
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("1.0.0".to_owned()),
                },
            )
        })
        .collect();
    store
        .commit_harness_snapshot(&detections, "2026-09-27T00:00:00Z", "probe_fixture")
        .expect("detect harnesses");
    (file, store)
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + seconds).expect("fixture timestamp")
}

fn admission(
    harness: AgentKind,
    protocol: NativeProtocol,
    workspace: &str,
) -> HarnessDispatchAdmission {
    HarnessDispatchAdmission {
        request_id: Uuid::new_v4(),
        harness,
        protocol,
        prompt: PromptDigest {
            sha256: Sha256Digest::of(FIXTURE_PROMPT.as_bytes()),
            byte_count: FIXTURE_PROMPT.len() as u64,
        },
        config_sha256: Sha256Digest::of(b"fixture config"),
        resource_key: resource_key(workspace),
        base_commit: "0123456789abcdef".to_owned(),
        timestamp: at(0),
    }
}

fn codex(workspace: &str) -> HarnessDispatchAdmission {
    admission(AgentKind::Codex, NativeProtocol::CodexExec, workspace)
}

fn report(
    request_id: Uuid,
    fence: ClaimFence,
    outcome: AttemptOutcome,
    error_code: Option<DispatchError>,
) -> HarnessDispatchFinish {
    HarnessDispatchFinish {
        request_id,
        fence,
        decision: OutcomeDecision {
            outcome,
            error_code,
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
            transcript_sha256: Sha256Digest::of(b"fixture transcript"),
        },
        timestamp: at(10),
    }
}

fn clean_finish(request_id: Uuid, fence: ClaimFence) -> HarnessDispatchFinish {
    report(request_id, fence, AttemptOutcome::Completed, None)
}

fn admit(store: &mut SqliteStore, admission: &HarnessDispatchAdmission) -> Uuid {
    store
        .admit_harness_dispatch(admission)
        .expect("admit")
        .record
        .request_id
}

fn claim(store: &mut SqliteStore, request_id: Uuid) -> ClaimFence {
    store
        .claim_harness_dispatch(request_id, at(1))
        .expect("claim")
        .fence
}

fn record(store: &SqliteStore, request_id: Uuid) -> HarnessDispatchRecord {
    store
        .harness_dispatch(request_id)
        .expect("dispatch query")
        .expect("dispatch record")
}

fn entities(store: &SqliteStore, record: &HarnessDispatchRecord) -> (Task, Run) {
    let load = |kind: &str, id: String| -> Value {
        store
            .projection(kind, &id)
            .expect("projection query")
            .expect("projection")
    };
    let task = serde_json::from_value(load("task", record.task_id.to_string())).expect("task");
    let run = serde_json::from_value(load("run", record.run_id.to_string())).expect("run");
    (task, run)
}

fn statuses(store: &SqliteStore, record: &HarnessDispatchRecord) -> (TaskStatus, RunStatus) {
    let (task, run) = entities(store, record);
    (task.status(), run.status())
}

fn event_type(commit: &HarnessDispatchCommit) -> &str {
    commit
        .event
        .as_ref()
        .expect("appended event")
        .event_type()
        .as_str()
}

fn event_count(store: &SqliteStore) -> usize {
    store.events().expect("events").len()
}

fn dispatch_error<T: std::fmt::Debug>(result: Result<T, StoreError>) -> DispatchError {
    match result {
        Err(StoreError::HarnessDispatch(error)) => error,
        other => panic!("expected a dispatch rejection, got {other:?}"),
    }
}

fn fence_column(path: &Path, request_id: Uuid) -> Option<String> {
    Connection::open(path)
        .expect("open raw database")
        .query_row(
            "SELECT claim_fence FROM harness_dispatches WHERE request_id = ?",
            [request_id.to_string()],
            |row| row.get(0),
        )
        .expect("fence column")
}

#[test]
fn admission_is_idempotent_and_a_changed_request_conflicts() {
    let (_file, mut store) = open_store(&["codex"]);
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness: AgentKind::Codex,
        prompt: FIXTURE_PROMPT.to_owned(),
    };
    let admission = HarnessDispatchAdmission {
        request_id: request.request_id,
        prompt: request.prompt_digest(),
        ..codex("/work/alpha")
    };

    let admitted = store.admit_harness_dispatch(&admission).expect("admit");
    assert_eq!(event_type(&admitted), "harness_dispatch_admitted");
    let event = admitted.event.as_ref().expect("admitted event");
    assert_eq!(admitted.sequence, event.sequence().get());
    assert!(!event.payload().value().to_string().contains(FIXTURE_PROMPT));
    assert_eq!(admitted.record.phase, DispatchPhase::Admitted);
    assert_eq!(admitted.record.version, 1);
    assert_eq!(
        admitted.record.fingerprint,
        request.fingerprint(
            admitted.record.project_id,
            NativeProtocol::CodexExec,
            admission.config_sha256
        )
    );
    let (task, run) = entities(&store, &admitted.record);
    assert_eq!(task.title(), "harness dispatch codex");
    assert_eq!(task.status(), TaskStatus::InProgress);
    assert_eq!(
        (run.harness(), run.role(), run.protocol(), run.base_commit()),
        ("codex", "dispatch", "codex_exec", "0123456789abcdef")
    );
    assert_eq!(run.status(), RunStatus::Preparing);
    let before = event_count(&store);

    let repeat = store.admit_harness_dispatch(&admission).expect("repeat");
    assert!(repeat.event.is_none());
    assert_eq!(repeat.record, admitted.record);
    assert_eq!(repeat.sequence, admitted.sequence);

    let changed_prompt = HarnessDispatchAdmission {
        prompt: PromptDigest {
            sha256: Sha256Digest::of(b"another prompt"),
            byte_count: 14,
        },
        ..admission.clone()
    };
    let changed_config = HarnessDispatchAdmission {
        config_sha256: Sha256Digest::of(b"another config"),
        ..admission.clone()
    };
    let changed_protocol = HarnessDispatchAdmission {
        protocol: NativeProtocol::CodexAppServer,
        ..admission.clone()
    };
    for changed in [changed_prompt, changed_config, changed_protocol] {
        assert_eq!(
            dispatch_error(store.admit_harness_dispatch(&changed)),
            DispatchError::Conflict
        );
    }
    assert_eq!(event_count(&store), before);
}

#[test]
fn one_active_dispatch_per_working_directory() {
    let (_file, mut store) = open_store(&["codex", "claude"]);
    let first = admit(&mut store, &codex("/work/alpha"));
    let same_directory = admission(
        AgentKind::Claude,
        NativeProtocol::ClaudeStreamJson,
        "/work/alpha",
    );
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&same_directory)),
        DispatchError::Busy
    );
    admit(&mut store, &codex("/work/beta"));

    let fence = claim(&mut store, first);
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&same_directory)),
        DispatchError::Busy
    );
    store
        .finish_harness_dispatch(&clean_finish(first, fence))
        .expect("finish");
    admit(&mut store, &same_directory);
}

#[test]
fn claim_is_single_use_and_the_fence_stays_out_of_records_and_events() {
    let (file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    assert_eq!(fence_column(file.path(), request_id), None);

    let claimed = store
        .claim_harness_dispatch(request_id, at(1))
        .expect("claim");
    assert_eq!(
        claimed.event.event_type().as_str(),
        "harness_dispatch_started"
    );
    assert_eq!(claimed.record.phase, DispatchPhase::Running);
    assert_eq!(claimed.record.version, 2);
    assert_eq!(
        statuses(&store, &claimed.record),
        (TaskStatus::InProgress, RunStatus::Running)
    );
    assert_eq!(
        dispatch_error(store.claim_harness_dispatch(request_id, at(2))),
        DispatchError::InvalidTransition
    );
    assert_eq!(
        dispatch_error(store.claim_harness_dispatch(Uuid::new_v4(), at(2))),
        DispatchError::NotFound
    );

    let fence = fence_column(file.path(), request_id).expect("minted fence");
    assert!(
        !serde_json::to_string(&claimed.record)
            .expect("record json")
            .contains(&fence)
    );
    let debug = format!("{claimed:?}");
    assert!(debug.contains("ClaimFence(<redacted>)") && !debug.contains(&fence));
    for event in store.events().expect("events") {
        assert!(
            !String::from_utf8(event.to_json_vec().expect("event json"))
                .expect("utf8")
                .contains(&fence)
        );
    }
}

#[test]
fn finish_requires_the_current_fence() {
    let (_file, mut store) = open_store(&["codex"]);
    let alpha = admit(&mut store, &codex("/work/alpha"));
    let beta = admit(&mut store, &codex("/work/beta"));
    let alpha_fence = claim(&mut store, alpha);
    let beta_fence = claim(&mut store, beta);
    assert_eq!(
        dispatch_error(store.finish_harness_dispatch(&clean_finish(alpha, beta_fence))),
        DispatchError::StaleFence
    );

    let finished = store
        .finish_harness_dispatch(&clean_finish(alpha, alpha_fence))
        .expect("finish");
    assert_eq!(event_type(&finished), "harness_dispatch_finished");
    assert_eq!(finished.record.phase, DispatchPhase::Completed);
    assert_eq!(finished.record.outcome, Some(AttemptOutcome::Completed));
    assert_eq!(finished.record.error_code, None);
    assert!(finished.record.process.is_some() && finished.record.capture.is_some());
    assert_eq!(
        statuses(&store, &finished.record),
        (TaskStatus::Done, RunStatus::Succeeded)
    );
    let before = event_count(&store);

    let repeat = store
        .finish_harness_dispatch(&clean_finish(alpha, alpha_fence))
        .expect("repeated finish");
    assert!(repeat.event.is_none());
    assert_eq!(repeat.record, finished.record);
    assert_eq!(
        dispatch_error(store.finish_harness_dispatch(&clean_finish(alpha, beta_fence))),
        DispatchError::StaleFence
    );
    assert_eq!(
        dispatch_error(store.cancel_harness_dispatch(alpha, at(11))),
        DispatchError::Terminal
    );
    assert_eq!(event_count(&store), before);
}

#[test]
fn succeeded_requires_a_clean_error_free_finish() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let fence = claim(&mut store, request_id);
    let before = event_count(&store);

    let incoherent = [
        (Some(DispatchError::TurnFailed), Some(0), false),
        (None, Some(1), false),
        (None, None, false),
        (None, Some(0), true),
    ];
    for (error_code, exit_code, forced_termination) in incoherent {
        let mut finish = clean_finish(request_id, fence);
        finish.decision.error_code = error_code;
        finish.process.exit_code = exit_code;
        finish.process.forced_termination = forced_termination;
        assert_eq!(
            dispatch_error(store.finish_harness_dispatch(&finish)),
            DispatchError::InvalidRequest
        );
    }
    let probe = report(request_id, fence, AttemptOutcome::Probed, None);
    assert_eq!(
        dispatch_error(store.finish_harness_dispatch(&probe)),
        DispatchError::InvalidTransition
    );
    assert_eq!(record(&store, request_id).phase, DispatchPhase::Running);
    assert_eq!(event_count(&store), before);

    let unverified = store
        .finish_harness_dispatch(&report(
            request_id,
            fence,
            AttemptOutcome::Unverified,
            Some(DispatchError::EofWithoutTerminal),
        ))
        .expect("unverified finish");
    assert_eq!(unverified.record.phase, DispatchPhase::Unverified);
    assert_eq!(
        unverified.record.error_code,
        Some(DispatchError::EofWithoutTerminal)
    );
    assert_eq!(
        statuses(&store, &unverified.record),
        (TaskStatus::Blocked, RunStatus::Failed)
    );
}

#[test]
fn cancel_before_claim_fails_the_run_and_releases_the_reservation() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));

    let cancelled = store
        .cancel_harness_dispatch(request_id, at(1))
        .expect("cancel");
    assert_eq!(event_type(&cancelled), "harness_dispatch_finished");
    let payload = cancelled.event.as_ref().expect("event").payload().value();
    assert_eq!(payload["from_phase"], json!("admitted"));
    assert_eq!(payload["phase"], json!("cancelled"));
    assert_eq!(payload["outcome"], Value::Null);
    assert_eq!(cancelled.record.phase, DispatchPhase::Cancelled);
    assert_eq!(
        statuses(&store, &cancelled.record),
        (TaskStatus::Cancelled, RunStatus::Failed)
    );

    let repeat = store
        .cancel_harness_dispatch(request_id, at(2))
        .expect("repeated cancel");
    assert!(repeat.event.is_none());
    assert_eq!(
        dispatch_error(store.claim_harness_dispatch(request_id, at(3))),
        DispatchError::Terminal
    );
    admit(&mut store, &codex("/work/alpha"));
}

#[test]
fn cancel_while_running_waits_for_the_fenced_finish() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let fence = claim(&mut store, request_id);

    let requested = store
        .cancel_harness_dispatch(request_id, at(2))
        .expect("cancel");
    assert_eq!(event_type(&requested), "harness_dispatch_cancel_requested");
    assert_eq!(requested.record.phase, DispatchPhase::CancelRequested);
    assert_eq!(
        statuses(&store, &requested.record),
        (TaskStatus::InProgress, RunStatus::Running)
    );
    let repeat = store
        .cancel_harness_dispatch(request_id, at(3))
        .expect("repeated cancel");
    assert!(repeat.event.is_none());
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&codex("/work/alpha"))),
        DispatchError::Busy
    );

    // Cancellation overrides the late completed report; the evidence stays.
    let finished = store
        .finish_harness_dispatch(&clean_finish(request_id, fence))
        .expect("finish");
    assert_eq!(finished.record.phase, DispatchPhase::Cancelled);
    assert_eq!(finished.record.outcome, Some(AttemptOutcome::Completed));
    assert_eq!(
        statuses(&store, &finished.record),
        (TaskStatus::Cancelled, RunStatus::Stopped)
    );
    admit(&mut store, &codex("/work/alpha"));
}

#[test]
fn recovery_fails_admitted_and_parks_claimed_attempts() {
    let (_file, mut store) = open_store(&["codex"]);
    let admitted = admit(&mut store, &codex("/work/alpha"));
    let running = admit(&mut store, &codex("/work/beta"));
    let running_fence = claim(&mut store, running);
    let requested = admit(&mut store, &codex("/work/gamma"));
    let requested_fence = claim(&mut store, requested);
    store
        .cancel_harness_dispatch(requested, at(2))
        .expect("cancel");
    let done = admit(&mut store, &codex("/work/delta"));
    let done_fence = claim(&mut store, done);
    store
        .finish_harness_dispatch(&clean_finish(done, done_fence))
        .expect("finish");

    let recovery = store.recover_harness_dispatches(at(20)).expect("recover");
    assert!(recovery.quarantined.is_empty());
    let recovered = recovery.recovered;
    let changed: Vec<(Uuid, DispatchPhase)> = recovered
        .iter()
        .map(|commit| (commit.record.request_id, commit.record.phase))
        .collect();
    assert_eq!(
        changed,
        [
            (admitted, DispatchPhase::Failed),
            (running, DispatchPhase::RecoveryPending),
            (requested, DispatchPhase::RecoveryPending),
        ]
    );
    for commit in &recovered {
        assert_eq!(event_type(commit), "harness_dispatch_recovered");
        assert_eq!(commit.record.error_code, Some(DispatchError::Interrupted));
    }
    assert_eq!(
        statuses(&store, &recovered[0].record),
        (TaskStatus::Blocked, RunStatus::Failed)
    );
    assert_eq!(
        statuses(&store, &recovered[1].record),
        (TaskStatus::Blocked, RunStatus::Stale)
    );
    assert_eq!(record(&store, done).phase, DispatchPhase::Completed);

    // Recovery invalidated every fence, so a stale executor cannot finish.
    for (request_id, fence) in [(running, running_fence), (requested, requested_fence)] {
        assert_eq!(
            dispatch_error(store.finish_harness_dispatch(&clean_finish(request_id, fence))),
            DispatchError::StaleFence
        );
    }
    assert_eq!(
        store
            .recover_harness_dispatches(at(21))
            .expect("second recovery"),
        HarnessDispatchRecovery::default()
    );
    assert_eq!(
        dispatch_error(store.claim_harness_dispatch(running, at(21))),
        DispatchError::InvalidTransition
    );

    // The reservation is kept until an operator acknowledges by cancelling.
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&codex("/work/beta"))),
        DispatchError::Busy
    );
    let acknowledged = store
        .cancel_harness_dispatch(running, at(22))
        .expect("acknowledge");
    assert_eq!(event_type(&acknowledged), "harness_dispatch_finished");
    assert_eq!(acknowledged.record.phase, DispatchPhase::Cancelled);
    assert_eq!(
        statuses(&store, &acknowledged.record),
        (TaskStatus::Cancelled, RunStatus::Stopped)
    );
    // The record keeps the latest code; the event carries only its own.
    assert_eq!(
        acknowledged.record.error_code,
        Some(DispatchError::Interrupted)
    );
    let payload = acknowledged
        .event
        .as_ref()
        .expect("event")
        .payload()
        .value();
    assert_eq!(payload["error_code"], Value::Null);
    // The acknowledged attempt has no fence left to present.
    assert_eq!(
        dispatch_error(store.finish_harness_dispatch(&clean_finish(running, running_fence))),
        DispatchError::StaleFence
    );
    admit(&mut store, &codex("/work/beta"));
    admit(&mut store, &codex("/work/alpha"));
}

#[test]
fn recovery_quarantines_defective_rows_and_recovers_the_rest() {
    let (file, mut store) = open_store(&["codex"]);
    let healthy = admit(&mut store, &codex("/work/alpha"));
    claim(&mut store, healthy);
    let corrupted = admit(&mut store, &codex("/work/beta"));
    claim(&mut store, corrupted);
    let unkeyed = admit(&mut store, &codex("/work/gamma"));
    let raw = Connection::open(file.path()).expect("open raw database");
    raw.execute(
        "UPDATE harness_dispatches SET record_json = replace(record_json, '\"phase\":\"running\"', '\"phase\":\"admitted\"') WHERE request_id = ?",
        [corrupted.to_string()],
    )
    .expect("corrupt record json");
    raw.execute(
        "UPDATE harness_dispatches SET request_id = 'not a request' WHERE request_id = ?",
        [unkeyed.to_string()],
    )
    .expect("corrupt request key");
    let corrupted_fence = fence_column(file.path(), corrupted);
    let before = event_count(&store);

    let recovery = store.recover_harness_dispatches(at(20)).expect("recover");
    let recovered: Vec<(Uuid, DispatchPhase)> = recovery
        .recovered
        .iter()
        .map(|commit| (commit.record.request_id, commit.record.phase))
        .collect();
    assert_eq!(recovered, [(healthy, DispatchPhase::RecoveryPending)]);
    let mismatch = "store_harness_dispatch_projection_mismatch";
    assert_eq!(
        recovery.quarantined,
        [
            QuarantinedDispatch {
                request_id: Some(corrupted),
                code: mismatch,
            },
            QuarantinedDispatch {
                request_id: None,
                code: mismatch,
            },
        ]
    );
    assert_eq!(event_count(&store), before + 1);

    // Quarantined rows are untouched and keep their reservations.
    let phase: String = raw
        .query_row(
            "SELECT phase FROM harness_dispatches WHERE request_id = ?",
            [corrupted.to_string()],
            |row| row.get(0),
        )
        .expect("phase column");
    assert_eq!(phase, "running");
    assert_eq!(fence_column(file.path(), corrupted), corrupted_fence);
    for workspace in ["/work/beta", "/work/gamma"] {
        assert_eq!(
            dispatch_error(store.admit_harness_dispatch(&codex(workspace))),
            DispatchError::Busy
        );
    }
}

#[test]
fn admission_requires_a_detected_executable_route() {
    let (_file, mut store) = open_store(&["claude", "opencode"]);
    let before = event_count(&store);
    let claude = || {
        admission(
            AgentKind::Claude,
            NativeProtocol::ClaudeStreamJson,
            "/work/alpha",
        )
    };

    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&codex("/work/alpha"))),
        DispatchError::NotDetected
    );
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&admission(
            AgentKind::OpenCode,
            NativeProtocol::Acp,
            "/work/alpha"
        ))),
        DispatchError::ExecutionDisabled
    );
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&HarnessDispatchAdmission {
            protocol: NativeProtocol::CodexExec,
            ..claude()
        })),
        DispatchError::InvalidRequest
    );
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&HarnessDispatchAdmission {
            request_id: Uuid::nil(),
            ..claude()
        })),
        DispatchError::InvalidRequest
    );
    assert_eq!(
        dispatch_error(store.admit_harness_dispatch(&HarnessDispatchAdmission {
            base_commit: "HEAD".to_owned(),
            ..claude()
        })),
        DispatchError::InvalidRequest
    );
    for byte_count in [0, MAX_PROMPT_BYTES as u64 + 1] {
        let mut prompt_out_of_bounds = claude();
        prompt_out_of_bounds.prompt.byte_count = byte_count;
        assert_eq!(
            dispatch_error(store.admit_harness_dispatch(&prompt_out_of_bounds)),
            DispatchError::InvalidPrompt
        );
    }
    assert_eq!(event_count(&store), before);
    admit(&mut store, &claude());
}

#[test]
fn dispatch_state_survives_reopen() {
    let (file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let fence = claim(&mut store, request_id);
    let claimed = record(&store, request_id);
    drop(store);

    let mut store = SqliteStore::open(file.path()).expect("reopen");
    assert_eq!(record(&store, request_id), claimed);
    store
        .finish_harness_dispatch(&clean_finish(request_id, fence))
        .expect("finish after reopen");
    drop(store);

    let store = SqliteStore::open(file.path()).expect("reopen again");
    let finished = record(&store, request_id);
    assert_eq!(finished.phase, DispatchPhase::Completed);
    assert_eq!(finished.version, 3);
    assert_eq!(
        statuses(&store, &finished),
        (TaskStatus::Done, RunStatus::Succeeded)
    );
}

#[test]
fn generic_commits_cannot_touch_dispatch_owned_entities() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let (task, run) = entities(&store, &record(&store, request_id));
    let draft = |key: &str| EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new("task_updated").expect("event type"),
        project_id: task.project_id(),
        task_id: None,
        run_id: None,
        causation_id: None,
        actor: ActorName::new("system").expect("actor"),
        timestamp: at(5),
        idempotency_key: Some(key.to_owned()),
        payload: EventPayload::new(json!({"fixture": true})).expect("payload"),
    };
    let blocked = task
        .transitioned(TaskStatus::Blocked)
        .expect("domain transition");
    assert!(matches!(
        store.commit_task(&blocked, draft("generic_task")),
        Err(StoreError::HarnessDispatchBoundProjection)
    ));
    assert!(matches!(
        store.commit_run(&run, draft("generic_run")),
        Err(StoreError::HarnessDispatchBoundProjection)
    ));
    // Nor can a new Run attach to the dispatch's Task.
    let foreign = Run::new(RunSpec {
        project_id: task.project_id(),
        task_id: task.task_id(),
        harness: "codex".to_owned(),
        role: "fixture".to_owned(),
        protocol: "pty".to_owned(),
        base_commit: "0123456789abcdef".to_owned(),
    })
    .expect("foreign run");
    assert!(matches!(
        store.commit_run(&foreign, draft("generic_foreign_run")),
        Err(StoreError::HarnessDispatchBoundProjection)
    ));
}

#[test]
fn corrupted_rows_fail_closed() {
    let (file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let admitted = record(&store, request_id);
    let raw = Connection::open(file.path()).expect("open raw database");

    let set_fingerprint = |fingerprint: String| {
        raw.execute(
            "UPDATE harness_dispatches SET fingerprint = ? WHERE request_id = ?",
            [fingerprint, request_id.to_string()],
        )
        .expect("edit fingerprint column");
    };
    set_fingerprint(Sha256Digest::of(b"forged").to_hex());
    assert!(matches!(
        store.harness_dispatch(request_id),
        Err(StoreError::HarnessDispatchProjectionMismatch)
    ));
    set_fingerprint(admitted.fingerprint.to_hex());
    assert_eq!(record(&store, request_id), admitted);

    let set_version = |version: u64| {
        raw.execute(
            "UPDATE harness_dispatches SET version = ? WHERE request_id = ?",
            rusqlite::params![version, request_id.to_string()],
        )
        .expect("edit version column");
    };
    set_version(7);
    assert!(matches!(
        store.harness_dispatch(request_id),
        Err(StoreError::HarnessDispatchProjectionMismatch)
    ));
    set_version(admitted.version);

    // The schema itself refuses a fence on an unclaimed attempt.
    assert!(
        raw.execute(
            "UPDATE harness_dispatches SET claim_fence = ? WHERE request_id = ?",
            [Uuid::new_v4().to_string(), request_id.to_string()],
        )
        .is_err()
    );

    // A Task moved outside the dispatch API no longer matches the phase.
    raw.execute(
        "UPDATE projections SET state_json = replace(state_json, '\"in_progress\"', '\"blocked\"') WHERE entity_kind = 'task' AND entity_id = ?",
        [admitted.task_id.to_string()],
    )
    .expect("edit task projection");
    let before = event_count(&store);
    assert!(matches!(
        store.claim_harness_dispatch(request_id, at(1)),
        Err(StoreError::HarnessDispatchProjectionMismatch)
    ));
    assert_eq!(event_count(&store), before);
}

#[test]
fn failed_and_cancelled_finishes_record_their_phase_and_a_failure_needs_its_code() {
    let (_file, mut store) = open_store(&["codex"]);
    let failed = admit(&mut store, &codex("/work/alpha"));
    let failed_fence = claim(&mut store, failed);
    let before = event_count(&store);
    for outcome in [AttemptOutcome::Failed, AttemptOutcome::Unverified] {
        assert_eq!(
            dispatch_error(store.finish_harness_dispatch(&report(
                failed,
                failed_fence,
                outcome,
                None
            ))),
            DispatchError::InvalidRequest
        );
    }
    assert_eq!(event_count(&store), before);

    let finished = store
        .finish_harness_dispatch(&report(
            failed,
            failed_fence,
            AttemptOutcome::Failed,
            Some(DispatchError::TurnFailed),
        ))
        .expect("failed finish");
    assert_eq!(finished.record.phase, DispatchPhase::Failed);
    assert_eq!(finished.record.error_code, Some(DispatchError::TurnFailed));
    assert_eq!(
        statuses(&store, &finished.record),
        (TaskStatus::Blocked, RunStatus::Failed)
    );

    let cancelled = admit(&mut store, &codex("/work/beta"));
    let cancelled_fence = claim(&mut store, cancelled);
    let finished = store
        .finish_harness_dispatch(&report(
            cancelled,
            cancelled_fence,
            AttemptOutcome::Cancelled,
            None,
        ))
        .expect("cancelled finish");
    assert_eq!(finished.record.phase, DispatchPhase::Cancelled);
    assert_eq!(finished.record.error_code, None);
    assert_eq!(
        statuses(&store, &finished.record),
        (TaskStatus::Cancelled, RunStatus::Stopped)
    );
    admit(&mut store, &codex("/work/alpha"));
    admit(&mut store, &codex("/work/beta"));
}

#[test]
fn a_repeated_finish_must_repeat_the_same_report() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    let fence = claim(&mut store, request_id);
    let finish = report(
        request_id,
        fence,
        AttemptOutcome::Failed,
        Some(DispatchError::TurnFailed),
    );
    let finished = store.finish_harness_dispatch(&finish).expect("finish");
    let before = event_count(&store);

    let mut retried_later = finish;
    retried_later.timestamp = at(30);
    let retried = store
        .finish_harness_dispatch(&retried_later)
        .expect("retried finish");
    assert!(retried.event.is_none());

    let mut more_output = finish;
    more_output.capture.record_count += 1;
    let other_code = report(
        request_id,
        fence,
        AttemptOutcome::Failed,
        Some(DispatchError::ProcessFailed),
    );
    for changed in [more_output, other_code, clean_finish(request_id, fence)] {
        assert_eq!(
            dispatch_error(store.finish_harness_dispatch(&changed)),
            DispatchError::Conflict
        );
    }
    assert_eq!(record(&store, request_id), finished.record);
    assert_eq!(event_count(&store), before);
}

#[test]
fn finish_before_claim_is_an_invalid_transition() {
    let (_file, mut store) = open_store(&["codex"]);
    let claimed = admit(&mut store, &codex("/work/alpha"));
    let other_fence = claim(&mut store, claimed);
    let request_id = admit(&mut store, &codex("/work/beta"));
    let before = event_count(&store);

    assert_eq!(
        dispatch_error(store.finish_harness_dispatch(&clean_finish(request_id, other_fence))),
        DispatchError::InvalidTransition
    );
    assert_eq!(record(&store, request_id).phase, DispatchPhase::Admitted);
    assert_eq!(event_count(&store), before);
}

#[test]
fn timestamps_never_move_backwards() {
    let (_file, mut store) = open_store(&["codex"]);
    let request_id = admit(&mut store, &codex("/work/alpha"));
    store
        .claim_harness_dispatch(request_id, at(30))
        .expect("claim");

    let cancelled = store
        .cancel_harness_dispatch(request_id, at(5))
        .expect("cancel with an earlier clock");
    assert_eq!(cancelled.record.created_at, at(0));
    assert_eq!(cancelled.record.updated_at, at(30));
    assert_eq!(cancelled.event.as_ref().expect("event").timestamp(), at(30));
}

#[test]
fn terminal_records_fail_closed_when_their_evidence_changes() {
    let (file, mut store) = open_store(&["codex"]);
    let failed = admit(&mut store, &codex("/work/alpha"));
    let failed_fence = claim(&mut store, failed);
    store
        .finish_harness_dispatch(&report(
            failed,
            failed_fence,
            AttemptOutcome::Failed,
            Some(DispatchError::TurnFailed),
        ))
        .expect("failed finish");
    let completed = admit(&mut store, &codex("/work/beta"));
    let completed_fence = claim(&mut store, completed);
    store
        .finish_harness_dispatch(&clean_finish(completed, completed_fence))
        .expect("clean finish");
    let raw = Connection::open(file.path()).expect("open raw database");

    // A failed Run promoted to succeeded outside the dispatch API.
    raw.execute(
        "UPDATE projections SET state_json = replace(state_json, '\"status\":\"failed\"', '\"status\":\"succeeded\"') WHERE entity_kind = 'run' AND entity_id = ?",
        [record(&store, failed).run_id.to_string()],
    )
    .expect("edit run projection");
    assert!(matches!(
        store.harness_dispatch(failed),
        Err(StoreError::HarnessDispatchProjectionMismatch)
    ));

    // A completed record whose exit evidence no longer justifies success.
    raw.execute(
        "UPDATE harness_dispatches SET record_json = replace(record_json, '\"exit_code\":0', '\"exit_code\":1') WHERE request_id = ?",
        [completed.to_string()],
    )
    .expect("edit record json");
    assert!(matches!(
        store.harness_dispatch(completed),
        Err(StoreError::HarnessDispatchProjectionMismatch)
    ));
}
