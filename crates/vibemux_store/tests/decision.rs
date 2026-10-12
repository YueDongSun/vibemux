//! Store schema 6 decision records (ADR 034 §10 and §11).

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::Value;
use tempfile::NamedTempFile;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vibemux_decision::{
    DECISION_TTL, DecisionError, DecisionKind, DecisionQuestion, DecisionRecord, ExternalResult,
    Label, NewDecision, SkipReason, Threshold,
    events::{
        DECISION_CONSUMED_EVENT, DECISION_RECORDED_EVENT, DecisionConsumedPayload,
        DecisionRecordedPayload, rebuild_record,
    },
    resolve_route,
};
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{DispatchError, NativeProtocol, PromptDigest, Sha256Digest, attempt::resource_key},
};
use vibemux_store::{HarnessDispatchAdmission, MAX_DECISION_PAGE, SqliteStore, StoreError};

const FIXTURE_PROMPT: &str = "fixture prompt";
const FORBIDDEN_KEYS: [&str; 9] = [
    "authorization",
    "content",
    "cookie",
    "environment",
    "message",
    "password",
    "prompt",
    "secret",
    "token",
];

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
        .commit_harness_snapshot(&detections, "2026-10-12T00:00:00Z", "probe_fixture")
        .expect("detect harnesses");
    (file, store)
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_790_000_000 + seconds).expect("fixture timestamp")
}

fn prompt() -> PromptDigest {
    PromptDigest {
        sha256: Sha256Digest::of(FIXTURE_PROMPT.as_bytes()),
        byte_count: FIXTURE_PROMPT.len() as u64,
    }
}

fn dispatch_config() -> Sha256Digest {
    Sha256Digest::of(b"fixture config")
}

/// A `harness_route` decision over Claude and Codex, resolved to the project
/// default when one is given and escalated otherwise.
fn decision(project_default: Option<AgentKind>) -> DecisionRecord {
    let eligible = [AgentKind::Claude, AgentKind::Codex];
    let threshold = Threshold::from_unit(0.9).expect("threshold");
    let question =
        DecisionQuestion::harness_route(&eligible, &BTreeMap::new(), threshold).expect("question");
    let resolution = resolve_route(
        &question,
        &eligible,
        &[],
        project_default,
        &[],
        ExternalResult::Skipped(SkipReason::NotRequested),
    );
    DecisionRecord::new(
        NewDecision {
            decision_id: Uuid::new_v4(),
            kind: DecisionKind::HarnessRoute,
            options: question.option_ids(),
            labels: vec![Label::new("refactor").expect("label")],
            prompt: prompt(),
            project_default,
            threshold,
            dispatch_config_sha256: dispatch_config(),
            decision_config_sha256: Sha256Digest::of(b"fixture decision config"),
            created_at: at(0),
        },
        resolution,
    )
}

fn admission(
    harness: AgentKind,
    workspace: &str,
    timestamp: OffsetDateTime,
) -> HarnessDispatchAdmission {
    let protocol = match harness {
        AgentKind::Claude => NativeProtocol::ClaudeStreamJson,
        _ => NativeProtocol::CodexExec,
    };
    HarnessDispatchAdmission {
        request_id: Uuid::new_v4(),
        harness,
        protocol,
        prompt: prompt(),
        config_sha256: dispatch_config(),
        resource_key: resource_key(workspace),
        base_commit: "0123456789abcdef".to_owned(),
        timestamp,
    }
}

fn event_count(store: &SqliteStore) -> usize {
    store.events().expect("events").len()
}

