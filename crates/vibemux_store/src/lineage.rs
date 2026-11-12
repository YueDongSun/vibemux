//! Known database-layout recognition for operator diagnostics and safe refusal.
//!
//! The current store identifies itself only through `schema_migrations`'
//! maximum version, which is not enough: two historical branches both used
//! version 10 for different DDL, the Python reference store uses the same
//! table name with a different `events` shape, and a marker alone cannot prove
//! that the required objects exist.
//!
//! `inspect` classifies a database from its `schema_migrations` history plus a
//! table/column/index set. `SqliteStore::open` refuses a recognized but
//! unsupported lineage (reports schema 10, chat schema 10-14) and a foreign
//! layout before writing anything, so the database is preserved for the
//! owner's upgrade path. A current or older Rust layout continues through the
//! existing forward migrations, followed by a post-migration object check.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::{STORE_SCHEMA_VERSION, StoreError};

/// The layout families this binary can distinguish.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaLineage {
    /// No store objects yet; `open` applies v1-v9.
    Empty,
    /// Rust schema 1-8: forward migration applies the missing steps.
    LegacyRust,
    /// Rust schema 9 with the complete object set.
    Current,
    /// PR #52: schema 10 with `workflow_worker_reports`.
    ReportsSchema10,
    /// PR #49: schema 10-14 with the chat tables.
    ChatSchema10,
    ChatSchema11,
    ChatSchema12,
    ChatSchema13,
    ChatSchema14,
    /// A Python VibeMux store (same `schema_migrations` name, different events).
    ForeignPython,
    /// A recognizable table set with an unknown marker combination.
    Unknown,
    /// A marker newer than every known lineage.
    Future,
    /// The current marker but missing required current objects.
    Incomplete,
}

impl SchemaLineage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::LegacyRust => "legacy_rust",
            Self::Current => "current",
            Self::ReportsSchema10 => "reports_schema10",
            Self::ChatSchema10 => "chat_schema10",
            Self::ChatSchema11 => "chat_schema11",
            Self::ChatSchema12 => "chat_schema12",
            Self::ChatSchema13 => "chat_schema13",
            Self::ChatSchema14 => "chat_schema14",
            Self::ForeignPython => "foreign_python",
            Self::Unknown => "unknown",
            Self::Future => "future",
            Self::Incomplete => "incomplete",
        }
    }

    /// Layouts this binary may open (forward migration or already current).
    #[must_use]
    pub const fn is_openable(self) -> bool {
        matches!(self, Self::Empty | Self::LegacyRust | Self::Current)
    }
}

/// A read-only description of one database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaInspection {
    pub found_version: u32,
    pub applied_versions: Vec<u32>,
    pub lineage: SchemaLineage,
    pub tables: Vec<String>,
    pub missing_required: Vec<String>,
}

/// Tables a complete current (v9) Rust store must contain.
pub const REQUIRED_CURRENT_TABLES: &[&str] = &[
    "schema_migrations",
    "events",
    "projections",
    "a2a_runs",
    "a2a_commands",
    "harness_dispatches",
    "workflows",
    "workflow_contracts",
    "workflow_slots",
    "workflow_leases",
    "workflow_attempts",
    "workflow_receipts",
    "workflow_messages",
    "content_store_entries",
    "prompt_policy_versions",
    "decision_records",
    "workflow_contract_authorities",
    "workflow_model_call_owners",
    "workflow_model_calls",
    "workflow_controllers",
    "workflow_controller_outbox",
    "workflow_checkpoints",
    "workflow_recovery_bindings",
    "workflow_supervisors",
    "workflow_supervisor_rounds",
    "workflow_supervisor_actions",
    "workflow_contract_edits",
];

/// Partial unique indexes whose absence changes runtime reservation semantics.
pub const REQUIRED_CURRENT_INDEXES: &[&str] = &[
    "a2a_runs_task",
    "harness_dispatches_active_resource",
    "workflow_leases_holding_slot",
    "workflow_leases_holding_worktree",
    "prompt_policy_versions_one_active",
];

/// Inspect a closed database without writing to it.
pub fn inspect_database(path: &Path) -> Result<SchemaInspection, StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    inspect(&connection)
}

