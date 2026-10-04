#![cfg(feature = "test_helpers")]
//! Cooperative dual-track workflow end to end (ADR 031): two writable
//! fixture workers build TaskBoard Lite in separate worktrees, exchange one
//! scoped context bundle, pass an independent review and the trusted
//! TaskBoard suites, and are integrated without touching the original
//! checkout. Synthetic workers: this is offline orchestration evidence.

mod workflow_support;

use serde_json::{Value, json};
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, good_files, phase, policy, review_pass,
    session_id, task_view, taskboard_file, track_a, track_b, workflow_id,
};

const QUESTION: &str = "Which error code does the store raise for a blank title?";
const ANSWER: &str = "The store raises invalid_title for a title that is blank after trimming.";
const STORE_FIRST_LINE: &str = "TaskBoard Lite store: calibration reference implementation";
const TURN_DELAY_MS: u64 = 1_500;

fn flagship_script() -> Value {
    json!({"steps": {
        "track_a/implement_1": {
            "write": good_files(&["src/server.mjs", "src/store.mjs"]),
            "delay_ms": TURN_DELAY_MS,
        },
        "track_b/implement_1": {
            "write": good_files(&["public/app.mjs", "public/index.html", "public/styles.css"]),
            "delay_ms": TURN_DELAY_MS,
            "checkpoint": {"messages": [{"kind": "question", "to": "track_a", "text": QUESTION}]},
        },
        "track_a/answer_2": {
            "checkpoint": {"messages": [{"kind": "answer", "to": "track_b", "text": ANSWER,
                "source_refs": [{"path": "src/store.mjs", "start_line": 1, "end_line": 12}]}]},
        },
        "track_b/implement_2": {"checkpoint": {"consume_context": true}},
        "track_a/review_1": review_pass(),
        "track_b/review_1": review_pass(),
    }})
}

fn started_at(record: &Value) -> u64 {
    record["at_ms"].as_u64().expect("at_ms")
}

/// The `finished` record that follows a `started` record of the same key
/// and claim.
fn finished_at(log: &[Value], key: &str) -> u64 {
    log.iter()
        .find(|record| record["event"] == "finished" && record["key"] == key)
        .map(started_at)
        .expect("finished record")
}

