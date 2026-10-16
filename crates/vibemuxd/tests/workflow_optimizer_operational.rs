#![cfg(feature = "test_helpers")]

mod workflow_support;

use std::path::Path;

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use vibemux_store::PolicyVersionRecord;
use vibemux_workflow::optimizer::{OptimizablePolicy, PolicyState, PolicyVersion};
use workflow_support::{
    FixtureOptions, WorkflowFixture, policy, remote, store_request, store_task,
};

const STORE_SUITE: [&str; 1] = ["store"];
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const SUITE_ID: &str = "legacy_suite";
const CANDIDATE_ID: &str = "legacy_candidate";

fn write_operator_file(fixture: &WorkflowFixture, directory: &str, id: &str, value: &Value) {
    let directory = fixture.state_dir.join(directory);
    std::fs::create_dir_all(&directory).expect("operator directory");
    std::fs::write(
        directory.join(format!("{id}.json")),
        serde_json::to_vec_pretty(value).expect("encode operator file"),
    )
    .expect("write operator file");
}

/// Seeds a record shaped like a previously persisted policy version while
/// the daemon writer is stopped. The row references an existing event so
/// the database foreign key remains valid.
fn seed_legacy_policy(database_path: &Path, policy: OptimizablePolicy) -> PolicyVersionRecord {
    let policy_digest = policy.digest().expect("policy digest");
    let record = PolicyVersionRecord {
        version: PolicyVersion {
            version: 1,
            parent: None,
            policy_digest,
            state: PolicyState::Active,
        },
        policy,
    };
    let record_json = serde_json::to_string(&record).expect("serialize policy history record");
    let digest_text = policy_digest.to_hex();
    let mut connection = Connection::open(database_path).expect("open stopped test database");
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .expect("enable test database foreign keys");
    let transaction = connection.transaction().expect("begin history seed");
    let updated_sequence: i64 = transaction
        .query_row("SELECT MAX(sequence) FROM events", [], |row| row.get(0))
        .expect("existing event sequence");
    transaction
        .execute(
            "INSERT INTO prompt_policy_versions(version, parent, policy_digest, state, record_json, updated_sequence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.version.version,
                record.version.parent,
                digest_text,
                "active",
                record_json,
                updated_sequence,
            ],
        )
        .expect("seed legacy policy version");
    transaction.commit().expect("commit history seed");
    record
}

fn evaluation_inputs(fixture: &WorkflowFixture) {
    let case = json!({
        "request": store_request(
            "legacy_case",
            &fixture.head,
            "cooperate",
            &store_task(&STORE_SUITE),
            false,
        ),
        "policy": policy(&STORE_SUITE, 2, false),
    });
    write_operator_file(
        fixture,
        "prompt_suites",
        SUITE_ID,
        &json!({
            "schema_version": 1,
            "suite_id": SUITE_ID,
            "train": {"dataset_id": "legacy_train", "split": "train", "case_ids": ["train_case"]},
            "dev": {"dataset_id": "legacy_dev", "split": "dev", "case_ids": ["dev_case"]},
            "holdout": {"dataset_id": "legacy_holdout", "split": "holdout", "case_ids": ["holdout_case"]},
            "cases": {
                "train_case": case,
                "dev_case": case,
                "holdout_case": case,
            },
            "limits": {"max_candidates": 1, "max_model_requests": 64},
            "case_deadline_ms": 120_000,
        }),
    );
    write_operator_file(
        fixture,
        "prompt_candidates",
        CANDIDATE_ID,
        &json!({
            "schema_version": 1,
            "candidate_id": CANDIDATE_ID,
            "diffs": [{
                "changes": [{
                    "field": "template_overrides.worker_v1.worker_method",
                    "value": "Use a concise implementation and verify its behavior."
                }],
                "expected_benefit": "Clearer worker guidance.",
                "possible_regressions": []
            }]
        }),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_active_policy_stays_readable_but_blocks_future_admission_and_evaluation() {
    let mut fixture = WorkflowFixture::start(FixtureOptions::default()).await;
    let mut template_only_policy = OptimizablePolicy::baseline();
    template_only_policy
        .template_overrides
        .entry("worker_v1".to_string())
        .or_default()
        .insert(
            "worker_method".to_string(),
            "A supported template-only update.".to_string(),
        );
    template_only_policy
        .validate_operational()
        .expect("template-only policy remains operational");

    let baseline_prepared = fixture
        .client
        .workflow_prepare(
            store_request(
                "baseline_before_legacy",
                &fixture.head,
                "cooperate",
                &store_task(&STORE_SUITE),
                false,
            ),
            policy(&STORE_SUITE, 2, false),
        )
        .await
        .expect("baseline policy remains admissible");
    assert!(baseline_prepared.get("start_contract").is_some());

    let mut legacy_policy = OptimizablePolicy::baseline();
    legacy_policy.summary_budget_bytes = 4096;
    legacy_policy
        .template_overrides
        .entry("worker_v1".to_string())
        .or_default()
        .insert(
            "worker_method".to_string(),
            "Legacy template wording remains stored.".to_string(),
        );
    let legacy_record =
        seed_legacy_policy(&fixture.state_dir.join(DATABASE_FILE_NAME), legacy_policy);
    evaluation_inputs(&fixture);
    fixture.restart().await;

    let versions = fixture
        .client
        .workflow_policy_versions()
        .await
        .expect("legacy history remains readable");
    assert_eq!(
        versions,
        json!([{
            "version": 1,
            "parent": null,
            "policy_digest": legacy_record.version.policy_digest,
            "state": "active",
        }])
    );

    assert_eq!(
        fixture
            .client
            .workflow_prepare(
                store_request(
                    "rejected_after_legacy",
                    &fixture.head,
                    "cooperate",
                    &store_task(&STORE_SUITE),
                    false,
                ),
                policy(&STORE_SUITE, 2, false),
            )
            .await,
        Err(remote("optimizer_parameter_unsupported"))
    );
    assert_eq!(
        fixture
            .client
            .workflow_policy_evaluate(CANDIDATE_ID, SUITE_ID)
            .await,
        Err(remote("optimizer_parameter_unsupported"))
    );
    assert!(fixture.started_keys().is_empty());

    fixture.shutdown().await;
}
