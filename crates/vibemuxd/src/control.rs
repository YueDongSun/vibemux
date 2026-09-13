//! Versioned, authenticated local control transport for the daemon writer.

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

#[cfg(windows)]
use tokio::time::{Instant, sleep};

use crate::plugin_configuration::PluginStartup;
use crate::plugin_registry::{
    PluginRegistry, PluginRegistryError, PluginStatus, PluginStatusReader,
};
use crate::supervisor_service::{SupervisorService, SupervisorServiceConfig};
use crate::{WriterError, WriterHealth, WriterWorker};

pub const CONTROL_PROTOCOL_VERSION: u32 = 2;
pub const LEGACY_CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_DESCRIPTOR_BYTES: usize = 4 * 1024;
pub const CONTROL_DEADLINE: Duration = Duration::from_secs(10);
pub const DESCRIPTOR_FILE_NAME: &str = "control.json";
const PEER_CLOSE_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOperation {
    Health,
    PluginStatus,
    Shutdown,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub version: u32,
    pub request_id: String,
    pub token: String,
    pub operation: ControlOperation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonHealth {
    pub healthy: bool,
    pub process_id: u32,
    pub store_schema_version: u32,
    pub queue_capacity: usize,
    #[serde(default)]
    pub queue_depth: usize,
    #[serde(default)]
    pub queue_high_watermark: usize,
    #[serde(default)]
    pub queue_saturated: bool,
}

impl DaemonHealth {
    fn from_writer(process_id: u32, writer: WriterHealth) -> Self {
        Self {
            healthy: writer.healthy,
            process_id,
            store_schema_version: writer.store_schema_version,
            queue_capacity: writer.queue_capacity,
            queue_depth: writer.queue_depth,
            queue_high_watermark: writer.queue_high_watermark,
            queue_saturated: writer.queue_saturated,
        }
    }
}

impl fmt::Debug for ControlRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlRequest")
            .field("version", &self.version)
            .field("request_id", &self.request_id)
            .field("token", &"[redacted]")
            .field("operation", &self.operation)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum ControlPayload {
    Health(DaemonHealth),
    PluginStatus(Vec<PluginStatus>),
    ShutdownAccepted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlResponse {
    pub version: u32,
    pub request_id: String,
    pub payload: Option<ControlPayload>,
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEndpointKind {
    WindowsNamedPipe,
    UnixSocket,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlArtifactInfo {
    pub protocol_version: u32,
    pub process_id: u32,
    pub endpoint_kind: ControlEndpointKind,
}

impl ControlResponse {
    fn success(request_id: String, payload: ControlPayload) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            payload: Some(payload),
            error_code: None,
        }
    }

    fn error(request_id: String, error: &ControlError) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            payload: None,
            error_code: Some(error.code().to_string()),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ControlDescriptor {
    version: u32,
    process_id: u32,
    endpoint: String,
    token: String,
}

impl fmt::Debug for ControlDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlDescriptor")
            .field("version", &self.version)
            .field("process_id", &self.process_id)
            .field("endpoint", &self.endpoint)
            .field("token", &"[redacted]")
            .finish()
    }
}

pub struct ControlArtifactSnapshot {
    path: PathBuf,
    encoded: Vec<u8>,
    #[cfg(unix)]
    descriptor: ControlDescriptor,
    info: ControlArtifactInfo,
}

impl fmt::Debug for ControlArtifactSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlArtifactSnapshot")
            .field("info", &self.info)
            .field("endpoint", &"[redacted]")
            .field("token", &"[redacted]")
            .finish()
    }
}

impl ControlArtifactSnapshot {
    #[must_use]
    pub const fn info(&self) -> ControlArtifactInfo {
        self.info
    }

    #[must_use]
    pub fn binding_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"vibemux-control-artifact-v1\0");
        hasher.update(&self.encoded);
        hasher.finalize().into()
    }

    pub fn remove_if_unchanged(&self) -> Result<(), ControlError> {
        self.require_unchanged()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;

            let socket_path = Path::new(&self.descriptor.endpoint);
            match std::fs::symlink_metadata(socket_path) {
                Ok(metadata) if metadata.file_type().is_socket() => {
                    self.require_unchanged()?;
                    std::fs::remove_file(socket_path)
                        .map_err(|_| ControlError::ArtifactCleanupFailed)?;
                }
                Ok(_) => return Err(ControlError::UnsafeArtifact),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(ControlError::ArtifactCleanupFailed),
            }
        }
        self.require_unchanged()?;
        std::fs::remove_file(&self.path).map_err(|_| ControlError::ArtifactCleanupFailed)
    }

    fn require_unchanged(&self) -> Result<(), ControlError> {
        let (encoded, _) = read_descriptor_with_bytes(&self.path)?;
        if encoded == self.encoded {
            Ok(())
        } else {
            Err(ControlError::ArtifactChanged)
        }
    }
}

pub fn inspect_control_artifact(
    path: &Path,
) -> Result<Option<ControlArtifactSnapshot>, ControlError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ControlError::DescriptorInvalid),
    }
    let (encoded, descriptor) = read_descriptor_with_bytes(path)?;
    let info = ControlArtifactInfo {
        protocol_version: descriptor.version,
        process_id: descriptor.process_id,
        endpoint_kind: endpoint_kind(&descriptor.endpoint),
    };
    Ok(Some(ControlArtifactSnapshot {
        path: path.to_path_buf(),
        encoded,
        #[cfg(unix)]
        descriptor,
        info,
    }))
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ControlError {
    #[error("local control descriptor already exists")]
    DescriptorExists,
    #[error("local control descriptor is invalid")]
    DescriptorInvalid,
    #[error("local control endpoint is unavailable")]
    EndpointUnavailable,
    #[error("local control frame exceeds the configured limit")]
    FrameTooLarge,
    #[error("local control frame is invalid")]
    InvalidFrame,
    #[error("local control request is invalid")]
    InvalidRequest,
    #[error("local control authentication failed")]
    Unauthorized,
    #[error("local control protocol version is unsupported")]
    UnsupportedVersion,
    #[error("local control authentication token could not be generated")]
    TokenUnavailable,
    #[error("local control runtime artifact path is unsafe")]
    UnsafeArtifact,
    #[error("local control runtime artifact changed after inspection")]
    ArtifactChanged,
    #[error("local control runtime artifact cleanup failed")]
    ArtifactCleanupFailed,
    #[error("local control runtime generation conflicts with legacy artifacts")]
    RuntimeGenerationConflict,
    #[error("daemon writer operation failed: {code}")]
    Writer { code: String },
    #[error("daemon plugin operation failed: {code}")]
    Plugin { code: String },
    #[error("local control operation exceeded its deadline")]
    Deadline,
    #[error("local control server terminated unexpectedly")]
    ServerTerminated,
    #[error("local control peer returned an error: {code}")]
    Remote { code: String },
}

