//! Daemon-owned plugin lifetime management with no canonical-state access.
//!
//! Status snapshots are ephemeral, bounded, and read-only. Registration is an
//! in-process daemon operation; it is deliberately not exposed through IPC.

use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, sleep},
};
use vibemux_plugin_protocol::wire::envelope;
use vibemux_plugin_supervisor::{
    PluginSession, PluginSupervisorConfig, PluginSupervisorError, spawn_plugin,
};

pub const DEFAULT_MAX_PLUGINS: usize = 16;
pub const HARD_MAX_PLUGINS: usize = 32;
pub const DEFAULT_MAX_RESTARTS: u32 = 3;
pub const HARD_MAX_RESTARTS: u32 = 16;
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(5);
pub const HARD_MAX_BACKOFF: Duration = Duration::from_secs(60);
pub const HARD_MAX_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
pub const HARD_MAX_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginRegistryConfig {
    pub max_plugins: usize,
}

impl Default for PluginRegistryConfig {
    fn default() -> Self {
        Self {
            max_plugins: DEFAULT_MAX_PLUGINS,
        }
    }
}

impl PluginRegistryConfig {
    pub fn validate(&self) -> Result<(), PluginRegistryError> {
        if !(1..=HARD_MAX_PLUGINS).contains(&self.max_plugins) {
            return Err(PluginRegistryError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Lifetime restart budget. Successful handshakes never reset consumed budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginRestartPolicy {
    /// Additional launches after the initial attempt; zero disables restarts.
    pub max_restarts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for PluginRestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: DEFAULT_MAX_RESTARTS,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }
}

impl PluginRestartPolicy {
    pub fn validate(&self) -> Result<(), PluginRegistryError> {
        if self.max_restarts > HARD_MAX_RESTARTS
            || self.initial_backoff.is_zero()
            || self.initial_backoff > self.max_backoff
            || self.max_backoff > HARD_MAX_BACKOFF
        {
            return Err(PluginRegistryError::InvalidConfiguration);
        }
        Ok(())
    }

    fn backoff(&self, completed_restarts: u32) -> Duration {
        let factor = 1_u32.checked_shl(completed_restarts).unwrap_or(u32::MAX);
        self.initial_backoff
            .saturating_mul(factor)
            .min(self.max_backoff)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Starting,
    Active,
    Backoff,
    Quarantined,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginStatus {
    pub plugin_id: String,
    pub state: PluginState,
    pub restarts: u32,
    pub max_restarts: u32,
    pub session_id: Option<String>,
    pub process_id: Option<u32>,
    pub last_error_code: Option<String>,
    pub graceful_shutdown: Option<bool>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PluginRegistryError {
    #[error("plugin registry configuration is invalid")]
    InvalidConfiguration,
    #[error("plugin registration is invalid: {code}")]
    InvalidRegistration { code: String },
    #[error("plugin registry capacity is exhausted")]
    CapacityExceeded,
    #[error("plugin identity is already registered")]
    DuplicatePlugin,
    #[error("plugin identity is not registered")]
    NotRegistered,
    #[error("plugin registry is stopped")]
    RegistryStopped,
    #[error("plugin registry requires an async runtime")]
    RuntimeUnavailable,
    #[error("plugin registry worker failed")]
    WorkerFailed,
    #[error("plugin cleanup failed: {code}")]
    CleanupFailed { code: String },
}

impl PluginRegistryError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "plugin_registry_invalid_configuration",
            Self::InvalidRegistration { .. } => "plugin_registry_invalid_registration",
            Self::CapacityExceeded => "plugin_registry_capacity_exceeded",
            Self::DuplicatePlugin => "plugin_registry_duplicate_plugin",
            Self::NotRegistered => "plugin_registry_not_registered",
            Self::RegistryStopped => "plugin_registry_stopped",
            Self::RuntimeUnavailable => "plugin_registry_runtime_unavailable",
            Self::WorkerFailed => "plugin_registry_worker_failed",
            Self::CleanupFailed { .. } => "plugin_registry_cleanup_failed",
        }
    }
}

/// This handle can observe status only; it cannot cancel or register plugins.
#[derive(Clone)]
pub struct PluginStatusReader {
    entries: watch::Receiver<Vec<watch::Receiver<PluginStatus>>>,
}

impl PluginStatusReader {
    #[must_use]
    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.entries
            .borrow()
            .iter()
            .map(|entry| entry.borrow().clone())
            .collect()
    }
}

struct RegistryEntry {
    cancellation: watch::Sender<bool>,
    status: watch::Receiver<PluginStatus>,
    status_updates: watch::Sender<PluginStatus>,
    worker: Option<JoinHandle<Result<(), PluginRegistryError>>>,
}

/// One daemon owns this object and explicitly awaits `shutdown` before releasing
/// its writer/runtime. Dropping it is only an emergency abort fallback.
pub struct PluginRegistry {
    config: PluginRegistryConfig,
    entries: BTreeMap<String, RegistryEntry>,
    snapshots: watch::Sender<Vec<watch::Receiver<PluginStatus>>>,
    stopped: bool,
}

impl PluginRegistry {
    pub fn new(config: PluginRegistryConfig) -> Result<Self, PluginRegistryError> {
        config.validate()?;
        let (snapshots, _) = watch::channel(Vec::new());
        Ok(Self {
            config,
            entries: BTreeMap::new(),
            snapshots,
            stopped: false,
        })
    }

    /// Validate every daemon startup registration before launching any plugin.
    pub fn validate_registration(
        config: &PluginSupervisorConfig,
        policy: &PluginRestartPolicy,
    ) -> Result<(), PluginRegistryError> {
        policy.validate()?;
        config
            .validate()
            .map_err(|error| PluginRegistryError::InvalidRegistration {
                code: error.code().to_string(),
            })?;
        if config.handshake_timeout > HARD_MAX_LIFECYCLE_TIMEOUT
            || config.receive_timeout > HARD_MAX_SESSION_TIMEOUT
            || config.shutdown_timeout > HARD_MAX_LIFECYCLE_TIMEOUT
            || config.receive_timeout
                <= Duration::from_millis(config.policy.heartbeat_interval_ms())
        {
            return Err(PluginRegistryError::InvalidConfiguration);
        }
        Ok(())
    }

    pub fn register(
        &mut self,
        config: PluginSupervisorConfig,
        policy: PluginRestartPolicy,
    ) -> Result<(), PluginRegistryError> {
        if self.stopped {
            return Err(PluginRegistryError::RegistryStopped);
        }
        if self.entries.contains_key(&config.manifest.plugin_id) {
            return Err(PluginRegistryError::DuplicatePlugin);
        }
        if self.entries.len() >= self.config.max_plugins {
            return Err(PluginRegistryError::CapacityExceeded);
        }
        Self::validate_registration(&config, &policy)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| PluginRegistryError::RuntimeUnavailable)?;
        let plugin_id = config.manifest.plugin_id.clone();
        let initial = PluginStatus {
            plugin_id: plugin_id.clone(),
            state: PluginState::Starting,
            restarts: 0,
            max_restarts: policy.max_restarts,
            session_id: None,
            process_id: None,
            last_error_code: None,
            graceful_shutdown: None,
        };
        let (status_sender, status) = watch::channel(initial);
        let (cancellation, cancel_receiver) = watch::channel(false);
        let worker = runtime.spawn(plugin_worker(
            config,
            policy,
            cancel_receiver,
            status_sender.clone(),
        ));
        self.entries.insert(
            plugin_id,
            RegistryEntry {
                cancellation,
                status,
                status_updates: status_sender,
                worker: Some(worker),
            },
        );
        self.snapshots.send_replace(
            self.entries
                .values()
                .map(|entry| entry.status.clone())
                .collect(),
        );
        Ok(())
    }

    #[must_use]
    pub fn status_reader(&self) -> PluginStatusReader {
        PluginStatusReader {
            entries: self.snapshots.subscribe(),
        }
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.status_reader().statuses()
    }

    /// Cancels a plugin lifetime, not a Task/Run/request. No IPC exposes this.
    /// Terminal entries remain visible and cannot be silently re-registered.
    pub async fn cancel(&mut self, plugin_id: &str) -> Result<(), PluginRegistryError> {
        let entry = self
            .entries
            .get_mut(plugin_id)
            .ok_or(PluginRegistryError::NotRegistered)?;
        entry.cancellation.send_replace(true);
        join_entry(entry).await
    }

    /// Signal all lifetimes first, then join each; cleanup continues after errors.
    pub async fn shutdown(&mut self) -> Result<(), PluginRegistryError> {
        self.stopped = true;
        for entry in self.entries.values() {
            entry.cancellation.send_replace(true);
        }
        let mut first_error = None;
        for entry in self.entries.values_mut() {
            if let Err(error) = join_entry(entry).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for PluginRegistry {
    fn drop(&mut self) {
        for entry in self.entries.values_mut() {
            entry.cancellation.send_replace(true);
            if let Some(worker) = entry.worker.take() {
                worker.abort();
            }
        }
    }
}

async fn join_entry(entry: &mut RegistryEntry) -> Result<(), PluginRegistryError> {
    // Keep the handle in the entry while awaiting: if this caller is cancelled,
    // a later shutdown still owns and joins the same worker.
    let Some(worker) = entry.worker.as_mut() else {
        return Ok(());
    };
    let result = worker.await;
    entry.worker.take();
    match result {
        Ok(result) => result,
        Err(_) => {
            mark_unconfirmed(
                &entry.status_updates,
                PluginRegistryError::WorkerFailed.code(),
            );
            Err(PluginRegistryError::WorkerFailed)
        }
    }
}

async fn plugin_worker(
    mut config: PluginSupervisorConfig,
    policy: PluginRestartPolicy,
    mut cancellation: watch::Receiver<bool>,
    status: watch::Sender<PluginStatus>,
) -> Result<(), PluginRegistryError> {
    let mut restarts = 0;
    loop {
        if is_cancelled(&cancellation) {
            mark_stopped(&status, None);
            return Ok(());
        }
        config.session_id = uuid::Uuid::new_v4().simple().to_string();
        status.send_modify(|snapshot| {
            snapshot.state = PluginState::Starting;
            snapshot.session_id = Some(config.session_id.clone());
            snapshot.process_id = None;
            snapshot.restarts = restarts;
        });
        // Do not drop this future on cancellation: the supervisor owns its child
        // and handshake I/O tasks until its validated handshake deadline.
        let failure = match spawn_plugin(config.clone()).await {
            Ok(mut session) => {
                status.send_modify(|snapshot| {
                    snapshot.state = PluginState::Active;
                    snapshot.process_id = session.process_id();
                });
                match monitor_session(&mut session, &mut cancellation, config.receive_timeout).await
                {
                    SessionOutcome::Cancelled => return stop_session(session, &status).await,
                    SessionOutcome::Failed(failure) => {
                        status.send_modify(|snapshot| {
                            snapshot.state = PluginState::Stopping;
                            snapshot.last_error_code = Some(failure.code.clone());
                        });
                        if let Err(error) = session.terminate().await {
                            mark_unconfirmed(&status, error.code());
                            return Err(PluginRegistryError::CleanupFailed {
                                code: error.code().to_string(),
                            });
                        }
                        failure
                    }
                }
            }
            Err(error) => SessionFailure::from_supervisor(error),
        };
        status.send_modify(|snapshot| {
            snapshot.process_id = None;
            snapshot.last_error_code = Some(failure.code.clone());
        });
        if is_cancelled(&cancellation) {
            mark_stopped(&status, None);
            return Ok(());
        }
        if !failure.retryable || restarts >= policy.max_restarts {
            status.send_modify(|snapshot| snapshot.state = PluginState::Quarantined);
            return Ok(());
        }
        status.send_modify(|snapshot| snapshot.state = PluginState::Backoff);
        tokio::select! {
            biased;
            _ = wait_for_cancellation(&mut cancellation) => {
                mark_stopped(&status, None);
                return Ok(());
            }
            _ = sleep(policy.backoff(restarts)) => {}
        }
        // The hard limit makes this increment safe and bounds total launches.
        restarts += 1;
    }
}

struct SessionFailure {
    code: String,
    retryable: bool,
}

impl SessionFailure {
    fn from_supervisor(error: PluginSupervisorError) -> Self {
        let retryable = matches!(
            error,
            PluginSupervisorError::SpawnFailed
                | PluginSupervisorError::HandshakeTimeout
                | PluginSupervisorError::QueueClosed
                | PluginSupervisorError::ReceiveTimeout
                | PluginSupervisorError::SessionClosed
        ) || matches!(&error, PluginSupervisorError::Protocol { code } if code == "plugin_frame_io");
        Self {
            code: error.code().to_string(),
            retryable,
        }
    }

    fn permanent(code: &'static str) -> Self {
        Self {
            code: code.to_string(),
            retryable: false,
        }
    }
}

enum SessionOutcome {
    Cancelled,
    Failed(SessionFailure),
}

async fn monitor_session(
    session: &mut PluginSession,
    cancellation: &mut watch::Receiver<bool>,
    heartbeat_timeout: Duration,
) -> SessionOutcome {
    let mut heartbeat_deadline = Instant::now() + heartbeat_timeout;
    loop {
        let remaining = heartbeat_deadline.saturating_duration_since(Instant::now());
        let envelope = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancellation) => return SessionOutcome::Cancelled,
            received = session.receive_with_timeout(remaining) => received,
        };
        match envelope {
            Ok(envelope) => match envelope.body {
                Some(envelope::Body::Heartbeat(_)) => {
                    heartbeat_deadline = Instant::now() + heartbeat_timeout
                }
                Some(envelope::Body::ProtocolError(_)) => {
                    return SessionOutcome::Failed(SessionFailure::permanent(
                        "plugin_registry_peer_error",
                    ));
                }
                _ => {
                    return SessionOutcome::Failed(SessionFailure::permanent(
                        "plugin_registry_unsupported_message",
                    ));
                }
            },
            Err(error) => return SessionOutcome::Failed(SessionFailure::from_supervisor(error)),
        }
    }
}

async fn stop_session(
    session: PluginSession,
    status: &watch::Sender<PluginStatus>,
) -> Result<(), PluginRegistryError> {
    status.send_modify(|snapshot| snapshot.state = PluginState::Stopping);
    match session.shutdown().await {
        Ok(report) => {
            mark_stopped(status, Some(report.graceful));
            Ok(())
        }
        Err(error) => {
            if matches!(
                error,
                PluginSupervisorError::ChildStatusUnavailable
                    | PluginSupervisorError::SessionClosed
            ) {
                mark_unconfirmed(status, error.code());
            } else {
                status.send_modify(|snapshot| {
                    snapshot.last_error_code = Some(error.code().to_string())
                });
                mark_stopped(status, Some(false));
            }
            Err(PluginRegistryError::CleanupFailed {
                code: error.code().to_string(),
            })
        }
    }
}

/// Failure to join/reap must retain the last observed PID as ownership evidence.
/// It is not permission to terminate a process later by this numeric PID.
fn mark_unconfirmed(status: &watch::Sender<PluginStatus>, code: &str) {
    status.send_modify(|snapshot| {
        snapshot.state = PluginState::Quarantined;
        snapshot.last_error_code = Some(code.to_string());
        snapshot.graceful_shutdown = Some(false);
    });
}
fn mark_stopped(status: &watch::Sender<PluginStatus>, graceful: Option<bool>) {
    status.send_modify(|snapshot| {
        snapshot.state = PluginState::Stopped;
        snapshot.process_id = None;
        snapshot.graceful_shutdown = graceful;
    });
}

fn is_cancelled(cancellation: &watch::Receiver<bool>) -> bool {
    *cancellation.borrow() || cancellation.has_changed().is_err()
}

async fn wait_for_cancellation(cancellation: &mut watch::Receiver<bool>) {
    loop {
        if is_cancelled(cancellation) || cancellation.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed_status() -> PluginStatus {
        PluginStatus {
            plugin_id: "mock.harness".to_string(),
            state: PluginState::Active,
            restarts: 1,
            max_restarts: 2,
            session_id: Some("observed_session".to_string()),
            process_id: Some(1234),
            last_error_code: None,
            graceful_shutdown: None,
        }
    }

    #[test]
    fn unconfirmed_cleanup_quarantines_without_erasing_ownership_evidence() {
        let (updates, status) = watch::channel(observed_status());
        mark_unconfirmed(&updates, "plugin_supervisor_child_status_unavailable");
        let snapshot = status.borrow().clone();
        assert_eq!(snapshot.state, PluginState::Quarantined);
        assert_eq!(snapshot.process_id, Some(1234));
        assert_eq!(snapshot.session_id.as_deref(), Some("observed_session"));
        assert_eq!(snapshot.restarts, 1);
        assert_eq!(snapshot.graceful_shutdown, Some(false));
        assert_eq!(
            snapshot.last_error_code.as_deref(),
            Some("plugin_supervisor_child_status_unavailable")
        );
    }

    #[tokio::test]
    async fn cancelled_join_retains_worker_and_failed_join_quarantines() {
        let (status_updates, status) = watch::channel(observed_status());
        let (cancellation, mut receiver) = watch::channel(false);
        let worker = tokio::spawn(async move {
            wait_for_cancellation(&mut receiver).await;
            Ok(())
        });
        let mut entry = RegistryEntry {
            cancellation,
            status,
            status_updates,
            worker: Some(worker),
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(10), join_entry(&mut entry))
                .await
                .is_err()
        );
        assert!(
            entry.worker.is_some(),
            "cancelled caller retains the join handle"
        );
        entry.cancellation.send_replace(true);
        join_entry(&mut entry)
            .await
            .expect("retained worker joined");
        assert!(entry.worker.is_none());

        // Simulate an internally failed worker without creating a real process.
        let failed = tokio::spawn(std::future::pending::<Result<(), PluginRegistryError>>());
        failed.abort();
        entry.worker = Some(failed);
        assert_eq!(
            join_entry(&mut entry).await,
            Err(PluginRegistryError::WorkerFailed)
        );
        assert!(entry.worker.is_none());
        let snapshot = entry.status.borrow().clone();
        assert_eq!(snapshot.state, PluginState::Quarantined);
        assert_eq!(snapshot.process_id, Some(1234));
        assert_eq!(
            snapshot.last_error_code.as_deref(),
            Some("plugin_registry_worker_failed")
        );
        join_entry(&mut entry)
            .await
            .expect("completed handle is not polled twice");
    }
    #[test]
    fn restart_backoff_is_exponential_capped_and_overflow_safe() {
        let policy = PluginRestartPolicy {
            max_restarts: HARD_MAX_RESTARTS,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(35),
        };
        policy.validate().expect("valid policy");
        assert_eq!(policy.backoff(0), Duration::from_millis(10));
        assert_eq!(policy.backoff(1), Duration::from_millis(20));
        assert_eq!(policy.backoff(2), Duration::from_millis(35));
        assert_eq!(policy.backoff(u32::MAX), Duration::from_millis(35));
    }

    #[test]
    fn invalid_bounds_are_rejected_without_starting_any_worker() {
        for max_plugins in [0, HARD_MAX_PLUGINS + 1, usize::MAX] {
            assert!(matches!(
                PluginRegistry::new(PluginRegistryConfig { max_plugins }),
                Err(PluginRegistryError::InvalidConfiguration)
            ));
        }
        for policy in [
            PluginRestartPolicy {
                max_restarts: HARD_MAX_RESTARTS + 1,
                ..PluginRestartPolicy::default()
            },
            PluginRestartPolicy {
                initial_backoff: Duration::ZERO,
                ..PluginRestartPolicy::default()
            },
            PluginRestartPolicy {
                initial_backoff: Duration::from_secs(6),
                max_backoff: Duration::from_secs(5),
                ..PluginRestartPolicy::default()
            },
            PluginRestartPolicy {
                max_backoff: HARD_MAX_BACKOFF + Duration::from_secs(1),
                ..PluginRestartPolicy::default()
            },
        ] {
            assert_eq!(
                policy.validate(),
                Err(PluginRegistryError::InvalidConfiguration)
            );
        }
    }

    #[test]
    fn only_transient_transport_and_process_failures_are_retryable() {
        for code in [
            "plugin_frame_too_large",
            "plugin_decode_failed",
            "plugin_invalid_transition",
            "plugin_negotiation_rejected",
            "plugin_heartbeat_not_monotonic",
        ] {
            assert!(
                !SessionFailure::from_supervisor(PluginSupervisorError::Protocol {
                    code: code.to_string()
                })
                .retryable
            );
        }
        assert!(
            SessionFailure::from_supervisor(PluginSupervisorError::Protocol {
                code: "plugin_frame_io".to_string()
            })
            .retryable
        );
        assert!(SessionFailure::from_supervisor(PluginSupervisorError::HandshakeTimeout).retryable);
        assert!(
            !SessionFailure::from_supervisor(PluginSupervisorError::InvalidConfiguration).retryable
        );
    }

    #[tokio::test]
    async fn empty_shutdown_is_idempotent_and_reader_outlives_registry() {
        let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
        let reader = registry.status_reader();
        registry.shutdown().await.expect("shutdown");
        registry.shutdown().await.expect("repeat shutdown");
        assert_eq!(
            registry.cancel("unknown").await,
            Err(PluginRegistryError::NotRegistered)
        );
        drop(registry);
        assert!(reader.statuses().is_empty());
    }
}
