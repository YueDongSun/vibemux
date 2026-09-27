//! Attempt phases, outcome decisions, and content-free dispatch events.

use std::collections::{BTreeSet, VecDeque};

use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchLimits, DispatchPhase, DispatchTrigger,
        NativeProtocol, PhaseTransition, ProcessExit, PromptDigest, SessionTerminal, Sha256Digest,
        TransitionResult,
        attempt::{ADMISSION_RUN_STATUS, ADMISSION_TASK_STATUS, FenceEffect, apply, resource_key},
        capture_budget::CaptureBudget,
        events::{
            self, AdmittedPayload, DispatchEventContext, FinishedPayload, ProcessSummary,
            RecoveredPayload,
        },
        outcome::decide_outcome,
    },
};
use vibemux_types::{ProjectId, RunCompletionAuthority, RunId, RunStatus, TaskId, TaskStatus};

use DispatchPhase as P;
use FenceEffect as F;

const TRIGGERS: [DispatchTrigger; 8] = [
    DispatchTrigger::Claim,
    DispatchTrigger::Finish(AttemptOutcome::Completed),
    DispatchTrigger::Finish(AttemptOutcome::Failed),
    DispatchTrigger::Finish(AttemptOutcome::Unverified),
    DispatchTrigger::Finish(AttemptOutcome::Cancelled),
    DispatchTrigger::Finish(AttemptOutcome::Probed),
    DispatchTrigger::Cancel,
    DispatchTrigger::Recover,
];

#[derive(Debug, PartialEq)]
enum Expected {
    To(
        DispatchPhase,
        Option<TaskStatus>,
        Option<RunStatus>,
        FenceEffect,
        Option<DispatchError>,
    ),
    Same,
    Error(DispatchError),
}

fn to(
    phase: DispatchPhase,
    task: Option<TaskStatus>,
    run: Option<RunStatus>,
    fence: FenceEffect,
) -> Expected {
    Expected::To(phase, task, run, fence, None)
}

/// ADR 029 §3, one column per entry of [`TRIGGERS`].
fn expected_table() -> Vec<(DispatchPhase, [Expected; 8])> {
    use DispatchError as E;
    use Expected::{Error, Same};
    use RunStatus as R;
    use TaskStatus as T;

    let invalid = || Error(E::InvalidTransition);
    let recovery = || {
        Expected::To(
            P::RecoveryPending,
            Some(T::Blocked),
            Some(R::Stale),
            F::Clear,
            Some(E::Interrupted),
        )
    };
    let cancelled_after_run = || {
        to(
            P::Cancelled,
            Some(T::Cancelled),
            Some(R::Stopped),
            F::Require,
        )
    };
    let terminal_row = |cancel: Expected| {
        [
            Error(E::Terminal),
            Error(E::StaleFence),
            Error(E::StaleFence),
            Error(E::StaleFence),
            Error(E::StaleFence),
            invalid(),
            cancel,
            Same,
        ]
    };
    vec![
        (
            P::Admitted,
            [
                to(P::Running, None, Some(R::Running), F::Mint),
                invalid(),
                invalid(),
                invalid(),
                invalid(),
                invalid(),
                to(P::Cancelled, Some(T::Cancelled), Some(R::Failed), F::Keep),
                Expected::To(
                    P::Failed,
                    Some(T::Blocked),
                    Some(R::Failed),
                    F::Keep,
                    Some(E::Interrupted),
                ),
            ],
        ),
        (
            P::Running,
            [
                invalid(),
                to(P::Completed, Some(T::Done), Some(R::Succeeded), F::Require),
                to(P::Failed, Some(T::Blocked), Some(R::Failed), F::Require),
                to(P::Unverified, Some(T::Blocked), Some(R::Failed), F::Require),
                cancelled_after_run(),
                invalid(),
                to(P::CancelRequested, None, None, F::Keep),
                recovery(),
            ],
        ),
        (
            P::CancelRequested,
            [
                invalid(),
                cancelled_after_run(),
                cancelled_after_run(),
                cancelled_after_run(),
                cancelled_after_run(),
                invalid(),
                Same,
                recovery(),
            ],
        ),
        (
            P::RecoveryPending,
            [
                invalid(),
                Error(E::StaleFence),
                Error(E::StaleFence),
                Error(E::StaleFence),
                Error(E::StaleFence),
                invalid(),
                to(P::Cancelled, Some(T::Cancelled), Some(R::Stopped), F::Keep),
                Same,
            ],
        ),
        (P::Completed, terminal_row(Error(E::Terminal))),
        (P::Failed, terminal_row(Error(E::Terminal))),
        (P::Unverified, terminal_row(Error(E::Terminal))),
        (P::Cancelled, terminal_row(Same)),
    ]
}