impl ControlError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::DescriptorExists => "control_descriptor_exists",
            Self::DescriptorInvalid => "control_descriptor_invalid",
            Self::EndpointUnavailable => "control_endpoint_unavailable",
            Self::FrameTooLarge => "control_frame_too_large",
            Self::InvalidFrame => "control_invalid_frame",
            Self::InvalidRequest => "control_invalid_request",
            Self::Unauthorized => "control_unauthorized",
            Self::UnsupportedVersion => "control_unsupported_version",
            Self::TokenUnavailable => "control_token_unavailable",
            Self::UnsafeArtifact => "control_artifact_unsafe_path",
            Self::ArtifactChanged => "control_artifact_changed",
            Self::ArtifactCleanupFailed => "control_artifact_cleanup_failed",
            Self::RuntimeGenerationConflict => "control_runtime_generation_conflict",
            Self::Writer { code } | Self::Plugin { code } | Self::Remote { code } => code,
            Self::Deadline => "control_deadline_exceeded",
            Self::ServerTerminated => "control_server_terminated",
        }
    }
}

impl From<WriterError> for ControlError {
    fn from(error: WriterError) -> Self {
        Self::Writer {
            code: error.code().to_string(),
        }
    }
}

impl From<PluginRegistryError> for ControlError {
    fn from(error: PluginRegistryError) -> Self {
        Self::Plugin {
            code: error.code().to_string(),
        }
    }
}

struct DescriptorGuard {
    path: PathBuf,
    descriptor: ControlDescriptor,
}

impl DescriptorGuard {
    fn publish(path: &Path, descriptor: ControlDescriptor) -> Result<Self, ControlError> {
        let encoded =
            serde_json::to_vec(&descriptor).map_err(|_| ControlError::DescriptorInvalid)?;
        if encoded.len() > MAX_DESCRIPTOR_BYTES {
            return Err(ControlError::DescriptorInvalid);
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                ControlError::DescriptorExists
            } else {
                ControlError::EndpointUnavailable
            }
        })?;
        if file
            .write_all(&encoded)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .is_err()
        {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(ControlError::EndpointUnavailable);
        }

        Ok(Self {
            path: path.to_path_buf(),
            descriptor,
        })
    }
}

