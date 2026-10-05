#![cfg(feature = "test_helpers")]
//! Workflow launch authority follows the admitted tool policy through
//! attempts and the fixture launcher. Synthetic workers: offline evidence.

mod workflow_support;

use std::{collections::BTreeSet, path::PathBuf};

use serde_json::{Value, json};
use vibemux_harness::AgentKind;
use vibemuxd::{
    control::{ControlClient, DaemonControlServer, ExecutionSettings},
    harness_dispatch::{DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings},
    workflow::WorkflowServiceSettings,
};
use workflow_support::{
    FixtureOptions, TRAMPOLINE, WorkflowFixture, cooperative_request, phase, policy, remote,
    review_pass, store_request, store_task, taskboard_file, track_a, track_b, workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const WORKFLOW_TOOLS: [&str; 5] = [
    "read_files",
    "search_files",
    "edit_files",
    "write_files",
    "run_dev_tests",
];
const READ_ONLY_TOOLS: [&str; 2] = ["read_files", "search_files"];
const QUESTION: &str = "Which file contains the task store?";
const ANSWER: &str = "The task store is in src/store.mjs.";

fn policy_without_tool(tool: &str) -> Value {
    let mut request_policy = policy(&SYNTAX, 2, false);
    request_policy["allowed_tools"]
        .as_array_mut()
        .expect("allowed tools")
        .retain(|allowed| allowed.as_str() != Some(tool));
    request_policy
}

fn tool_names(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .expect("effective tools")
        .iter()
        .map(|tool| tool.as_str().expect("tool name").to_string())
        .collect()
}

fn expected_names(allowed: &[&str]) -> BTreeSet<String> {
    allowed.iter().map(|tool| (*tool).to_string()).collect()
}

fn expected_grant(allowed: &[&str]) -> Value {
    json!({
        "read_files": allowed.contains(&"read_files"),
        "search_files": allowed.contains(&"search_files"),
        "edit_files": allowed.contains(&"edit_files"),
        "write_files": allowed.contains(&"write_files"),
        "run_dev_tests": allowed.contains(&"run_dev_tests"),
    })
}

fn assert_attempt_grant(attempt: &Value, launch_record: &Value, expected: &[&str]) {
    let execution = &attempt["execution"];
    assert_eq!(
        tool_names(&execution["effective_tools"]),
        expected_names(expected),
        "attempt execution grant: {attempt:#}"
    );
    assert!(
        execution["harness_profile"].as_str().is_some(),
        "attempt omitted its harness profile: {attempt:#}"
    );
    assert!(
        !execution["effective_route"].is_null(),
        "attempt omitted its resolved route: {attempt:#}"
    );
    assert_eq!(
        launch_record["grant"],
        expected_grant(expected),
        "fixture did not receive the admitted grant: {launch_record:#}"
    );
    let granted_by_launcher = launch_record["grant"]
        .as_object()
        .expect("strict fixture grant object")
        .iter()
        .filter(|(_, enabled)| enabled.as_bool() == Some(true))
        .map(|(tool, _)| tool.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        granted_by_launcher,
        tool_names(&execution["effective_tools"])
    );
}

async fn inspect_attempts(
    fixture: &WorkflowFixture,
    workflow: uuid::Uuid,
    task_key: &str,
    slot_id: &str,
) -> Vec<Value> {
    let status = fixture.status(workflow).await;
    let sessions: Vec<_> = status["sessions"]
        .as_array()
        .expect("persisted sessions")
        .iter()
        .filter(|session| session["task_key"] == task_key && session["slot_id"] == slot_id)
        .collect();
    assert_eq!(
        sessions.len(),
        1,
        "exact owned worker/reviewer session: {status:#}"
    );
    let session_id = uuid::Uuid::parse_str(
        sessions[0]["session_id"]
            .as_str()
            .expect("session identity"),
    )
    .expect("persisted session UUID");
    let inspected = fixture
        .client
        .workflow_session_inspect(session_id)
        .await
        .expect("session inspect");
    inspected["attempts"]
        .as_array()
        .expect("session attempts")
        .clone()
}

fn latest_started_record(fixture: &WorkflowFixture, key: &str) -> Value {
    let mut records = fixture.started(key);
    records
        .pop()
        .unwrap_or_else(|| panic!("fixture start record for {key} is missing"))
}

fn worker_implementation_step(task_key: &str, omitted_tool: &str) -> Value {
    if omitted_tool == "edit_files" {
        let path = match task_key {
            "track_a" => "tests/worker_a/launch_authority.mjs",
            "track_b" => "tests/worker_b/launch_authority.mjs",
            _ => panic!("unexpected worker task key: {task_key}"),
        };
        return json!({"write": {path: "export const launch_authority_probe = true;\n"}});
    }

    let (path, fixture_path) = match task_key {
        "track_a" => ("src/store.mjs", "calibration/good/src/store.mjs"),
        "track_b" => ("public/app.mjs", "calibration/good/public/app.mjs"),
        _ => panic!("unexpected worker task key: {task_key}"),
    };
    let mut content = taskboard_file(fixture_path);
    content.push_str("\n// fixture launch authority candidate\n");
    json!({"write": {path: content}})
}

fn assert_no_admitted_candidate(status: &Value) {
    let candidates = status["receipts"]["candidates"]
        .as_array()
        .expect("candidate receipts");
    assert!(
        !candidates.is_empty(),
        "failed candidate evidence must remain auditable: {status:#}"
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate["admitted"].as_bool() == Some(false)),
        "a denied tool action must not admit a candidate: {status:#}"
    );
    assert!(
        status["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .all(|task| task["accepted_candidate"].is_null()),
        "a denied tool action must not produce a workflow winner: {status:#}"
    );
}

fn expected_worker_tools(omitted: &str) -> Vec<&'static str> {
    WORKFLOW_TOOLS
        .iter()
        .copied()
        .filter(|tool| *tool != omitted)
        .collect()
}