fn observe(result: Result<TransitionResult, DispatchError>) -> Expected {
    match result {
        Ok(TransitionResult::Changed(transition)) => Expected::To(
            transition.to,
            transition.task_status,
            transition.run_status,
            transition.fence,
            transition.error_code,
        ),
        Ok(TransitionResult::Unchanged) => Expected::Same,
        Err(error) => Expected::Error(error),
    }
}

fn transitions() -> impl Iterator<Item = PhaseTransition> {
    P::ALL.into_iter().flat_map(|from| {
        TRIGGERS
            .into_iter()
            .filter_map(move |trigger| match apply(from, trigger) {
                Ok(TransitionResult::Changed(transition)) => Some(transition),
                _ => None,
            })
    })
}

#[test]
fn every_phase_and_trigger_matches_the_adr_table() {
    let table = expected_table();
    let phases: Vec<_> = table.iter().map(|(phase, _)| *phase).collect();
    assert_eq!(phases, P::ALL, "the table covers every phase once");
    for (from, row) in table {
        for (trigger, expected) in TRIGGERS.into_iter().zip(row) {
            assert_eq!(
                observe(apply(from, trigger)),
                expected,
                "{from:?} {trigger:?}"
            );
        }
    }
}

#[test]
fn every_edge_is_a_legal_canonical_task_and_run_transition() {
    for transition in transitions() {
        let label = format!("{transition:?}");
        let (task, run) = transition
            .from
            .canonical_statuses()
            .expect("only non-terminal phases change");
        if let Some(next) = transition.task_status {
            assert!(task.can_transition_to(next), "{label}");
        }
        if let Some(next) = transition.run_status {
            assert!(run.can_transition_to(next), "{label}");
        }
        let task = transition.task_status.unwrap_or(task);
        let run = transition.run_status.unwrap_or(run);
        match transition.to.canonical_statuses() {
            Some(canonical) => assert_eq!(canonical, (task, run), "{label}"),
            None => {
                assert!(transition.task_status.is_some(), "{label}");
                assert!(
                    matches!(
                        run,
                        RunStatus::Succeeded | RunStatus::Failed | RunStatus::Stopped
                    ),
                    "{label}"
                );
            }
        }
    }
    assert_eq!(
        P::Admitted.canonical_statuses(),
        Some((ADMISSION_TASK_STATUS, ADMISSION_RUN_STATUS))
    );
    assert!(TaskStatus::Open.can_transition_to(ADMISSION_TASK_STATUS));
}

#[test]
fn allowed_statuses_are_exactly_the_pairs_the_machine_reaches() {
    let mut reached = vec![(P::Admitted, ADMISSION_TASK_STATUS, ADMISSION_RUN_STATUS)];
    for transition in transitions() {
        let (task, run) = transition
            .from
            .canonical_statuses()
            .expect("only non-terminal phases change");
        let pair = (
            transition.task_status.unwrap_or(task),
            transition.run_status.unwrap_or(run),
        );
        assert!(
            transition.to.allowed_statuses().contains(&pair),
            "{transition:?}"
        );
        reached.push((transition.to, pair.0, pair.1));
    }
    for phase in P::ALL {
        if let Some(canonical) = phase.canonical_statuses() {
            assert_eq!(phase.allowed_statuses(), [canonical], "{phase:?}");
        }
        for &(task, run) in phase.allowed_statuses() {
            assert!(
                reached.contains(&(phase, task, run)),
                "{phase:?} ({task:?}, {run:?}) is never reached"
            );
        }
    }
}