/// Classify a database from its migration history and object shape.
pub fn inspect(connection: &Connection) -> Result<SchemaInspection, StoreError> {
    let tables = list_objects(connection, "table")?;
    let indexes = list_objects(connection, "index")?;
    let has = |name: &str| tables.iter().any(|table| table == name);
    if !has("schema_migrations") {
        let lineage = if tables.is_empty() {
            SchemaLineage::Empty
        } else if has("projects") && has("tasks") && has("runs") {
            SchemaLineage::ForeignPython
        } else {
            SchemaLineage::Unknown
        };
        let missing_required = required_missing(&tables, &indexes);
        return Ok(SchemaInspection {
            found_version: 0,
            applied_versions: Vec::new(),
            lineage,
            tables,
            missing_required,
        });
    }
    let applied_versions = applied_versions(connection)?;
    let found_version = applied_versions.iter().copied().max().unwrap_or(0);
    let missing_required = required_missing(&tables, &indexes);
    let has_events = has("events");
    let events_is_foreign = has_events && !has_column(connection, "events", "envelope_json")?;
    let python_markers = has("projects") && has("tasks") && has("runs");
    let lineage = if events_is_foreign || (!has_events && found_version == 0 && python_markers) {
        SchemaLineage::ForeignPython
    } else if found_version == 0 && !has_events {
        if tables.len() == 1 {
            SchemaLineage::Empty
        } else {
            SchemaLineage::Unknown
        }
    } else if has("chat_sessions") {
        match found_version {
            10 => SchemaLineage::ChatSchema10,
            11 => SchemaLineage::ChatSchema11,
            12 => SchemaLineage::ChatSchema12,
            13 => SchemaLineage::ChatSchema13,
            14 => SchemaLineage::ChatSchema14,
            _ => SchemaLineage::Unknown,
        }
    } else if found_version == 10 && has("workflow_worker_reports") {
        SchemaLineage::ReportsSchema10
    } else if found_version > STORE_SCHEMA_VERSION {
        SchemaLineage::Future
    } else if found_version == STORE_SCHEMA_VERSION {
        if missing_required.is_empty() {
            SchemaLineage::Current
        } else {
            SchemaLineage::Incomplete
        }
    } else if found_version >= 1 {
        SchemaLineage::LegacyRust
    } else {
        SchemaLineage::Unknown
    };
    Ok(SchemaInspection {
        found_version,
        applied_versions,
        lineage,
        tables,
        missing_required,
    })
}

/// Fail closed unless the database now contains every current object.
pub(crate) fn verify_current(connection: &Connection) -> Result<(), StoreError> {
    let tables = list_objects(connection, "table")?;
    let indexes = list_objects(connection, "index")?;
    let missing = required_missing(&tables, &indexes);
    if missing.is_empty() {
        return Ok(());
    }
    Err(StoreError::SchemaIncomplete {
        found: STORE_SCHEMA_VERSION,
        missing: missing.join(","),
    })
}

fn applied_versions(connection: &Connection) -> Result<Vec<u32>, StoreError> {
    let mut statement =
        connection.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| row.get::<_, u32>(0))?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

