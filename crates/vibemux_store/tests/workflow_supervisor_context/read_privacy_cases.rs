//! Production-store regressions for read evidence and conservative information flow.
use super::*;
use vibemux_store::MessageChange;
use vibemux_workflow::{
    messages::{MessageEnvelope, MessageKind, Participant},
    supervisor::{
        SupervisorAction,
        context_actions::{effect_id, recipient},
    },
};

const SOURCE_ONLY_TEXT: &str =
    "Source-only worker observation: never disclose this through a free-form send.";

fn message(fixture: &Fixture) -> MessageEnvelope {
    let task_key = id("track_a");
    let binding = &fixture.execution.tasks[&task_key];
    let message_id = Uuid::new_v4();
    let timestamp_ms = u64::try_from(now().unix_timestamp_nanos() / 1_000_000).expect("time");
    MessageEnvelope {
        message_id,
        workflow_id: fixture.start.workflow_id,
        sender: recipient(&fixture.execution, &task_key).expect("current source session"),
        recipient: Participant::Supervisor,
        kind: MessageKind::Answer,
        correlation_id: message_id,
        reply_to: None,
        depth: 0,
        contract_id: binding.contract_id,
        contract_version: binding.contract_version,
        source_refs: vec![],
        body_sha256: Sha256Digest::of(SOURCE_ONLY_TEXT.as_bytes()),
        body_bytes: SOURCE_ONLY_TEXT.len() as u64,
        created_at_ms: timestamp_ms,
        expires_at_ms: timestamp_ms + 60_000,
        audience: vec![Participant::Supervisor],
        escalated: false,
    }
}

