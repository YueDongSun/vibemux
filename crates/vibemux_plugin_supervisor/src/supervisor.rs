use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout},
};
use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{read_envelope, write_envelope},
    lifecycle::{CoreSessionLifecycle, CoreSessionPhase},
    manifest::PluginManifest,
    negotiation::{CorePluginPolicy, negotiate_hello},
    wire::{Cancel, Drain, Envelope, Heartbeat, Shutdown, envelope, validate_envelope},
};

use crate::{PluginExitReport, PluginStderrReport, PluginSupervisorError, ResolvedPluginLaunch};

pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;
pub const MAX_QUEUE_CAPACITY: usize = 4096;
pub const DEFAULT_STDERR_LIMIT_BYTES: usize = 64 * 1024;
pub const MAX_STDERR_LIMIT_BYTES: usize = 1024 * 1024;
const SESSION_ENVIRONMENT_KEY: &str = "VIBEMUX_PLUGIN_SESSION_ID";

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
pub struct PluginSupervisorConfig {
    pub manifest: PluginManifest,
    pub launch: ResolvedPluginLaunch,
    pub policy: CorePluginPolicy,
    pub session_id: String,
    pub handshake_timeout: Duration,
    pub receive_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub outbound_capacity: usize,
    pub inbound_capacity: usize,
    pub stderr_limit_bytes: usize,
}

impl PluginSupervisorConfig {
    pub fn new(
        manifest: PluginManifest,
        launch: ResolvedPluginLaunch,
        policy: CorePluginPolicy,
        session_id: String,
    ) -> Self {
        Self {
            manifest,
            launch,
            policy,
            session_id,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            receive_timeout: DEFAULT_RECEIVE_TIMEOUT,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            outbound_capacity: DEFAULT_QUEUE_CAPACITY,
            inbound_capacity: DEFAULT_QUEUE_CAPACITY,
            stderr_limit_bytes: DEFAULT_STDERR_LIMIT_BYTES,
        }
    }

    pub fn validate(&self) -> Result<(), PluginSupervisorError> {
        self.manifest.validate()?;
        self.launch.validate()?;
        if self.handshake_timeout.is_zero()
            || self.receive_timeout.is_zero()
            || self.shutdown_timeout.is_zero()
            || self.outbound_capacity == 0
            || self.outbound_capacity > MAX_QUEUE_CAPACITY
            || self.inbound_capacity == 0
            || self.inbound_capacity > MAX_QUEUE_CAPACITY
            || self.stderr_limit_bytes == 0
            || self.stderr_limit_bytes > MAX_STDERR_LIMIT_BYTES
            || self
                .launch
                .environment
                .contains_key(SESSION_ENVIRONMENT_KEY)
        {
            return Err(PluginSupervisorError::InvalidConfiguration);
        }
        validate_envelope(&Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "session_validation".to_string(),
            correlation_id: None,
            causation_id: None,
            body: Some(envelope::Body::Ready(
                vibemux_plugin_protocol::wire::Ready {
                    session_id: self.session_id.clone(),
                },
            )),
        })?;
        Ok(())
    }
}

struct StderrState {
    captured_bytes: AtomicU64,
    truncated: AtomicBool,
}

impl StderrState {
    fn report(&self) -> PluginStderrReport {
        PluginStderrReport {
            captured_bytes: self.captured_bytes.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
        }
    }
}