impl Drop for DescriptorGuard {
    fn drop(&mut self) {
        let matches = read_descriptor(&self.path)
            .ok()
            .is_some_and(|current| current == self.descriptor);
        if matches {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
struct SocketPathGuard {
    path: PathBuf,
    owner: Option<(PathBuf, ControlDescriptor)>,
}

#[cfg(unix)]
impl SocketPathGuard {
    fn unpublished(path: PathBuf) -> Self {
        Self { path, owner: None }
    }

    fn bind_owner(&mut self, descriptor_path: PathBuf, descriptor: ControlDescriptor) {
        self.owner = Some((descriptor_path, descriptor));
    }
}

#[cfg(unix)]
impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::FileTypeExt;

        let is_socket = std::fs::symlink_metadata(&self.path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_socket());
        let owner_matches = self
            .owner
            .as_ref()
            .is_none_or(|(descriptor_path, descriptor)| {
                read_descriptor(descriptor_path)
                    .ok()
                    .is_some_and(|current| current == *descriptor)
            });
        if is_socket && owner_matches {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct ServerState {
    writer: Mutex<Option<WriterWorker>>,
    plugins: PluginStatusReader,
    process_id: u32,
    token: String,
}

pub struct DaemonControlServer {
    descriptor_path: PathBuf,
    writer_lock_path: PathBuf,
    task: Option<JoinHandle<Result<(), ControlError>>>,
    stop: tokio::sync::watch::Sender<bool>,
    a2a_base_url: Option<String>,
    a2a_grpc_base_url: Option<String>,
}

impl DaemonControlServer {
    pub async fn start_for_paths(
        paths: &crate::process::DaemonPaths,
    ) -> Result<Self, ControlError> {
        Self::start_for_paths_with_plugins(paths, PluginStartup::default()).await
    }

    pub async fn start_for_paths_with_plugins(
        paths: &crate::process::DaemonPaths,
        plugins: PluginStartup,
    ) -> Result<Self, ControlError> {
        paths
            .validate_daemon_start()
            .map_err(|_| ControlError::RuntimeGenerationConflict)?;
        Self::start_with_writer_locks(
            paths.database_path(),
            paths.runtime_dir(),
            paths.writer_lock_path(),
            paths
                .has_distinct_legacy_runtime()
                .then(|| paths.legacy_writer_lock_path()),
            plugins,
            None,
        )
        .await
    }

    /// Explicit trusted startup configuration; never called from status IPC.
    pub async fn start_with_plugins(
        database_path: &Path,
        runtime_dir: &Path,
        plugins: PluginStartup,
    ) -> Result<Self, ControlError> {
        let writer_lock_path = crate::writer_lock_path_for_database(database_path);
        Self::start_with_writer_locks(
            database_path,
            runtime_dir,
            &writer_lock_path,
            None,
            plugins,
            None,
        )
        .await
    }

    pub async fn start(database_path: &Path, runtime_dir: &Path) -> Result<Self, ControlError> {
        let writer_lock_path = crate::writer_lock_path_for_database(database_path);
        Self::start_with_writer_lock(database_path, runtime_dir, &writer_lock_path).await
    }

    pub async fn start_with_writer_lock(
        database_path: &Path,
        runtime_dir: &Path,
        writer_lock_path: &Path,
    ) -> Result<Self, ControlError> {
        Self::start_with_writer_locks(
            database_path,
            runtime_dir,
            writer_lock_path,
            None,
            PluginStartup::default(),
            None,
        )
        .await
    }

    pub async fn start_with_writer_and_compatibility_lock(
        database_path: &Path,
        runtime_dir: &Path,
        writer_lock_path: &Path,
        compatibility_lock_path: &Path,
    ) -> Result<Self, ControlError> {
        Self::start_with_writer_locks(
            database_path,
            runtime_dir,
            writer_lock_path,
            Some(compatibility_lock_path),
            PluginStartup::default(),
            None,
        )
        .await
    }

    pub async fn start_for_paths_with_supervisor(
        paths: &crate::process::DaemonPaths,
        plugins: PluginStartup,
        supervisor: SupervisorServiceConfig,
    ) -> Result<Self, ControlError> {
        let configured_root = std::fs::canonicalize(&supervisor.supervisor.project_root)
            .map_err(|_| ControlError::InvalidRequest)?;
        if configured_root != paths.project_root() {
            return Err(ControlError::InvalidRequest);
        }
        paths
            .validate_daemon_start()
            .map_err(|_| ControlError::RuntimeGenerationConflict)?;
        Self::start_with_writer_locks(
            paths.database_path(),
            paths.runtime_dir(),
            paths.writer_lock_path(),
            paths
                .has_distinct_legacy_runtime()
                .then(|| paths.legacy_writer_lock_path()),
            plugins,
            Some(supervisor),
        )
        .await
    }

    async fn start_with_writer_locks(
        database_path: &Path,
        runtime_dir: &Path,
        writer_lock_path: &Path,
        compatibility_lock_path: Option<&Path>,
        plugins: PluginStartup,
        supervisor: Option<SupervisorServiceConfig>,
    ) -> Result<Self, ControlError> {
        plugins.validate()?;
        std::fs::create_dir_all(runtime_dir).map_err(|_| ControlError::EndpointUnavailable)?;
        let runtime_dir =
            std::fs::canonicalize(runtime_dir).map_err(|_| ControlError::EndpointUnavailable)?;
        let descriptor_path = runtime_dir.join(DESCRIPTOR_FILE_NAME);
        if descriptor_path.exists() {
            return Err(ControlError::DescriptorExists);
        }

        let database_path = database_path.to_path_buf();
        let writer_lock_path = writer_lock_path.to_path_buf();
        let compatibility_lock_path = compatibility_lock_path.map(Path::to_path_buf);
        let writer = tokio::task::spawn_blocking(move || match compatibility_lock_path {
            Some(compatibility_lock_path) => WriterWorker::start_with_compatibility_lock_path(
                &database_path,
                &writer_lock_path,
                &compatibility_lock_path,
            ),
            None => WriterWorker::start_with_lock_path(&database_path, &writer_lock_path),
        })
        .await
        .map_err(|_| ControlError::ServerTerminated)??;
        let writer_lock_path = writer.lock_path().to_path_buf();
        let endpoint = endpoint_name(&runtime_dir);
        let descriptor = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            process_id: std::process::id(),
            endpoint: endpoint.clone(),
            token: control_token()?,
        };

        #[cfg(windows)]
        {
            let listener = windows_server_options(true)
                .create(&endpoint)
                .map_err(|_| ControlError::EndpointUnavailable)?;
            let descriptor_guard = DescriptorGuard::publish(&descriptor_path, descriptor.clone())?;
            let mut registry = PluginRegistry::new(plugins.registry_config.clone())?;
            for (config, policy) in plugins.registrations {
                if let Err(error) = registry.register(config, policy) {
                    let _ = registry.shutdown().await;
                    return Err(error.into());
                }
            }
            let supervisor = match supervisor {
                Some(config) => match SupervisorService::start(writer.handle()?, config).await {
                    Ok(service) => Some(service),
                    Err(error) => {
                        let _ = registry.shutdown().await;
                        return Err(ControlError::Plugin {
                            code: error.code().to_string(),
                        });
                    }
                },
                None => None,
            };
            let a2a_base_url = supervisor
                .as_ref()
                .map(|service| service.base_url().to_string());
            let a2a_grpc_base_url = supervisor.as_ref().map(SupervisorService::grpc_base_url);
            let (stop, stop_receiver) = tokio::sync::watch::channel(false);
            let state = Arc::new(ServerState {
                writer: Mutex::new(Some(writer)),
                plugins: registry.status_reader(),
                process_id: descriptor.process_id,
                token: descriptor.token,
            });
            let task = tokio::spawn(async move {
                let _descriptor_guard = descriptor_guard;
                serve_owned(
                    run_server(listener, endpoint, state.clone()),
                    state,
                    registry,
                    stop_receiver,
                    supervisor,
                )
                .await
            });
            return Ok(Self {
                descriptor_path,
                writer_lock_path,
                task: Some(task),
                stop,
                a2a_base_url,
                a2a_grpc_base_url,
            });
        }

        #[cfg(unix)]
        {
            use tokio::net::UnixListener;

            let socket_path = PathBuf::from(&endpoint);
            let listener =
                UnixListener::bind(&socket_path).map_err(|_| ControlError::EndpointUnavailable)?;
            let mut socket_guard = SocketPathGuard::unpublished(socket_path);
            let descriptor_guard = DescriptorGuard::publish(&descriptor_path, descriptor.clone())?;
            socket_guard.bind_owner(descriptor_path.clone(), descriptor.clone());
            let mut registry = PluginRegistry::new(plugins.registry_config.clone())?;
            for (config, policy) in plugins.registrations {
                if let Err(error) = registry.register(config, policy) {
                    let _ = registry.shutdown().await;
                    return Err(error.into());
                }
            }
            let supervisor = match supervisor {
                Some(config) => match SupervisorService::start(writer.handle()?, config).await {
                    Ok(service) => Some(service),
                    Err(error) => {
                        let _ = registry.shutdown().await;
                        return Err(ControlError::Plugin {
                            code: error.code().to_string(),
                        });
                    }
                },
                None => None,
            };
            let a2a_base_url = supervisor
                .as_ref()
                .map(|service| service.base_url().to_string());
            let a2a_grpc_base_url = supervisor.as_ref().map(SupervisorService::grpc_base_url);
            let (stop, stop_receiver) = tokio::sync::watch::channel(false);
            let state = Arc::new(ServerState {
                writer: Mutex::new(Some(writer)),
                plugins: registry.status_reader(),
                process_id: descriptor.process_id,
                token: descriptor.token,
            });
            let task = tokio::spawn(async move {
                let _descriptor_guard = descriptor_guard;
                let _socket_guard = socket_guard;
                serve_owned(
                    run_server(listener, state.clone()),
                    state,
                    registry,
                    stop_receiver,
                    supervisor,
                )
                .await
            });
            return Ok(Self {
                descriptor_path,
                writer_lock_path,
                task: Some(task),
                stop,
                a2a_base_url,
                a2a_grpc_base_url,
            });
        }

        #[allow(unreachable_code)]
        Err(ControlError::EndpointUnavailable)
    }

    #[must_use]
    pub fn a2a_base_url(&self) -> Option<&str> {
        self.a2a_base_url.as_deref()
    }

    #[must_use]
    pub fn a2a_grpc_base_url(&self) -> Option<&str> {
        self.a2a_grpc_base_url.as_deref()
    }

    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    #[must_use]
    pub fn writer_lock_path(&self) -> &Path {
        &self.writer_lock_path
    }

    pub async fn shutdown(mut self) -> Result<(), ControlError> {
        self.stop.send_replace(true);
        self.join().await
    }

    pub async fn wait(mut self) -> Result<(), ControlError> {
        self.join().await
    }

    async fn join(&mut self) -> Result<(), ControlError> {
        // Keep ownership during await: cancellation drops Self and signals stop.
        let task = self.task.as_mut().ok_or(ControlError::ServerTerminated)?;
        let result = task.await.map_err(|_| ControlError::ServerTerminated)?;
        self.task.take();
        result
    }
}

impl Drop for DaemonControlServer {
    fn drop(&mut self) {
        // Never abort the owner while it is reaping children. Explicit wait/shutdown
        // joins cleanup; Drop is a cancellation signal, not a completion receipt.
        self.stop.send_replace(true);
    }
}

async fn serve_owned(
    server: impl std::future::Future<Output = Result<(), ControlError>>,
    state: Arc<ServerState>,
    mut registry: PluginRegistry,
    mut stop: tokio::sync::watch::Receiver<bool>,
    supervisor: Option<SupervisorService>,
) -> Result<(), ControlError> {
    let result = tokio::select! {
        biased;
        _ = stop.changed() => Ok(()),
        result = server => result,
    };
    // Signal all plugins together, join their cleanup, then release the one writer.
    let supervisor_result = match supervisor {
        Some(service) => service
            .shutdown()
            .await
            .map_err(|error| ControlError::Plugin {
                code: error.code().to_string(),
            }),
        None => Ok(()),
    };
    let plugins_result = registry.shutdown().await.map_err(ControlError::from);
    let writer_result = shutdown_writer(state).await;
    result
        .and(supervisor_result)
        .and(plugins_result)
        .and(writer_result)
}

#[derive(Clone, Debug)]
pub struct ControlClient {
    descriptor: ControlDescriptor,
}

impl ControlClient {
    pub fn from_descriptor(path: &Path) -> Result<Self, ControlError> {
        let descriptor = read_descriptor(path)?;
        if !supported_control_version(descriptor.version) {
            return Err(ControlError::UnsupportedVersion);
        }
        Ok(Self { descriptor })
    }

    pub async fn health(&self) -> Result<DaemonHealth, ControlError> {
        self.health_with_deadline(CONTROL_DEADLINE).await
    }

    pub async fn health_with_deadline(
        &self,
        deadline: Duration,
    ) -> Result<DaemonHealth, ControlError> {
        match self.request(ControlOperation::Health, deadline).await? {
            ControlPayload::Health(health) => Ok(health),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn plugin_status(&self) -> Result<Vec<PluginStatus>, ControlError> {
        if self.descriptor.version < CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::UnsupportedVersion);
        }
        match self
            .request(ControlOperation::PluginStatus, CONTROL_DEADLINE)
            .await?
        {
            ControlPayload::PluginStatus(statuses) => Ok(statuses),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn shutdown(&self) -> Result<(), ControlError> {
        self.shutdown_with_deadline(CONTROL_DEADLINE).await
    }

    pub async fn shutdown_with_deadline(&self, deadline: Duration) -> Result<(), ControlError> {
        match self.request(ControlOperation::Shutdown, deadline).await? {
            ControlPayload::ShutdownAccepted => Ok(()),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    async fn request(
        &self,
        operation: ControlOperation,
        deadline: Duration,
    ) -> Result<ControlPayload, ControlError> {
        self.request_raw_with_deadline(
            ControlRequest {
                version: self.descriptor.version,
                request_id: Uuid::new_v4().to_string(),
                token: self.descriptor.token.clone(),
                operation,
            },
            deadline,
        )
        .await
    }

    #[cfg(test)]
    async fn request_raw(&self, request: ControlRequest) -> Result<ControlPayload, ControlError> {
        self.request_raw_with_deadline(request, CONTROL_DEADLINE)
            .await
    }

    async fn request_raw_with_deadline(
        &self,
        request: ControlRequest,
        deadline: Duration,
    ) -> Result<ControlPayload, ControlError> {
        let response = timeout(
            deadline,
            request_over_local_transport(&self.descriptor.endpoint, &request, deadline),
        )
        .await
        .map_err(|_| ControlError::Deadline)??;
        if response.version != request.version {
            return Err(ControlError::UnsupportedVersion);
        }
        if response.request_id != request.request_id {
            return Err(ControlError::InvalidFrame);
        }
        if let Some(code) = response.error_code {
            return Err(remote_error(code));
        }
        response.payload.ok_or(ControlError::InvalidFrame)
    }
}

async fn handle_connection<S>(stream: &mut S, state: Arc<ServerState>) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = match timeout(CONTROL_DEADLINE, read_frame::<ControlRequest, _>(stream)).await {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            send_response(stream, &ControlResponse::error(String::new(), &error)).await;
            return false;
        }
        Err(_) => {
            send_response(
                stream,
                &ControlResponse::error(String::new(), &ControlError::Deadline),
            )
            .await;
            return false;
        }
    };
    if !constant_time_token_eq(&state.token, &request.token) {
        let mut response = ControlResponse::error(request.request_id, &ControlError::Unauthorized);
        response.version = request.version;
        send_response(stream, &response).await;
        return false;
    }
    if !valid_request_id(&request.request_id) {
        let mut response =
            ControlResponse::error(request.request_id, &ControlError::InvalidRequest);
        response.version = request.version;
        send_response(stream, &response).await;
        return false;
    }
    if !supported_control_version(request.version)
        || (request.version == LEGACY_CONTROL_PROTOCOL_VERSION
            && request.operation == ControlOperation::PluginStatus)
    {
        let mut response =
            ControlResponse::error(request.request_id, &ControlError::UnsupportedVersion);
        response.version = request.version;
        send_response(stream, &response).await;
        return false;
    }

    let request_id = request.request_id;
    let (mut response, should_shutdown) = match request.operation {
        ControlOperation::Health => match writer_health(state).await {
            Ok(health) => (
                ControlResponse::success(request_id, ControlPayload::Health(health)),
                false,
            ),
            Err(error) => (ControlResponse::error(request_id, &error), false),
        },
        ControlOperation::PluginStatus => (
            ControlResponse::success(
                request_id,
                ControlPayload::PluginStatus(state.plugins.statuses()),
            ),
            false,
        ),
        // Acceptance only. The owner joins plugins before releasing the writer.
        ControlOperation::Shutdown => (
            ControlResponse::success(request_id, ControlPayload::ShutdownAccepted),
            true,
        ),
    };
    response.version = request.version;
    send_response(stream, &response).await;
    should_shutdown
}

async fn send_response<S>(stream: &mut S, response: &ControlResponse)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if matches!(
        timeout(CONTROL_DEADLINE, write_frame(stream, response)).await,
        Ok(Ok(()))
    ) {
        let mut peer_data = [0_u8; 1];
        let _ = timeout(PEER_CLOSE_DEADLINE, stream.read(&mut peer_data)).await;
    }
}

async fn writer_health(state: Arc<ServerState>) -> Result<DaemonHealth, ControlError> {
    let process_id = state.process_id;
    let writer_health = tokio::task::spawn_blocking(move || {
        let guard = state
            .writer
            .lock()
            .map_err(|_| ControlError::ServerTerminated)?;
        guard
            .as_ref()
            .ok_or(ControlError::ServerTerminated)?
            .health()
            .map_err(ControlError::from)
    })
    .await
    .map_err(|_| ControlError::ServerTerminated)??;
    Ok(DaemonHealth::from_writer(process_id, writer_health))
}

async fn shutdown_writer(state: Arc<ServerState>) -> Result<(), ControlError> {
    tokio::task::spawn_blocking(move || {
        let writer = {
            let mut guard = state
                .writer
                .lock()
                .map_err(|_| ControlError::ServerTerminated)?;
            guard.take().ok_or(ControlError::ServerTerminated)?
        };
        writer.shutdown().map_err(ControlError::from)
    })
    .await
    .map_err(|_| ControlError::ServerTerminated)?
}

#[cfg(windows)]
async fn run_server(
    mut listener: tokio::net::windows::named_pipe::NamedPipeServer,
    endpoint: String,
    state: Arc<ServerState>,
) -> Result<(), ControlError> {
    let mut connections = tokio::task::JoinSet::new();
    let (shutdown_request, mut shutdown_requested) = tokio::sync::watch::channel(false);
    let result = loop {
        tokio::select! {
            biased;
            _ = shutdown_requested.changed() => break Ok(()),
            connected = listener.connect() => {
                connected.map_err(|_| ControlError::EndpointUnavailable)?;
                // Hand the connected instance to its own task and create the
                // next pipe instance immediately: a slow or idle client must
                // not stall health checks and other control traffic.
                let mut connection = listener;
                listener = windows_server_options(false)
                    .create(&endpoint)
                    .map_err(|_| ControlError::EndpointUnavailable)?;
                let connection_state = Arc::clone(&state);
                let shutdown_request = shutdown_request.clone();
                connections.spawn(async move {
                    if handle_connection(&mut connection, connection_state).await {
                        shutdown_request.send_replace(true);
                    }
                });
            }
        }
    };
    // Graceful shutdown: drain in-flight requests before the writer closes.
    // An external stop cancels this future and aborts the tasks instead.
    while connections.join_next().await.is_some() {}
    result
}

#[cfg(windows)]
fn windows_server_options(first_instance: bool) -> tokio::net::windows::named_pipe::ServerOptions {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut options = ServerOptions::new();
    // The server always holds one listening instance and each in-flight
    // connection holds another; 16 bounds concurrent control clients while
    // leaving headroom above the JoinSet drain window.
    options
        .first_pipe_instance(first_instance)
        .reject_remote_clients(true)
        .max_instances(16);
    options
}

#[cfg(unix)]
async fn run_server(
    listener: tokio::net::UnixListener,
    state: Arc<ServerState>,
) -> Result<(), ControlError> {
    let mut connections = tokio::task::JoinSet::new();
    let (shutdown_request, mut shutdown_requested) = tokio::sync::watch::channel(false);
    let result = loop {
        tokio::select! {
            biased;
            _ = shutdown_requested.changed() => break Ok(()),
            accepted = listener.accept() => {
                let (mut stream, _) =
                    accepted.map_err(|_| ControlError::EndpointUnavailable)?;
                // Each connection is handled in its own task so one slow or
                // idle client cannot stall health checks and other traffic.
                let connection_state = Arc::clone(&state);
                let shutdown_request = shutdown_request.clone();
                connections.spawn(async move {
                    if handle_connection(&mut stream, connection_state).await {
                        shutdown_request.send_replace(true);
                    }
                });
            }
        }
    };
    // Graceful shutdown: drain in-flight requests before the writer closes.
    // An external stop cancels this future and aborts the tasks instead.
    while connections.join_next().await.is_some() {}
    result
}

#[cfg(windows)]
async fn request_over_local_transport(
    endpoint: &str,
    request: &ControlRequest,
    deadline: Duration,
) -> Result<ControlResponse, ControlError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let deadline = Instant::now() + deadline;
    let mut client = loop {
        match ClientOptions::new().open(endpoint) {
            Ok(client) => break client,
            Err(error)
                if matches!(error.raw_os_error(), Some(2 | 231)) && Instant::now() < deadline =>
            {
                sleep(Duration::from_millis(10)).await;
            }
            Err(_) => return Err(ControlError::EndpointUnavailable),
        }
    };
    write_frame(&mut client, request).await?;
    read_frame(&mut client).await
}

#[cfg(unix)]
async fn request_over_local_transport(
    endpoint: &str,
    request: &ControlRequest,
    _deadline: Duration,
) -> Result<ControlResponse, ControlError> {
    let mut stream = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|_| ControlError::EndpointUnavailable)?;
    write_frame(&mut stream, request).await?;
    read_frame(&mut stream).await
}

async fn read_frame<T, S>(stream: &mut S) -> Result<T, ControlError>
where
    T: DeserializeOwned,
    S: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|_| ControlError::InvalidFrame)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 {
        return Err(ControlError::InvalidFrame);
    }
    if length > MAX_CONTROL_FRAME_BYTES {
        return Err(ControlError::FrameTooLarge);
    }
    let mut encoded = vec![0_u8; length];
    stream
        .read_exact(&mut encoded)
        .await
        .map_err(|_| ControlError::InvalidFrame)?;
    serde_json::from_slice(&encoded).map_err(|_| ControlError::InvalidFrame)
}

