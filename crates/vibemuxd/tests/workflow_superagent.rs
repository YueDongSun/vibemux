#![cfg(feature = "test_helpers")]
//! Prototype-inspired intake, follow-ups and report behavior over real IPC,
//! owned fixture processes and temporary Git repositories (ADR 057).

mod workflow_support;

use serde_json::{Value, json};
use uuid::Uuid;
use vibemuxd::control::ControlError;
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, good_files, policy, remote, review_pass,
    session_id, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const QUESTION: &str = "Should the existing contract stay unchanged?";
const PRIVATE_SUMMARY: &str = "PRIVATE worker summary for the operator";

fn request(fixture: &WorkflowFixture, key: &str) -> Value {
    let mut request = cooperative_request(
        key,
        &fixture.head,
        &track_a(&SYNTAX),
        &track_b(&SYNTAX),
        &SYNTAX,
    );
    for spec in request["task_specs"].as_array_mut().expect("specs") {
        spec["template_version"] = json!("worker_v2");
    }
    request
}

fn script(blocked: bool) -> Value {
    let state = if blocked { "blocked" } else { "ready" };
    let mut report = json!({"state":state,"summary":PRIVATE_SUMMARY});
    if blocked {
        report["human_action"] = json!(QUESTION);
    }
    json!({"steps":{
        "track_a/implement_1":{"write":good_files(&["src/server.mjs","src/store.mjs"]),
            "checkpoint":{"worker_report":report}},
        "track_b/implement_1":{"write":good_files(&["public/app.mjs"]),
            "checkpoint":{"worker_report":{"state":"ready","summary":"Frontend ready"}}},
        "track_a/answer_2":{"checkpoint":{"worker_report":{"state":"ready","summary":"Clarification resolved"}}},
        "track_a/review_1":review_pass(),"track_b/review_1":review_pass()
    }})
}

