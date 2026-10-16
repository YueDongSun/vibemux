use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::{Semaphore, watch},
    time::{Instant, timeout_at},
};
use uuid::Uuid;
use vibemux_plugin_protocol::{
    frame::FrameCodecConfig,
    manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    negotiation::CorePluginPolicy,
    wire::{Request as WireRequest, ResponseStatus, envelope},
};
use vibemux_plugin_supervisor::{PluginSupervisorConfig, ResolvedPluginLaunch, spawn_plugin};

use crate::{
    Error,
    config::Config,
    contract::{
        Completion, EXECUTE_METHOD, Harness, Outcome, PROBE_METHOD, RECORD_EVENT, Record, Request,
        SCHEMA_VERSION, Status,
    },
    plugin::{PLUGIN_ID, envelope, unix_ms},
    process::environment,
};

#[derive(Default)]
struct State {
    seen: BTreeSet<Uuid>,
    active: BTreeSet<String>,
}

/// Single-owner dispatch registry. Duplicate suppression is local to this instance,
/// not durable idempotency or a fence against a different daemon/process.
pub struct Dispatcher {
    config_path: PathBuf,
    config_digest: String,
    plugin_executable: PathBuf,
    config: Config,
    slots: Arc<Semaphore>,
    state: Arc<Mutex<State>>,
}

struct Lease {
    state: Arc<Mutex<State>>,
    resource: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.active.remove(&self.resource);
        }
    }
}