// During handshake the session does not exist yet. Abort stderr collection if
// the spawn future is cancelled before it can transfer task ownership.
struct HandshakeStderrGuard {
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl Drop for HandshakeStderrGuard {
    fn drop(&mut self) {
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
    }
}

type InboundItem = Result<Envelope, PluginSupervisorError>;

pub struct PluginSession {
    session_id: String,
    lifecycle: CoreSessionLifecycle,
    child: Option<Child>,
    outbound: mpsc::Sender<Envelope>,
    inbound: mpsc::Receiver<InboundItem>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    stderr_state: Arc<StderrState>,
    receive_timeout: Duration,
    shutdown_timeout: Duration,
    message_counter: u64,
    io_tasks_finished: bool,
    last_heartbeat_sequence: Option<u64>,
    last_heartbeat_at: Option<Instant>,
}

pub async fn spawn_plugin(
    config: PluginSupervisorConfig,
) -> Result<PluginSession, PluginSupervisorError> {
    config.validate()?;
    let mut command = Command::new(&config.launch.executable);
    command
        .args(&config.launch.arguments)
        .current_dir(&config.launch.working_directory)
        .env_clear()
        .envs(&config.launch.environment)
        .env(SESSION_ENVIRONMENT_KEY, &config.session_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .spawn()
        .map_err(|_| PluginSupervisorError::SpawnFailed)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(PluginSupervisorError::MissingPipe)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(PluginSupervisorError::MissingPipe)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(PluginSupervisorError::MissingPipe)?;
    let stderr_state = Arc::new(StderrState {
        captured_bytes: AtomicU64::new(0),
        truncated: AtomicBool::new(false),
    });
    let stderr_task = tokio::spawn(collect_stderr(
        stderr,
        config.stderr_limit_bytes,
        stderr_state.clone(),
    ));

    let mut stderr_abort_guard = HandshakeStderrGuard {
        abort_handle: Some(stderr_task.abort_handle()),
    };

    let handshake = async {
        let frame_config = config.policy.frame_config();
        let hello_envelope = read_envelope(&mut stdout, frame_config).await?;
        let hello = match hello_envelope.body.as_ref() {
            Some(envelope::Body::Hello(hello)) => hello,
            _ => return Err(PluginSupervisorError::HandshakeRejected),
        };
        let negotiated =
            negotiate_hello(&config.manifest, hello, &config.session_id, &config.policy)?;
        let mut lifecycle = CoreSessionLifecycle::new();
        lifecycle.receive(&hello_envelope)?;
        let core_hello_envelope = Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "core_hello_1".to_string(),
            correlation_id: Some(config.session_id.clone()),
            causation_id: Some(hello_envelope.message_id.clone()),
            body: Some(envelope::Body::CoreHello(negotiated.core_hello.clone())),
        };
        write_envelope(&mut stdin, &core_hello_envelope, frame_config).await?;
        lifecycle.mark_core_hello_sent(&negotiated.core_hello)?;
        let ready = read_envelope(&mut stdout, frame_config).await?;
        lifecycle.receive(&ready)?;
        Ok::<_, PluginSupervisorError>((lifecycle, frame_config))
    };

    let (lifecycle, frame_config) = match timeout(config.handshake_timeout, handshake).await {
        Ok(Ok(handshake)) => handshake,
        Ok(Err(error)) => {
            let cleanup = terminate_child(&mut child, config.shutdown_timeout).await;
            stderr_task.abort();
            let _ = stderr_task.await;
            cleanup?;
            return Err(error);
        }
        Err(_) => {
            let cleanup = terminate_child(&mut child, config.shutdown_timeout).await;
            stderr_task.abort();
            let _ = stderr_task.await;
            cleanup?;
            return Err(PluginSupervisorError::HandshakeTimeout);
        }
    };

    let (outbound, mut outbound_receiver) = mpsc::channel(config.outbound_capacity);
    let (inbound_sender, inbound) = mpsc::channel(config.inbound_capacity);
    let writer_error_sender = inbound_sender.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(envelope) = outbound_receiver.recv().await {
            if let Err(error) = write_envelope(&mut stdin, &envelope, frame_config).await {
                let _ = writer_error_sender.send(Err(error.into())).await;
                break;
            }
        }
    });
    let reader_task = tokio::spawn(async move {
        loop {
            match read_envelope(&mut stdout, frame_config).await {
                Ok(envelope) => {
                    if inbound_sender.send(Ok(envelope)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = inbound_sender.send(Err(error.into())).await;
                    break;
                }
            }
        }
    });

    stderr_abort_guard.abort_handle.take();
    Ok(PluginSession {
        session_id: config.session_id,
        lifecycle,
        child: Some(child),
        outbound,
        inbound,
        reader_task,
        writer_task,
        stderr_task,
        stderr_state,
        receive_timeout: config.receive_timeout,
        shutdown_timeout: config.shutdown_timeout,
        message_counter: 1,
        io_tasks_finished: false,
        last_heartbeat_sequence: None,
        last_heartbeat_at: None,
    })
}

