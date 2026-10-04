#![forbid(unsafe_code)]
//! Daemon-owned DSH session service (ADR 030 Stage 3).
//!
//! The daemon is the only component allowed to perform authoritative state
//! transitions, so it is also the right owner of a long-lived vendor session:
//! one live session per project, reached through the Control v6 operations.
//! This service holds that session and translates each operation into one
//! [`DshDriver`] call. It performs no framing, no containment, and no record
//! bookkeeping itself; those live in `vibemux_dsh`.
//!
//! Bounds are inherited, not re-invented. The driver owns the frame ceiling,
//! record ceiling, and deadlines; this layer owns the *control* frame budget,
//! because a poll page must fit the 64 KiB IPC frame even when the driver's
//! own page bound is larger.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use vibemux_dsh::{
    DSH_SESSION_CONFIG_FILE_NAME, DshDriver, DshDriverError, DshPoll, DshPollRecord,
    DshSessionConfig, DshSessionConfigSummary, load_session_config,
};
use vibemux_harness::dsh::{
    DshPhase, DshRecordKind, DshSessionLimits, DshSessionSettings, DshTurnOutcome,
};

/// Control frame budget for one poll page, below the payload ceiling.
pub const MAX_POLL_PAYLOAD_BYTES: usize = 48 * 1024;
/// Records one poll page may carry, below the driver's own page bound.
pub const MAX_POLL_RECORDS: usize = 256;

/// Stable, machine-readable service failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DshServiceError {
    #[error("no DSH session is configured for this project")]
    Unconfigured,
    #[error("the DSH session configuration is invalid or untrusted")]
    ConfigInvalid,
    #[error("a DSH session is already live for this project")]
    Busy,
    #[error("no live DSH session")]
    NotFound,
    #[error("the DSH session refused the request in its current state")]
    Refused,
    #[error("the DSH prompt is empty or exceeds its bound")]
    PromptInvalid,
    #[error("one DSH record cannot fit the control frame")]
    RecordTooLarge,
    #[error("the DSH session settings are invalid")]
    SettingsInvalid,
}

impl DshServiceError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unconfigured => "dsh_session_unconfigured",
            Self::ConfigInvalid => "dsh_session_config_invalid",
            Self::Busy => "dsh_session_busy",
            Self::NotFound => "dsh_session_not_found",
            Self::Refused => "dsh_session_refused",
            Self::PromptInvalid => "dsh_session_prompt_invalid",
            Self::RecordTooLarge => "dsh_session_record_too_large",
            Self::SettingsInvalid => "dsh_session_settings_invalid",
        }
    }

    /// Maps a driver failure onto the service vocabulary, preserving the
    /// driver's own code for operational failures so an operator sees the
    /// real cause (spawn, timeout, transport, trust) rather than a generic
    /// refusal.
    #[must_use]
    pub const fn from_driver(error: &DshDriverError) -> Self {
        match error {
            DshDriverError::Unconfigured => Self::Unconfigured,
            DshDriverError::ConfigInvalid
            | DshDriverError::ExecutableUntrusted
            | DshDriverError::EntryScriptUntrusted => Self::ConfigInvalid,
            DshDriverError::InvalidState | DshDriverError::StdinUnavailable => Self::Refused,
            _ => Self::Refused,
        }
    }
}

/// One polled record, owned so the page survives the session lock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionRecord {
    pub sequence: u64,
    pub kind: DshRecordKind,
    /// Byte-exact runtime line. The poll operation is the only DSH command
    /// that returns vendor content.
    pub raw_json: String,
}

/// Content-free view of the live session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionStatus {
    pub session_id: String,
    pub phase: DshPhase,
    pub provider: String,
    pub model: String,
    pub server_version: Option<String>,
    pub turns_started: u64,
    pub completed_turns: u64,
    pub failed_turns: u64,
    pub cancelled_turns: u64,
    pub unverified_turns: u64,
    pub last_outcome: Option<DshTurnOutcome>,
    pub records: u64,
    pub record_bytes: u64,
    /// Records the runtime produced for other sessions in the same process.
    pub foreign_notifications: u64,
    /// Whether the runtime's stdout is still open.
    pub transport_open: bool,
    /// Provider prompt bytes sent so far; never the prompt text.
    pub prompt_bytes_total: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub config: DshSessionConfigSummary,
}

/// Answer to a poll: records after the caller's cursor, plus the status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionPollPage {
    pub records: Vec<DshSessionRecord>,
    /// Cursor to pass to the next poll.
    pub next: u64,
    /// More records are already available.
    pub more: bool,
    /// The caller's cursor predated the retained window.
    pub truncated: bool,
    pub status: DshSessionStatus,
}

/// Argument of the open operation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionOpenRequest {
    /// Operator-chosen SDK session id. Absent means the daemon allocates one.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Argument of the prompt operation.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionPromptRequest {
    pub prompt: String,
}