fn followup(workflow: Uuid, status: &Value) -> Value {
    let task = status["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|task| task["task_key"] == "track_a")
        .expect("task");
    json!({"schema_version":1,"event_id":Uuid::new_v4(),"workflow_id":workflow,
        "expected_workflow_version":status["version"], "task_key":"track_a",
        "session_id":session_id(workflow,"track_a","slot_a"),
        "contract_id":task["contract_id"],"contract_version":task["contract_version"],
        "kind":"question","text":"Keep the admitted contract. Literal data: $(touch forbidden) `echo unsafe`; 范围不变。"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_intake_is_idempotent_and_ready_still_requires_review_and_verification() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&script(false));
    let envelope = json!({"schema_version":1,"event_id":Uuid::new_v4(),
        "request":request(&fixture,"operator_request"),"policy":policy(&SYNTAX,2,true)});
    let first = fixture
        .client
        .workflow_intake(envelope.clone())
        .await
        .expect("intake");
    let second = fixture
        .client
        .workflow_intake(envelope.clone())
        .await
        .expect("duplicate intake");
    assert_eq!(first["authority"], "authenticated_operator");
    assert_eq!(first["prepared"]["duplicate"], false);
    assert_eq!(second["prepared"]["duplicate"], true);
    assert_eq!(
        first["prepared"]["workflow_id"],
        second["prepared"]["workflow_id"]
    );
    assert_eq!(first["mandate"], second["mandate"]);
    let mut altered = envelope.clone();
    altered["policy"]["max_parallel_slots"] = json!(1);
    assert_eq!(
        fixture.client.workflow_intake(altered).await,
        Err(remote("workflow_request_conflict"))
    );
    let mut forged = envelope;
    forged["authority"] = json!("remote_actor");
    assert_eq!(
        fixture.client.workflow_intake(forged).await,
        Err(ControlError::InvalidRequest)
    );
    let prepared = first["prepared"].clone();
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    fixture.assert_phase(&status, "accepted").await;
    assert!(
        !status["receipts"]["reviews"]
            .as_array()
            .expect("reviews")
            .is_empty()
    );
    assert!(
        !status["receipts"]["verifications"]
            .as_array()
            .expect("verifications")
            .is_empty()
    );
    let reports = fixture
        .client
        .workflow_reports(workflow, 0, false)
        .await
        .expect("reports");
    assert_eq!(reports["reports"].as_array().expect("reports").len(), 2);
    assert!(!reports.to_string().contains(PRIVATE_SUMMARY));
    assert!(!reports.to_string().contains(QUESTION));
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attention_report_survives_restart_and_a_bound_followup_resolves_it_at_a_turn_boundary()
{
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&script(true));
    let prepared = fixture
        .prepare(
            request(&fixture, "operator_followup"),
            policy(&SYNTAX, 2, true),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["blocked", "accepted", "failed"])
        .await;
    fixture.assert_phase(&status, "blocked").await;
    assert_eq!(
        status["blocked_reason"],
        "workflow_worker_report_attention_required"
    );
    let reports = fixture
        .client
        .workflow_reports(workflow, 0, false)
        .await
        .expect("metadata");
    let content = fixture
        .client
        .workflow_reports(workflow, 0, true)
        .await
        .expect("explicit content");
    assert!(!reports.to_string().contains(PRIVATE_SUMMARY));
    assert!(content.to_string().contains(PRIVATE_SUMMARY));
    assert!(content.to_string().contains(QUESTION));
    fixture.restart().await;
    assert_eq!(
        fixture
            .client
            .workflow_reports(workflow, 0, false)
            .await
            .expect("durable"),
        reports
    );
    let status = fixture.status(workflow).await;
    let request = followup(workflow, &status);
    let mut stale = request.clone();
    stale["expected_workflow_version"] = json!(0);
    assert_eq!(
        fixture.client.workflow_followup(stale).await,
        Err(remote("workflow_followup_invalid"))
    );
    let mut stale = request.clone();
    stale["expected_workflow_version"] = json!(1);
    assert_eq!(
        fixture.client.workflow_followup(stale).await,
        Err(remote("workflow_followup_stale"))
    );
    let mut unknown = request.clone();
    unknown["session_id"] = json!(Uuid::new_v4());
    assert_eq!(
        fixture.client.workflow_followup(unknown).await,
        Err(remote("workflow_followup_unknown_session"))
    );
    let mut forged = request.clone();
    forged["sender"] = json!("supervisor");
    assert_eq!(
        fixture.client.workflow_followup(forged).await,
        Err(ControlError::InvalidRequest)
    );
    let mut grant = request.clone();
    grant["kind"] = json!("context_grant");
    assert_eq!(
        fixture.client.workflow_followup(grant).await,
        Err(remote("workflow_followup_invalid"))
    );
    let (first, repeated) = tokio::join!(
        fixture.client.workflow_followup(request.clone()),
        fixture.client.workflow_followup(request.clone())
    );
    let first = first.expect("followup");
    let repeated = repeated.expect("repeated");
    assert_ne!(first["duplicate"], repeated["duplicate"]);
    assert_eq!(
        first["message"]["envelope"]["message_id"],
        repeated["message"]["envelope"]["message_id"]
    );
    assert_eq!(first["context_only"], true);
    let mut conflicting = request.clone();
    conflicting["text"] = json!("A different request under the same event");
    assert_eq!(
        fixture.client.workflow_followup(conflicting).await,
        Err(remote("store_workflow_message_conflict"))
    );
    let message_id = first["message"]["envelope"]["message_id"].clone();
    fixture.restart().await;
    let unchanged = fixture.status(workflow).await;
    assert_eq!(unchanged["tasks"], status["tasks"]);
    fixture.start_workflow(&prepared).await;
    let final_status = fixture
        .wait_phase(workflow, &["accepted", "blocked", "failed"])
        .await;
    fixture.assert_phase(&final_status, "accepted").await;
    let turns = fixture.started("track_a/answer_2");
    assert_eq!(turns.len(), 1);
    assert!(
        turns[0]["prompt"]
            .as_str()
            .expect("prompt")
            .contains("$(touch forbidden)")
    );
    let message = final_status["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["message_id"] == message_id)
        .expect("message");
    assert_eq!(message["state"], "acknowledged");
    assert!(!fixture.project_root.join("forbidden").exists());
    let final_reports = fixture
        .client
        .workflow_reports(workflow, 0, true)
        .await
        .expect("resolved reports");
    assert_eq!(
        final_reports["reports"].as_array().expect("reports").len(),
        3
    );
    let cursor = final_reports["next"].as_u64().expect("cursor");
    assert!(
        fixture
            .client
            .workflow_reports(workflow, cursor, false)
            .await
            .expect("next page")["reports"]
            .as_array()
            .expect("empty")
            .is_empty()
    );
    fixture
        .client
        .workflow_purge_content(workflow)
        .await
        .expect("purge");
    let purged = fixture
        .client
        .workflow_reports(workflow, 0, true)
        .await
        .expect("purged report content");
    assert!(!purged.to_string().contains(PRIVATE_SUMMARY));
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn structured_reports_keep_only_metadata_when_content_retention_is_disabled() {
    let fixture = WorkflowFixture::start(FixtureOptions {
        content_retention_days: None,
        ..FixtureOptions::default()
    })
    .await;
    fixture.write_script(&script(true));
    let prepared = fixture
        .prepare(
            request(&fixture, "private_report"),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["blocked", "accepted", "failed"])
        .await;
    fixture.assert_phase(&status, "blocked").await;
    let reports = fixture
        .client
        .workflow_reports(workflow, 0, true)
        .await
        .expect("reports");
    assert!(!reports.to_string().contains(PRIVATE_SUMMARY));
    assert!(!reports.to_string().contains(QUESTION));
    assert!(
        reports["reports"]
            .as_array()
            .expect("reports")
            .iter()
            .any(|report| report["claim"]["needs_attention"] == true)
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_content_does_not_transfer_retention_permission_between_workflows() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&script(true));
    let retained = fixture
        .prepare(
            request(&fixture, "retained_owner"),
            policy(&SYNTAX, 2, true),
        )
        .await;
    let retained_id = workflow_id(&retained);
    fixture.start_workflow(&retained).await;
    fixture.wait_phase(retained_id, &["blocked"]).await;
    let private = fixture
        .prepare(
            request(&fixture, "private_owner"),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let private_id = workflow_id(&private);
    fixture.start_workflow(&private).await;
    fixture.wait_phase(private_id, &["blocked"]).await;
    let retained_reports = fixture
        .client
        .workflow_reports(retained_id, 0, true)
        .await
        .expect("retained");
    assert!(retained_reports.to_string().contains(PRIVATE_SUMMARY));
    let private_reports = fixture
        .client
        .workflow_reports(private_id, 0, true)
        .await
        .expect("private");
    assert!(!private_reports.to_string().contains(PRIVATE_SUMMARY));
    assert!(!private_reports.to_string().contains(QUESTION));
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_structured_reports_cannot_succeed_after_an_operator_restart() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut missing = script(false);
    missing["steps"]["track_a/implement_1"]["no_report"] = json!(true);
    fixture.write_script(&missing);
    let prepared = fixture
        .prepare(
            request(&fixture, "missing_structured"),
            policy(&SYNTAX, 2, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["blocked", "failed", "accepted"])
        .await;
    fixture.assert_phase(&status, "blocked").await;
    assert_eq!(
        status["blocked_reason"],
        "workflow_worker_checkpoint_invalid"
    );
    fixture.restart().await;
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["blocked", "failed", "accepted"])
        .await;
    assert_ne!(status["phase"], "accepted");
    fixture.shutdown().await;
}