async fn write_frame<T, S>(stream: &mut S, value: &T) -> Result<(), ControlError>
where
    T: Serialize,
    S: AsyncWrite + Unpin,
{
    let encoded = serde_json::to_vec(value).map_err(|_| ControlError::InvalidFrame)?;
    if encoded.is_empty() || encoded.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(ControlError::FrameTooLarge);
    }
    let length = u32::try_from(encoded.len()).map_err(|_| ControlError::FrameTooLarge)?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|_| ControlError::EndpointUnavailable)?;
    stream
        .write_all(&encoded)
        .await
        .map_err(|_| ControlError::EndpointUnavailable)?;
    stream
        .flush()
        .await
        .map_err(|_| ControlError::EndpointUnavailable)
}

fn read_descriptor(path: &Path) -> Result<ControlDescriptor, ControlError> {
    read_descriptor_with_bytes(path).map(|(_, descriptor)| descriptor)
}

fn read_descriptor_with_bytes(path: &Path) -> Result<(Vec<u8>, ControlDescriptor), ControlError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ControlError::DescriptorInvalid)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ControlError::UnsafeArtifact);
    }
    if metadata.len() == 0 || metadata.len() > MAX_DESCRIPTOR_BYTES as u64 {
        return Err(ControlError::DescriptorInvalid);
    }
    let file = File::open(path).map_err(|_| ControlError::DescriptorInvalid)?;
    let mut encoded = Vec::new();
    file.take((MAX_DESCRIPTOR_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| ControlError::DescriptorInvalid)?;
    if encoded.is_empty() || encoded.len() > MAX_DESCRIPTOR_BYTES {
        return Err(ControlError::DescriptorInvalid);
    }
    let descriptor: ControlDescriptor =
        serde_json::from_slice(&encoded).map_err(|_| ControlError::DescriptorInvalid)?;
    validate_descriptor(&descriptor)?;
    Ok((encoded, descriptor))
}

