#![cfg(feature = "test_helpers")]
//! Lease loss must interrupt an owned turn, not only reject its late receipt.

mod workflow_support;

use serde_json::json;
use std::time::Duration;
use uuid::Uuid;
use workflow_support::{
    FixtureOptions, WorkflowFixture, good_files, policy, review_pass, store_request, store_task,
    workflow_id,
};

const SYNTAX: [&str; 1] = ["syntax"];
const FIXTURE_TURN_DELAY_MS: u64 = 60_000;
const LEASE_LOSS_SETTLE_LIMIT: Duration = Duration::from_secs(10);

async fn check_quarantined_turn(review: bool) {
    let fixture = WorkflowFixture::start(FixtureOptions {
        heartbeat_ms: 1_000,
        ..FixtureOptions::default()
    })
    .await;
    let mut implementation = json!({"write": good_files(&["src/store.mjs"])});
    let mut review_step = review_pass();
    let key = if review {
        review_step["delay_ms"] = json!(FIXTURE_TURN_DELAY_MS);
        "store/review_1"
    } else {
        implementation["delay_ms"] = json!(FIXTURE_TURN_DELAY_MS);
        "store/implement_1"
    };
    fixture.write_script(&json!({"steps": {
        "store/implement_1": implementation,
        "store/review_1": review_step,
    }}));
    let prepared = fixture
        .prepare(
            store_request(
                "lease_loss",
                &fixture.head,
                "cooperate",
                &store_task(&SYNTAX),
                false,
            ),
            policy(&SYNTAX, 1, false),
        )
        .await;
    let workflow = workflow_id(&prepared);
    fixture.start_workflow(&prepared).await;
    fixture.wait_for_turn(key).await;
    let status = fixture.status(workflow).await;
    let slot = if review { "review" } else { "slot_a" };
    let lease = status["leases"]
        .as_array()
        .expect("leases")
        .iter()
        .find(|lease| lease["slot_id"] == slot && lease["state"] == "active")
        .expect("active lease");
    let lease_id = Uuid::parse_str(lease["lease_id"].as_str().expect("lease id")).expect("uuid");
    fixture.quarantine_lease(lease_id).await;

    // Preserve cleanup even on the expected pre-fix timeout.
    let result = tokio::time::timeout(LEASE_LOSS_SETTLE_LIMIT, fixture.wait_idle(workflow)).await;
    let finished = fixture
        .log()
        .into_iter()
        .filter(|record| record["event"] == "finished" && record["key"] == key)
        .count();
    fixture.shutdown().await;

    let status = result.expect("a quarantined lease left its turn running");
    assert_eq!(status["phase"], "failed", "{status:#}");
    assert_eq!(
        status["last_run"]["end"],
        json!({"end": "failed", "code": "workflow_lease_lost"}),
        "{status:#}"
    );
    assert_eq!(
        finished, 0,
        "the delayed worker finished instead of being interrupted"
    );
    assert!(status["receipts"]["selection"].is_null(), "{status:#}");
    assert!(status["receipts"]["integration"].is_null(), "{status:#}");
    assert!(
        status["receipts"]["reviews"]
            .as_array()
            .expect("reviews")
            .is_empty(),
        "{status:#}"
    );
    if !review {
        assert!(
            status["receipts"]["candidates"]
                .as_array()
                .expect("candidates")
                .is_empty(),
            "{status:#}"
        );
    }
    assert!(
        status["leases"]
            .as_array()
            .expect("leases")
            .iter()
            .all(|lease| lease["state"] == "released" || lease["state"] == "revoked"),
        "{status:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quarantined_worker_lease_cancels_the_owned_turn() {
    check_quarantined_turn(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quarantined_reviewer_lease_cancels_the_owned_turn() {
    check_quarantined_turn(true).await;
}
