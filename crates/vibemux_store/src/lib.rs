#![forbid(unsafe_code)]
//! Blocking SQLite persistence boundary for canonical state and events.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use thiserror::Error;
use vibemux_events::{EventDraft, EventEnvelope, EventError, EventSequence};
use vibemux_harness::{HarnessConfigSnapshot, HarnessRegistrySnapshot, seed_config, seed_registry};
use vibemux_types::{ProjectId, Run, Task};

mod a2a;
pub use a2a::A2aCommitOutcome;

pub const STORE_SCHEMA_VERSION: u32 = 3;
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

/// Migration 3 seeds the harness registry and default-harness projections so
/// the detection gate holds across restarts even before the first refresh.
/// The rows mirror the Python `harnesses.json` shape (plus the persisted
/// default the Python sidecar never carried); the daemon supplies the
/// project-scoped seed event and rows, so this constant only bumps the
/// schema version marker.
const MIGRATION_V3: &str = r#"
BEGIN IMMEDIATE;
INSERT INTO schema_migrations(version, applied_at)
VALUES (3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
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
    #[error("harness snapshot state is invalid: {0}")]
    InvalidHarnessState(&'static str),
    #[error("unknown harness name: {0}")]
    UnknownHarness(String),
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
            Self::InvalidHarnessState(code) => code,
            Self::UnknownHarness(_) => "store_unknown_harness",
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

/// Outcome of a harness snapshot commit: the event plus the registry that
/// was persisted (roles carried over from the prior snapshot).
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessSnapshotOutcome {
    pub event: EventEnvelope,
    pub duplicate: bool,
    pub registry: HarnessRegistrySnapshot,
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

    pub fn harness_registry(&self) -> Result<HarnessRegistrySnapshot, StoreError> {
        let snapshot = self
            .harness_projection::<HarnessRegistrySnapshot>(
                vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
            )?
            .unwrap_or_else(seed_registry);
        snapshot
            .validate()
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        Ok(snapshot)
    }

    pub fn harness_config(&self) -> Result<HarnessConfigSnapshot, StoreError> {
        let config = self
            .harness_projection::<HarnessConfigSnapshot>(
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            )?
            .unwrap_or_else(seed_config);
        if config.default_harness != vibemux_harness::NO_DEFAULT_HARNESS
            && vibemux_harness::profile_by_name(&config.default_harness).is_none()
        {
            return Err(StoreError::UnknownHarness(config.default_harness));
        }
        Ok(config)
    }

    fn harness_projection<T: serde::de::DeserializeOwned>(
        &self,
        entity_kind: &str,
    ) -> Result<Option<T>, StoreError> {
        let encoded: Option<String> = self
            .connection
            .query_row(
                "SELECT state_json FROM projections WHERE entity_kind = ? AND entity_id = ?",
                params![entity_kind, entity_kind],
                |row| row.get(0),
            )
            .optional()?;
        encoded
            .map(|value| serde_json::from_str(&value).map_err(StoreError::from))
            .transpose()
    }

    /// Atomically persist a harness detection snapshot (registry projection)
    /// plus the canonical `harness_probed` event. Roles carry over from the
    /// previously persisted registry; detection replaces availability. The
    /// default-harness projection is written alongside only when it does not
    /// exist yet, so a refresh never silently overrides an operator's
    /// `switch` selection. `checked_at` stamps the new snapshot.
    pub fn commit_harness_snapshot(
        &mut self,
        config: &HarnessConfigSnapshot,
        draft: EventDraft,
        detections: &std::collections::BTreeMap<String, vibemux_harness::HarnessDetection>,
        checked_at: &str,
    ) -> Result<HarnessSnapshotOutcome, StoreError> {
        let previous = self.harness_registry()?;
        let (registry, _payload) =
            vibemux_harness::build_refresh(&previous, detections, checked_at);
        registry
            .validate()
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        let idempotency_key = draft
            .idempotency_key
            .clone()
            .ok_or(StoreError::IdempotencyKeyRequired)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(event) = Self::find_duplicate(&transaction, &idempotency_key)? {
            transaction.commit()?;
            let registry = self.harness_registry()?;
            return Ok(HarnessSnapshotOutcome {
                event,
                duplicate: true,
                registry,
            });
        }
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        Self::upsert_harness_projection(
            &transaction,
            vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
            &serde_json::to_string(&registry)?,
            raw_sequence,
            event.project_id(),
        )?;
        // Only seed the default when absent: a refresh must never clobber a
        // prior switch selection.
        let config_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM projections WHERE entity_kind = ? AND entity_id = ?)",
            params![
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND
            ],
            |row| row.get(0),
        )?;
        if !config_exists {
            Self::upsert_harness_projection(
                &transaction,
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                &serde_json::to_string(config)?,
                raw_sequence,
                event.project_id(),
            )?;
        }
        transaction.commit()?;
        Ok(HarnessSnapshotOutcome {
            event,
            duplicate: false,
            registry,
        })
    }

    /// Atomically persist a default-harness switch (config projection) plus
    /// the canonical `harness_switched` event. The harness must be detected
    /// in the current registry snapshot, mirroring the Python spawn/switch
    /// detection gate.
    pub fn commit_harness_switch(
        &mut self,
        harness_name: &str,
        draft: EventDraft,
    ) -> Result<CommitOutcome, StoreError> {
        if vibemux_harness::profile_by_name(harness_name).is_none() {
            return Err(StoreError::UnknownHarness(harness_name.to_string()));
        }
        let registry = self.harness_registry()?;
        let detected = registry
            .state(harness_name)
            .is_some_and(|state| state.detected);
        if !detected {
            return Err(StoreError::InvalidHarnessState("harness_not_detected"));
        }
        let config = HarnessConfigSnapshot {
            default_harness: harness_name.to_string(),
        };
        let idempotency_key = draft
            .idempotency_key
            .clone()
            .ok_or(StoreError::IdempotencyKeyRequired)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(event) = Self::find_duplicate(&transaction, &idempotency_key)? {
            transaction.commit()?;
            return Ok(CommitOutcome {
                event,
                duplicate: true,
            });
        }
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        Self::upsert_harness_projection(
            &transaction,
            vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            &serde_json::to_string(&config)?,
            raw_sequence,
            event.project_id(),
        )?;
        transaction.commit()?;
        Ok(CommitOutcome {
            event,
            duplicate: false,
        })
    }

    fn find_duplicate(
        transaction: &rusqlite::Transaction<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventEnvelope>, StoreError> {
        let encoded: Option<String> = transaction
            .query_row(
                "SELECT envelope_json FROM events WHERE idempotency_key = ?",
                [idempotency_key],
                |row| row.get(0),
            )
            .optional()?;
        encoded
            .map(|value| EventEnvelope::from_json_slice(value.as_bytes()).map_err(StoreError::from))
            .transpose()
    }

    fn insert_event(
        transaction: &rusqlite::Transaction<'_>,
        draft: EventDraft,
    ) -> Result<(EventEnvelope, i64), StoreError> {
        draft.validate()?;
        let idempotency_key = draft
            .idempotency_key
            .clone()
            .ok_or(StoreError::IdempotencyKeyRequired)?;
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
        Ok((event, raw_sequence))
    }

    fn upsert_harness_projection(
        transaction: &rusqlite::Transaction<'_>,
        entity_kind: &str,
        state_json: &str,
        raw_sequence: i64,
        project_id: ProjectId,
    ) -> Result<(), StoreError> {
        transaction.execute(
            r#"
            INSERT INTO projections(entity_kind, entity_id, project_id, state_json, updated_sequence)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
                project_id = excluded.project_id,
                state_json = excluded.state_json,
                updated_sequence = excluded.updated_sequence
            "#,
            params![
                entity_kind,
                entity_kind,
                project_id.to_string(),
                state_json,
                raw_sequence
            ],
        )?;
        Ok(())
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
        if found < 3 {
            self.connection.execute_batch(MIGRATION_V3)?;
            self.seed_harness_projections()?;
        }
        Ok(())
    }

    /// The v3 migration cannot know the project id, so the seed projections
    /// are only written when the events table is still empty (fresh database)
    /// or a project already exists (upgrade from v2). The daemon's first
    /// harness refresh back-fills the projections on databases created
    /// between v2 and v3 with no events at all.
    fn seed_harness_projections(&mut self) -> Result<(), StoreError> {
        let has_events: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events LIMIT 1)",
            [],
            |row| row.get(0),
        )?;
        let project_id: Option<String> = if has_events {
            self.connection
                .query_row("SELECT envelope_json FROM events LIMIT 1", [], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .and_then(|envelope| {
                    serde_json::from_str::<Value>(&envelope)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("project_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                })
        } else {
            None
        };
        let Some(project_id) = project_id else {
            return Ok(());
        };
        let project_id: ProjectId = project_id.parse().map_err(|_| {
            StoreError::InvalidHarnessState(vibemux_harness::HarnessError::InvalidState.code())
        })?;
        let draft = vibemux_harness::events::seed(project_id)
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        let _ = self.commit_harness_snapshot(
            &seed_config(),
            draft,
            &std::collections::BTreeMap::new(),
            "migration-v3",
        )?;
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
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL); INSERT INTO schema_migrations VALUES (4, 'future');",
            )
            .expect("future migration registry");
        drop(connection);
        assert!(matches!(
            SqliteStore::open(temporary.path()),
            Err(StoreError::NewerSchema {
                found: 4,
                supported: STORE_SCHEMA_VERSION
            })
        ));
    }

    fn detected(name: &str) -> HarnessRegistrySnapshot {
        let mut registry = seed_registry();
        registry
            .harnesses
            .iter_mut()
            .find(|entry| entry.name == name)
            .expect("entry")
            .state
            .detected = true;
        registry.checked_at = Some("2026-09-16T00:00:00Z".to_string());
        registry
    }

    fn detections_for(
        names: &[&str],
    ) -> std::collections::BTreeMap<String, vibemux_harness::HarnessDetection> {
        names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    vibemux_harness::HarnessDetection {
                        detected: true,
                        path: Some(format!("C:\\tools\\{name}.exe")),
                        launcher: None,
                        version: Some("1.0.0".to_string()),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn harness_refresh_and_switch_persist_across_reopen() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let project_id = ProjectId::new();
        let _registry = detected("claude");
        {
            let mut store = SqliteStore::open(temporary.path()).expect("open store");
            let probed = vibemux_harness::events::probed(
                project_id,
                "probe-1".to_string(),
                json!({"detected": ["claude"], "missing": []}),
            )
            .expect("draft");
            let outcome = store
                .commit_harness_snapshot(
                    &seed_config(),
                    probed,
                    &detections_for(&["claude"]),
                    "2026-09-16T00:00:00Z",
                )
                .expect("commit snapshot");
            assert!(!outcome.duplicate);
            assert_eq!(
                store.harness_config().expect("config").default_harness,
                vibemux_harness::NO_DEFAULT_HARNESS
            );
            let switched = vibemux_harness::events::switched(
                project_id,
                "switch-1".to_string(),
                "none",
                "claude",
            )
            .expect("draft");
            store
                .commit_harness_switch("claude", switched)
                .expect("switch");
        }
        let store = SqliteStore::open(temporary.path()).expect("reopen store");
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            "claude"
        );
        let persisted = store.harness_registry().expect("registry");
        assert!(persisted.state("claude").expect("claude").detected);
        assert!(!persisted.state("codex").expect("codex").detected);
        let event_types: Vec<String> = store
            .events()
            .expect("events")
            .iter()
            .map(|event| event.event_type().as_str().to_string())
            .collect();
        assert!(event_types.iter().any(|t| t == "harness_probed"));
        assert!(event_types.iter().any(|t| t == "harness_switched"));
    }

    #[test]
    fn harness_switch_rejects_undetected_and_unknown() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let project_id = ProjectId::new();
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        store
            .commit_harness_snapshot(
                &seed_config(),
                vibemux_harness::events::probed(
                    project_id,
                    "probe-1".to_string(),
                    json!({"detected": ["claude"], "missing": []}),
                )
                .expect("draft"),
                &detections_for(&["claude"]),
                "2026-09-16T00:00:00Z",
            )
            .expect("commit");
        // Unknown name fails before any state change.
        assert!(matches!(
            store.commit_harness_switch(
                "unknown",
                vibemux_harness::events::switched(project_id, "s1".to_string(), "none", "unknown")
                    .expect("draft"),
            ),
            Err(StoreError::UnknownHarness(_))
        ));
        // Known but undetected harness is gated.
        assert!(matches!(
            store.commit_harness_switch(
                "codex",
                vibemux_harness::events::switched(project_id, "s2".to_string(), "none", "codex")
                    .expect("draft"),
            ),
            Err(StoreError::InvalidHarnessState("harness_not_detected"))
        ));
        // Neither rejection appended an event or moved the default.
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            vibemux_harness::NO_DEFAULT_HARNESS
        );
    }

    #[test]
    fn harness_refresh_idempotent_and_preserves_default() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let project_id = ProjectId::new();
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        let _registry = detected("claude");
        let draft = vibemux_harness::events::probed(
            project_id,
            "probe-1".to_string(),
            json!({"detected": ["claude"], "missing": []}),
        )
        .expect("draft");
        store
            .commit_harness_snapshot(
                &seed_config(),
                draft,
                &detections_for(&["claude"]),
                "2026-09-16T00:00:00Z",
            )
            .expect("commit");
        // Set the default via switch.
        store
            .commit_harness_switch(
                "claude",
                vibemux_harness::events::switched(project_id, "sw-1".to_string(), "none", "claude")
                    .expect("draft"),
            )
            .expect("switch");
        // A duplicate refresh (same idempotency key) is a no-op.
        let duplicate = store
            .commit_harness_snapshot(
                &HarnessConfigSnapshot {
                    default_harness: "codex".to_string(),
                },
                vibemux_harness::events::probed(
                    project_id,
                    "probe-1".to_string(),
                    json!({"detected": ["claude"], "missing": []}),
                )
                .expect("draft"),
                &detections_for(&["claude"]),
                "2026-09-16T00:00:01Z",
            )
            .expect("duplicate commit");
        assert!(duplicate.duplicate);
        // A fresh refresh with a different key must not clobber the switch.
        store
            .commit_harness_snapshot(
                &seed_config(),
                vibemux_harness::events::probed(
                    project_id,
                    "probe-2".to_string(),
                    json!({"detected": ["claude"], "missing": []}),
                )
                .expect("draft"),
                &detections_for(&["claude"]),
                "2026-09-16T00:00:02Z",
            )
            .expect("refresh 2");
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            "claude"
        );
    }
}
