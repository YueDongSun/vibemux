//! Included by the production-store supervisor test fixture.
use super::*;
use vibemux_workflow::{
    messages::{MessageEnvelope, MessageKind, Participant},
    supervisor::{
        SupervisorAction, SupervisorMessageBinding,
        context_actions::{effect_id, recipient},
    },
};

const PRIVATE_BODY: &str =
    "Please inspect the worker state; this transient text must never enter action JSON.";

fn pending_send(fixture: &mut Fixture, key: &str) -> SupervisorAction {
    let (round, proposal) = fixture.proposal(
        key,
        SupervisorTool::SendMessage {
            to: id("track_a"),
            text: PRIVATE_BODY.into(),
        },
    );
    let commit = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("commit send intent");
    assert!(commit.accepted, "{:?}", commit.round.rejection_codes);
    let action = commit.actions[0].clone();
    let serialized = serde_json::to_string(&action).expect("content-free intent");
    assert!(!serialized.contains(PRIVATE_BODY));
    assert_eq!(
        action.action,
        SupervisorTool::SendMessage {
            to: id("track_a"),
            text: String::new()
        }
    );
    assert_eq!(
        action.message_body,
        Some(SupervisorMessageBinding::for_text(PRIVATE_BODY))
    );
    let replay = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("same-round replay");
    assert!(replay.duplicate);
    assert_eq!(replay.actions, vec![action.clone()]);
    let mut conflict = proposal;
    if let SupervisorTool::SendMessage { text, .. } = &mut conflict.decisions[0].action {
        text.push('!');
    }
    assert!(
        fixture
            .store
            .commit_supervisor_proposal(round.round_id, &conflict, now())
            .is_err()
    );
    action
}

fn envelope(fixture: &Fixture, action: &SupervisorAction) -> MessageEnvelope {
    let to = id("track_a");
    let binding = &fixture.execution.tasks[&to];
    let target = recipient(&fixture.execution, &to).expect("one admitted recipient");
    let message_id = effect_id(action.action_id, "message");
    let now_ms = u64::try_from(now().unix_timestamp_nanos() / 1_000_000).expect("current time");
    MessageEnvelope {
        message_id,
        workflow_id: fixture.start.workflow_id,
        sender: Participant::Supervisor,
        recipient: target.clone(),
        kind: MessageKind::Question,
        correlation_id: message_id,
        reply_to: None,
        depth: 0,
        contract_id: binding.contract_id,
        contract_version: binding.contract_version,
        source_refs: vec![],
        body_sha256: Sha256Digest::of(PRIVATE_BODY.as_bytes()),
        body_bytes: PRIVATE_BODY.len() as u64,
        created_at_ms: now_ms,
        expires_at_ms: now_ms + 60_000,
        audience: vec![target, Participant::Supervisor],
        escalated: false,
    }
}

fn finish(action: &SupervisorAction) -> SupervisorActionFinish {
    SupervisorActionFinish {
        action_id: action.action_id,
        generation: action.generation,
        state: SupervisorActionState::Applied,
        result_refs: vec![effect_id(action.action_id, "message")],
        result_code: Some(id("supervisor_message_admitted")),
        timestamp: now(),
    }
}

#[test]
fn typed_message_intents_are_content_free_and_admit_exactly_one_fixed_effect() {
    let mut fixture = Fixture::configured(16, true);
    let pending = pending_send(&mut fixture, "send_once");
    let message = envelope(&fixture, &pending);
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                pending.action_id,
                pending.generation,
                &fixture.start.full_plan,
                &message,
                now()
            )
            .is_err(),
        "claim is mandatory"
    );
    let action = fixture
        .store
        .claim_supervisor_action(pending.action_id, pending.generation, now())
        .expect("one claim winner");
    assert!(
        fixture
            .store
            .claim_supervisor_action(action.action_id, action.generation, now())
            .is_err()
    );
    let admitted = fixture
        .store
        .admit_supervisor_message(
            action.action_id,
            action.generation,
            &fixture.start.full_plan,
            &message,
            now(),
        )
        .expect("admit fixed message");
    assert!(admitted.event.is_some());
    let replay = fixture
        .store
        .admit_supervisor_message(
            action.action_id,
            action.generation,
            &fixture.start.full_plan,
            &message,
            now(),
        )
        .expect("same admitted effect");
    assert!(replay.event.is_none());
    fixture
        .store
        .finish_supervisor_action(&finish(&action))
        .expect("applied proves admission only");
    let serialized = serde_json::to_string(
        &fixture
            .store
            .supervisor_status(fixture.start.workflow_id)
            .expect("status"),
    )
    .expect("status JSON");
    assert!(!serialized.contains(PRIVATE_BODY));
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                action.action_id,
                action.generation,
                &fixture.start.full_plan,
                &message,
                now()
            )
            .is_err(),
        "applied action cannot submit again"
    );
}