impl Dispatcher {
    pub fn new(config_path: &Path, plugin_executable: &Path) -> Result<Self, Error> {
        let config_path = config_path
            .canonicalize()
            .map_err(|source| Error::with_source("harness_config_unavailable", source))?;
        let (config, config_digest) = Config::snapshot(&config_path)?;
        let plugin_executable = plugin_executable
            .canonicalize()
            .map_err(|source| Error::with_source("harness_plugin_unavailable", source))?;
        #[cfg(windows)]
        if !plugin_executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            return Err(Error::new("harness_native_executable_required"));
        }
        Ok(Self {
            config_path,
            config_digest,
            plugin_executable,
            slots: Arc::new(Semaphore::new(config.limits.maximum_in_flight)),
            state: Arc::new(Mutex::new(State::default())),
            config,
        })
    }

    pub fn catalog(&self) -> serde_json::Value {
        self.config.catalog()
    }

    pub async fn execute(
        &self,
        request: Request,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Outcome, Error> {
        request.validate()?;
        self.run(request, false, cancellation).await
    }

    pub async fn probe(&self, harness: Harness) -> Result<Outcome, Error> {
        let (_sender, receiver) = watch::channel(false);
        self.run(
            Request {
                schema_version: SCHEMA_VERSION,
                request_id: Uuid::new_v4(),
                harness,
                prompt: String::new(),
            },
            true,
            receiver,
        )
        .await
    }

    async fn run(
        &self,
        request: Request,
        probe: bool,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<Outcome, Error> {
        let route = self.config.route(request.harness)?;
        if !probe && !route.allow_execution {
            return Err(Error::new("harness_execution_disabled"));
        }
        let _slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("harness_busy"))?;
        let working_directory = route
            .working_directory
            .canonicalize()
            .map_err(|source| Error::with_source("harness_invalid_cwd", source))?;
        let resource = if probe {
            format!("probe:{:?}", request.harness)
        } else {
            let value = working_directory.to_string_lossy().into_owned();
            if cfg!(windows) {
                value.to_lowercase()
            } else {
                value
            }
        };
        let _lease = self.reserve(request.request_id, resource)?;
        if *cancellation.borrow() {
            return Err(Error::new("harness_cancelled"));
        }
        let method = if probe { PROBE_METHOD } else { EXECUTE_METHOD };
        let request_id = request.request_id.to_string();
        let harness_name = serde_json::to_value(request.harness)
            .map_err(|source| Error::with_source("harness_encode_failed", source))?;
        let manifest = PluginManifest {
            schema_version: 1,
            plugin_id: PLUGIN_ID.to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: ManifestPluginKind::Harness,
            entry_point: vec![
                "vibemux_harness_native_plugin".to_string(),
                "--config".to_string(),
                self.config_path.to_string_lossy().into_owned(),
                "--config_digest".to_string(),
                self.config_digest.clone(),
                "--harness".to_string(),
                harness_name
                    .as_str()
                    .ok_or_else(|| Error::new("harness_invalid_route"))?
                    .to_string(),
            ],
            capabilities: if route.allow_execution {
                vec![PROBE_METHOD.to_string(), EXECUTE_METHOD.to_string()]
            } else {
                vec![PROBE_METHOD.to_string()]
            },
            requested_permissions: vec![],
            supported_platforms: vec![
                SupportedPlatform::Windows,
                SupportedPlatform::Linux,
                SupportedPlatform::Macos,
            ],
            protocol_major: 1,
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        };
        let platform = if cfg!(windows) {
            SupportedPlatform::Windows
        } else if cfg!(target_os = "macos") {
            SupportedPlatform::Macos
        } else {
            SupportedPlatform::Linux
        };
        let policy = CorePluginPolicy::new(
            vec![method.to_string()],
            vec![],
            FrameCodecConfig::default(),
            1,
            500,
            platform,
        )
        .map_err(|source| Error::with_source("harness_policy_failed", source))?;
        let launch = ResolvedPluginLaunch::new(
            &manifest,
            self.plugin_executable.clone(),
            working_directory,
            environment(route)?,
        )
        .map_err(|source| Error::with_source("harness_invalid_launch", source))?;
        let session_id = Uuid::new_v4().to_string();
        let mut supervisor =
            PluginSupervisorConfig::new(manifest, launch, policy, session_id.clone());
        supervisor.handshake_timeout =
            Duration::from_millis(self.config.limits.timeout_ms.min(5_000));
        supervisor.shutdown_timeout = Duration::from_millis(self.config.limits.shutdown_ms);
        supervisor.inbound_capacity = self.config.limits.queue_capacity;
        supervisor.outbound_capacity = self.config.limits.queue_capacity;
        let mut session = spawn_plugin(supervisor)
            .await
            .map_err(|source| Error::with_source("harness_plugin_start_failed", source))?;
        let deadline = Instant::now()
            + Duration::from_millis(self.config.limits.timeout_ms + self.config.limits.shutdown_ms);
        let payload = serde_json::to_vec(&request)
            .map_err(|source| Error::with_source("harness_encode_failed", source))?;
        let mut records = Vec::<Record>::new();
        let mut captured_bytes = 0_usize;
        let mut validator = crate::session::Session::new(route.clone(), request.clone(), probe);
        let exchange = async {
            session.try_send(envelope("harness_request".to_string(), Some(request_id.clone()),
                envelope::Body::Request(WireRequest {
                    request_id: request_id.clone(), method: method.to_string(), payload,
                    deadline_unix_ms: unix_ms()?.checked_add(self.config.limits.timeout_ms)
                        .ok_or_else(|| Error::new("harness_clock_error"))?,
                    idempotency_key: Some(request_id.clone()),
                }))).map_err(|source| Error::with_source("harness_send_failed", source))?;
            let mut cancel_sent = false;
            loop {
                let incoming = tokio::select! {
                    changed = cancellation.changed(), if !cancel_sent => {
                        if changed.is_err() || *cancellation.borrow() {
                            cancel_sent = true;
                            session.try_cancel(&request_id, "harness:cancelled")
                                .map_err(|source| Error::with_source("harness_cancel_failed", source))?;
                        }
                        continue;
                    }
                    incoming = timeout_at(deadline, session.receive_with_timeout(Duration::from_millis(self.config.limits.timeout_ms + self.config.limits.shutdown_ms))) => {
                        incoming.map_err(|_| Error::new("harness_dispatch_timeout"))?
                            .map_err(|source| Error::with_source("harness_receive_failed", source))?
                    }
                };
                if matches!(incoming.body, Some(envelope::Body::Heartbeat(_))) {
                    if incoming.correlation_id.as_deref() != Some(session_id.as_str()) {
                        return Err(Error::new("harness_uncorrelated_heartbeat"));
                    }
                    continue;
                }
                if incoming.correlation_id.as_deref() != Some(request_id.as_str()) {
                    return Err(Error::new("harness_uncorrelated_response"));
                }
                match incoming.body {
                    Some(envelope::Body::Event(event)) if event.event_type == RECORD_EVENT => {
                        let wire_record: Record = serde_json::from_slice(&event.payload)
                            .map_err(|source| Error::with_source("harness_invalid_record", source))?;
                        if wire_record.sequence != records.len() as u64 + 1
                            || wire_record.raw_json().len() > self.config.limits.frame_bytes {
                            return Err(Error::new("harness_invalid_sequence"));
                        }
                        let record = Record::parse(route.protocol, wire_record.sequence, wire_record.raw_json().to_string())?;
                        if record.kind != wire_record.kind { return Err(Error::new("harness_invalid_record_kind")); }
                        captured_bytes = captured_bytes.checked_add(record.raw_json().len())
                            .ok_or_else(|| Error::new("harness_capture_too_large"))?;
                        if captured_bytes > self.config.limits.capture_bytes || records.len() >= self.config.limits.record_count {
                            return Err(Error::new("harness_capture_too_large"));
                        }
                        let validation = validator.receive(&record.value()?, record.kind);
                        records.push(record);
                        validation?;
                    }
                    Some(envelope::Body::Response(response)) => {
                        if response.request_id != request_id { return Err(Error::new("harness_uncorrelated_response")); }
                        if response.payload.is_empty() { return Err(Error::new("harness_plugin_rejected")); }
                        let mut completion: Completion = serde_json::from_slice(&response.payload)
                            .map_err(|source| Error::with_source("harness_invalid_completion", source))?;
                        let expected_status = match completion.status {
                            Status::Completed | Status::Probed => ResponseStatus::Ok,
                            Status::Failed | Status::Unverified => ResponseStatus::Error,
                            Status::Cancelled => ResponseStatus::Cancelled,
                        };
                        if completion.schema_version != SCHEMA_VERSION || completion.record_count != records.len() as u64
                            || completion.stdout_bytes < captured_bytes as u64
                            || response.status != i32::from(expected_status)
                            || (!probe && completion.status == Status::Probed)
                            || (probe && completion.status == Status::Completed)
                            || (matches!(completion.status, Status::Completed | Status::Probed)
                                && (completion.exit_code != Some(0) || completion.forced_termination
                                    || completion.stdout_bytes != captured_bytes as u64
                                    || validator.status != Some(completion.status)
                                    || completion.error_code.is_some()))
                            || (completion.status == Status::Cancelled
                                && validator.status != Some(Status::Cancelled) && !cancel_sent)
                        {
                            return Err(Error::new("harness_invalid_completion"));
                        }
                        if completion.error_code.as_ref().is_some_and(|code| !safe_error_code(code)) {
                            completion.error_code = Some("harness_vendor_failed".to_string());
                        }
                        return Ok(completion);
                    }
                    _ => return Err(Error::new("harness_unexpected_plugin_message")),
                }
            }
        }.await;
        if exchange.is_err() {
            let _ = session.try_cancel(&request_id, "harness:cancelled");
            let drain_deadline =
                Instant::now() + Duration::from_millis(self.config.limits.shutdown_ms);
            while let Ok(Ok(incoming)) = timeout_at(
                drain_deadline,
                session.receive_with_timeout(Duration::from_millis(self.config.limits.shutdown_ms)),
            )
            .await
            {
                if matches!(incoming.body, Some(envelope::Body::Response(response)) if response.request_id == request_id)
                {
                    break;
                }
            }
        }
        let shutdown = session.shutdown().await;
        let mut completion = match exchange {
            Ok(completion) => completion,
            Err(error) => Completion {
                schema_version: SCHEMA_VERSION,
                status: Status::Failed,
                record_count: records.len() as u64,
                stdout_bytes: captured_bytes as u64,
                stderr_bytes: 0,
                exit_code: None,
                forced_termination: true,
                error_code: Some(error.code().to_string()),
            },
        };
        if !shutdown.is_ok_and(|report| report.graceful && report.success) {
            completion.status = Status::Failed;
            completion.error_code = Some("harness_plugin_shutdown_failed".to_string());
        }
        Ok(Outcome {
            request_id: request.request_id,
            harness: request.harness,
            records,
            completion,
        })
    }

    fn reserve(&self, request_id: Uuid, resource: String) -> Result<Lease, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::new("harness_registry_poisoned"))?;
        if state.seen.contains(&request_id) {
            return Err(Error::new("harness_duplicate_request"));
        }
        if state.active.contains(&resource) {
            return Err(Error::new("harness_busy"));
        }
        if state.seen.len() >= self.config.limits.request_history {
            return Err(Error::new("harness_history_full"));
        }
        state.seen.insert(request_id);
        state.active.insert(resource.clone());
        Ok(Lease {
            state: self.state.clone(),
            resource,
        })
    }
}

fn safe_error_code(code: &str) -> bool {
    matches!(
        code,
        "harness_cancelled"
            | "harness_deadline_exceeded"
            | "harness_backpressure_timeout"
            | "harness_capture_too_large"
            | "harness_frame_too_large"
            | "harness_empty_record"
            | "harness_invalid_json"
            | "harness_invalid_utf8"
            | "harness_invalid_record"
            | "harness_process_failed"
            | "harness_eof_without_terminal"
            | "harness_vendor_failed"
            | "harness_rpc_error"
            | "harness_invalid_terminal"
            | "harness_uncorrelated_response"
            | "harness_uncorrelated_session"
            | "harness_uncorrelated_turn"
            | "harness_invalid_initialize"
            | "harness_unsupported_protocol_version"
            | "harness_unexpected_terminal"
            | "harness_write_timeout"
            | "harness_write_failed"
            | "harness_stdin_closed"
    )
}