fn validate_descriptor(descriptor: &ControlDescriptor) -> Result<(), ControlError> {
    if descriptor.version == 0
        || descriptor.process_id == 0
        || descriptor.token.len() != 64
        || !descriptor
            .token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !valid_endpoint(&descriptor.endpoint)
    {
        return Err(ControlError::DescriptorInvalid);
    }
    Ok(())
}

#[cfg(windows)]
fn valid_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with(r"\\.\pipe\vibemux_") && endpoint.len() <= 256
}

#[cfg(windows)]
fn endpoint_kind(_endpoint: &str) -> ControlEndpointKind {
    ControlEndpointKind::WindowsNamedPipe
}

#[cfg(unix)]
fn valid_endpoint(endpoint: &str) -> bool {
    Path::new(endpoint).is_absolute() && endpoint.ends_with(".sock")
}

#[cfg(unix)]
fn endpoint_kind(_endpoint: &str) -> ControlEndpointKind {
    ControlEndpointKind::UnixSocket
}

fn supported_control_version(version: u32) -> bool {
    matches!(
        version,
        LEGACY_CONTROL_PROTOCOL_VERSION | CONTROL_PROTOCOL_VERSION
    )
}

fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= 128
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn constant_time_token_eq(expected: &str, supplied: &str) -> bool {
    bool::from(expected.as_bytes().ct_eq(supplied.as_bytes()))
}