#[test]
fn only_a_structured_completion_carries_success_authority() {
    let mut succeeded = 0;
    for transition in transitions() {
        let success = transition.run_status == Some(RunStatus::Succeeded);
        assert_eq!(
            transition.completion_authority,
            success.then_some(RunCompletionAuthority::StructuredAdapter),
            "{transition:?}"
        );
        if success {
            succeeded += 1;
            assert_eq!((transition.from, transition.to), (P::Running, P::Completed));
        }
    }
    assert_eq!(succeeded, 1);
}

#[test]
fn fences_and_reservations_follow_the_executor_lifecycle() {
    for transition in transitions() {
        let label = format!("{transition:?}");
        assert_eq!(
            transition.releases_reservation,
            transition.from.holds_reservation() && !transition.to.holds_reservation(),
            "{label}"
        );
        match transition.fence {
            F::Mint => assert_eq!((transition.from, transition.to), (P::Admitted, P::Running)),
            F::Require => assert!(
                matches!(transition.from, P::Running | P::CancelRequested),
                "{label}"
            ),
            F::Clear => assert_eq!(transition.to, P::RecoveryPending, "{label}"),
            F::Keep => {}
        }
        if transition.to == P::RecoveryPending {
            assert!(!transition.releases_reservation, "{label}");
        }
    }
    for phase in P::ALL {
        assert_eq!(phase.holds_reservation(), !phase.is_terminal());
        assert_eq!(phase.canonical_statuses().is_some(), !phase.is_terminal());
    }
}

#[test]
fn every_phase_is_reachable_from_admission() {
    let mut seen = BTreeSet::from([P::Admitted.as_str()]);
    let mut queue = VecDeque::from([P::Admitted]);
    while let Some(from) = queue.pop_front() {
        for trigger in TRIGGERS {
            if let Ok(TransitionResult::Changed(transition)) = apply(from, trigger) {
                if seen.insert(transition.to.as_str()) {
                    queue.push_back(transition.to);
                }
            }
        }
    }
    assert_eq!(seen.len(), P::ALL.len());
}

#[test]
fn phase_names_round_trip_through_text_and_serde() {
    for phase in P::ALL {
        assert_eq!(P::from_name(phase.as_str()), Some(phase));
        assert_eq!(serde_json::to_value(phase).unwrap(), json!(phase.as_str()));
        assert_eq!(
            serde_json::from_value::<DispatchPhase>(json!(phase.as_str())).unwrap(),
            phase
        );
    }
    assert_eq!(P::from_name("Running"), None);
    assert_eq!(P::from_name(""), None);
}

#[test]
fn resource_keys_are_deterministic_domain_separated_hashes() {
    let root = r"C:\projects\synthetic";
    assert_eq!(resource_key(root), resource_key(root));
    assert_ne!(resource_key(root), resource_key(r"C:\projects\synthetic2"));
    assert_ne!(resource_key(root), Sha256Digest::of(root.as_bytes()));
    assert!(!resource_key(root).to_hex().contains("synthetic"));
}