/// Every decision rebuilt from the event log alone.
fn decisions_from_events(store: &SqliteStore) -> BTreeMap<Uuid, DecisionRecord> {
    let mut recorded = BTreeMap::new();
    let mut consumed = BTreeMap::new();
    for event in store.events().expect("events") {
        let payload = event.payload().value().clone();
        match event.event_type().as_str() {
            DECISION_RECORDED_EVENT => {
                let payload: DecisionRecordedPayload =
                    serde_json::from_value(payload).expect("recorded payload");
                assert!(recorded.insert(payload.decision_id, payload).is_none());
            }
            DECISION_CONSUMED_EVENT => {
                let payload: DecisionConsumedPayload =
                    serde_json::from_value(payload).expect("consumed payload");
                assert!(consumed.insert(payload.decision_id, payload).is_none());
            }
            _ => {}
        }
    }
    recorded
        .into_iter()
        .map(|(decision_id, payload)| {
            let record =
                rebuild_record(payload, consumed.remove(&decision_id).as_ref()).expect("rebuild");
            (decision_id, record)
        })
        .collect()
}

fn assert_no_forbidden_keys(value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                assert!(
                    !FORBIDDEN_KEYS.contains(&key.to_ascii_lowercase().as_str()),
                    "forbidden payload key {key}"
                );
                assert_no_forbidden_keys(nested);
            }
        }
        Value::Array(items) => items.iter().for_each(assert_no_forbidden_keys),
        _ => {}
    }
}

#[test]
fn recording_is_idempotent_and_shares_the_event_sequence() {
    let (_file, mut store) = open_store(&["codex"]);
    let record = decision(Some(AgentKind::Codex));
    let first = store.record_decision(&record).expect("record");
    let event = first.event.expect("recorded event");
    assert_eq!(event.event_type().as_str(), "decision_recorded");
    assert_eq!(event.task_id(), None);
    assert_eq!(first.decision.created_sequence, event.sequence().get());
    assert_eq!(first.decision.updated_sequence, event.sequence().get());
    assert_no_forbidden_keys(event.payload().value());

    let repeat = store.record_decision(&record).expect("repeat");
    assert_eq!(repeat.event, None);
    assert_eq!(repeat.decision, first.decision);
    assert_eq!(
        store.decision(record.decision_id).expect("load"),
        Some(first.decision)
    );

    let mut changed = record.clone();
    changed.labels.clear();
    let before = event_count(&store);
    assert!(matches!(
        store.record_decision(&changed),
        Err(StoreError::DecisionConflict)
    ));
    assert_eq!(event_count(&store), before);
}

#[test]
fn consumed_or_inconsistent_records_are_not_recorded() {
    let (_file, mut store) = open_store(&["codex"]);
    let mut consumed = decision(Some(AgentKind::Codex));
    consumed
        .mark_consumed(Uuid::new_v4(), at(1))
        .expect("consume in memory");
    assert!(matches!(
        store.record_decision(&consumed),
        Err(StoreError::Decision(DecisionError::InvalidRequest))
    ));
    let mut inconsistent = decision(Some(AgentKind::Codex));
    inconsistent.expires_at = inconsistent.created_at + DECISION_TTL + Duration::seconds(1);
    assert!(matches!(
        store.record_decision(&inconsistent),
        Err(StoreError::Decision(DecisionError::Internal))
    ));
    assert_eq!(
        store.decisions_after(0, MAX_DECISION_PAGE).expect("page"),
        []
    );
}