/// A question bound to a failed answer attempt remains unacknowledged.
/// The later repair turn must launch without rendering or rebinding it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_answer_delivery_does_not_block_later_repair() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut first_files = good_files(&["src/server.mjs", "src/store.mjs"]);
    first_files["src/store.mjs"] = json!(taskboard_file(
        "calibration/mutants/blank_title_accepted/src/store.mjs"
    ));
    fixture.write_script(&json!({"steps": {
        "track_a/implement_1": {"write": first_files},
        "track_b/implement_1": {
            "write": good_files(&["public/app.mjs", "public/index.html", "public/styles.css"]),
            "checkpoint": {"messages": [{"kind": "question", "to": "track_a", "text": QUESTION}]},
        },
        "track_a/answer_2": {"fail": true},
        "track_a/repair_3": {"write": good_files(&["src/store.mjs"])},
        "track_a/review": review_pass(),
        "track_b/review": review_pass(),
    }}));
    let suites = ["store", "browser_frontend_only"];
    let prepared = fixture
        .prepare(
            cooperative_request(
                "failed_answer_delivery",
                &fixture.head,
                &track_a(&["store"]),
                &track_b(&["browser_frontend_only"]),
                &suites,
            ),
            policy(&suites, 2, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(workflow).await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    assert_eq!(fixture.started("track_a/answer_2").len(), 1);
    let repair = fixture.started("track_a/repair_3");
    assert_eq!(repair.len(), 1, "repair did not launch");
    assert!(
        !repair[0]["prompt"]
            .as_str()
            .expect("prompt")
            .contains(QUESTION)
    );
    let question = status["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["kind"] == "question")
        .expect("question");
    assert_eq!(question["state"], "delivered");
    let exported = fixture.export_all(workflow).await;
    let answer = exported
        .iter()
        .find(|item| {
            item["kind"] == "attempt"
                && item["body"]["task_key"] == "track_a"
                && item["body"]["purpose"] == "answer"
        })
        .expect("failed answer attempt");
    let message = exported
        .iter()
        .find(|item| {
            item["kind"] == "message"
                && item["body"]["envelope"]["message_id"] == question["message_id"]
        })
        .expect("message evidence");
    assert_eq!(
        message["body"]["delivered_in_attempt"],
        answer["body"]["request_id"]
    );
    assert!(message["body"]["acknowledged_in_attempt"].is_null());
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    fixture.write_script(&flagship_script());
    let suites = ["api", "store", "browser", "browser_frontend_only"];
    let request = cooperative_request(
        "taskboard_flagship",
        &fixture.head,
        &track_a(&["api", "store"]),
        &track_b(&["browser_frontend_only"]),
        &["api", "store", "browser"],
    );
    let prepared = fixture
        .prepare(request.clone(), policy(&suites, 2, true))
        .await;
    assert_eq!(prepared["phase"], "prepared");
    assert_eq!(prepared["duplicate"], false);
    assert_eq!(
        prepared["contracts"].as_array().expect("contracts").len(),
        2
    );
    // P01: preparing the same request again reuses the stored admission
    // and the byte-identical contracts.
    let replay = fixture.prepare(request, policy(&suites, 2, true)).await;
    assert_eq!(replay["duplicate"], true);
    assert_eq!(replay["contracts"], prepared["contracts"]);
    assert_eq!(replay["start_contract"], prepared["start_contract"]);

    let workflow = workflow_id(&prepared);
    let started = fixture.start_workflow(&prepared).await;
    assert_eq!(started["duplicate"], false);
    let status = fixture.wait_idle(workflow).await;
    assert_eq!(phase(&status), "accepted", "status: {status:#}");
    assert_eq!(status["last_run"]["end"]["end"], "accepted");

    // S01: both implementation turns ran in separate sessions and overlapped.
    let log = fixture.log();
    let a_started = started_at(&fixture.started("track_a/implement_1")[0]);
    let b_started = started_at(&fixture.started("track_b/implement_1")[0]);
    assert!(a_started < finished_at(&log, "track_b/implement_1"));
    assert!(b_started < finished_at(&log, "track_a/implement_1"));
    let session_a = session_id(workflow, "track_a", "slot_a");
    let session_b = session_id(workflow, "track_b", "slot_b");
    assert_ne!(session_a, session_b);

    // C01: B's question reached A, A's answer carried a bundle cut from A's
    // own snapshot, and B's follow-up turn received exactly that bundle.
    let messages = status["messages"].as_array().expect("messages");
    let question = messages
        .iter()
        .find(|message| message["kind"] == "question")
        .expect("question");
    assert_eq!(question["recipient"], "worker:track_a");
    assert_eq!(question["state"], "acknowledged");
    let answer = messages
        .iter()
        .find(|message| message["kind"] == "answer")
        .expect("answer");
    assert_eq!(answer["recipient"], "worker:track_b");
    assert_eq!(answer["reply_to"], question["message_id"]);
    assert_eq!(answer["state"], "acknowledged");
    let bundles = status["receipts"]["bundles"].as_array().expect("bundles");
    assert_eq!(bundles.len(), 1);
    let bundle = &bundles[0];
    assert_eq!(bundle["source_task"], "track_a");
    assert_eq!(bundle["request_message_id"], question["message_id"]);
    let bundle_id = bundle["bundle_id"].as_str().expect("bundle id");
    let follow_up = &fixture.started("track_b/implement_2")[0];
    let follow_up_prompt = follow_up["prompt"].as_str().expect("prompt");
    assert!(follow_up_prompt.contains(&format!("bundle {bundle_id}")));
    assert!(follow_up_prompt.contains(ANSWER));
    assert!(follow_up_prompt.contains(STORE_FIRST_LINE));
    let answer_prompt = fixture.started("track_a/answer_2")[0]["prompt"]
        .as_str()
        .expect("prompt")
        .to_string();
    assert!(answer_prompt.contains(QUESTION));
    // Track A never received B's files or a bundle of its own.
    assert!(!answer_prompt.contains(&format!("bundle {bundle_id}")));

    // V02: the accepted candidate of each task is the one reviewed and the
    // one the trusted suites verified, unchanged.
    for (task_key, required) in [
        ("track_a", vec!["api", "store"]),
        ("track_b", vec!["browser_frontend_only"]),
    ] {
        let task = task_view(&status, task_key);
        assert_eq!(task["progress"], "accepted");
        let accepted = task["accepted_candidate"].as_str().expect("accepted");
        let review = status["receipts"]["reviews"]
            .as_array()
            .expect("reviews")
            .iter()
            .find(|review| review["candidate_digest"] == accepted)
            .expect("review of the accepted candidate");
        assert_eq!(review["verdict"], "pass");
        assert_eq!(review["candidate_unchanged"], true);
        assert_ne!(
            review["reviewer_session_id"],
            json!(session_id(workflow, task_key, "review"))
        );
        let verification = status["receipts"]["verifications"]
            .as_array()
            .expect("verifications")
            .iter()
            .find(|verification| verification["subject_digest"] == accepted)
            .expect("verification of the accepted candidate");
        assert_eq!(verification["subject_unchanged"], true);
        let suites: Vec<&str> = verification["suites"]
            .as_array()
            .expect("suites")
            .iter()
            .map(|suite| {
                assert_eq!(suite["status"], "passed", "suite {suite}");
                assert_eq!(suite["tests_failed"], 0);
                suite["suite_id"].as_str().expect("suite id")
            })
            .collect();
        assert_eq!(suites, required);
    }

    // V03: integration applied both accepted candidates in an owned
    // worktree, the trusted suites passed on the merged tree, and the
    // original checkout is untouched.
    let integration = &status["receipts"]["integration"];
    assert_eq!(integration["conflicts"], json!([]));
    let applied = integration["applied_candidates"]
        .as_array()
        .expect("applied");
    assert_eq!(applied.len(), 2);
    let integrated = status["receipts"]["verifications"]
        .as_array()
        .expect("verifications")
        .iter()
        .find(|verification| verification["task_key"].is_null())
        .expect("integration verification");
    assert_eq!(
        integrated["subject_digest"],
        integration["integration_tree_digest"]
    );
    assert!(
        integrated["suites"]
            .as_array()
            .expect("suites")
            .iter()
            .all(|suite| suite["status"] == "passed")
    );
    assert_eq!(fixture.checkout_status(), "");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), fixture.head);
    assert_eq!(
        std::fs::read_to_string(fixture.project_root.join("src").join("store.mjs"))
            .expect("original store"),
        workflow_support::taskboard_file("base/src/store.mjs")
    );
    // W03: the dirty owned worktrees are retained and reported, never
    // removed with their content.
    let cleanup = status["last_run"]["cleanup"].as_array().expect("cleanup");
    assert!(!cleanup.is_empty());
    for entry in cleanup {
        assert!(
            entry["removed"] == true || entry["reason_code"].is_string(),
            "cleanup entry {entry}"
        );
    }

    // E01: the export is complete and content-free.
    let items = fixture.export_all(workflow).await;
    let encoded = serde_json::to_string(&items).expect("encode");
    for secret in [QUESTION, ANSWER, STORE_FIRST_LINE, "TaskSpec digest:"] {
        assert!(!encoded.contains(secret), "the export leaked {secret}");
    }
    for kind in [
        "status", "record", "contract", "receipt", "attempt", "message", "lease",
    ] {
        assert!(
            items.iter().any(|item| item["kind"] == kind),
            "the export has no {kind} item"
        );
    }

    // The sent prompts are retained (opt-in) and match what the fixture saw.
    let inspect = fixture
        .client
        .workflow_session_inspect(session_b)
        .await
        .expect("inspect");
    assert_eq!(inspect["session"]["attempts"], 2);
    assert_eq!(inspect["attempts"].as_array().expect("attempts").len(), 2);
    assert_eq!(inspect["native_tui_attachable"], false);
    let sent = fixture
        .client
        .workflow_session_prompt(session_b, 1)
        .await
        .expect("prompt");
    assert_eq!(sent["source"], "sent");
    assert_eq!(sent["text"].as_str(), Some(follow_up_prompt));
    assert_eq!(
        fixture.client.workflow_session_attach(session_b).await,
        Err(workflow_support::remote("workflow_native_tui_unavailable"))
    );
    fixture
        .record_evidence("cooperate_flagship", workflow)
        .await;
    fixture.shutdown().await;
}
