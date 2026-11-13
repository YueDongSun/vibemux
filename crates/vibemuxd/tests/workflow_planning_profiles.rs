#![cfg(feature = "test_helpers")]
//! Planning profile catalog: the daemon pins each configured profile to the
//! pinned workflow-config digest and reports only content-free fields. This
//! is the configuration and selection surface only; no planning compiler or
//! goal submission runs here.

mod workflow_support;

use serde_json::{Value, json};
use workflow_support::{FixtureOptions, WorkflowFixture};

fn profile() -> Value {
    json!({
        "profile_id": "fixture_small",
        "workflow_key": "taskboard",
        "mode": "cooperate",
        "worker_slots": ["slot_a", "slot_b"],
        "reviewer_slot": "review",
        "integration_suites": ["api", "store"],
        "max_tasks": 2,
        "allow_execution": true,
        "fixture": true,
    })
}

fn is_digest(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_profiles_report_pinned_digests_and_stay_content_free() {
    let fixture = WorkflowFixture::start(FixtureOptions {
        planning_profiles: vec![profile()],
        ..FixtureOptions::default()
    })
    .await;

    let catalog = fixture
        .client
        .workflow_planning_profiles()
        .await
        .expect("planning profiles");
    assert_eq!(catalog["schema_version"], json!(1));
    let profiles = catalog["profiles"].as_array().expect("profiles");
    assert_eq!(profiles.len(), 1);
    let observed = &profiles[0];
    assert_eq!(observed["profile_id"], json!("fixture_small"));
    assert_eq!(observed["workflow_key"], json!("taskboard"));
    assert_eq!(observed["mode"], json!("cooperate"));
    assert_eq!(observed["max_tasks"], json!(2));
    assert_eq!(observed["allow_execution"], json!(true));
    assert!(is_digest(&observed["profile_digest"]));
    assert_eq!(
        observed
            .as_object()
            .expect("object")
            .keys()
            .collect::<Vec<_>>(),
        [
            "allow_execution",
            "max_tasks",
            "mode",
            "profile_digest",
            "profile_id",
            "workflow_key"
        ]
    );

    // Stable across calls, and free of slot wiring, suite argv, and paths.
    let again = fixture
        .client
        .workflow_planning_profiles()
        .await
        .expect("repeat");
    assert_eq!(again, catalog);
    let text = catalog.to_string();
    for absent in [
        "\"worker_slots\"",
        "\"reviewer_slot\"",
        "\"integration_suites\"",
        "\"fixture\"",
        "slot_a",
        "run_verifier",
    ] {
        assert!(!text.contains(absent), "catalog leaked {absent}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unconfigured_planning_catalog_is_empty() {
    let fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let catalog = fixture
        .client
        .workflow_planning_profiles()
        .await
        .expect("planning profiles");
    assert_eq!(catalog, json!({"schema_version": 1, "profiles": []}));
}