fn expected_read_only_tools(omitted: &str) -> Vec<&'static str> {
    READ_ONLY_TOOLS
        .iter()
        .copied()
        .filter(|tool| *tool != omitted)
        .collect()
}

/// Each operator omission survives preparation, worker, answer, and reviewer
/// launches. Workers make real in-scope edits with the remaining file tool;
/// a follow-up consumes the answer before the independent review and verifier.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_tool_narrowing_reaches_every_fixture_launch() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;

    for (index, omitted_tool) in WORKFLOW_TOOLS.iter().enumerate() {
        let mut track_b_implementation = worker_implementation_step("track_b", omitted_tool);
        track_b_implementation["checkpoint"] = json!({"messages": [
            {"kind": "question", "to": "track_a", "text": QUESTION}
        ]});
        fixture.write_script(&json!({"steps": {
            "track_a/implement_1": worker_implementation_step("track_a", omitted_tool),
            "track_b/implement_1": track_b_implementation,
            "track_a/answer_2": {"checkpoint": {"messages": [
                {"kind": "answer", "to": "track_b", "text": ANSWER,
                    "source_refs": [{"path": "src/store.mjs", "start_line": 1, "end_line": 1}]}
            ]}},
            "track_b/implement_2": {"checkpoint": {"consume_context": true}},
            "track_a/review_1": review_pass(),
            "track_b/review_1": review_pass(),
        }}));
        let request_key = format!("launch_authority_{index}");
        let prepared = fixture
            .prepare(
                cooperative_request(
                    &request_key,
                    &fixture.head,
                    &track_a(&SYNTAX),
                    &track_b(&SYNTAX),
                    &SYNTAX,
                ),
                policy_without_tool(omitted_tool),
            )
            .await;
        let workflow = workflow_id(&prepared);
        fixture.start_workflow(&prepared).await;
        let status = fixture
            .wait_phase(workflow, &["accepted", "failed", "blocked"])
            .await;
        assert_eq!(phase(&status), "accepted", "status: {status:#}");

        let worker_tools = expected_worker_tools(omitted_tool);
        let read_only_tools = expected_read_only_tools(omitted_tool);
        let track_a_attempts = inspect_attempts(&fixture, workflow, "track_a", "slot_a").await;
        let track_b_attempts = inspect_attempts(&fixture, workflow, "track_b", "slot_b").await;
        let track_a_reviews = inspect_attempts(&fixture, workflow, "track_a", "review").await;
        let track_b_reviews = inspect_attempts(&fixture, workflow, "track_b", "review").await;
        assert_eq!(track_a_attempts.len(), 2, "implement and answer attempts");
        assert_eq!(
            track_b_attempts.len(),
            2,
            "implement and follow-up attempts"
        );
        assert_eq!(track_a_reviews.len(), 1, "track A review attempt");
        assert_eq!(track_b_reviews.len(), 1, "track B review attempt");

        for (attempt, key, expected) in [
            (
                &track_a_attempts[0],
                "track_a/implement_1",
                worker_tools.as_slice(),
            ),
            (
                &track_b_attempts[0],
                "track_b/implement_1",
                worker_tools.as_slice(),
            ),
            (
                &track_b_attempts[1],
                "track_b/implement_2",
                worker_tools.as_slice(),
            ),
            (
                &track_a_attempts[1],
                "track_a/answer_2",
                read_only_tools.as_slice(),
            ),
            (
                &track_a_reviews[0],
                "track_a/review_1",
                read_only_tools.as_slice(),
            ),
            (
                &track_b_reviews[0],
                "track_b/review_1",
                read_only_tools.as_slice(),
            ),
        ] {
            assert_attempt_grant(attempt, &latest_started_record(&fixture, key), expected);
        }
        assert_eq!(fixture.checkout_status(), "");
        assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), fixture.head);
    }
    fixture.shutdown().await;
}