#[test]
fn message_admission_rejects_wrong_recipient_body_contract_plan_generation_and_expiry() {
    let mut fixture = Fixture::configured(16, true);
    let pending = pending_send(&mut fixture, "send_bound");
    let action = fixture
        .store
        .claim_supervisor_action(pending.action_id, pending.generation, now())
        .expect("claim");
    let valid = envelope(&fixture, &action);
    let mut variants = Vec::new();
    let mut wrong = valid.clone();
    wrong.message_id = Uuid::new_v4();
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.workflow_id = Uuid::new_v4();
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.recipient = recipient(&fixture.execution, &id("track_b")).expect("target");
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.body_sha256 = Sha256Digest::of(b"different");
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.contract_version += 1;
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.expires_at_ms = wrong.created_at_ms;
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.sender = Participant::Operator;
    variants.push(wrong);
    for wrong in variants {
        assert!(
            fixture
                .store
                .admit_supervisor_message(
                    action.action_id,
                    action.generation,
                    &fixture.start.full_plan,
                    &wrong,
                    now()
                )
                .is_err()
        );
    }
    let mut changed_plan = fixture.start.full_plan.clone();
    changed_plan["specs"]["track_a"]["context_policy"]["max_bundle_bytes"] = json!(999);
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                action.action_id,
                action.generation,
                &changed_plan,
                &valid,
                now()
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                action.action_id,
                action.generation + 1,
                &fixture.start.full_plan,
                &valid,
                now()
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .finish_supervisor_action(&finish(&action))
            .is_err(),
        "no message means no applied receipt"
    );
}

#[test]
fn permission_omission_and_preclaim_receipts_cannot_authorize_a_send() {
    let mut fixture = Fixture::new();
    let pending = pending_send(&mut fixture, "send_denied");
    let message = envelope(&fixture, &pending);
    fixture
        .store
        .admit_workflow_message(&message, now())
        .expect("unrelated preclaim message fixture");
    let action = fixture
        .store
        .claim_supervisor_action(pending.action_id, pending.generation, now())
        .expect("claim");
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                action.action_id,
                action.generation,
                &fixture.start.full_plan,
                &message,
                now()
            )
            .is_err()
    );
    assert!(
        fixture
            .store
            .finish_supervisor_action(&finish(&action))
            .is_err(),
        "preclaim receipt cannot prove effect"
    );
}

#[test]
fn outstanding_messages_consume_the_recipient_budget_and_unknown_claims_fence_restart() {
    let mut fixture = Fixture::configured(16, true);
    let pending = pending_send(&mut fixture, "first_message");
    let action = fixture
        .store
        .claim_supervisor_action(pending.action_id, pending.generation, now())
        .expect("claim");
    let message = envelope(&fixture, &action);
    fixture
        .store
        .admit_supervisor_message(
            action.action_id,
            action.generation,
            &fixture.start.full_plan,
            &message,
            now(),
        )
        .expect("first admitted");
    fixture
        .store
        .finish_supervisor_action(&finish(&action))
        .expect("finish first");
    let pending = pending_send(&mut fixture, "second_message");
    let second = fixture
        .store
        .claim_supervisor_action(pending.action_id, pending.generation, now())
        .expect("claim second");
    let message = envelope(&fixture, &second);
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                second.action_id,
                second.generation,
                &fixture.start.full_plan,
                &message,
                now()
            )
            .is_err(),
        "bounded outstanding messages"
    );
    fixture
        .store
        .recover_supervisors(now())
        .expect("recover claimed outcome");
    assert_eq!(
        fixture
            .store
            .supervisor_status(fixture.start.workflow_id)
            .expect("status")
            .expect("execution")
            .execution
            .state,
        SupervisorExecutionState::RecoveryRequired
    );
    assert!(
        fixture
            .store
            .admit_supervisor_message(
                second.action_id,
                second.generation,
                &fixture.start.full_plan,
                &message,
                now()
            )
            .is_err()
    );
}

#[test]
fn a_query_cannot_report_applied_without_retrieved_references() {
    let mut fixture = Fixture::configured(16, true);
    let (round, proposal) = fixture.proposal(
        "query",
        SupervisorTool::QueryContext {
            task_key: id("track_a"),
        },
    );
    let commit = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("query intent");
    let action = fixture
        .store
        .claim_supervisor_action(
            commit.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim query");
    let mut result = finish(&action);
    result.result_refs.clear();
    assert!(fixture.store.finish_supervisor_action(&result).is_err());
}