impl PluginSession {
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub fn try_send(&self, envelope: Envelope) -> Result<(), PluginSupervisorError> {
        validate_envelope(&envelope)?;
        if self.lifecycle.phase() != CoreSessionPhase::Active
            || envelope.body.as_ref().is_none_or(|body| {
                !matches!(
                    body,
                    envelope::Body::Request(_)
                        | envelope::Body::Response(_)
                        | envelope::Body::Event(_)
                        | envelope::Body::Heartbeat(_)
                        | envelope::Body::Cancel(_)
                        | envelope::Body::ProtocolError(_)
                )
            })
        {
            return Err(vibemux_plugin_protocol::PluginProtocolError::InvalidTransition.into());
        }
        self.outbound
            .try_send(envelope)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => PluginSupervisorError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => PluginSupervisorError::QueueClosed,
            })
    }

    pub async fn receive(&mut self) -> Result<Envelope, PluginSupervisorError> {
        self.receive_with_timeout(self.receive_timeout).await
    }

    pub async fn receive_with_timeout(
        &mut self,
        deadline: Duration,
    ) -> Result<Envelope, PluginSupervisorError> {
        let item = timeout(deadline, self.inbound.recv())
            .await
            .map_err(|_| PluginSupervisorError::ReceiveTimeout)?
            .ok_or(PluginSupervisorError::SessionClosed)??;
        self.lifecycle.receive(&item)?;
        self.record_heartbeat(&item)?;
        Ok(item)
    }

    fn record_heartbeat(&mut self, item: &Envelope) -> Result<(), PluginSupervisorError> {
        if let Some(envelope::Body::Heartbeat(Heartbeat { sequence })) = item.body.as_ref() {
            if self
                .last_heartbeat_sequence
                .is_some_and(|last| *sequence <= last)
            {
                return Err(PluginSupervisorError::Protocol {
                    code: "plugin_heartbeat_not_monotonic".to_string(),
                });
            }
            self.last_heartbeat_sequence = Some(*sequence);
            self.last_heartbeat_at = Some(Instant::now());
        }
        Ok(())
    }

    #[must_use]
    pub fn heartbeat_sequence(&self) -> Option<u64> {
        self.last_heartbeat_sequence
    }

    #[must_use]
    pub fn heartbeat_age(&self) -> Option<Duration> {
        self.last_heartbeat_at.map(|at| at.elapsed())
    }

    #[must_use]
    pub fn heartbeat_is_fresh(&self, maximum_age: Duration) -> bool {
        self.heartbeat_age().is_some_and(|age| age <= maximum_age)
    }

    pub fn try_cancel(
        &mut self,
        request_id: impl Into<String>,
        reason_code: impl Into<String>,
    ) -> Result<(), PluginSupervisorError> {
        let envelope = self.next_envelope(envelope::Body::Cancel(Cancel {
            request_id: request_id.into(),
            reason_code: reason_code.into(),
        }));
        self.try_send(envelope)
    }

    pub async fn wait_for_exit(
        &mut self,
        deadline: Duration,
    ) -> Result<PluginExitReport, PluginSupervisorError> {
        let child = self
            .child
            .as_mut()
            .ok_or(PluginSupervisorError::SessionClosed)?;
        let status = timeout(deadline, child.wait())
            .await
            .map_err(|_| PluginSupervisorError::ReceiveTimeout)?
            .map_err(|_| PluginSupervisorError::ChildStatusUnavailable)?;
        self.child.take();
        self.finish_report(status, false).await
    }

    pub async fn shutdown(mut self) -> Result<PluginExitReport, PluginSupervisorError> {
        match self.shutdown_gracefully().await {
            Ok(status) => {
                self.child.take();
                self.finish_report(status, true).await
            }
            Err(error) => {
                // Preserve the original failure while completing exact-child cleanup.
                // Cleanup failures take priority because the owner must not restart a
                // plugin until its previous process is known to have been reaped.
                self.terminate_in_place().await?;
                Err(error)
            }
        }
    }

    /// Terminates and reaps this session's exact child and joins its I/O tasks.
    /// A successful report is returned only after the child has been reaped.
    pub async fn terminate(mut self) -> Result<PluginExitReport, PluginSupervisorError> {
        self.terminate_in_place().await
    }

    async fn shutdown_gracefully(
        &mut self,
    ) -> Result<std::process::ExitStatus, PluginSupervisorError> {
        self.lifecycle.begin_drain()?;
        let deadline_at = Instant::now() + self.shutdown_timeout;
        let drain = self.next_envelope(envelope::Body::Drain(Drain {
            deadline_unix_ms: unix_deadline_ms(self.shutdown_timeout),
        }));
        self.send_critical(drain, deadline_at).await?;
        self.lifecycle.mark_shutdown_sent()?;
        let shutdown = self.next_envelope(envelope::Body::Shutdown(Shutdown {
            reason_code: "core:shutdown".to_string(),
        }));
        self.send_critical(shutdown, deadline_at).await?;
        self.receive_shutdown_acknowledgement(deadline_at).await?;

        let remaining = deadline_at.saturating_duration_since(Instant::now());
        let child = self
            .child
            .as_mut()
            .ok_or(PluginSupervisorError::SessionClosed)?;
        timeout(remaining, child.wait())
            .await
            .map_err(|_| PluginSupervisorError::ShutdownTimeout)?
            .map_err(|_| PluginSupervisorError::ChildStatusUnavailable)
    }

    async fn receive_shutdown_acknowledgement(
        &mut self,
        deadline_at: Instant,
    ) -> Result<(), PluginSupervisorError> {
        loop {
            if Instant::now() >= deadline_at {
                return Err(PluginSupervisorError::ShutdownTimeout);
            }
            let remaining = deadline_at.saturating_duration_since(Instant::now());
            let item = timeout(remaining, self.inbound.recv())
                .await
                .map_err(|_| PluginSupervisorError::ShutdownTimeout)?
                .ok_or(PluginSupervisorError::SessionClosed)??;
            validate_envelope(&item)?;
            match item.body.as_ref() {
                Some(envelope::Body::Shutdown(_)) => {
                    if item.correlation_id.as_deref() != Some(self.session_id.as_str()) {
                        return Err(
                            vibemux_plugin_protocol::PluginProtocolError::InvalidTransition.into(),
                        );
                    }
                    self.lifecycle.receive(&item)?;
                    return Ok(());
                }
                // These frames may already be in flight when Shutdown is sent.
                // Consume only drain-legal traffic under the original deadline;
                // never route it into canonical Task/Run state.
                Some(
                    envelope::Body::Response(_)
                    | envelope::Body::Event(_)
                    | envelope::Body::Heartbeat(_)
                    | envelope::Body::ProtocolError(_),
                ) => self.record_heartbeat(&item)?,
                _ => {
                    return Err(
                        vibemux_plugin_protocol::PluginProtocolError::InvalidTransition.into(),
                    );
                }
            }
        }
    }
    async fn send_critical(
        &self,
        envelope: Envelope,
        deadline: Instant,
    ) -> Result<(), PluginSupervisorError> {
        validate_envelope(&envelope)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        timeout(remaining, self.outbound.send(envelope))
            .await
            .map_err(|_| PluginSupervisorError::ShutdownTimeout)?
            .map_err(|_| PluginSupervisorError::QueueClosed)
    }

    fn next_envelope(&mut self, body: envelope::Body) -> Envelope {
        self.message_counter = self.message_counter.saturating_add(1);
        Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: format!("core_{}", self.message_counter),
            correlation_id: Some(self.session_id.clone()),
            causation_id: None,
            body: Some(body),
        }
    }

    async fn finish_report(
        &mut self,
        status: std::process::ExitStatus,
        graceful: bool,
    ) -> Result<PluginExitReport, PluginSupervisorError> {
        self.finish_io_tasks(true).await;
        Ok(PluginExitReport {
            graceful,
            success: status.success(),
            exit_code: status.code(),
            stderr: self.stderr_state.report(),
        })
    }

    async fn terminate_in_place(&mut self) -> Result<PluginExitReport, PluginSupervisorError> {
        let status = if let Some(child) = self.child.as_mut() {
            // start_kill can fail for an already-exited child; wait is the authority
            // for whether this exact process has been reaped.
            let _ = child.start_kill();
            timeout(self.shutdown_timeout, child.wait())
                .await
                .map_err(|_| PluginSupervisorError::ChildStatusUnavailable)
                .and_then(|result| {
                    result.map_err(|_| PluginSupervisorError::ChildStatusUnavailable)
                })
        } else {
            Err(PluginSupervisorError::SessionClosed)
        };
        if status.is_ok() {
            self.child.take();
        }
        self.finish_io_tasks(status.is_ok()).await;
        let status = status?;
        Ok(PluginExitReport {
            graceful: false,
            success: status.success(),
            exit_code: status.code(),
            stderr: self.stderr_state.report(),
        })
    }

    async fn finish_io_tasks(&mut self, drain_stderr: bool) {
        if self.io_tasks_finished {
            return;
        }
        self.reader_task.abort();
        self.writer_task.abort();
        let _ = tokio::join!(&mut self.reader_task, &mut self.writer_task);
        if !drain_stderr
            || timeout(Duration::from_secs(1), &mut self.stderr_task)
                .await
                .is_err()
        {
            self.stderr_task.abort();
            let _ = (&mut self.stderr_task).await;
        }
        self.io_tasks_finished = true;
    }
}
impl Drop for PluginSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
        self.reader_task.abort();
        self.writer_task.abort();
        self.stderr_task.abort();
    }
}