/// A grant is checked before the fixture performs reads, searches, writes, or
/// development tests. A denied action cannot create a candidate or reach review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_omitted_tool_denies_its_corresponding_worker_effect() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let denied_edit = taskboard_file("calibration/mutants/blank_title_accepted/src/store.mjs");
    let cases = [
        (
            "deny_read",
            "read_files",
            json!({"read": ["src/store.mjs"]}),
        ),
        ("deny_search", "search_files", json!({"search": ["src"]})),
        (
            "deny_edit_existing",
            "edit_files",
            json!({"write": {"src/store.mjs": denied_edit}}),
        ),
        (
            "deny_new_file",
            "write_files",
            json!({"write": {"tests/worker_store/helper.mjs": "// denied creation\n"}}),
        ),
        (
            "deny_dev_test",
            "run_dev_tests",
            json!({"run_dev_test": true}),
        ),
    ];

    for (request_key, omitted_tool, effect) in cases {
        // This test isolates one denied effect; repair behavior has separate coverage.
        let mut request_policy = policy_without_tool(omitted_tool);
        request_policy["budget_caps"]["max_repairs"] = json!(0);
        fixture.write_script(&json!({"steps": {"store/implement_1": effect}}));
        let prepared = fixture
            .prepare(
                store_request(
                    request_key,
                    &fixture.head,
                    "cooperate",
                    &store_task(&SYNTAX),
                    false,
                ),
                request_policy,
            )
            .await;
        let workflow = workflow_id(&prepared);
        fixture.start_workflow(&prepared).await;
        let status = fixture.wait_phase(workflow, &["failed", "accepted"]).await;
        assert_eq!(phase(&status), "failed", "status: {status:#}");
        assert_no_admitted_candidate(&status);
        assert!(fixture.started("store/review_1").is_empty());
        let attempts = inspect_attempts(&fixture, workflow, "store", "slot_a").await;
        assert_eq!(attempts.len(), 1, "denied effect attempt");
        let effective_tools = tool_names(&attempts[0]["execution"]["effective_tools"]);
        assert!(!effective_tools.contains(omitted_tool));
        let record = latest_started_record(&fixture, "store/implement_1");
        assert_eq!(record["grant"][omitted_tool], false);
        assert_eq!(fixture.checkout_status(), "");
        assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), fixture.head);
    }
    fixture.shutdown().await;
}