#[test]
fn outcomes_prefer_exchange_errors_then_process_exit_then_terminal_evidence() {
    let clean = ProcessExit {
        exit_code: Some(0),
        forced_termination: false,
    };
    let crashed = ProcessExit {
        exit_code: Some(3),
        forced_termination: false,
    };
    let killed = ProcessExit {
        exit_code: None,
        forced_termination: true,
    };
    let forced_zero = ProcessExit {
        exit_code: Some(0),
        forced_termination: true,
    };
    let signalled = ProcessExit {
        exit_code: None,
        forced_termination: false,
    };
    use AttemptOutcome as O;
    use DispatchError as E;
    use SessionTerminal as S;
    let cases = [
        (
            Err(E::Cancelled),
            None,
            killed,
            O::Cancelled,
            Some(E::Cancelled),
        ),
        (
            Err(E::FrameTooLarge),
            Some(S::Completed),
            clean,
            O::Failed,
            Some(E::FrameTooLarge),
        ),
        (
            Err(E::DeadlineExceeded),
            None,
            killed,
            O::Failed,
            Some(E::DeadlineExceeded),
        ),
        (
            Ok(()),
            Some(S::Completed),
            crashed,
            O::Failed,
            Some(E::ProcessFailed),
        ),
        (
            Ok(()),
            Some(S::Completed),
            killed,
            O::Failed,
            Some(E::ProcessFailed),
        ),
        (
            Ok(()),
            Some(S::Completed),
            forced_zero,
            O::Failed,
            Some(E::ProcessFailed),
        ),
        (
            Ok(()),
            Some(S::Completed),
            signalled,
            O::Failed,
            Some(E::ProcessFailed),
        ),
        (
            Ok(()),
            None,
            clean,
            O::Unverified,
            Some(E::EofWithoutTerminal),
        ),
        (Ok(()), Some(S::Completed), clean, O::Completed, None),
        (
            Ok(()),
            Some(S::Failed),
            clean,
            O::Failed,
            Some(E::TurnFailed),
        ),
        (Ok(()), Some(S::Cancelled), clean, O::Cancelled, None),
        (Ok(()), Some(S::Probed), clean, O::Probed, None),
    ];
    for (exchange, terminal, exit, outcome, error_code) in cases {
        let decision = decide_outcome(exchange, terminal, exit);
        assert_eq!(
            (decision.outcome, decision.error_code),
            (outcome, error_code),
            "{exchange:?} {terminal:?} {exit:?}"
        );
    }
    assert!(clean.is_clean());
    for exit in [crashed, killed, forced_zero, signalled] {
        assert!(!exit.is_clean(), "{exit:?}");
    }
}