#[test]
fn a_decision_is_consumed_once_by_its_dispatch() {
    let (_file, mut store) = open_store(&["codex", "claude"]);
    let record = decision(Some(AgentKind::Codex));
    store.record_decision(&record).expect("record");
    let admitted = admission(AgentKind::Codex, "/work/a", at(60));

    let commit = store
        .admit_decided_dispatch(record.decision_id, &admitted)
        .expect("decided dispatch");
    let dispatch_event = commit.dispatch.event.as_ref().expect("admitted event");
    let consumed_event = commit.consumed_event.as_ref().expect("consumed event");
    assert_eq!(consumed_event.event_type().as_str(), "decision_consumed");
    assert_eq!(
        consumed_event.task_id(),
        Some(commit.dispatch.record.task_id)
    );
    assert_eq!(consumed_event.run_id(), Some(commit.dispatch.record.run_id));
    assert!(consumed_event.sequence() > dispatch_event.sequence());
    assert_no_forbidden_keys(consumed_event.payload().value());
    let consumption = commit.decision.record.consumption.expect("consumption");
    assert_eq!(consumption.request_id, admitted.request_id);
    assert_eq!(
        commit.decision.updated_sequence,
        consumed_event.sequence().get()
    );
    assert_eq!(
        store.decision(record.decision_id).expect("load"),
        Some(commit.decision.clone())
    );

    // The same request replays without writing.
    let before = event_count(&store);
    let replay = store
        .admit_decided_dispatch(record.decision_id, &admitted)
        .expect("replay");
    assert_eq!(replay.dispatch.event, None);
    assert_eq!(replay.consumed_event, None);
    assert_eq!(replay.decision, commit.decision);
    assert_eq!(event_count(&store), before);

    // Any other request is refused, even after the first one finished.
    let other = admission(AgentKind::Codex, "/work/b", at(61));
    assert!(matches!(
        store.admit_decided_dispatch(record.decision_id, &other),
        Err(StoreError::Decision(DecisionError::AlreadyConsumed))
    ));
    assert_eq!(event_count(&store), before);
    assert_eq!(
        store.harness_dispatch(other.request_id).expect("load"),
        None
    );
}

#[test]
fn rejected_decided_admissions_write_nothing() {
    let (_file, mut store) = open_store(&["codex", "claude"]);
    let resolved = decision(Some(AgentKind::Codex));
    let escalated = decision(None);
    store.record_decision(&resolved).expect("record resolved");
    store.record_decision(&escalated).expect("record escalated");
    let before = event_count(&store);

    let mut other_prompt = admission(AgentKind::Codex, "/work/a", at(60));
    other_prompt.prompt = PromptDigest {
        sha256: Sha256Digest::of(b"another prompt"),
        byte_count: 14,
    };
    let mut other_config = admission(AgentKind::Codex, "/work/a", at(60));
    other_config.config_sha256 = Sha256Digest::of(b"edited config");
    let expired = admission(
        AgentKind::Codex,
        "/work/a",
        resolved.created_at + DECISION_TTL,
    );
    let cases: [(Uuid, HarnessDispatchAdmission, DecisionError); 6] = [
        (
            resolved.decision_id,
            admission(AgentKind::Claude, "/work/a", at(60)),
            DecisionError::HarnessMismatch,
        ),
        (
            resolved.decision_id,
            other_prompt,
            DecisionError::PromptMismatch,
        ),
        (
            resolved.decision_id,
            other_config,
            DecisionError::ConfigChanged,
        ),
        (resolved.decision_id, expired, DecisionError::Expired),
        (
            escalated.decision_id,
            admission(AgentKind::Codex, "/work/a", at(60)),
            DecisionError::Escalated,
        ),
        (
            Uuid::new_v4(),
            admission(AgentKind::Codex, "/work/a", at(60)),
            DecisionError::NotFound,
        ),
    ];
    for (decision_id, admitted, expected) in cases {
        match store.admit_decided_dispatch(decision_id, &admitted) {
            Err(StoreError::Decision(error)) => assert_eq!(error, expected),
            other => panic!("expected {expected:?}, got {other:?}"),
        }
        assert_eq!(
            store.harness_dispatch(admitted.request_id).expect("load"),
            None
        );
    }
    assert_eq!(event_count(&store), before);
    let stored = store
        .decision(resolved.decision_id)
        .expect("load")
        .expect("stored");
    assert_eq!(stored.record.consumption, None);
}

