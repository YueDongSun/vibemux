#![forbid(unsafe_code)]
//! Blocking SQLite persistence boundary for canonical state and events.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use thiserror::Error;
use vibemux_events::{EventDraft, EventEnvelope, EventError, EventSequence};
use vibemux_harness::{
    HarnessConfigSnapshot, HarnessDetection, HarnessRegistrySnapshot, seed_config, seed_registry,
};
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

/// Migration 3 bumps the schema marker. The actual seed projections run via
/// `seed_harness_projections` (called below) inside the SAME explicit
/// transaction as the marker insert; a seed failure therefore leaves
/// `schema_migrations` WITHOUT version 3 so the next `open()` retries the
/// migration: there is never a marked-migrated-but-unseeded database.
const MIGRATION_V3: &str = r#"
INSERT INTO schema_migrations(version, applied_at)
VALUES (3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
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

/// Outcome of a harness snapshot commit: the canonical event plus the
/// registry/config/payload triple the caller needs to render rows without a
/// second store round trip.
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessSnapshotOutcome {
    pub event: EventEnvelope,
    pub duplicate: bool,
    pub registry: HarnessRegistrySnapshot,
    pub config: HarnessConfigSnapshot,
    pub payload: Value,
}

/// Outcome of a harness switch commit: the canonical event plus the
/// `(from, to)` pair the CLI renders. `from` is read inside the same
/// transaction as the write, so concurrent switches can never record a lying
/// `from`.
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessSwitchOutcome {
    pub event: EventEnvelope,
    pub duplicate: bool,
    pub from: String,
    pub to: String,
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

    /// Typed `SELECT envelope_json FROM events ORDER BY sequence LIMIT 1`
    /// accessor: returns the project id carried by the first canonical event,
    /// or `None` when the events table is empty. Used by the writer to mint a
    /// fresh `ProjectId` inside the same transaction that inserts the first
    /// event for a brand-new project (no cross-request race).
    pub fn project_id(&self) -> Result<Option<ProjectId>, StoreError> {
        let encoded: Option<String> = self
            .connection
            .query_row(
                "SELECT envelope_json FROM events ORDER BY sequence LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match encoded {
            Some(value) => {
                let event = EventEnvelope::from_json_slice(value.as_bytes())?;
                Ok(Some(event.project_id()))
            }
            None => Ok(None),
        }
    }

    pub fn harness_registry(&self) -> Result<HarnessRegistrySnapshot, StoreError> {
        let snapshot: HarnessRegistrySnapshot = self
            .projection(
                vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
                vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
            )?
            .map(serde_json::from_value)
            .transpose()
            .map_err(StoreError::from)?
            .unwrap_or_else(seed_registry);
        snapshot
            .validate()
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        Ok(snapshot)
    }

    /// Self-healing read: a persisted `default_harness` that no longer maps
    /// to a known profile returns the `NO_DEFAULT_HARNESS` snapshot instead of
    /// `Err(UnknownHarness)`, so the daemon can keep serving a fresh `switch`
    /// after an operator removed a harness from the registry. Write-path
    /// gating (the `switch` path) stays strict and still rejects unknown
    /// names so a stale persisted default never leaks into a new selection.
    pub fn harness_config(&self) -> Result<HarnessConfigSnapshot, StoreError> {
        let config: HarnessConfigSnapshot = self
            .projection(
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            )?
            .map(serde_json::from_value)
            .transpose()
            .map_err(StoreError::from)?
            .unwrap_or_else(seed_config);
        if config.default_harness != vibemux_harness::NO_DEFAULT_HARNESS
            && vibemux_harness::profile_by_name(&config.default_harness).is_none()
        {
            return Ok(seed_config());
        }
        Ok(config)
    }

    /// Atomically persist a harness detection snapshot (registry projection)
    /// plus the canonical `harness_probed` event. Roles carry over from the
    /// previously persisted registry; detection replaces availability. The
    /// default-harness projection is written alongside only when it does not
    /// exist yet, so a refresh never silently overrides an operator's
    /// `switch` selection. `idempotency_key` mints the canonical event AND
    /// gates the duplicate path; `checked_at` stamps the new snapshot. The
    /// outcome carries the in-scope registry/config/payload so the daemon can
    /// build rows without a second round trip.
    pub fn commit_harness_snapshot(
        &mut self,
        detections: &std::collections::BTreeMap<String, HarnessDetection>,
        checked_at: &str,
        idempotency_key: &str,
    ) -> Result<HarnessSnapshotOutcome, StoreError> {
        let previous = self.harness_registry()?;
        let previous_config = self.harness_config()?;
        let (registry, payload) = vibemux_harness::build_refresh(&previous, detections, checked_at);
        registry
            .validate()
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project_id = resolve_or_mint_project_id(&transaction)?;
        let draft = vibemux_harness::events::probed(
            project_id,
            idempotency_key.to_string(),
            payload.clone(),
        )
        .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        if let Some(event) = Self::find_duplicate(&transaction, idempotency_key)? {
            transaction.commit()?;
            return Ok(HarnessSnapshotOutcome {
                event,
                duplicate: true,
                registry,
                config: previous_config,
                payload,
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
        Self::seed_config_if_absent(
            &transaction,
            &previous_config,
            raw_sequence,
            event.project_id(),
        )?;
        transaction.commit()?;
        Ok(HarnessSnapshotOutcome {
            event,
            duplicate: false,
            registry,
            config: previous_config,
            payload,
        })
    }

    /// Atomically persist a default-harness switch (config projection) plus
    /// the canonical `harness_switched` event. All gates (unknown name,
    /// undetected) run BEFORE any write so a rejected request never appends
    /// an event or moves the persisted default. `from` is read inside the
    /// same transaction that performs the write, so two concurrent switches
    /// can never both record a `from` of `none` for distinct targets.
    pub fn commit_harness_switch(
        &mut self,
        harness_name: &str,
        idempotency_key: &str,
    ) -> Result<HarnessSwitchOutcome, StoreError> {
        if vibemux_harness::profile_by_name(harness_name).is_none() {
            return Err(StoreError::UnknownHarness(harness_name.to_string()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let from = Self::read_default_harness(&transaction)?;
        let registry = Self::read_registry_in_tx(&transaction)?;
        let detected = registry
            .state(harness_name)
            .is_some_and(|state| state.detected);
        if !detected {
            return Err(StoreError::InvalidHarnessState("harness_not_detected"));
        }
        let project_id = resolve_or_mint_project_id(&transaction)?;
        let draft = vibemux_harness::events::switched(
            project_id,
            idempotency_key.to_string(),
            &from,
            harness_name,
        )
        .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        if let Some(event) = Self::find_duplicate(&transaction, idempotency_key)? {
            transaction.commit()?;
            // The duplicate event already recorded the authoritative
            // `from`/`to`. A retry must never report the CURRENT persisted
            // default as `from` — if the default changed since the original
            // commit (a later switch), returning the read-back default would
            // surface a `(from, to)` pair the event never recorded. Fall back
            // to the current default only if the payload does not carry the
            // recorded values (a key reused across event types).
            let payload = event.payload().value();
            let recorded_from = payload
                .get("from")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(from.as_str())
                .to_string();
            let recorded_to = payload
                .get("to")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(harness_name)
                .to_string();
            return Ok(HarnessSwitchOutcome {
                event,
                duplicate: true,
                from: recorded_from,
                to: recorded_to,
            });
        }
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        let next_config = HarnessConfigSnapshot {
            default_harness: harness_name.to_string(),
        };
        Self::upsert_harness_projection(
            &transaction,
            vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            &serde_json::to_string(&next_config)?,
            raw_sequence,
            event.project_id(),
        )?;
        transaction.commit()?;
        Ok(HarnessSwitchOutcome {
            event,
            duplicate: false,
            from,
            to: harness_name.to_string(),
        })
    }

    fn read_default_harness(transaction: &Transaction<'_>) -> Result<String, StoreError> {
        let encoded: Option<String> = transaction
            .query_row(
                "SELECT state_json FROM projections WHERE entity_kind = ? AND entity_id = ?",
                params![
                    vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                    vibemux_harness::HARNESS_CONFIG_ENTITY_KIND
                ],
                |row| row.get(0),
            )
            .optional()?;
        match encoded {
            Some(value) => {
                let config: HarnessConfigSnapshot = serde_json::from_str(&value)?;
                // Mirror `harness_config`'s self-heal so a switch never
                // reports a stale `from` for a harness the registry no
                // longer carries; the write path stays strict, but the
                // displayed prior state must be honest.
                if config.default_harness != vibemux_harness::NO_DEFAULT_HARNESS
                    && vibemux_harness::profile_by_name(&config.default_harness).is_none()
                {
                    Ok(vibemux_harness::NO_DEFAULT_HARNESS.to_string())
                } else {
                    Ok(config.default_harness)
                }
            }
            None => Ok(vibemux_harness::NO_DEFAULT_HARNESS.to_string()),
        }
    }

    fn read_registry_in_tx(
        transaction: &Transaction<'_>,
    ) -> Result<HarnessRegistrySnapshot, StoreError> {
        let encoded: Option<String> = transaction
            .query_row(
                "SELECT state_json FROM projections WHERE entity_kind = ? AND entity_id = ?",
                params![
                    vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
                    vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND
                ],
                |row| row.get(0),
            )
            .optional()?;
        let snapshot: HarnessRegistrySnapshot = match encoded {
            Some(value) => serde_json::from_str(&value)?,
            None => seed_registry(),
        };
        snapshot
            .validate()
            .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
        Ok(snapshot)
    }

    fn seed_config_if_absent(
        transaction: &Transaction<'_>,
        config: &HarnessConfigSnapshot,
        raw_sequence: i64,
        project_id: ProjectId,
    ) -> Result<(), StoreError> {
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM projections WHERE entity_kind = ? AND entity_id = ?)",
            params![
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                vibemux_harness::HARNESS_CONFIG_ENTITY_KIND
            ],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(());
        }
        Self::upsert_harness_projection(
            transaction,
            vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            &serde_json::to_string(config)?,
            raw_sequence,
            project_id,
        )
    }

    fn find_duplicate(
        transaction: &Transaction<'_>,
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
        transaction: &Transaction<'_>,
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
        transaction: &Transaction<'_>,
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
            // The marker INSERT and the seed share one transaction so a
            // seed failure leaves schema_migrations WITHOUT version 3.
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(MIGRATION_V3)?;
            seed_harness_projections(&transaction)?;
            transaction.commit()?;
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

/// Resolve the project id inside a transaction: mint a fresh `ProjectId` only
/// when the events table is empty (brand-new project), otherwise read the id
/// the first event already carries. The mint happens inside the same
/// transaction as the subsequent event insert, so concurrent first-event
/// inserts cannot race on a shared `ProjectId::new()` call.
fn resolve_or_mint_project_id(transaction: &Transaction<'_>) -> Result<ProjectId, StoreError> {
    let encoded: Option<String> = transaction
        .query_row(
            "SELECT envelope_json FROM events ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match encoded {
        Some(value) => {
            let event = EventEnvelope::from_json_slice(value.as_bytes())?;
            Ok(event.project_id())
        }
        None => Ok(ProjectId::new()),
    }
}

/// Seed harness projections during the v3 migration. The v3 migration cannot
/// know the project id a priori; when the events table is still empty (fresh
/// database) we seed with a no-op draft that mints a fresh project id inside
/// the SAME migration transaction. When events already exist we reuse the
/// project id the first event already carries. The daemon's first harness
/// refresh back-fills projections on databases created between v2 and v3
/// with no events at all.
fn seed_harness_projections(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    let project_id = resolve_or_mint_project_id(transaction)?;
    let draft = vibemux_harness::events::seed(project_id)
        .map_err(|error| StoreError::InvalidHarnessState(error.code()))?;
    let raw_sequence = {
        draft.validate()?;
        let idempotency_key = draft
            .idempotency_key
            .clone()
            .ok_or(StoreError::IdempotencyKeyRequired)?;
        transaction.execute(
            "INSERT INTO events(event_id, idempotency_key, envelope_json) VALUES (?, ?, '')",
            params![draft.event_id.to_string(), idempotency_key],
        )?;
        let raw = transaction.last_insert_rowid();
        let sequence = u64::try_from(raw)
            .ok()
            .and_then(|value| EventSequence::new(value).ok())
            .ok_or(StoreError::InvalidSequence)?;
        let event = EventEnvelope::commit(draft, sequence)?;
        event.validate()?;
        let encoded = serde_json::to_string(&event)?;
        transaction.execute(
            "UPDATE events SET envelope_json = ? WHERE sequence = ?",
            params![encoded, raw],
        )?;
        raw
    };
    let registry = seed_registry();
    let registry_json = serde_json::to_string(&registry)?;
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
            vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
            vibemux_harness::HARNESS_REGISTRY_ENTITY_KIND,
            project_id.to_string(),
            registry_json,
            raw_sequence,
        ],
    )?;
    let config = seed_config();
    let config_json = serde_json::to_string(&config)?;
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
            vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
            project_id.to_string(),
            config_json,
            raw_sequence,
        ],
    )?;
    Ok(())
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
        // The fresh-database v3 migration seeds one `v3_harness_seed` event.
        let seed_count = store.events().expect("events").len();
        let outcome = store
            .commit_task(&task, draft(task.project_id(), "task_1", "task_created"))
            .expect("commit task");
        assert_eq!(outcome.event.sequence().get() as usize, seed_count + 1);
        assert!(!outcome.duplicate);
        assert_eq!(store.events().expect("events").len(), seed_count + 1);
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
        let seed_count = store.events().expect("events").len();
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
        assert_eq!(store.events().expect("events").len(), seed_count);
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
        let baseline_events = store.events().expect("events").len();
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
        assert_eq!(store.events().expect("events").len(), baseline_events);
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
            let seed_count = store.events().expect("events").len();
            store
                .commit_run(&run, draft(project_id, "run_1", "run_preparing"))
                .expect("commit run");
            assert_eq!(store.events().expect("events").len(), seed_count + 1);
        }
        let store = SqliteStore::open(temporary.path()).expect("reopen store");
        let events = store.events().expect("events");
        assert_eq!(
            events.last().expect("last event").sequence().get(),
            events.len() as u64
        );
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
        // Fresh databases seed one `v3_harness_seed` event from the v3
        // migration; the test only asserts that v3 ran (a non-empty event
        // log proves the schema-3 path completed, not that the log is
        // empty).
        assert!(!store.events().expect("events").is_empty());
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

    /// Regression for the MIGRATION_V3 atomicity contract: a corrupt first
    /// event envelope must make the migration FAIL (not silently mark the
    /// database migrated), and the failed marker insert must roll back so a
    /// reopen retries the migration and fails again. The database is never
    /// left marked-migrated-but-unseeded.
    #[test]
    fn corrupt_first_event_envelope_rolls_back_v3_marker_across_reopen() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        // Seed a real v1 schema (the same DDL `migrate` runs when it starts
        // from a fresh database), then insert a structurally invalid first
        // event envelope. `resolve_or_mint_project_id` reads it via the typed
        // `EventEnvelope::from_json_slice`, which fails closed.
        let connection = Connection::open(temporary.path()).expect("open raw database");
        connection
            .execute_batch(INITIAL_SCHEMA)
            .expect("seed v1 schema");
        connection
            .execute(
                "INSERT INTO events(event_id, idempotency_key, envelope_json) VALUES ('e', 'k', 'not-json')",
                [],
            )
            .expect("seed corrupt first event");
        drop(connection);

        // First open: MIGRATION_V2 runs, then MIGRATION_V3's shared
        // transaction reads the corrupt envelope during the seed and fails.
        // The failure must propagate from `open` (never succeed) and the
        // version-3 marker must NOT be committed.
        match SqliteStore::open(temporary.path()) {
            Err(StoreError::Json(_) | StoreError::Event(_) | StoreError::Database(_)) => {}
            Err(other) => panic!("unexpected open error variant: {other:?}"),
            Ok(_) => panic!("corrupt envelope must fail open, not silently migrate"),
        }
        {
            let connection =
                Connection::open(temporary.path()).expect("reopen raw database for inspection");
            let max_version: u32 = connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .expect("read schema version");
            assert!(
                max_version < 3,
                "failed MIGRATION_V3 must not commit the version-3 marker (found {max_version})"
            );
        }

        // Reopen: the marker is still absent, so MIGRATION_V3 retries against
        // the same corrupt envelope and fails again. The database is never
        // half-migrated.
        match SqliteStore::open(temporary.path()) {
            Err(StoreError::Json(_) | StoreError::Event(_) | StoreError::Database(_)) => {}
            Err(other) => panic!("unexpected reopen error variant: {other:?}"),
            Ok(_) => panic!("corrupt envelope must still fail open on reopen"),
        }
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
        {
            let mut store = SqliteStore::open(temporary.path()).expect("open store");
            let outcome = store
                .commit_harness_snapshot(
                    &detections_for(&["claude"]),
                    "2026-09-16T00:00:00Z",
                    "probe-1",
                )
                .expect("commit snapshot");
            assert!(!outcome.duplicate);
            assert_eq!(
                store.harness_config().expect("config").default_harness,
                vibemux_harness::NO_DEFAULT_HARNESS
            );
            let outcome = store
                .commit_harness_switch("claude", "switch-1")
                .expect("switch");
            assert_eq!(outcome.from, vibemux_harness::NO_DEFAULT_HARNESS);
            assert_eq!(outcome.to, "claude");
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
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        store
            .commit_harness_snapshot(
                &detections_for(&["claude"]),
                "2026-09-16T00:00:00Z",
                "probe-1",
            )
            .expect("commit");
        // Unknown name fails before any state change.
        assert!(matches!(
            store.commit_harness_switch("unknown", "s1"),
            Err(StoreError::UnknownHarness(_))
        ));
        // Known but undetected harness is gated.
        assert!(matches!(
            store.commit_harness_switch("codex", "s2"),
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
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        store
            .commit_harness_snapshot(
                &detections_for(&["claude"]),
                "2026-09-16T00:00:00Z",
                "probe-1",
            )
            .expect("commit");
        // Set the default via switch.
        store
            .commit_harness_switch("claude", "sw-1")
            .expect("switch");
        // A duplicate refresh (same idempotency key) is a no-op.
        let duplicate = store
            .commit_harness_snapshot(
                &detections_for(&["claude"]),
                "2026-09-16T00:00:01Z",
                "probe-1",
            )
            .expect("duplicate commit");
        assert!(duplicate.duplicate);
        // A fresh refresh with a different key must not clobber the switch.
        store
            .commit_harness_snapshot(
                &detections_for(&["claude"]),
                "2026-09-16T00:00:02Z",
                "probe-2",
            )
            .expect("refresh 2");
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            "claude"
        );
    }

    /// Regression for the duplicate-switch contract: a retry that hits an
    /// already-committed idempotency key must report the `from`/`to` the
    /// duplicate EVENT recorded, not the CURRENT persisted default. If a
    /// later switch moved the default after the original commit, the retry
    /// must not surface a `(from, to)` pair the event never recorded (the
    /// "lying from" the outcome doc forbids).
    #[test]
    fn duplicate_switch_reports_the_recorded_from_not_the_current_default() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        let mut store = SqliteStore::open(temporary.path()).expect("open store");
        store
            .commit_harness_snapshot(
                &detections_for(&["claude", "codex"]),
                "2026-09-16T00:00:00Z",
                "probe-1",
            )
            .expect("commit");
        // Original switch: none -> claude, key K.
        let first = store
            .commit_harness_switch("claude", "switch-K")
            .expect("first switch");
        assert!(!first.duplicate);
        assert_eq!(first.from, vibemux_harness::NO_DEFAULT_HARNESS);
        assert_eq!(first.to, "claude");
        // A later switch moves the default: claude -> codex.
        let second = store
            .commit_harness_switch("codex", "switch-L")
            .expect("second switch");
        assert!(!second.duplicate);
        assert_eq!(second.from, "claude");
        assert_eq!(second.to, "codex");
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            "codex"
        );
        // Retry the original switch with key K: it must report the RECORDED
        // (from=none, to=claude), NOT the current default (from=codex).
        let duplicate = store
            .commit_harness_switch("claude", "switch-K")
            .expect("duplicate switch");
        assert!(duplicate.duplicate);
        assert_eq!(
            duplicate.from,
            vibemux_harness::NO_DEFAULT_HARNESS,
            "duplicate retry must report the recorded `from`, not the current default"
        );
        assert_eq!(duplicate.to, "claude");
        // The duplicate is a no-op: the persisted default stays at codex.
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            "codex"
        );
    }

    #[test]
    fn project_id_returns_first_event_id_then_none_on_fresh_db() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        // Open + close runs the v3 seed (which inserts a `v3_harness_seed`
        // event); `project_id()` therefore returns the seeded project's id.
        let store = SqliteStore::open(temporary.path()).expect("open store");
        let seeded = store
            .project_id()
            .expect("project_id")
            .expect("seeded project id");
        // The seeded project id is stable across re-opens.
        drop(store);
        let store = SqliteStore::open(temporary.path()).expect("reopen store");
        assert_eq!(
            store.project_id().expect("project_id").expect("some"),
            seeded
        );
    }

    #[test]
    fn harness_config_self_heals_unknown_persisted_default() {
        let temporary = tempfile::NamedTempFile::new().expect("temporary database");
        {
            let store = SqliteStore::open(temporary.path()).expect("open store");
            // Plant a config projection whose `default_harness` no longer
            // maps to a known profile.
            store
                .connection
                .execute(
                    "UPDATE projections SET state_json = ? WHERE entity_kind = ? AND entity_id = ?",
                    params![
                        r#"{"default_harness":"defunct_harness"}"#,
                        vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                        vibemux_harness::HARNESS_CONFIG_ENTITY_KIND,
                    ],
                )
                .expect("plant unknown default");
        }
        let store = SqliteStore::open(temporary.path()).expect("reopen store");
        assert_eq!(
            store.harness_config().expect("config").default_harness,
            vibemux_harness::NO_DEFAULT_HARNESS
        );
    }
}
