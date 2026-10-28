#![cfg(all(feature = "test_helpers", windows))]
//! Explicit installed Claude against synthetic loopback inference. No live key.
mod installed_controller_support;
mod workflow_support;
use installed_controller_support::{Scenario, SyntheticApi};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf};
use vibemux_harness::AgentKind;
use vibemuxd::{control::HarnessDispatchOutputQuery, harness_dispatch::OutputReassembler};
use workflow_support::{
    FixtureOptions, InstalledControllerSetup, SlotSetup, WorkflowFixture, phase, policy,
    store_request, store_task, taskboard_file, workflow_id,
};

const MARKER: &str = "UNRELATED_CUSTOMIZATION_CANARY_6437";
const GOOD_STORE: &str = "calibration/good/src/store.mjs";
fn call(name: &str, input: Value) -> Value {
    json!({"name":name,"input":input})
}
fn file_policy() -> Value {
    let mut value = policy(&["store"], 1, false);
    value["allowed_tools"]
        .as_array_mut()
        .unwrap()
        .retain(|tool| tool != "run_dev_tests");
    value["budget_caps"]["max_repairs"] = json!(0);
    value
}
async fn fixture(api: &SyntheticApi) -> WorkflowFixture {
    fixture_with_role(api, false).await
}
async fn fixture_with_role(api: &SyntheticApi, native_review: bool) -> WorkflowFixture {
    let executable = PathBuf::from(
        std::env::var_os("VIBEMUX_QUALIFY_CLAUDE").expect("explicit installed Claude path"),
    );
    let git_bash = PathBuf::from(
        std::env::var_os("VIBEMUX_QUALIFY_GIT_BASH").expect("explicit Git Bash path"),
    );
    WorkflowFixture::start(FixtureOptions {
        installed_controller: Some(InstalledControllerSetup {
            executable,
            git_bash,
            endpoint: api.address,
        }),
        slots: vec![
            SlotSetup {
                slot_id: "slot_a",
                harness: if native_review {
                    AgentKind::Codex
                } else {
                    AgentKind::Claude
                },
                route_role: "worker_a",
            },
            SlotSetup {
                slot_id: "review",
                harness: if native_review {
                    AgentKind::Claude
                } else {
                    AgentKind::Codex
                },
                route_role: "reviewer",
            },
        ],
        ..FixtureOptions::default()
    })
    .await
}
async fn attempt_records(fixture: &WorkflowFixture, id: uuid::Uuid) -> Vec<(Value, Vec<Value>)> {
    let status = fixture.status(id).await;
    let mut result = vec![];
    for session in status["sessions"].as_array().unwrap() {
        let id = uuid::Uuid::parse_str(session["session_id"].as_str().unwrap()).unwrap();
        let view = fixture.client.workflow_session_inspect(id).await.unwrap();
        for attempt in view["attempts"].as_array().unwrap() {
            let request_id =
                uuid::Uuid::parse_str(attempt["request_id"].as_str().unwrap()).unwrap();
            let mut output = OutputReassembler::starting_after(0);
            let mut records = vec![];
            for _ in 0..64 {
                let page = fixture
                    .client
                    .harness_dispatch_output(HarnessDispatchOutputQuery {
                        request_id,
                        cursor: output.cursor(),
                    })
                    .await
                    .unwrap();
                let complete = page.complete;
                records.extend(
                    output
                        .accept(page)
                        .unwrap()
                        .into_iter()
                        .map(|record| serde_json::from_str::<Value>(&record.raw_json).unwrap()),
                );
                if complete {
                    break;
                }
            }
            let dispatch = fixture
                .client
                .harness_dispatch_status(request_id)
                .await
                .unwrap();
            eprintln!(
                "qualification dispatch phase={:?} error={:?} process={:?}",
                dispatch.phase, dispatch.error_code, dispatch.process
            );
            result.push((attempt.clone(), records));
        }
    }
    result
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit installed Claude and Git Bash paths; synthetic loopback API only"]
async fn installed_claude_edits_through_the_broker_and_reaches_verified_workflow() {
    let api=SyntheticApi::start(Scenario {
        worker_calls:vec![
            call("mcp__worker__read_file",json!({"path":"src/store.mjs"})),
            call("mcp__worker__edit_file",json!({"path":"src/store.mjs","expected_revision":0,"content":taskboard_file(GOOD_STORE)})),
            call("mcp__worker__create_file",json!({"path":"tests/worker_store/helper.mjs","content":"export const helper = true;\n"})),
        ],
        reviewer_calls:vec![call("mcp__worker__read_file",json!({"path":"tests/worker_store/helper.mjs"}))],
        changed_files:vec!["src/store.mjs".into(),"tests/worker_store/helper.mjs".into()],
        forbidden_marker:MARKER.into(),hold_after_call:false,
    }).await;
    let fixture = fixture(&api).await;
    fixture.write_script(&json!({"steps":{"store/review":workflow_support::review_pass()}}));
    let prepared = fixture
        .prepare(
            store_request(
                "installed_broker_positive",
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            file_policy(),
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(id).await;
    let attempts = attempt_records(&fixture, id).await;
    let observed = api.observations();
    let failures = api.failures();
    assert!(
        !fixture
            .project_root
            .join("unexpected_hook_marker.txt")
            .exists()
    );
    let checkout = fixture.checkout_status();
    fixture.shutdown().await;
    api.shutdown().await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(attempts.len(), 2);
    assert!(!observed.is_empty());
    for observation in &observed {
        assert!(observation.synthetic_auth);
        assert_eq!(observation.model, "fixture-model");
        assert!(!observation.forbidden_context);
        assert!(observation.result_errors.iter().all(|error| !error));
        assert!(observation.tools.iter().all(|tool| matches!(
            tool.as_str(),
            "mcp__worker__read_file"
                | "mcp__worker__search_paths"
                | "mcp__worker__edit_file"
                | "mcp__worker__create_file"
        )));
    }
    for (attempt, records) in attempts {
        if attempt["execution"]["harness_profile"] == "codex_fixture_broker_v1" {
            continue;
        }
        assert_eq!(
            attempt["execution"]["harness_profile"],
            "claude_broker_qualification_v1"
        );
        let init = records
            .iter()
            .find(|value| value["type"] == "system" && value["subtype"] == "init")
            .expect("actual init inventory");
        let actual: BTreeSet<&str> = init["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool.as_str().unwrap())
            .collect();
        let expected: BTreeSet<&str> = if attempt["purpose"] == "review" {
            ["mcp__worker__read_file", "mcp__worker__search_paths"]
                .into_iter()
                .collect()
        } else {
            [
                "mcp__worker__read_file",
                "mcp__worker__search_paths",
                "mcp__worker__edit_file",
                "mcp__worker__create_file",
            ]
            .into_iter()
            .collect()
        };
        assert_eq!(actual, expected, "native or unexpected tool exposed");
    }
    assert_eq!(checkout, "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit installed Claude and Git Bash paths; synthetic loopback API only"]
async fn installed_claude_refuses_missing_existing_ungranted_native_and_unknown_effects() {
    let cases = [
        (
            "edit_missing",
            "write_files",
            call(
                "mcp__worker__edit_file",
                json!({"path":"tests/worker_store/missing.mjs","expected_revision":0,"content":"forbidden"}),
            ),
            Some("worker_tools_missing"),
        ),
        (
            "create_existing",
            "edit_files",
            call(
                "mcp__worker__create_file",
                json!({"path":"src/store.mjs","content":"forbidden"}),
            ),
            Some("worker_tools_already_exists"),
        ),
        (
            "ungranted_edit",
            "edit_files",
            call(
                "mcp__worker__edit_file",
                json!({"path":"src/store.mjs","expected_revision":0,"content":"forbidden"}),
            ),
            None,
        ),
        (
            "native_edit",
            "",
            call(
                "Edit",
                json!({"file_path":"src/native_escape.txt","old_string":"","new_string":"forbidden"}),
            ),
            None,
        ),
        (
            "unknown_mcp",
            "",
            call("mcp__worker__shell_exec", json!({"command":"forbidden"})),
            None,
        ),
        (
            "extra_argument",
            "",
            call(
                "mcp__worker__edit_file",
                json!({"path":"src/store.mjs","expected_revision":0,"content":"forbidden","command":"forbidden"}),
            ),
            Some("invalid_arguments"),
        ),
    ];
    for (key, omitted, operation, refusal) in cases {
        let api = SyntheticApi::start(Scenario {
            worker_calls: vec![operation],
            reviewer_calls: vec![],
            changed_files: vec![],
            forbidden_marker: MARKER.into(),
            hold_after_call: false,
        })
        .await;
        let fixture = fixture(&api).await;
        let mut policy = file_policy();
        policy["allowed_tools"]
            .as_array_mut()
            .unwrap()
            .retain(|tool| tool != omitted);
        let prepared = fixture
            .prepare(
                store_request(
                    &format!("installed_{key}"),
                    &fixture.head,
                    "cooperate",
                    &store_task(&["store"]),
                    false,
                ),
                policy,
            )
            .await;
        let id = workflow_id(&prepared);
        fixture.start_workflow(&prepared).await;
        let status = fixture.wait_idle(id).await;
        let attempts = attempt_records(&fixture, id).await;
        let observations = api.observations();
        let failures = api.failures();
        let marker_absent = !fixture
            .project_root
            .join("unexpected_hook_marker.txt")
            .exists();
        let checkout = fixture.checkout_status();
        fixture.shutdown().await;
        api.shutdown().await;
        assert_eq!(phase(&status), "failed", "{key}: {status:#}");
        assert_eq!(
            attempts.len(),
            1,
            "no independent review for a refused effect"
        );
        assert!(status["tasks"][0]["accepted_candidate"].is_null());
        assert!(
            status["receipts"]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|value| value["admitted"] == false)
        );
        assert!(failures.is_empty(), "{key}: {failures:?}");
        assert!(
            observations
                .iter()
                .any(|value| value.result_errors.contains(&true)),
            "{key}: no actual refusal"
        );
        if let Some(refusal) = refusal {
            assert!(
                observations
                    .iter()
                    .any(|value| value.denial_codes.iter().any(|code| code == refusal)),
                "{key}: broker refusal code missing"
            );
        }
        assert!(
            observations
                .iter()
                .all(|value| value.synthetic_auth && !value.forbidden_context)
        );
        let init = attempts[0]
            .1
            .iter()
            .find(|value| value["type"] == "system" && value["subtype"] == "init")
            .unwrap();
        let actual: BTreeSet<_> = init["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let mut expected: BTreeSet<_> = [
            "mcp__worker__read_file",
            "mcp__worker__search_paths",
            "mcp__worker__edit_file",
            "mcp__worker__create_file",
        ]
        .into_iter()
        .collect();
        if omitted == "write_files" {
            expected.remove("mcp__worker__create_file");
        }
        if omitted == "edit_files" {
            expected.remove("mcp__worker__edit_file");
        }
        assert_eq!(actual, expected, "{key}: tool inventory");
        assert!(marker_absent);
        assert_eq!(checkout, "");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit installed Claude and Git Bash paths; synthetic loopback API only"]
async fn installed_claude_cancellation_after_a_broker_effect_never_promotes_it() {
    let api = SyntheticApi::start(Scenario {
        worker_calls: vec![call(
            "mcp__worker__edit_file",
            json!({
                "path":"src/store.mjs","expected_revision":0,"content":taskboard_file(GOOD_STORE)
            }),
        )],
        reviewer_calls: vec![],
        changed_files: vec!["src/store.mjs".into()],
        forbidden_marker: MARKER.into(),
        hold_after_call: true,
    })
    .await;
    let fixture = fixture(&api).await;
    let prepared = fixture
        .prepare(
            store_request(
                "installed_cancel",
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            file_policy(),
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let observed = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            if api
                .observations()
                .iter()
                .any(|value| value.result_errors == vec![false])
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    fixture.client.workflow_cancel(id).await.unwrap();
    let status = fixture.wait_idle(id).await;
    let attempts = attempt_records(&fixture, id).await;
    let checkout = fixture.checkout_status();
    fixture.shutdown().await;
    api.shutdown().await;
    observed.expect("real completed broker effect before cancellation");
    assert_eq!(phase(&status), "cancelled", "{status:#}");
    assert_eq!(attempts.len(), 1);
    assert!(status["tasks"][0]["accepted_candidate"].is_null());
    assert!(
        status["receipts"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value["admitted"] == false)
    );
    assert_eq!(checkout, "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit installed Claude and Git Bash paths; synthetic loopback API only"]
async fn installed_claude_review_has_only_the_read_only_broker_surface() {
    let api = SyntheticApi::start(Scenario {
        worker_calls: vec![],
        reviewer_calls: vec![call(
            "mcp__worker__read_file",
            json!({"path":"src/store.mjs"}),
        )],
        changed_files: vec!["src/store.mjs".into()],
        forbidden_marker: MARKER.into(),
        hold_after_call: false,
    })
    .await;
    let fixture = fixture_with_role(&api, true).await;
    fixture.write_script(&json!({"steps":{"store/implement_1":{"write":{
        "src/store.mjs":taskboard_file(GOOD_STORE)
    }}}}));
    let prepared = fixture
        .prepare(
            store_request(
                "installed_read_only_review",
                &fixture.head,
                "cooperate",
                &store_task(&["store"]),
                false,
            ),
            file_policy(),
        )
        .await;
    let id = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    let status = fixture.wait_idle(id).await;
    let attempts = attempt_records(&fixture, id).await;
    let observations = api.observations();
    let failures = api.failures();
    let marker_absent = !fixture
        .project_root
        .join("unexpected_hook_marker.txt")
        .exists();
    let checkout = fixture.checkout_status();
    fixture.shutdown().await;
    api.shutdown().await;
    assert_eq!(phase(&status), "accepted", "{status:#}");
    assert_eq!(attempts.len(), 2);
    let (attempt, records) = attempts
        .iter()
        .find(|(attempt, _)| {
            attempt["execution"]["harness_profile"] == "claude_broker_qualification_v1"
        })
        .unwrap();
    assert_eq!(attempt["purpose"], "review");
    let init = records
        .iter()
        .find(|value| value["type"] == "system" && value["subtype"] == "init")
        .unwrap();
    let actual: BTreeSet<_> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(
        actual,
        ["mcp__worker__read_file", "mcp__worker__search_paths"]
            .into_iter()
            .collect()
    );
    assert!(
        observations
            .iter()
            .any(|value| value.result_errors == vec![false])
    );
    assert!(observations.iter().all(|value| value.synthetic_auth
        && !value.forbidden_context
        && value.tools.iter().all(|tool| matches!(
            tool.as_str(),
            "mcp__worker__read_file" | "mcp__worker__search_paths"
        ))));
    assert!(failures.is_empty(), "{failures:?}");
    assert!(marker_absent);
    assert_eq!(checkout, "");
}

#[tokio::test]
async fn synthetic_api_observes_token_count_context_and_exact_authentication() {
    let api = SyntheticApi::start(Scenario {
        worker_calls: vec![],
        reviewer_calls: vec![],
        changed_files: vec![],
        forbidden_marker: MARKER.into(),
        hold_after_call: false,
    })
    .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for key in ["synthetic-fixture-key", "synthetic-fixture-key-with-suffix"] {
        let response = client
            .post(format!("http://{}/v1/messages/count_tokens", api.address))
            .header("x-api-key", key)
            .json(&json!({"model":"fixture-model","messages":[],"system":MARKER,"tools":[]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let observed = api.observations();
    let failures = api.failures();
    api.shutdown().await;
    assert_eq!(observed.len(), 2);
    assert!(observed.iter().all(|value| value.forbidden_context));
    assert!(observed[0].synthetic_auth);
    assert!(!observed[1].synthetic_auth);
    assert!(failures.is_empty());
}