/// An empty policy grant stays empty at the launcher. Its attempted edit is
/// denied and cannot produce an admitted candidate or review attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_tool_policy_stays_empty_and_has_no_winner() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut store_content = taskboard_file("calibration/good/src/store.mjs");
    store_content.push_str("\n// empty tool policy denial probe\n");
    fixture.write_script(&json!({"steps": {
        "store/implement_1": {"write": {"src/store.mjs": store_content}}
    }}));
    let mut request_policy = policy(&SYNTAX, 2, false);
    request_policy["allowed_tools"] = json!([]);
    request_policy["budget_caps"]["max_repairs"] = json!(0);
    let prepared = fixture
        .prepare(
            store_request(
                "empty_tool_policy",
                &fixture.head,
                "cooperate",
                &store_task(&SYNTAX),
                false,
            ),
            request_policy,
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture
        .wait_phase(workflow, &["accepted", "failed", "blocked"])
        .await;
    assert_eq!(phase(&status), "failed", "status: {status:#}");
    assert_no_admitted_candidate(&status);
    assert!(fixture.started("store/review_1").is_empty());
    let attempts = inspect_attempts(&fixture, workflow, "store", "slot_a").await;
    assert_eq!(attempts.len(), 1, "empty-grant implementation attempt");
    assert_attempt_grant(
        &attempts[0],
        &latest_started_record(&fixture, "store/implement_1"),
        &[],
    );
    assert_eq!(fixture.checkout_status(), "");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), fixture.head);
    fixture.shutdown().await;
}

/// A route's provider registry labels and `allow_execution` flag are only
/// declarations. Without the opaque fixture guard, Codex stays uncertified
/// for writable workflow admission and no worker process is launched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registry_label_without_the_opaque_guard_cannot_certify_codex() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let original_head = fixture.head.clone();
    fixture.stop_server_for_test().await;

    let database_directory = fixture.scratch("unguarded_database");
    let runtime_directory = fixture.scratch("unguarded_runtime");
    let settings = ExecutionSettings {
        dispatch: DispatchServiceSettings {
            project_root: fixture.project_root.clone(),
            config_path: fixture.state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            trampoline: Some(PathBuf::from(TRAMPOLINE)),
        },
        workflow: Some(WorkflowServiceSettings::for_project(
            &fixture.project_root,
            &fixture.state_dir,
        )),
    };
    let server = DaemonControlServer::start_with_execution(
        &database_directory.join("vibemux_rust.sqlite3"),
        &runtime_directory,
        settings,
    )
    .await
    .expect("unguarded fixture control server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("control client");

    let catalog = client.harness_dispatch_catalog().await.expect("catalog");
    assert!(
        catalog
            .iter()
            .any(|entry| { entry.harness == AgentKind::Codex && entry.allow_execution })
    );
    let slots = client.workflow_slots().await.expect("workflow slots");
    let codex_route = slots["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .find(|route| route["slot_id"] == "slot_a")
        .expect("Codex worker route");
    assert_eq!(codex_route["executes"], false);
    let codex_slot = slots["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|slot| slot["slot_id"] == "slot_a")
        .expect("Codex worker slot");
    assert!(
        codex_slot["capabilities"]
            .as_object()
            .expect("capabilities")
            .values()
            .all(|evidence| evidence["level"] == "declared"),
        "a registry label must not create execution certification: {codex_slot:#}"
    );

    assert_eq!(
        client
            .workflow_prepare(
                store_request(
                    "registry_label_without_guard",
                    &fixture.head,
                    "cooperate",
                    &store_task(&SYNTAX),
                    false,
                ),
                policy(&SYNTAX, 2, false),
            )
            .await,
        Err(remote("workflow_slot_unavailable"))
    );
    assert!(fixture.started_keys().is_empty());
    assert_eq!(fixture.checkout_status(), "");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), original_head);

    server.shutdown().await.expect("unguarded server shutdown");
    fixture.shutdown().await;
}