fn claimed_query(fixture: &mut Fixture) -> SupervisorAction {
    let (round, proposal) = fixture.proposal(
        "private_query",
        SupervisorTool::QueryContext {
            task_key: id("track_a"),
        },
    );
    let commit = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("query proposal");
    assert!(commit.accepted);
    fixture
        .store
        .claim_supervisor_action(
            commit.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim read")
}

fn result(action: &SupervisorAction, refs: Vec<Uuid>) -> SupervisorActionFinish {
    SupervisorActionFinish {
        action_id: action.action_id,
        generation: action.generation,
        state: SupervisorActionState::Applied,
        result_refs: refs,
        result_code: Some(id("supervisor_context_read")),
        timestamp: now(),
    }
}

fn record_read(
    fixture: &mut Fixture,
    action: &SupervisorAction,
    refs: &[Uuid],
) -> Result<(), vibemux_store::StoreError> {
    fixture.store.record_supervisor_context_read(
        action.action_id,
        action.generation,
        &fixture.start.full_plan,
        refs,
        now(),
    )
}

#[test]
fn a_live_message_is_not_read_evidence_until_its_claim_bound_read_is_recorded() {
    let mut fixture = Fixture::configured(16, true);
    let first = message(&fixture);
    let mut second = first.clone();
    second.message_id = Uuid::new_v4();
    second.correlation_id = second.message_id;
    fixture
        .store
        .admit_workflow_message(&first, now())
        .expect("source message");
    fixture
        .store
        .admit_workflow_message(&second, now())
        .expect("second source message");
    let action = claimed_query(&mut fixture);
    assert!(
        fixture
            .store
            .finish_supervisor_action(&result(&action, vec![first.message_id]))
            .is_err(),
        "pre-existing message alone proves no read"
    );
    record_read(&mut fixture, &action, &[first.message_id]).expect("post-claim read evidence");
    record_read(&mut fixture, &action, &[first.message_id])
        .expect("identical evidence is idempotent");
    assert!(
        record_read(&mut fixture, &action, &[second.message_id]).is_err(),
        "read evidence cannot retarget its refs"
    );
    assert!(
        fixture
            .store
            .finish_supervisor_action(&result(&action, vec![second.message_id]))
            .is_err()
    );
    fixture
        .store
        .finish_supervisor_action(&result(&action, vec![first.message_id]))
        .expect("only exact proved read applies");
}

#[test]
fn query_evidence_refuses_wrong_sessions_contracts_tasks_and_dead_messages() {
    let mut fixture = Fixture::configured(16, true);
    let action = claimed_query(&mut fixture);
    let valid = message(&fixture);
    let mut variants = Vec::new();
    let mut wrong = valid.clone();
    wrong.sender = Participant::Worker {
        task_key: id("track_a"),
        session_id: Uuid::new_v4(),
    };
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.contract_id = fixture.execution.tasks[&id("track_b")].contract_id;
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.contract_version += 1;
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.sender = recipient(&fixture.execution, &id("track_b")).expect("other worker");
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.created_at_ms += 20_000;
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong.expires_at_ms = wrong.created_at_ms.saturating_sub(1);
    variants.push(wrong);
    let mut wrong = valid.clone();
    wrong
        .audience
        .push(recipient(&fixture.execution, &id("track_b")).expect("unapproved audience"));
    variants.push(wrong);
    for mut wrong in variants {
        wrong.message_id = Uuid::new_v4();
        wrong.correlation_id = wrong.message_id;
        fixture
            .store
            .admit_workflow_message(&wrong, now())
            .expect("trusted fixture injects a malformed message record");
        assert!(
            record_read(&mut fixture, &action, &[wrong.message_id]).is_err(),
            "invalid source cannot acquire read proof"
        );
        assert!(
            fixture
                .store
                .finish_supervisor_action(&result(&action, vec![wrong.message_id]))
                .is_err()
        );
    }
    fixture
        .store
        .admit_workflow_message(&valid, now())
        .expect("live source");
    let digest = Sha256Digest::of_fields(
        "vibemux.workflow.supervisor_inbox.v1",
        &[valid.message_id.as_bytes()],
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    let attempt = uuid::Builder::from_random_bytes(bytes).into_uuid();
    fixture
        .store
        .change_workflow_message(valid.message_id, &MessageChange::Deliver { attempt }, now())
        .expect("deliver to exact supervisor inbox");
    assert!(
        record_read(&mut fixture, &action, &[valid.message_id]).is_err(),
        "already-delivered messages are not pending reads"
    );
    fixture
        .store
        .change_workflow_message(
            valid.message_id,
            &MessageChange::Acknowledge {
                by: Participant::Supervisor,
                attempt,
            },
            now(),
        )
        .expect("acknowledge");
    assert!(
        record_read(&mut fixture, &action, &[valid.message_id]).is_err(),
        "dead message cannot acquire read proof"
    );
}

#[test]
fn expiry_after_read_evidence_still_prevents_an_applied_query() {
    let mut fixture = Fixture::configured(16, true);
    let envelope = message(&fixture);
    fixture
        .store
        .admit_workflow_message(&envelope, now())
        .expect("source");
    let action = claimed_query(&mut fixture);
    record_read(&mut fixture, &action, &[envelope.message_id]).expect("read while live");
    let expired_at =
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(envelope.expires_at_ms) * 1_000_000)
            .expect("expiry");
    let mut finish = result(&action, vec![envelope.message_id]);
    finish.timestamp = expired_at;
    assert!(
        fixture.store.finish_supervisor_action(&finish).is_err(),
        "an expired reference cannot be applied even with earlier read evidence"
    );
    fixture
        .store
        .change_workflow_message(envelope.message_id, &MessageChange::Expire, expired_at)
        .expect("expire source");
    assert!(fixture.store.finish_supervisor_action(&finish).is_err());
}

#[test]
fn private_read_history_forbids_free_form_send_after_the_read_leaves_recent_status() {
    let mut fixture = Fixture::configured(32, true);
    let envelope = message(&fixture);
    fixture
        .store
        .admit_workflow_message(&envelope, now())
        .expect("source-only supervisor message");
    let query = claimed_query(&mut fixture);
    record_read(&mut fixture, &query, &[envelope.message_id]).expect("actual read owner evidence");
    fixture
        .store
        .finish_supervisor_action(&result(&query, vec![envelope.message_id]))
        .expect("read applied");
    for index in 0..9 {
        let (round, proposal) = fixture.proposal(
            &format!("later_inspect_{index}"),
            SupervisorTool::InspectWorkflow {},
        );
        let commit = fixture
            .store
            .commit_supervisor_proposal(round.round_id, &proposal, now())
            .expect("inspection");
        let inspect = fixture
            .store
            .claim_supervisor_action(
                commit.actions[0].action_id,
                fixture.execution.generation,
                now(),
            )
            .expect("claim inspection");
        fixture
            .store
            .finish_supervisor_action(&result(&inspect, vec![]))
            .expect("inspect applied");
    }
    assert!(
        !fixture
            .store
            .supervisor_observation(fixture.start.workflow_id)
            .expect("bounded model observation")
            .last_actions
            .iter()
            .any(|action| action.action_id == query.action_id)
    );
    let (round, proposal) = fixture.proposal(
        "leak_source_text",
        SupervisorTool::SendMessage {
            to: id("track_b"),
            text: SOURCE_ONLY_TEXT.into(),
        },
    );
    let rejected = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("durable policy rejection");
    assert!(!rejected.accepted && rejected.actions.is_empty());
    assert!(
        rejected
            .round
            .rejection_codes
            .contains(&"supervisor_message_after_private_read".into())
    );
    assert_eq!(
        fixture
            .store
            .workflow_snapshot(fixture.start.workflow_id)
            .expect("snapshot")
            .expect("workflow")
            .messages
            .len(),
        1,
        "zero forwarded effects"
    );
}

#[test]
fn send_admission_rechecks_private_reads_that_applied_after_the_batch_was_committed() {
    let mut fixture = Fixture::configured(16, true);
    let source = message(&fixture);
    fixture
        .store
        .admit_workflow_message(&source, now())
        .expect("source-only data");
    let observed = fixture.observe();
    let round = fixture.reserve(&observed);
    fixture.complete(&round);
    let proposal = SupervisorProposal {
        decisions: vec![
            SupervisorDecision {
                idempotency_key: "query_before_send".into(),
                expected_workflow_version: observed.workflow.version,
                action: SupervisorTool::QueryContext {
                    task_key: id("track_a"),
                },
            },
            SupervisorDecision {
                idempotency_key: "send_after_query".into(),
                expected_workflow_version: observed.workflow.version,
                action: SupervisorTool::SendMessage {
                    to: id("track_b"),
                    text: SOURCE_ONLY_TEXT.into(),
                },
            },
        ],
        rationale: "Fixture concurrent policy boundary".into(),
        evidence_refs: vec![],
    };
    let committed = fixture
        .store
        .commit_supervisor_proposal(round.round_id, &proposal, now())
        .expect("initially permitted batch");
    assert!(committed.accepted);
    let query = fixture
        .store
        .claim_supervisor_action(
            committed.actions[0].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim query");
    let send = fixture
        .store
        .claim_supervisor_action(
            committed.actions[1].action_id,
            fixture.execution.generation,
            now(),
        )
        .expect("claim send");
    record_read(&mut fixture, &query, &[source.message_id]).expect("read after batch claim");
    fixture
        .store
        .finish_supervisor_action(&result(&query, vec![source.message_id]))
        .expect("read applies before send");
    let mut target = source.clone();
    target.message_id = effect_id(send.action_id, "message");
    target.correlation_id = target.message_id;
    target.sender = Participant::Supervisor;
    target.recipient = recipient(&fixture.execution, &id("track_b")).expect("target");
    target.kind = MessageKind::Question;
    target.contract_id = fixture.execution.tasks[&id("track_b")].contract_id;
    target.audience = vec![target.recipient.clone(), Participant::Supervisor];
    let error = fixture
        .store
        .admit_supervisor_message(
            send.action_id,
            send.generation,
            &fixture.start.full_plan,
            &target,
            now(),
        )
        .expect_err("atomic no-declassification guard");
    assert_eq!(error.code(), "supervisor_message_after_private_read");
    assert!(
        fixture
            .store
            .workflow_message(target.message_id)
            .expect("message lookup")
            .is_none()
    );
    assert!(
        fixture
            .store
            .finish_supervisor_action(&result(&send, vec![target.message_id]))
            .is_err()
    );
}
