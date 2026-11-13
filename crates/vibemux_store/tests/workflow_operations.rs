//! Goal admission is durable, fenced and observable after losing an owner.

use tempfile::NamedTempFile;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{SqliteStore, StoreError, WorkflowOperationState as State};
use vibemux_workflow::Sha256Digest;

#[test]
fn duplicate_event_never_replaces_admission_or_replays_unknown() {
    let file = NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open(file.path()).unwrap();
    let event = Uuid::new_v4();
    let fingerprint = Sha256Digest::of(b"strict request and operator policy");
    let now = OffsetDateTime::now_utc();
    let first = store
        .admit_workflow_operation(event, fingerprint, now)
        .unwrap();
    assert!(!first.duplicate);
    assert_eq!(first.operation.state, State::Accepted);
    let duplicate = store
        .admit_workflow_operation(event, fingerprint, now)
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.operation, first.operation);
    assert!(matches!(
        store.admit_workflow_operation(event, Sha256Digest::of(b"changed policy"), now),
        Err(StoreError::Workflow("workflow_operation_conflict"))
    ));
    store
        .finish_workflow_operation(event, State::Preparing, None, None, None, now)
        .unwrap();
    drop(store);
    let mut reopened = SqliteStore::open(file.path()).unwrap();
    assert_eq!(reopened.recover_workflow_operations(now).unwrap(), 1);
    let observed = reopened.workflow_operation(event).unwrap().unwrap();
    assert_eq!(observed.state, State::Unknown);
    assert_eq!(
        observed.code.as_deref(),
        Some("workflow_operation_owner_lost")
    );
    let duplicate = reopened
        .admit_workflow_operation(event, fingerprint, now)
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.operation, observed);
    assert!(
        reopened
            .finish_workflow_operation(event, State::Preparing, None, None, None, now)
            .is_err()
    );
    assert_eq!(reopened.recover_workflow_operations(now).unwrap(), 0);
}

#[test]
fn invalid_state_binding_and_contentful_codes_are_refused_without_mutation() {
    let file = NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open(file.path()).unwrap();
    let now = OffsetDateTime::now_utc();
    assert!(
        store
            .admit_workflow_operation(Uuid::nil(), Sha256Digest::of(b"x"), now)
            .is_err()
    );
    let event = Uuid::new_v4();
    store
        .admit_workflow_operation(event, Sha256Digest::of(b"x"), now)
        .unwrap();
    assert!(
        store
            .finish_workflow_operation(event, State::Running, None, None, None, now)
            .is_err()
    );
    store
        .finish_workflow_operation(event, State::Preparing, None, None, None, now)
        .unwrap();
    assert!(
        store
            .finish_workflow_operation(event, State::Running, None, None, None, now)
            .is_err()
    );
    assert!(
        store
            .finish_workflow_operation(
                event,
                State::Failed,
                None,
                None,
                Some("SECRET or a /private/path".into()),
                now
            )
            .is_err()
    );
    assert_eq!(
        store.workflow_operation(event).unwrap().unwrap().state,
        State::Preparing
    );
    store
        .finish_workflow_operation(
            event,
            State::Blocked,
            None,
            None,
            Some("workflow_unconfigured".into()),
            now,
        )
        .unwrap();
    assert_eq!(store.recover_workflow_operations(now).unwrap(), 0);
}

#[test]
fn startup_recovery_pages_all_owned_admissions_without_replaying_any() {
    let file = NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open(file.path()).unwrap();
    let now = OffsetDateTime::now_utc();
    for value in 1..=257 {
        store
            .admit_workflow_operation(Uuid::from_u128(value), Sha256Digest::of(b"x"), now)
            .unwrap();
    }
    assert_eq!(store.recover_workflow_operations(now).unwrap(), 257);
    assert_eq!(store.recover_workflow_operations(now).unwrap(), 0);
    for value in [1, 256, 257] {
        assert_eq!(
            store
                .workflow_operation(Uuid::from_u128(value))
                .unwrap()
                .unwrap()
                .state,
            State::Unknown
        );
    }
}

#[test]
fn a_projection_that_disagrees_with_its_audit_event_is_rejected() {
    let file = NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open(file.path()).unwrap();
    let event = Uuid::new_v4();
    store
        .admit_workflow_operation(event, Sha256Digest::of(b"x"), OffsetDateTime::now_utc())
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    connection
        .execute("UPDATE workflow_operations SET state='running'", [])
        .unwrap();
    drop(connection);
    let reopened = SqliteStore::open(file.path()).unwrap();
    assert!(matches!(
        reopened.workflow_operation(event),
        Err(StoreError::Workflow(
            "workflow_operation_projection_mismatch"
        ))
    ));
}

#[test]
fn conflicting_native_session_schema_lineage_is_not_migrated() {
    let file = NamedTempFile::new().unwrap();
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    connection.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL); INSERT INTO schema_migrations VALUES(10,'native branch'); CREATE TABLE native_conversations(id TEXT);").unwrap();
    drop(connection);
    assert!(matches!(
        SqliteStore::open(file.path()),
        Err(StoreError::Workflow("store_schema_lineage_incompatible"))
    ));
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    let version: u32 = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 10);
    let operations: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='workflow_operations')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!operations);
}

#[test]
fn schema_ten_upgrades_without_rewriting_audit_and_failed_migration_rolls_back() {
    let file = NamedTempFile::new().unwrap();
    drop(SqliteStore::open(file.path()).unwrap());
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    connection
        .execute_batch(
            "DROP TABLE workflow_operations; DELETE FROM schema_migrations WHERE version=15;",
        )
        .unwrap();
    let before: Vec<String> = connection
        .prepare("SELECT envelope_json FROM events ORDER BY sequence")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(connection);
    drop(SqliteStore::open(file.path()).unwrap());
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    let after: Vec<String> = connection
        .prepare("SELECT envelope_json FROM events ORDER BY sequence")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(before, after);
    let version: u32 = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 15);
    connection
        .execute_batch("DELETE FROM schema_migrations WHERE version=15;")
        .unwrap();
    drop(connection);
    assert!(
        SqliteStore::open(file.path()).is_err(),
        "a colliding table is not silently treated as a migration"
    );
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    let version: u32 = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 10);
}