async fn collect_stderr(
    mut stderr: tokio::process::ChildStderr,
    limit: usize,
    state: Arc<StderrState>,
) {
    let mut buffer = [0_u8; 4096];
    let mut total = 0_u64;
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                total = total.saturating_add(read as u64);
                state
                    .captured_bytes
                    .store(total.min(limit as u64), Ordering::Relaxed);
                if total > limit as u64 {
                    state.truncated.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

async fn terminate_child(
    child: &mut Child,
    deadline: Duration,
) -> Result<(), PluginSupervisorError> {
    let _ = child.start_kill();
    timeout(deadline, child.wait())
        .await
        .map_err(|_| PluginSupervisorError::ChildStatusUnavailable)?
        .map_err(|_| PluginSupervisorError::ChildStatusUnavailable)?;
    Ok(())
}

fn unix_deadline_ms(deadline: Duration) -> u64 {
    let milliseconds = SystemTime::now()
        .checked_add(deadline)
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(u128::from(u64::MAX), |duration| duration.as_millis());
    u64::try_from(milliseconds).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use vibemux_plugin_protocol::wire::{Heartbeat, envelope};

    use super::*;

    #[test]
    fn bounded_outbound_channel_reports_full() {
        let (sender, _receiver) = mpsc::channel(1);
        let envelope = Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "heartbeat_1".to_string(),
            correlation_id: None,
            causation_id: None,
            body: Some(envelope::Body::Heartbeat(Heartbeat { sequence: 1 })),
        };
        sender.try_send(envelope.clone()).expect("first item");
        assert!(matches!(
            sender.try_send(envelope),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }
}