impl std::fmt::Debug for DshSessionPromptRequest {
    /// Redacted: a prompt never reaches `Debug`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DshSessionPromptRequest")
            .field("prompt_bytes", &self.prompt.len())
            .finish()
    }
}

/// Argument of the poll operation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionPollQuery {
    #[serde(default)]
    pub cursor: u64,
}

struct LiveSession {
    status: DshSessionStatus,
    driver: DshDriver,
}

/// One live DSH session per project, or none.
pub struct DshSessionService {
    project_root: PathBuf,
    config_path: PathBuf,
    /// Deliberately a Tokio mutex: every critical section awaits one driver
    /// call, so a `std` mutex would violate the no-blocking-across-await rule.
    /// The critical sections are bounded by the driver's own deadlines, and at
    /// most one session exists per project, so contention is a single waiter.
    session: Mutex<Option<LiveSession>>,
}

impl std::fmt::Debug for DshSessionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DshSessionService")
            .field("config_path_bytes", &self.config_path.as_os_str().len())
            .finish_non_exhaustive()
    }
}

impl DshSessionService {
    /// Builds the service for one project. Nothing is read until `open`, so a
    /// project without a DSH configuration costs nothing.
    #[must_use]
    pub fn new(project_root: &Path, state_dir: &Path) -> Self {
        let project_root =
            std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.into());
        Self {
            project_root,
            config_path: state_dir.join(DSH_SESSION_CONFIG_FILE_NAME),
            session: Mutex::new(None),
        }
    }

    /// The configuration path this service reads, for diagnostics only.
    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Reports why there is no live session. An unconfigured project and an
    /// idle configured one are different operator problems, so the caller
    /// learns which it is instead of a single opaque "not found".
    fn no_session(&self) -> DshServiceError {
        match self.load_config() {
            Ok(_) => DshServiceError::NotFound,
            Err(error) => error,
        }
    }

    /// Loads and verifies the operator configuration. Absent means DSH is off.
    fn load_config(&self) -> Result<DshSessionConfig, DshServiceError> {
        load_session_config(&self.config_path, &self.project_root)
            .map_err(|error| DshServiceError::from_driver(&error))?
            .ok_or(DshServiceError::Unconfigured)
    }

    /// Opens a session. Refused while another live session exists, so one
    /// project cannot accumulate vendor processes (ADR 030 Decision 10).
    pub async fn open(
        &self,
        request: &DshSessionOpenRequest,
    ) -> Result<DshSessionStatus, DshServiceError> {
        let config = self.load_config()?;
        let session_id = match &request.session_id {
            Some(session_id) => session_id.clone(),
            None => format!("vibemux_{}", uuid::Uuid::new_v4().simple()),
        };
        let settings: DshSessionSettings = config
            .settings(session_id, &self.project_root)
            .map_err(|_| DshServiceError::SettingsInvalid)?;
        let limits: DshSessionLimits = config.limits;
        let launch = config
            .launch(&self.project_root)
            .map_err(|error| DshServiceError::from_driver(&error))?;
        let summary = DshSessionConfigSummary::from(&config);

        let mut guard = self.session.lock().await;
        if let Some(live) = guard.as_ref() {
            if !live.driver.phase().is_terminal() {
                return Err(DshServiceError::Busy);
            }
            // A finished session is replaced, but its child must be reaped
            // first; otherwise a restart loop would leak processes.
            if let Some(finished) = guard.take() {
                let mut driver = finished.driver;
                let _ = driver.close().await;
            }
        }

        let mut driver = DshDriver::spawn(&launch, settings, limits)
            .await
            .map_err(|error| DshServiceError::from_driver(&error))?;
        if let Err(error) = driver.await_ready().await {
            // A failed handshake still owns a child; reap it before returning.
            let _ = driver.close().await;
            return Err(DshServiceError::from_driver(&error));
        }
        let status = status_of(&driver, summary, OffsetDateTime::now_utc());
        guard.replace(LiveSession {
            status: status.clone(),
            driver,
        });
        Ok(status)
    }

    /// Sends one prompt, refusing while the session is not ready.
    pub async fn prompt(
        &self,
        request: &DshSessionPromptRequest,
    ) -> Result<DshSessionStatus, DshServiceError> {
        if request.prompt.trim().is_empty() {
            return Err(DshServiceError::PromptInvalid);
        }
        let mut guard = self.session.lock().await;
        let Some(live) = guard.as_mut() else {
            return Err(self.no_session());
        };
        live.driver
            .prompt(&request.prompt)
            .await
            .map_err(|error| DshServiceError::from_driver(&error))?;
        live.status = refresh(&live.driver, &live.status);
        Ok(live.status.clone())
    }

    /// Reads records after `cursor`, sized to fit one control frame.
    pub async fn poll(
        &self,
        query: DshSessionPollQuery,
    ) -> Result<DshSessionPollPage, DshServiceError> {
        let mut guard = self.session.lock().await;
        let Some(live) = guard.as_mut() else {
            return Err(self.no_session());
        };
        let poll = live
            .driver
            .poll(query.cursor)
            .map_err(|_| DshServiceError::Refused)?;
        live.status = refresh(&live.driver, &live.status);
        let status = live.status.clone();
        fit_page(poll, status)
    }

    /// The content-free status, without reading records.
    pub async fn status(&self) -> Result<DshSessionStatus, DshServiceError> {
        let mut guard = self.session.lock().await;
        let Some(live) = guard.as_mut() else {
            return Err(self.no_session());
        };
        live.status = refresh(&live.driver, &live.status);
        Ok(live.status.clone())
    }

    /// Cancels the turn by terminating the contained tree. The session stays
    /// addressable so its terminal status can still be read.
    pub async fn cancel(&self) -> Result<DshSessionStatus, DshServiceError> {
        let mut guard = self.session.lock().await;
        let Some(live) = guard.as_mut() else {
            return Err(self.no_session());
        };
        live.driver
            .cancel()
            .await
            .map_err(|error| DshServiceError::from_driver(&error))?;
        live.status = refresh(&live.driver, &live.status);
        Ok(live.status.clone())
    }

    /// Closes the session, reaping the runtime, and returns its final status.
    pub async fn close(&self) -> Result<DshSessionStatus, DshServiceError> {
        let mut guard = self.session.lock().await;
        let Some(mut live) = guard.take() else {
            return Err(self.no_session());
        };
        // `close` reports a timeout when a cooperative shutdown was ignored;
        // the teardown still happened, so the final status is what matters.
        let _ = live.driver.close().await;
        live.status = refresh(&live.driver, &live.status);
        Ok(live.status)
    }

    /// Closes any live session. Called on daemon shutdown so no vendor process
    /// outlives the daemon.
    pub async fn shutdown(&self) {
        let mut guard = self.session.lock().await;
        if let Some(mut live) = guard.take() {
            let _ = live.driver.close().await;
        }
    }
}