#[test]
fn dispatch_gates_still_apply_and_leave_the_decision_unconsumed() {
    // Codex is decided but not detected.
    let (_file, mut store) = open_store(&["claude"]);
    let record = decision(Some(AgentKind::Codex));
    store.record_decision(&record).expect("record");
    let undetected = admission(AgentKind::Codex, "/work/a", at(60));
    assert!(matches!(
        store.admit_decided_dispatch(record.decision_id, &undetected),
        Err(StoreError::HarnessDispatch(DispatchError::NotDetected))
    ));

    // The working directory is reserved by another dispatch.
    let (_file, mut store) = open_store(&["codex"]);
    store.record_decision(&record).expect("record");
    store
        .admit_harness_dispatch(&admission(AgentKind::Codex, "/work/a", at(30)))
        .expect("plain dispatch");
    assert!(matches!(
        store.admit_decided_dispatch(
            record.decision_id,
            &admission(AgentKind::Codex, "/work/a", at(60))
        ),
        Err(StoreError::HarnessDispatch(DispatchError::Busy))
    ));

    // A request admitted earlier without the decision cannot consume it.
    let earlier = admission(AgentKind::Codex, "/work/b", at(40));
    store
        .admit_harness_dispatch(&earlier)
        .expect("plain dispatch");
    let mut same_request = earlier.clone();
    same_request.timestamp = at(60);
    assert!(matches!(
        store.admit_decided_dispatch(record.decision_id, &same_request),
        Err(StoreError::HarnessDispatch(DispatchError::Conflict))
    ));
    let stored = store
        .decision(record.decision_id)
        .expect("load")
        .expect("stored");
    assert_eq!(stored.record.consumption, None);
}

#[test]
fn decisions_page_in_creation_order() {
    let (_file, mut store) = open_store(&["codex"]);
    let records: Vec<_> = (0..3).map(|_| decision(Some(AgentKind::Codex))).collect();
    for record in &records {
        store.record_decision(record).expect("record");
    }
    let first_page = store.decisions_after(0, 2).expect("first page");
    assert_eq!(
        first_page
            .iter()
            .map(|stored| stored.record.decision_id)
            .collect::<Vec<_>>(),
        [records[0].decision_id, records[1].decision_id]
    );
    let rest = store
        .decisions_after(first_page[1].created_sequence, 2)
        .expect("second page");
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].record.decision_id, records[2].decision_id);
    for limit in [0, MAX_DECISION_PAGE + 1] {
        assert!(matches!(
            store.decisions_after(0, limit),
            Err(StoreError::Decision(DecisionError::InvalidRequest))
        ));
    }
}

#[test]
fn a_tampered_row_fails_closed() {
    let (file, mut store) = open_store(&["codex"]);
    let record = decision(Some(AgentKind::Codex));
    store.record_decision(&record).expect("record");
    drop(store);
    Connection::open(file.path())
        .expect("open raw database")
        .execute(
            "UPDATE decision_records SET status = 'escalated' WHERE decision_id = ?",
            [record.decision_id.as_hyphenated().to_string()],
        )
        .expect("tamper");
    let store = SqliteStore::open(file.path()).expect("reopen");
    assert!(matches!(
        store.decision(record.decision_id),
        Err(StoreError::DecisionProjectionMismatch)
    ));
    assert!(matches!(
        store.decisions_after(0, 10),
        Err(StoreError::DecisionProjectionMismatch)
    ));
}

#[test]
fn the_projection_rebuilds_from_its_events() {
    let (_file, mut store) = open_store(&["codex", "claude"]);
    let consumed = decision(Some(AgentKind::Codex));
    let unconsumed = decision(Some(AgentKind::Codex));
    let escalated = decision(None);
    for record in [&consumed, &unconsumed, &escalated] {
        store.record_decision(record).expect("record");
    }
    store
        .admit_decided_dispatch(
            consumed.decision_id,
            &admission(AgentKind::Codex, "/work/a", at(60)),
        )
        .expect("decided dispatch");

    let projection: BTreeMap<Uuid, DecisionRecord> = store
        .decisions_after(0, MAX_DECISION_PAGE)
        .expect("page")
        .into_iter()
        .map(|stored| (stored.record.decision_id, stored.record))
        .collect();
    assert_eq!(projection.len(), 3);
    assert!(projection[&consumed.decision_id].consumption.is_some());
    assert_eq!(decisions_from_events(&store), projection);
}
