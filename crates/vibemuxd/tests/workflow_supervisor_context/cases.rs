//! Actual bounded model -> broker -> guarded worker -> model data round trip.
use super::*;

const SOURCE_TEXT: &str = "export const supervisor_context_payload = 'fixture_only';\n";
const REQUEST_TEXT: &str =
    "Please share the admitted API candidate excerpt for this worker request.";
const MESSAGE_TEXT: &str =
    "Describe the current admitted candidate without changing the task contract.";
const ANSWER_TEXT: &str = "The supervisor question reached the exact worker session; this is an untrusted fixture answer.";

pub(super) fn leak_actions(
    index: usize,
    observation: &SupervisorObservation,
    context: &Value,
) -> Vec<Value> {
    match index {
        0 => vec![
            json!({"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"slot_a"}}),
        ],
        1 => {
            let candidate = observation
                .candidates
                .iter()
                .find(|candidate| candidate.candidate.task_key.as_str() == "track_a")
                .expect("source-only candidate");
            vec![
                json!({"tool":"read_artifact","arguments":{"artifact_digest":candidate.candidate.candidate_digest}}),
            ]
        }
        2 => {
            assert!(
                context["tool_results"]
                    .as_array()
                    .expect("genuine private retrieval")
                    .iter()
                    .any(|result| result["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(SOURCE_TEXT)))
            );
            vec![json!({"tool":"send_message","arguments":{"to":"track_b","text":SOURCE_TEXT}})]
        }
        3 => {
            assert!(
                observation
                    .last_round
                    .as_ref()
                    .expect("rejected proposal")
                    .rejection_codes
                    .contains(&"supervisor_message_after_private_read".into())
            );
            vec![
                json!({"tool":"report_blocked","arguments":{"reason_code":"privacy_refusal_observed"}}),
            ]
        }
        _ => panic!("privacy scenario exceeded its bounded round count"),
    }
}

pub(super) fn actions(
    index: usize,
    observation: &SupervisorObservation,
    context: &Value,
) -> Vec<Value> {
    match index {
        0 => vec![json!({"tool":"send_message","arguments":{"to":"track_a","text":MESSAGE_TEXT}})],
        1 => vec![
            json!({"tool":"assign_task","arguments":{"task_key":"track_a","slot_id":"slot_a"}}),
            json!({"tool":"assign_task","arguments":{"task_key":"track_b","slot_id":"slot_b"}}),
        ],
        2 => {
            let candidate = observation
                .candidates
                .iter()
                .find(|candidate| candidate.candidate.task_key.as_str() == "track_a")
                .expect("collected source");
            vec![
                json!({"tool":"read_artifact","arguments":{"artifact_digest":candidate.candidate.candidate_digest}}),
                json!({"tool":"query_context","arguments":{"task_key":"track_b"}}),
            ]
        }
        3 => {
            let results = context["tool_results"]
                .as_array()
                .expect("bounded actual retrieved data");
            assert!(results.iter().any(|result| {
                result["trust"] == "untrusted_data"
                    && result["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(SOURCE_TEXT))
            }));
            let request = results
                .iter()
                .find(|result| result["text"].as_str() == Some(REQUEST_TEXT))
                .expect("actual worker request body, not metadata");
            vec![
                json!({"tool":"share_context","arguments":{"from_task":"track_a","to_task":"track_b","request_message_id":request["reference_id"]}}),
            ]
        }
        4 => vec![
            json!({"tool":"assign_task","arguments":{"task_key":"track_b","slot_id":"slot_b"}}),
        ],
        5 => vec![json!({"tool":"query_context","arguments":{"task_key":"track_a"}})],
        6 => {
            assert!(
                context["tool_results"]
                    .as_array()
                    .expect("reply context")
                    .iter()
                    .any(|result| result["text"].as_str() == Some(ANSWER_TEXT))
            );
            vec![
                json!({"tool":"report_blocked","arguments":{"reason_code":"context_fixture_done"}}),
            ]
        }
        _ => panic!("context scenario exceeded its bounded round count"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn supervisor_reads_messages_and_context_shares_reach_the_next_real_bounded_context() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::ContextActions).await;
    fixture.write_script(&json!({"steps":{
        "track_a/implement_1":{"write":{"src/store.mjs":SOURCE_TEXT},
            "checkpoint":{"messages":[{"kind":"answer","to":"supervisor","text":ANSWER_TEXT}]}},
        "track_b/implement_1":{"write":{"public/app.mjs":"export const page_context = 'fixture';\n"},
            "checkpoint":{"messages":[{"kind":"context_request","to":"track_a","text":REQUEST_TEXT}]}},
        "track_b/repair_2":{"checkpoint":{"consume_context":true}}
    }}));
    let mut request = cooperative_request(
        "supervisor_context_actions",
        &fixture.head,
        &track_a(&SYNTAX),
        &track_b(&SYNTAX),
        &SYNTAX,
    );
    for spec in request["task_specs"].as_array_mut().expect("task specs") {
        spec["context_policy"]["retrieval_order"] =
            json!(["candidate_snapshot", "public_messages"]);
    }
    request["task_specs"][1]["read_scopes"] = json!(["CONTRACT.md", "src/store.mjs"]);
    let prepared = client
        .workflow_prepare(request, policy(&SYNTAX, 2, true))
        .await
        .expect("admitted scoped contracts");
    let workflow = workflow_id(&prepared);
    client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("binding"),
            Uuid::new_v4(),
        )
        .await
        .expect("start model driver");
    let status = fixture
        .wait_phase(workflow, &["blocked", "failed", "accepted"])
        .await;
    assert_eq!(phase(&status), "blocked", "{status:#}");
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        7,
        "all data/effects settled before next round"
    );
    let source_turns = fixture.started("track_a/implement_1");
    assert_eq!(source_turns.len(), 1);
    assert!(
        source_turns[0]["prompt"]
            .as_str()
            .expect("rendered question")
            .contains(MESSAGE_TEXT)
    );
    let target_turns = fixture.started("track_b/repair_2");
    assert_eq!(target_turns.len(), 1);
    let prompt = target_turns[0]["prompt"]
        .as_str()
        .expect("rendered scoped bundle");
    assert!(
        prompt.contains(SOURCE_TEXT),
        "actual bundle bytes must reach the recipient"
    );
    let messages = status["messages"].as_array().expect("message evidence");
    assert!(
        messages
            .iter()
            .any(|message| message["kind"] == "context_grant"
                && message["sender"] == "supervisor"
                && message["recipient"] == "worker:track_b"
                && message["state"] == "acknowledged")
    );
    assert!(messages.iter().any(|message| message["kind"] == "question"
        && message["sender"] == "supervisor"
        && message["recipient"] == "worker:track_a"
        && message["state"] == "acknowledged"));
    let status_text = serde_json::to_string(&status).expect("public status");
    assert!(!status_text.contains(MESSAGE_TEXT));
    assert!(!status_text.contains(SOURCE_TEXT));
    {
        let contexts = model.contexts.lock().expect("contexts");
        for context in contexts.iter() {
            assert!(
                serde_json::to_vec(&context["tool_results"])
                    .expect("bounded data")
                    .len()
                    <= vibemux_workflow::supervisor::MAX_SUPERVISOR_CONTEXT_BYTES + 64
            );
        }
    }
    server.shutdown().await.expect("server shutdown");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_candidate_text_cannot_be_sent_to_a_recipient_without_shared_read_scope() {
    let (fixture, server, client, model) = start_model_fixture(Strategy::ContextLeakAttack).await;
    fixture.write_script(
        &json!({"steps":{"track_a/implement_1":{"write":{"src/store.mjs":SOURCE_TEXT}}}}),
    );
    let mut request = cooperative_request(
        "supervisor_context_no_declassification",
        &fixture.head,
        &track_a(&SYNTAX),
        &track_b(&SYNTAX),
        &SYNTAX,
    );
    for spec in request["task_specs"].as_array_mut().expect("specs") {
        spec["context_policy"]["retrieval_order"] =
            json!(["candidate_snapshot", "public_messages"]);
        spec["communication_policy"]["may_message"] = json!([]);
    }
    // Track B keeps CONTRACT.md as its sole read scope. It cannot receive A's
    // source bytes through ShareContext, so free-form text must not bypass it.
    assert_eq!(
        request["task_specs"][1]["read_scopes"],
        json!(["CONTRACT.md"])
    );
    let prepared = client
        .workflow_prepare(request, policy(&SYNTAX, 2, true))
        .await
        .expect("isolated source scopes");
    let workflow = workflow_id(&prepared);
    client
        .workflow_supervisor_start(
            prepared["start_contract"].as_str().expect("binding"),
            Uuid::new_v4(),
        )
        .await
        .expect("start guarded fixture");
    let status = fixture
        .wait_phase(workflow, &["blocked", "failed", "accepted"])
        .await;
    assert_eq!(phase(&status), "blocked", "{status:#}");
    assert_eq!(model.calls.load(Ordering::SeqCst), 4);
    assert!(
        status["messages"]
            .as_array()
            .expect("message evidence")
            .is_empty(),
        "zero forwarding effects"
    );
    assert_eq!(fixture.started_keys(), vec!["track_a/implement_1"]);
    let supervisor = client
        .workflow_supervisor_status(workflow)
        .await
        .expect("supervisor status");
    assert!(
        !serde_json::to_string(&supervisor)
            .expect("status JSON")
            .contains(SOURCE_TEXT)
    );
    server.shutdown().await.expect("server shutdown");
    fixture.shutdown().await;
}