/// Copies live counters onto a status snapshot.
fn refresh(driver: &DshDriver, previous: &DshSessionStatus) -> DshSessionStatus {
    let mut status = previous.clone();
    apply_summary(driver, &mut status);
    status
}

fn apply_summary(driver: &DshDriver, status: &mut DshSessionStatus) {
    let summary = driver.summary();
    status.phase = summary.phase;
    status.provider = summary.provider;
    status.model = summary.model;
    status.server_version = summary.server_version;
    status.turns_started = summary.turns_started;
    status.completed_turns = summary.completed_turns;
    status.failed_turns = summary.failed_turns;
    status.cancelled_turns = summary.cancelled_turns;
    status.unverified_turns = summary.unverified_turns;
    status.last_outcome = summary.last_outcome;
    status.records = summary.records;
    status.record_bytes = summary.record_bytes;
    status.foreign_notifications = summary.foreign_notifications;
    status.prompt_bytes_total = summary.prompt_bytes_total;
    status.transport_open = driver.transport_open();
}

fn status_of(
    driver: &DshDriver,
    config: DshSessionConfigSummary,
    created_at: OffsetDateTime,
) -> DshSessionStatus {
    let mut status = DshSessionStatus {
        session_id: driver.summary().session_id,
        phase: driver.phase(),
        provider: String::new(),
        model: String::new(),
        server_version: None,
        turns_started: 0,
        completed_turns: 0,
        failed_turns: 0,
        cancelled_turns: 0,
        unverified_turns: 0,
        last_outcome: None,
        records: 0,
        record_bytes: 0,
        foreign_notifications: 0,
        transport_open: driver.transport_open(),
        prompt_bytes_total: 0,
        created_at,
        config,
    };
    apply_summary(driver, &mut status);
    status
}

/// Trims a driver page until it fits the control frame. A single record that
/// cannot fit is refused rather than silently dropped, because a caller
/// polling by cursor would otherwise skip it forever.
fn fit_page(
    poll: DshPoll,
    status: DshSessionStatus,
) -> Result<DshSessionPollPage, DshServiceError> {
    let mut records: Vec<DshSessionRecord> = poll
        .records
        .into_iter()
        .map(|record: DshPollRecord| DshSessionRecord {
            sequence: record.sequence,
            kind: record.kind,
            raw_json: record.raw_json,
        })
        .collect();
    let mut more = poll.more;
    if records.len() > MAX_POLL_RECORDS {
        records.truncate(MAX_POLL_RECORDS);
        more = true;
    }
    loop {
        let next = records.last().map_or(poll.next, |record| record.sequence);
        let page = DshSessionPollPage {
            records,
            next,
            more,
            truncated: poll.truncated,
            status: status.clone(),
        };
        let encoded = serde_json::to_vec(&page).map_err(|_| DshServiceError::RecordTooLarge)?;
        if encoded.len() <= MAX_POLL_PAYLOAD_BYTES || page.records.is_empty() {
            return Ok(page);
        }
        if page.records.len() == 1 {
            return Err(DshServiceError::RecordTooLarge);
        }
        records = page.records;
        records.pop();
        more = true;
    }
}