fn context() -> DispatchEventContext {
    DispatchEventContext {
        project_id: ProjectId::from_uuid(Uuid::from_u128(1)).unwrap(),
        task_id: TaskId::from_uuid(Uuid::from_u128(2)).unwrap(),
        run_id: RunId::from_uuid(Uuid::from_u128(3)).unwrap(),
        request_id: Uuid::from_u128(0xabcd),
        timestamp: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn every_dispatch_event_is_valid_scoped_and_content_free() {
    const PROMPT: &str = "synthetic prompt SYNTHETIC_SECRET_TOKEN";
    let context = context();
    let admitted = AdmittedPayload::new(
        context.request_id,
        AgentKind::Codex,
        NativeProtocol::CodexAppServer,
        PromptDigest {
            sha256: Sha256Digest::of(PROMPT.as_bytes()),
            byte_count: PROMPT.len() as u64,
        },
        Sha256Digest::of(b"config"),
        "0123456789abcdef0123456789abcdef01234567".to_string(),
    );
    let mut budget =
        CaptureBudget::new(NativeProtocol::CodexExec, &DispatchLimits::default()).expect("limits");
    for line in include_str!("fixtures/dispatch/codex_exec.jsonl").lines() {
        budget.accept(line.to_string()).expect("record");
    }
    let finished = FinishedPayload {
        request_id: context.request_id,
        from_phase: P::Running,
        phase: P::Unverified,
        outcome: Some(AttemptOutcome::Unverified),
        error_code: Some(DispatchError::EofWithoutTerminal),
        process: Some(ProcessSummary::new(
            ProcessExit {
                exit_code: Some(0),
                forced_termination: false,
            },
            42,
        )),
        capture: Some(budget.summary()),
    };
    let recovered = RecoveredPayload {
        request_id: context.request_id,
        from_phase: P::Running,
        phase: P::RecoveryPending,
        error_code: DispatchError::Interrupted,
    };
    let drafts = [
        events::admitted(&context, &admitted).unwrap(),
        events::started(&context).unwrap(),
        events::cancel_requested(&context, P::Running).unwrap(),
        events::finished(&context, &finished).unwrap(),
        events::recovered(&context, &recovered).unwrap(),
    ];
    let types: Vec<&str> = drafts
        .iter()
        .map(|draft| draft.event_type.as_str())
        .collect();
    assert_eq!(
        types,
        [
            events::HARNESS_DISPATCH_ADMITTED_EVENT,
            events::HARNESS_DISPATCH_STARTED_EVENT,
            events::HARNESS_DISPATCH_CANCEL_REQUESTED_EVENT,
            events::HARNESS_DISPATCH_FINISHED_EVENT,
            events::HARNESS_DISPATCH_RECOVERED_EVENT,
        ]
    );
    let mut keys = BTreeSet::new();
    for draft in &drafts {
        draft.validate().expect("valid draft");
        assert_eq!(draft.project_id, context.project_id);
        assert_eq!(draft.task_id, Some(context.task_id));
        assert_eq!(draft.run_id, Some(context.run_id));
        assert_eq!(draft.actor.as_str(), "vibemuxd");
        assert_eq!(draft.timestamp, context.timestamp);
        let key = draft.idempotency_key.clone().expect("idempotency key");
        assert_eq!(
            key,
            events::idempotency_key(context.request_id, draft.event_type.as_str())
        );
        assert!(key.starts_with("harness_dispatch:00000000-0000-0000-0000-00000000abcd:"));
        assert!(keys.insert(key));
        let text = draft.payload.value().to_string();
        assert!(!text.contains("synthetic"), "{text}");
        assert!(!text.contains("SYNTHETIC"), "{text}");
        assert_eq!(
            draft.payload.value()["request_id"],
            json!(context.request_id)
        );
    }

    let admitted = drafts[0].payload.value();
    assert_eq!(
        object_keys(admitted),
        [
            "base_commit",
            "config_sha256",
            "harness",
            "prompt_bytes",
            "prompt_sha256",
            "protocol",
            "request_id"
        ]
    );
    assert_eq!(admitted["harness"], json!("codex"));
    assert_eq!(admitted["protocol"], json!("codex_app_server"));
    assert_eq!(
        admitted["prompt_sha256"],
        json!(Sha256Digest::of(PROMPT.as_bytes()).to_hex())
    );
    let finished = drafts[3].payload.value();
    assert_eq!(
        finished["error_code"],
        json!("harness_protocol_eof_without_terminal")
    );
    assert_eq!(finished["phase"], json!("unverified"));
    assert_eq!(finished["process"]["stderr_bytes"], json!(42));
    assert_eq!(finished["capture"]["record_count"], json!(7));
    assert_eq!(finished["capture"]["kinds"]["tool"], json!(1));
    assert_eq!(
        drafts[4].payload.value()["error_code"],
        json!("harness_dispatch_interrupted")
    );
}

#[test]
fn event_drafts_are_stable_per_request_and_distinct_across_requests() {
    let first = context();
    let mut second = context();
    second.request_id = Uuid::from_u128(0xdcba);
    let key = |context: &DispatchEventContext| {
        events::started(context)
            .unwrap()
            .idempotency_key
            .expect("key")
    };
    assert_eq!(key(&first), key(&first));
    assert_ne!(key(&first), key(&second));
    let event_ids = [
        events::started(&first).unwrap().event_id,
        events::started(&first).unwrap().event_id,
    ];
    assert_ne!(event_ids[0], event_ids[1], "event IDs are fresh per draft");
}

#[test]
fn error_codes_serialize_as_their_fixed_strings_only() {
    for error in DispatchError::ALL {
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value, json!(error.code()));
        assert_eq!(
            serde_json::from_value::<DispatchError>(value).unwrap(),
            *error
        );
        assert_eq!(error.to_string(), error.code());
    }
    for invalid in [
        json!("harness_dispatch_unknown"),
        json!("Busy"),
        json!(1),
        Value::Null,
    ] {
        assert!(
            serde_json::from_value::<DispatchError>(invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}

fn object_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("object payload")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}