fn list_objects(connection: &Connection, kind: &str) -> Result<Vec<String>, StoreError> {
    // `kind` is a fixed literal from this module, never caller input.
    let sql = format!(
        "SELECT name FROM sqlite_master WHERE type='{kind}' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, StoreError> {
    let mut statement = connection.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let rows = statement.query_map([table], |row| row.get::<_, String>(0))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn required_missing(tables: &[String], indexes: &[String]) -> Vec<String> {
    let mut missing: Vec<String> = REQUIRED_CURRENT_TABLES
        .iter()
        .filter(|name| !tables.iter().any(|table| table == *name))
        .map(|name| (*name).to_string())
        .collect();
    missing.extend(
        REQUIRED_CURRENT_INDEXES
            .iter()
            .filter(|name| !indexes.iter().any(|index| index == *name))
            .map(|name| (*name).to_string()),
    );
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn current_database(path: &Path) {
        let store = crate::SqliteStore::open(path).expect("current store");
        drop(store);
    }

    fn apply_sql(path: &Path, sql: &str) {
        let connection = Connection::open(path).expect("open fixture");
        connection.execute_batch(sql).expect("apply fixture SQL");
    }

    fn count(path: &Path, table: &str) -> i64 {
        let connection = Connection::open(path).expect("open");
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count")
    }

    #[test]
    fn current_database_is_recognized_and_reopen_is_idempotent() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("store.sqlite3");
        current_database(&path);
        let inspection = inspect_database(&path).expect("inspect");
        assert_eq!(inspection.lineage, SchemaLineage::Current);
        assert_eq!(inspection.found_version, STORE_SCHEMA_VERSION);
        assert!(inspection.missing_required.is_empty());
        let before = count(&path, "schema_migrations");
        current_database(&path);
        assert_eq!(count(&path, "schema_migrations"), before);
    }

    #[test]
    fn reports_schema_ten_is_recognized_and_preserved() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("reports.sqlite3");
        current_database(&path);
        apply_sql(
            &path,
            r#"
CREATE TABLE workflow_worker_reports (
    request_id TEXT PRIMARY KEY REFERENCES workflow_attempts(request_id),
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence)
);
CREATE INDEX workflow_worker_reports_page ON workflow_worker_reports(workflow_id, created_sequence);
INSERT INTO schema_migrations(version, applied_at)
VALUES (10, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#,
        );
        let inspection = inspect_database(&path).expect("inspect");
        assert_eq!(inspection.lineage, SchemaLineage::ReportsSchema10);
        let error = crate::SqliteStore::open(&path)
            .err()
            .expect("known lineage is refused");
        assert!(matches!(
            error,
            StoreError::KnownLineage {
                lineage: "reports_schema10",
                found: 10,
                ..
            }
        ));
        assert_eq!(count(&path, "schema_migrations"), 10);
        assert_eq!(count(&path, "workflow_worker_reports"), 0);
        assert!(
            inspect_database(&path).expect("still inspectable").lineage
                == SchemaLineage::ReportsSchema10
        );
    }

    #[test]
    fn chat_schema_ten_and_fourteen_are_recognized_and_preserved() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("chat10.sqlite3");
        current_database(&path);
        apply_sql(
            &path,
            r#"
CREATE TABLE chat_sessions(
 chat_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, state TEXT NOT NULL,
 generation INTEGER NOT NULL, version INTEGER NOT NULL, resource_key TEXT NOT NULL,
 owner_fence TEXT, record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE chat_turns(
 request_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 turn_id TEXT NOT NULL UNIQUE, task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
 generation INTEGER NOT NULL, version INTEGER NOT NULL, phase TEXT NOT NULL,
 resource_key TEXT NOT NULL, send_claimed INTEGER NOT NULL, send_fence TEXT,
 binding_json TEXT NOT NULL, finish_json TEXT, record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE chat_content(blob_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id));
CREATE TABLE chat_records(record_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id));
CREATE TABLE chat_operations(operation_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id));
INSERT INTO schema_migrations(version, applied_at)
VALUES (10, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#,
        );
        assert_eq!(
            inspect_database(&path).expect("inspect").lineage,
            SchemaLineage::ChatSchema10
        );
        let error = crate::SqliteStore::open(&path)
            .err()
            .expect("chat lineage is refused");
        assert!(matches!(
            error,
            StoreError::KnownLineage {
                lineage: "chat_schema10",
                found: 10,
                ..
            }
        ));
        assert_eq!(count(&path, "chat_sessions"), 0);

        let fourteen = temp.path().join("chat14.sqlite3");
        fs::copy(&path, &fourteen).expect("copy fixture");
        apply_sql(
            &fourteen,
            "INSERT INTO schema_migrations(version, applied_at) VALUES
             (11,'x'),(12,'x'),(13,'x'),(14,'x');",
        );
        let inspection = inspect_database(&fourteen).expect("inspect v14");
        assert_eq!(inspection.found_version, 14);
        assert_eq!(inspection.lineage, SchemaLineage::ChatSchema14);
        let error = crate::SqliteStore::open(&fourteen)
            .err()
            .expect("chat v14 is refused");
        assert!(matches!(
            error,
            StoreError::KnownLineage {
                lineage: "chat_schema14",
                found: 14,
                ..
            }
        ));
        assert_eq!(count(&fourteen, "schema_migrations"), 14);
    }

    #[test]
    fn foreign_python_store_is_refused_without_modification() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("python.sqlite3");
        apply_sql(
            &path,
            r#"
CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
INSERT INTO schema_migrations VALUES (2, 'python');
CREATE TABLE events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL,
 project_id TEXT, task_id TEXT, run_id TEXT, payload TEXT NOT NULL, timestamp TEXT NOT NULL, actor TEXT);
CREATE TABLE projects(project_id TEXT PRIMARY KEY);
CREATE TABLE tasks(task_id TEXT PRIMARY KEY);
CREATE TABLE runs(run_id TEXT PRIMARY KEY);
"#,
        );
        assert_eq!(
            inspect_database(&path).expect("inspect").lineage,
            SchemaLineage::ForeignPython
        );
        let error = crate::SqliteStore::open(&path)
            .err()
            .expect("foreign store is refused");
        assert!(matches!(error, StoreError::UnsupportedDatabase { .. }));
        assert_eq!(count(&path, "schema_migrations"), 1);
    }

    #[test]
    fn future_and_incomplete_layouts_are_distinct() {
        let temp = tempfile::tempdir().expect("temp");
        let future = temp.path().join("future.sqlite3");
        current_database(&future);
        apply_sql(
            &future,
            "INSERT INTO schema_migrations(version, applied_at) VALUES (15,'future');",
        );
        assert_eq!(
            inspect_database(&future).expect("inspect").lineage,
            SchemaLineage::Future
        );
        assert!(matches!(
            crate::SqliteStore::open(&future)
                .err()
                .expect("future refused"),
            StoreError::NewerSchema { found: 15, .. }
        ));

        let incomplete = temp.path().join("incomplete.sqlite3");
        current_database(&incomplete);
        apply_sql(&incomplete, "DROP TABLE workflow_contract_edits;");
        let inspection = inspect_database(&incomplete).expect("inspect");
        assert_eq!(inspection.lineage, SchemaLineage::Incomplete);
        assert!(
            inspection
                .missing_required
                .contains(&"workflow_contract_edits".to_string())
        );
        assert!(matches!(
            crate::SqliteStore::open(&incomplete)
                .err()
                .expect("incomplete refused"),
            StoreError::SchemaIncomplete { .. }
        ));
    }
}
