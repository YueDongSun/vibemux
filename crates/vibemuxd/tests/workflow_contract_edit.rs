#![cfg(feature = "test_helpers")]
//! Real Control/writer contract generations without overwriting admitted history.
mod workflow_support;

use serde_json::{Value, json};
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, WorkflowFixture, cooperative_request, policy, track_a, track_b, workflow_id,
};

const SUITES: [&str; 1] = ["syntax"];

fn edit_request(parent: &Value, parent_version: u64, original: &Value, suffix: &str) -> Value {
    let mut request = original.clone();
    request["request_key"] = json!(format!("successor_{suffix}"));
    let objective = request["task_specs"][0]["objective"]
        .as_str()
        .expect("objective")
        .to_owned();
    request["task_specs"][0]["objective"] =
        json!(format!("{objective} Preserve deterministic diagnostics."));
    request["task_specs"][0]["contract_version"] = json!(2);
    let mut expected = serde_json::Map::new();
    for contract in parent["contracts"].as_array().expect("contracts") {
        expected.insert(
            contract["task_key"].as_str().expect("key").to_owned(),
            contract["contract_id"].clone(),
        );
    }
    json!({"schema_version":1,"edit_id":Uuid::new_v4(),"parent_workflow_id":workflow_id(parent),
        "expected_parent_version":parent_version,"expected_contracts":expected,"disposition":"retire_quiescent_parent","request":request})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_edit_is_a_replayable_successor_and_preserves_original_contract() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let original = cooperative_request(
        "semantic_edit_parent",
        &fixture.head,
        &track_a(&SUITES),
        &track_b(&SUITES),
        &SUITES,
    );
    let prepared = fixture
        .client
        .workflow_prepare(original.clone(), policy(&SUITES, 2, false))
        .await
        .expect("parent");
    let parent_id = workflow_id(&prepared);
    let before = fixture.status(parent_id).await;
    let edit = edit_request(
        &prepared,
        before["version"].as_u64().expect("parent version"),
        &original,
        "wording",
    );
    let result = fixture
        .client
        .workflow_contract_edit(edit.clone())
        .await
        .expect("operator edit");
    let successor = &result["successor"];
    let successor_id = workflow_id(successor);
    assert_ne!(successor_id, parent_id);
    assert_eq!(fixture.status(parent_id).await["phase"], "cancelled");
    assert_eq!(fixture.status(successor_id).await["phase"], "prepared");
    assert_eq!(successor["contracts"][0]["contract_version"], 2);
    let replay = fixture
        .client
        .workflow_contract_edit(edit.clone())
        .await
        .expect("exact replay");
    assert_eq!(workflow_id(&replay["successor"]), successor_id);
    assert_eq!(replay["successor"]["duplicate"], true);
    let mut conflict = edit;
    conflict["request"]["task_specs"][0]["objective"] = json!("Different operator edit");
    assert!(
        fixture
            .client
            .workflow_contract_edit(conflict)
            .await
            .is_err()
    );
    let old_status = fixture.status(parent_id).await;
    assert_eq!(old_status["tasks"][0]["contract_version"], 1);
    assert!(
        fixture.started_keys().is_empty(),
        "editing must not submit a turn"
    );
    assert_eq!(fixture.checkout_status(), "");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_edit_and_stale_parent_leave_original_prepared() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let original = cooperative_request(
        "semantic_edit_rejections",
        &fixture.head,
        &track_a(&SUITES),
        &track_b(&SUITES),
        &SUITES,
    );
    let prepared = fixture
        .client
        .workflow_prepare(original.clone(), policy(&SUITES, 2, false))
        .await
        .expect("parent");
    let parent_id = workflow_id(&prepared);
    let before = fixture.status(parent_id).await;
    let valid = edit_request(
        &prepared,
        before["version"].as_u64().expect("version"),
        &original,
        "refused",
    );
    let mut stale = valid.clone();
    stale["expected_parent_version"] = json!(999);
    assert!(fixture.client.workflow_contract_edit(stale).await.is_err());
    let mut unsafe_edit = valid;
    unsafe_edit["request"]["task_specs"][0]["acceptance_checks"] = json!([]);
    assert!(
        fixture
            .client
            .workflow_contract_edit(unsafe_edit)
            .await
            .is_err()
    );
    assert_eq!(fixture.status(parent_id).await["phase"], "prepared");
    assert!(fixture.started_keys().is_empty());
    fixture.shutdown().await;
}
