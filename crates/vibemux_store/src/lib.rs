#![forbid(unsafe_code)]
//! Blocking SQLite persistence boundary for canonical state and events.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use thiserror::Error;
use vibemux_events::{EventDraft, EventEnvelope, EventError, EventSequence};
use vibemux_types::{Run, Task};

mod a2a;
pub use a2a::A2aCommitOutcome;

pub const STORE_SCHEMA_VERSION: u32 = 2;
pub const SQLITE_BUSY_TIMEOUT_MILLISECONDS: u64 = 5_000;

const INITIAL_SCHEMA: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT UNIQUE NOT NULL,
    idempotency_key TEXT UNIQUE NOT NULL,
    envelope_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projections (
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    state_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL,
    PRIMARY KEY(entity_kind, entity_id)
);
INSERT INTO schema_migrations(version, applied_at)
VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
"#;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("A2A state validation failed")]
    A2aState(#[source] vibemux_types::a2a::A2aStateError),
    #[error("A2A run was not found")]
    A2aNotFound,
    #[error("A2A state version changed")]
    A2aVersionConflict,
    #[error("A2A idempotency key content conflicts")]
    A2aIdempotencyConflict,
    #[error("A2A binding or workspace ownership conflicts")]
    A2aBindingConflict,
    #[error("A2A lifecycle transition is invalid")]
    A2aInvalidTransition,
    #[error("A2A verification evidence is rejected")]
    A2aVerificationRejected,
    #[error("A2A task run capacity is exhausted")]
    A2aCapacity,
    #[error("A2A projection integrity check failed")]
    A2aProjectionMismatch,
    #[error("A2A-bound entities require the validated A2A command API")]
    A2aBoundProjection,
    #[error("SQLite store operation failed")]
    Database(#[source] rusqlite::Error),
    #[error("canonical JSON operation failed")]
    Json(#[source] serde_json::Error),
    #[error("canonical event validation failed")]
    Event(#[source] EventError),
    #[error("state-changing commits require an idempotency key")]
    IdempotencyKeyRequired,
    #[error("SQLite allocated an invalid event sequence")]
    InvalidSequence,
    #[error("database schema {found} is newer than supported version {supported}")]
    NewerSchema { found: u32, supported: u32 },
}

impl StoreError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::A2aState(_) => "store_a2a_invalid_input",
            Self::A2aNotFound => "store_a2a_not_found",
            Self::A2aVersionConflict => "store_a2a_version_conflict",
            Self::A2aIdempotencyConflict => "store_a2a_idempotency_conflict",
            Self::A2aBindingConflict => "store_a2a_binding_conflict",
            Self::A2aInvalidTransition => "store_a2a_invalid_transition",
            Self::A2aVerificationRejected => "store_a2a_verification_rejected",
            Self::A2aCapacity => "store_a2a_capacity_exhausted",
            Self::A2aProjectionMismatch => "store_a2a_projection_mismatch",
            Self::A2aBoundProjection => "store_a2a_bound_projection",
            Self::Database(_) => "store_database_error",
            Self::Json(_) => "store_json_error",
            Self::Event(_) => "store_event_error",
            Self::IdempotencyKeyRequired => "store_idempotency_key_required",
            Self::InvalidSequence => "store_invalid_sequence",
            Self::NewerSchema { .. } => "store_newer_schema",
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<EventError> for StoreError {
    fn from(error: EventError) -> Self {
        Self::Event(error)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitOutcome {
    pub event: EventEnvelope,
    pub duplicate: bool,
}

enum Projection<'a> {
    Task(&'a Task),
    Run(&'a Run),
}

impl Projection<'_> {
    fn parts(&self) -> Result<(&'static str, String, String, String), StoreError> {
        match self {
            Self::Task(task) => Ok((
                "task",
                task.task_id().to_string(),
                task.project_id().to_string(),
                serde_json::to_string(task)?,
            )),
            Self::Run(run) => Ok((
                "run",
                run.run_id().to_string(),
                run.project_id().to_string(),
                serde_json::to_string(run)?,
            )),
        }
    }
}

pub struct SqliteStore {
    connection: Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MILLISECONDS))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn commit_task(
        &mut self,
        task: &Task,
        draft: EventDraft,
    ) -> Result<CommitOutcome, StoreError> {
        self.commit_projection(Projection::Task(task), draft)
    }

    pub fn commit_run(
        &mut self,
        run: &Run,
        draft: EventDraft,
    ) -> Result<CommitOutcome, StoreError> {
        self.commit_projection(Projection::Run(run), draft)
    }

    pub fn projection(
        &self,
        entity_kind: &str,
        entity_id: &str,
    ) -> Result<Option<Value>, StoreError> {
        let encoded: Option<String> = self
            .connection
            .query_row(
                "SELECT state_json FROM projections WHERE entity_kind = ? AND entity_id = ?",
                params![entity_kind, entity_id],
                |row| row.get(0),
            )
            .optional()?;
        encoded
            .map(|value| serde_json::from_str(&value).map_err(StoreError::from))
            .transpose()
    }

    pub fn events(&self) -> Result<Vec<EventEnvelope>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT envelope_json FROM events ORDER BY sequence")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(EventEnvelope::from_json_slice(row?.as_bytes())?);
        }
        Ok(events)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let table_exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;
        let found: u32 = if table_exists {
            self.connection.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )?
        } else {
            0
        };
        if found > STORE_SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                found,
                supported: STORE_SCHEMA_VERSION,
            });
        }
        if found == 0 {
            self.connection.execute_batch(INITIAL_SCHEMA)?;
        }
        if found < 2 {
            self.connection.execute_batch(a2a::MIGRATION_V2)?;
        }
        Ok(())
    }

    fn commit_projection(
        &mut self,
        projection: Projection<'_>,
        draft: EventDraft,
    ) -> Result<CommitOutcome, StoreError> {
        draft.validate()?;
        let bound: bool = match &projection {
            Projection::Task(task) => self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM a2a_runs WHERE task_id=?)",
                [task.task_id().to_string()],
                |row| row.get(0),
            )?,
            Projection::Run(run) => self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM a2a_runs WHERE run_id=?)",
                [run.run_id().to_string()],
                |row| row.get(0),
            )?,
        };
        if bound {
            return Err(StoreError::A2aBoundProjection);
        }
        let idempotency_key = draft
            .idempotency_key
            .clone()
            .ok_or(StoreError::IdempotencyKeyRequired)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let duplicate: Option<String> = transaction
            .query_row(
                "SELECT envelope_json FROM events WHERE idempotency_key = ?",
                [&idempotency_key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(encoded) = duplicate {
            let event = EventEnvelope::from_json_slice(encoded.as_bytes())?;
            transaction.commit()?;
            return Ok(CommitOutcome {
                event,
                duplicate: true,
            });
        }

        transaction.execute(
            "INSERT INTO events(event_id, idempotency_key, envelope_json) VALUES (?, ?, '')",
            params![draft.event_id.to_string(), idempotency_key],
        )?;
        let raw_sequence = transaction.last_insert_rowid();
        let sequence = u64::try_from(raw_sequence)
            .ok()
            .and_then(|value| EventSequence::new(value).ok())
            .ok_or(StoreError::InvalidSequence)?;
        let event = EventEnvelope::commit(draft, sequence)?;
        event.validate()?;
        let encoded_event = serde_json::to_string(&event)?;
        transaction.execute(
            "UPDATE events SET envelope_json = ? WHERE sequence = ?",
            params![encoded_event, raw_sequence],
        )?;

        let (entity_kind, entity_id, project_id, state_json) = projection.parts()?;
        transaction.execute(
            r#"
            INSERT INTO projections(entity_kind, entity_id, project_id, state_json, updated_sequence)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
                project_id = excluded.project_id,
                state_json = excluded.state_json,
                updated_sequence = excluded.updated_sequence
            "#,
            params![entity_kind, entity_id, project_id, state_json, raw_sequence],
        )?;
        transaction.commit()?;
        Ok(CommitOutcome {
            event,
            duplicate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::OffsetDateTime;
    use vibemux_events::{ActorName, EventPayload, EventType};
    use vibemux_types::{EventId, ProjectId, Run, RunSpec, Task, TaskSpec, TaskStatus};

    use super::*;

    fn draft(project_id: ProjectId, key: &str, event_type: &str) -> EventDraft {
        EventDraft {
            event_id: EventId::new(),
            event_type: EventType::new(event_type).expect("valid fixture event type"),
            project_id,
            task_id: None,
            run_id: None,
            causation_id: None,
            actor: ActorName::new("system").expect("valid fixture actor"),
            timestamp: OffsetDateTime::now_utc(),
            idempotency_key: Some(key.to_owned()),
            payload: EventPayload::new(json!({"fixture": true})).expect("valid fixture payload"),
        }
    }

    #[test]
    fn task_state_and_event_commit_atomically() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        let task = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "atomic".to_owned(),
            description: String::new(),
        })
        .expect("valid task");
        let outcome = store
            .commit_task(&task, draft(task.project_id(), "task_1", "task_created"))
            .expect("commit task");
        assert_eq!(outcome.event.sequence().get(), 1);
        assert!(!outcome.duplicate);
        assert_eq!(store.events().expect("events").len(), 1);
        assert_eq!(
            store
                .projection("task", &task.task_id().to_string())
                .expect("projection")
                .expect("task projection")["status"],
            json!("open")
        );
    }

    #[test]
    fn duplicate_idempotency_key_does_not_change_projection() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        let task = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "idempotent".to_owned(),
            description: String::new(),
        })
        .expect("valid task");
        store
            .commit_task(&task, draft(task.project_id(), "same_key", "task_created"))
            .expect("first commit");
        let changed = task
            .transitioned(TaskStatus::InProgress)
            .expect("valid transition");
        let duplicate = store
            .commit_task(
                &changed,
                draft(task.project_id(), "same_key", "task_started"),
            )
            .expect("duplicate commit");
        assert!(duplicate.duplicate);
        assert_eq!(store.events().expect("events").len(), 1);
        assert_eq!(
            store
                .projection("task", &task.task_id().to_string())
                .expect("projection")
                .expect("task projection")["status"],
            json!("open")
        );
    }

    #[test]
    fn projection_failure_rolls_back_event() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_projection BEFORE INSERT ON projections BEGIN SELECT RAISE(ABORT, 'test'); END;",
            )
            .expect("test trigger");
        let task = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "rollback".to_owned(),
            description: String::new(),
        })
        .expect("valid task");
        assert!(
            store
                .commit_task(&task, draft(task.project_id(), "rollback", "task_created"))
                .is_err()
        );
        assert!(store.events().expect("events").is_empty());
    }

    #[test]
    fn run_projection_and_replay_survive_reopen() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let project_id = ProjectId::new();
        let task_id = vibemux_types::TaskId::new();
        let run = Run::new(RunSpec {
            project_id,
            task_id,
            harness: "mock".to_owned(),
            role: "worker".to_owned(),
            protocol: "mock".to_owned(),
            base_commit: "0123456789abcdef".to_owned(),
        })
        .expect("valid run");
        {
            let mut store = SqliteStore::open(temporary.path()).expect("open store");
            store
                .commit_run(&run, draft(project_id, "run_1", "run_preparing"))
                .expect("commit run");
        }
        let store = SqliteStore::open(temporary.path()).expect("reopen store");
        assert_eq!(store.events().expect("events")[0].sequence().get(), 1);
        assert_eq!(
            store
                .projection("run", &run.run_id().to_string())
                .expect("projection")
                .expect("run projection")["base_commit"],
            json!("0123456789abcdef")
        );
    }

    #[test]
    fn empty_migration_registry_is_completed() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = Connection::open(temporary.path()).expect("open raw database");
        connection
            .execute(
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
                [],
            )
            .expect("empty migration registry");
        drop(connection);
        let store = SqliteStore::open(temporary.path()).expect("migrate store");
        assert!(store.events().expect("events").is_empty());
    }

    #[test]
    fn newer_schema_fails_closed() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let connection = Connection::open(temporary.path()).expect("open raw database");
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL); INSERT INTO schema_migrations VALUES (3, 'future');",
            )
            .expect("future migration registry");
        drop(connection);
        assert!(matches!(
            SqliteStore::open(temporary.path()),
            Err(StoreError::NewerSchema {
                found: 3,
                supported: STORE_SCHEMA_VERSION
            })
        ));
    }
}