fn control_token() -> Result<String, ControlError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|_| ControlError::TokenUnavailable)?;
    let mut token = String::with_capacity(64);
    for byte in random {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}

#[cfg(windows)]
fn endpoint_name(_runtime_dir: &Path) -> String {
    format!(r"\\.\pipe\vibemux_{}", Uuid::new_v4().simple())
}

#[cfg(unix)]
fn endpoint_name(runtime_dir: &Path) -> String {
    runtime_dir
        .join(format!("vibemux_control_{}.sock", Uuid::new_v4().simple()))
        .to_string_lossy()
        .into_owned()
}

fn remote_error(code: String) -> ControlError {
    match code.as_str() {
        "control_frame_too_large" => ControlError::FrameTooLarge,
        "control_invalid_frame" => ControlError::InvalidFrame,
        "control_invalid_request" => ControlError::InvalidRequest,
        "control_unauthorized" => ControlError::Unauthorized,
        "control_unsupported_version" => ControlError::UnsupportedVersion,
        "writer_invalid_capacity"
        | "writer_lock_held"
        | "writer_lock_io"
        | "writer_queue_full"
        | "writer_stopped"
        | "writer_response_timeout"
        | "writer_store_error"
        | "writer_thread_terminated" => ControlError::Remote { code },
        _ => ControlError::Remote {
            code: "control_remote_error".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_debug_redacts_token() {
        let descriptor = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            process_id: 1,
            endpoint: "local_endpoint".to_string(),
            token: "a".repeat(64),
        };
        let rendered = format!("{descriptor:?}");
        assert!(rendered.contains("[redacted]"));
        assert!(!rendered.contains(&descriptor.token));

        let request = ControlRequest {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: "request_1".to_string(),
            token: descriptor.token,
            operation: ControlOperation::Health,
        };
        let rendered = format!("{request:?}");
        assert!(rendered.contains("[redacted]"));
        assert!(!rendered.contains(&request.token));
    }

    #[test]
    fn token_comparison_checks_content_and_length() {
        assert!(constant_time_token_eq("abcd", "abcd"));
        assert!(!constant_time_token_eq("abcd", "abce"));
        assert!(!constant_time_token_eq("abcd", "abc"));
        assert!(!constant_time_token_eq("abcd", "abcde"));
    }

    #[test]
    fn generated_token_is_256_bit_hex() {
        let token = control_token().expect("OS random token");
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn unknown_remote_error_text_is_not_reflected() {
        let secret_like_code = "a".repeat(64);
        let error = remote_error(secret_like_code.clone());
        assert_eq!(error.code(), "control_remote_error");
        assert!(!format!("{error:?}").contains(&secret_like_code));
    }

    #[test]
    fn descriptor_guard_does_not_remove_replacement() {
        let temp = tempfile::tempdir().expect("temp directory");
        let descriptor_path = temp.path().join(DESCRIPTOR_FILE_NAME);
        let owner = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            process_id: 1,
            endpoint: endpoint_name(temp.path()),
            token: "a".repeat(64),
        };
        let guard =
            DescriptorGuard::publish(&descriptor_path, owner.clone()).expect("publish descriptor");
        let replacement = ControlDescriptor {
            token: "b".repeat(64),
            ..owner
        };
        std::fs::write(
            &descriptor_path,
            serde_json::to_vec(&replacement).expect("encode replacement"),
        )
        .expect("replace descriptor contents");

        drop(guard);
        assert!(descriptor_path.exists());
    }

    #[test]
    fn recovery_snapshot_is_redacted_and_removes_only_unchanged_descriptor() {
        let temp = tempfile::tempdir().expect("temp directory");
        let descriptor_path = temp.path().join(DESCRIPTOR_FILE_NAME);
        let owner = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            process_id: 42,
            endpoint: endpoint_name(temp.path()),
            token: "a".repeat(64),
        };
        let guard =
            DescriptorGuard::publish(&descriptor_path, owner.clone()).expect("publish descriptor");
        let snapshot = inspect_control_artifact(&descriptor_path)
            .expect("inspect descriptor")
            .expect("descriptor snapshot");
        assert_eq!(snapshot.info().process_id, 42);
        let rendered = format!("{snapshot:?}");
        assert!(!rendered.contains(&owner.token));
        assert!(!rendered.contains(&owner.endpoint));
        snapshot
            .remove_if_unchanged()
            .expect("remove unchanged descriptor");
        assert!(!descriptor_path.exists());
        drop(guard);

        let replacement_guard = DescriptorGuard::publish(&descriptor_path, owner.clone())
            .expect("republish descriptor");
        let snapshot = inspect_control_artifact(&descriptor_path)
            .expect("inspect replacement descriptor")
            .expect("replacement snapshot");
        let replacement = ControlDescriptor {
            token: "b".repeat(64),
            ..owner
        };
        std::fs::write(
            &descriptor_path,
            serde_json::to_vec(&replacement).expect("encode replacement"),
        )
        .expect("replace descriptor contents");
        assert_eq!(
            snapshot
                .remove_if_unchanged()
                .expect_err("changed descriptor must fail"),
            ControlError::ArtifactChanged
        );
        assert!(descriptor_path.exists());
        drop(replacement_guard);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_guard_does_not_remove_replacement() {
        let temp = tempfile::tempdir().expect("temp directory");
        let socket_path = temp.path().join("replacement.sock");
        let descriptor_path = temp.path().join(DESCRIPTOR_FILE_NAME);
        let owner = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            process_id: 1,
            endpoint: socket_path.to_string_lossy().into_owned(),
            token: "a".repeat(64),
        };
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind socket");
        let descriptor_guard =
            DescriptorGuard::publish(&descriptor_path, owner.clone()).expect("publish descriptor");
        let mut guard = SocketPathGuard::unpublished(socket_path.clone());
        guard.bind_owner(descriptor_path.clone(), owner.clone());
        drop(listener);
        std::fs::remove_file(&socket_path).expect("remove original socket");
        let replacement_listener =
            tokio::net::UnixListener::bind(&socket_path).expect("bind replacement socket");
        let replacement = ControlDescriptor {
            token: "b".repeat(64),
            ..owner
        };
        std::fs::write(
            &descriptor_path,
            serde_json::to_vec(&replacement).expect("encode replacement"),
        )
        .expect("replace descriptor contents");

        drop(guard);
        assert!(socket_path.exists());
        drop(descriptor_guard);
        assert!(descriptor_path.exists());
        drop(replacement_listener);
        std::fs::remove_file(&socket_path).expect("remove replacement socket");
        std::fs::remove_file(&descriptor_path).expect("remove replacement descriptor");
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_payload_read() {
        let (mut client, mut server) = tokio::io::duplex(8);
        client
            .write_all(&((MAX_CONTROL_FRAME_BYTES + 1) as u32).to_be_bytes())
            .await
            .expect("write frame prefix");
        let error = read_frame::<ControlRequest, _>(&mut server)
            .await
            .expect_err("oversized frame must fail");
        assert_eq!(error, ControlError::FrameTooLarge);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_control_health_auth_version_and_shutdown_round_trip() {
        let temp = tempfile::tempdir().expect("temp directory");
        let database_path = temp.path().join("vibemux.sqlite3");
        let runtime_dir = temp.path().join(".vibemux");
        let server = DaemonControlServer::start(&database_path, &runtime_dir)
            .await
            .expect("start control server");
        let descriptor_path = server.descriptor_path().to_path_buf();
        let writer_lock_path = server.writer_lock_path().to_path_buf();
        assert!(descriptor_path.is_file());
        assert!(writer_lock_path.is_file());

        let client = ControlClient::from_descriptor(&descriptor_path).expect("control client");
        #[cfg(windows)]
        assert!(client.descriptor.endpoint.starts_with(r"\\.\pipe\vibemux_"));
        #[cfg(unix)]
        let socket_path = {
            use std::os::unix::fs::PermissionsExt;

            assert!(client.descriptor.endpoint.ends_with(".sock"));
            let mode = std::fs::metadata(&descriptor_path)
                .expect("descriptor metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            PathBuf::from(&client.descriptor.endpoint)
        };
        let health = client.health().await.expect("health response");
        assert!(health.healthy);
        assert_eq!(health.process_id, std::process::id());

        let mut wrong_token = client.clone();
        wrong_token.descriptor.token = "f".repeat(64);
        assert_eq!(
            wrong_token
                .health()
                .await
                .expect_err("wrong token must fail"),
            ControlError::Unauthorized
        );

        let wrong_version = ControlRequest {
            version: CONTROL_PROTOCOL_VERSION + 1,
            request_id: Uuid::new_v4().to_string(),
            token: client.descriptor.token.clone(),
            operation: ControlOperation::Health,
        };
        assert_eq!(
            client
                .request_raw(wrong_version)
                .await
                .expect_err("wrong version must fail"),
            ControlError::UnsupportedVersion
        );

        server.shutdown().await.expect("clean shutdown");
        assert!(!descriptor_path.exists());
        assert!(!writer_lock_path.exists());
        #[cfg(windows)]
        assert!(
            tokio::net::windows::named_pipe::ClientOptions::new()
                .open(&client.descriptor.endpoint)
                .is_err()
        );
        #[cfg(unix)]
        {
            assert!(!socket_path.exists());
            assert!(tokio::net::UnixStream::connect(&socket_path).await.is_err());
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_client_does_not_block_other_control_traffic() {
        // Regression for serial accept: a client that connects and goes quiet
        // must not stop the server from answering health on another
        // connection.
        let temp = tempfile::tempdir().expect("temp directory");
        let server = DaemonControlServer::start(&temp.path().join("state.sqlite3"), temp.path())
            .await
            .expect("start control server");
        let descriptor_path = server.descriptor_path().to_path_buf();
        let client = ControlClient::from_descriptor(&descriptor_path).expect("control client");
        let endpoint = client.descriptor.endpoint.clone();

        #[cfg(windows)]
        let idle = tokio::spawn(async move {
            let stream = tokio::net::windows::named_pipe::ClientOptions::new()
                .open(&endpoint)
                .expect("idle client connects");
            // Hold the connection without ever sending a request.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(stream);
        });

        #[cfg(unix)]
        let idle = tokio::spawn(async move {
            let stream = tokio::net::UnixStream::connect(&endpoint)
                .await
                .expect("idle client connects");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(stream);
        });

        // Give the idle client time to occupy a connection first.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let started = std::time::Instant::now();
        let health = tokio::time::timeout(std::time::Duration::from_secs(2), client.health())
            .await
            .expect("health must not time out behind an idle client")
            .expect("health response");
        assert!(health.healthy);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        idle.abort();
        server.shutdown().await.expect("clean shutdown");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plugin_status_requires_auth_and_legacy_health_remains_compatible() {
        let temp = tempfile::tempdir().expect("temp");
        let server = DaemonControlServer::start(&temp.path().join("state.sqlite3"), temp.path())
            .await
            .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        assert!(
            client
                .plugin_status()
                .await
                .expect("empty status")
                .is_empty()
        );
        let error = client
            .request_raw(ControlRequest {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: "status_denied".to_string(),
                token: "wrong".to_string(),
                operation: ControlOperation::PluginStatus,
            })
            .await
            .expect_err("unauthenticated status");
        assert_eq!(error, ControlError::Unauthorized);
        let mut legacy = client.clone();
        legacy.descriptor.version = LEGACY_CONTROL_PROTOCOL_VERSION;
        assert!(
            legacy
                .health()
                .await
                .expect("v1 health request and response")
                .healthy
        );
        assert_eq!(
            legacy
                .plugin_status()
                .await
                .expect_err("status requires v2"),
            ControlError::UnsupportedVersion
        );
        let error = legacy
            .request_raw(ControlRequest {
                version: LEGACY_CONTROL_PROTOCOL_VERSION,
                request_id: "legacy_auth".to_string(),
                token: "wrong".to_string(),
                operation: ControlOperation::Health,
            })
            .await
            .expect_err("v1 auth failure");
        assert_eq!(error, ControlError::Unauthorized);
        legacy.shutdown().await.expect("v1 shutdown");
        server.wait().await.expect("joined shutdown");
    }

    #[test]
    fn control_contract_has_no_plugin_or_task_mutation_operations() {
        for operation in [
            "plugin_start",
            "plugin_stop",
            "plugin_cancel",
            "task_create",
            "run_update",
        ] {
            let request = serde_json::json!({"version":2,"request_id":"denied","token":"redacted","operation":operation});
            assert!(serde_json::from_value::<ControlRequest>(request).is_err());
        }
        let status = PluginStatus {
            plugin_id: "p".repeat(128),
            state: crate::plugin_registry::PluginState::Quarantined,
            restarts: 16,
            max_restarts: 16,
            session_id: Some("s".repeat(128)),
            process_id: Some(u32::MAX),
            last_error_code: Some("plugin_registry_unsupported_message".to_string()),
            graceful_shutdown: Some(false),
        };
        let payload = ControlResponse::success(
            "r".repeat(128),
            ControlPayload::PluginStatus(vec![status; crate::plugin_registry::HARD_MAX_PLUGINS]),
        );
        assert!(serde_json::to_vec(&payload).expect("status JSON").len() < MAX_CONTROL_FRAME_BYTES);
    }
}
