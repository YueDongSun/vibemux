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

use crate::{WriterError, WriterHealth, WriterWorker};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_DESCRIPTOR_BYTES: usize = 4 * 1024;
pub const CONTROL_DEADLINE: Duration = Duration::from_secs(10);
pub const DESCRIPTOR_FILE_NAME: &str = "control.json";
const PEER_CLOSE_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOperation {
    Health,
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
    Health(WriterHealth),
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
    endpoint: String,
    token: String,
}

impl fmt::Debug for ControlDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlDescriptor")
            .field("version", &self.version)
            .field("endpoint", &self.endpoint)
            .field("token", &"[redacted]")
            .finish()
    }
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
    #[error("daemon writer operation failed: {code}")]
    Writer { code: String },
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
            Self::Writer { code } | Self::Remote { code } => code,
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
    token: String,
}

pub struct DaemonControlServer {
    descriptor_path: PathBuf,
    writer_lock_path: PathBuf,
    task: Option<JoinHandle<Result<(), ControlError>>>,
}

impl DaemonControlServer {
    pub async fn start(database_path: &Path, runtime_dir: &Path) -> Result<Self, ControlError> {
        std::fs::create_dir_all(runtime_dir).map_err(|_| ControlError::EndpointUnavailable)?;
        let runtime_dir =
            std::fs::canonicalize(runtime_dir).map_err(|_| ControlError::EndpointUnavailable)?;
        let descriptor_path = runtime_dir.join(DESCRIPTOR_FILE_NAME);
        if descriptor_path.exists() {
            return Err(ControlError::DescriptorExists);
        }

        let database_path = database_path.to_path_buf();
        let writer = tokio::task::spawn_blocking(move || WriterWorker::start(&database_path))
            .await
            .map_err(|_| ControlError::ServerTerminated)??;
        let writer_lock_path = writer.lock_path().to_path_buf();
        let endpoint = endpoint_name(&runtime_dir);
        let descriptor = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
            endpoint: endpoint.clone(),
            token: control_token()?,
        };

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;

            let listener = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&endpoint)
                .map_err(|_| ControlError::EndpointUnavailable)?;
            let descriptor_guard = DescriptorGuard::publish(&descriptor_path, descriptor.clone())?;
            let state = Arc::new(ServerState {
                writer: Mutex::new(Some(writer)),
                token: descriptor.token,
            });
            let task = tokio::spawn(async move {
                let _descriptor_guard = descriptor_guard;
                run_server(listener, endpoint, state).await
            });
            return Ok(Self {
                descriptor_path,
                writer_lock_path,
                task: Some(task),
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
            let state = Arc::new(ServerState {
                writer: Mutex::new(Some(writer)),
                token: descriptor.token,
            });
            let task = tokio::spawn(async move {
                let _descriptor_guard = descriptor_guard;
                let _socket_guard = socket_guard;
                run_server(listener, state).await
            });
            return Ok(Self {
                descriptor_path,
                writer_lock_path,
                task: Some(task),
            });
        }

        #[allow(unreachable_code)]
        Err(ControlError::EndpointUnavailable)
    }

    #[must_use]
    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    #[must_use]
    pub fn writer_lock_path(&self) -> &Path {
        &self.writer_lock_path
    }

    pub async fn shutdown(mut self) -> Result<(), ControlError> {
        ControlClient::from_descriptor(&self.descriptor_path)?
            .shutdown()
            .await?;
        let task = self.task.take().ok_or(ControlError::ServerTerminated)?;
        match timeout(CONTROL_DEADLINE, task).await {
            Err(_) => Err(ControlError::Deadline),
            Ok(Err(_)) => Err(ControlError::ServerTerminated),
            Ok(Ok(result)) => result,
        }
    }
}

impl Drop for DaemonControlServer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControlClient {
    descriptor: ControlDescriptor,
}

impl ControlClient {
    pub fn from_descriptor(path: &Path) -> Result<Self, ControlError> {
        let descriptor = read_descriptor(path)?;
        if descriptor.version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::UnsupportedVersion);
        }
        Ok(Self { descriptor })
    }

    pub async fn health(&self) -> Result<WriterHealth, ControlError> {
        match self.request(ControlOperation::Health).await? {
            ControlPayload::Health(health) => Ok(health),
            ControlPayload::ShutdownAccepted => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn shutdown(&self) -> Result<(), ControlError> {
        match self.request(ControlOperation::Shutdown).await? {
            ControlPayload::ShutdownAccepted => Ok(()),
            ControlPayload::Health(_) => Err(ControlError::InvalidFrame),
        }
    }

    async fn request(&self, operation: ControlOperation) -> Result<ControlPayload, ControlError> {
        self.request_raw(ControlRequest {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            token: self.descriptor.token.clone(),
            operation,
        })
        .await
    }

    async fn request_raw(&self, request: ControlRequest) -> Result<ControlPayload, ControlError> {
        let response = timeout(
            CONTROL_DEADLINE,
            request_over_local_transport(&self.descriptor.endpoint, &request),
        )
        .await
        .map_err(|_| ControlError::Deadline)??;
        if response.version != CONTROL_PROTOCOL_VERSION {
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
        let response = ControlResponse::error(request.request_id, &ControlError::Unauthorized);
        send_response(stream, &response).await;
        return false;
    }
    if !valid_request_id(&request.request_id) {
        let response = ControlResponse::error(request.request_id, &ControlError::InvalidRequest);
        send_response(stream, &response).await;
        return false;
    }
    if request.version != CONTROL_PROTOCOL_VERSION {
        let response =
            ControlResponse::error(request.request_id, &ControlError::UnsupportedVersion);
        send_response(stream, &response).await;
        return false;
    }

    let request_id = request.request_id;
    let (response, should_shutdown) = match request.operation {
        ControlOperation::Health => match writer_health(state).await {
            Ok(health) => (
                ControlResponse::success(request_id, ControlPayload::Health(health)),
                false,
            ),
            Err(error) => (ControlResponse::error(request_id, &error), false),
        },
        ControlOperation::Shutdown => match shutdown_writer(state).await {
            Ok(()) => (
                ControlResponse::success(request_id, ControlPayload::ShutdownAccepted),
                true,
            ),
            Err(error) => (ControlResponse::error(request_id, &error), true),
        },
    };
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

async fn writer_health(state: Arc<ServerState>) -> Result<WriterHealth, ControlError> {
    tokio::task::spawn_blocking(move || {
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
    .map_err(|_| ControlError::ServerTerminated)?
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
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        listener
            .connect()
            .await
            .map_err(|_| ControlError::EndpointUnavailable)?;
        let should_shutdown = handle_connection(&mut listener, state.clone()).await;
        if should_shutdown {
            return Ok(());
        }
        listener
            .disconnect()
            .map_err(|_| ControlError::EndpointUnavailable)?;
        listener = ServerOptions::new()
            .create(&endpoint)
            .map_err(|_| ControlError::EndpointUnavailable)?;
    }
}

#[cfg(unix)]
async fn run_server(
    listener: tokio::net::UnixListener,
    state: Arc<ServerState>,
) -> Result<(), ControlError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| ControlError::EndpointUnavailable)?;
        if handle_connection(&mut stream, state.clone()).await {
            return Ok(());
        }
    }
}

#[cfg(windows)]
async fn request_over_local_transport(
    endpoint: &str,
    request: &ControlRequest,
) -> Result<ControlResponse, ControlError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let deadline = Instant::now() + CONTROL_DEADLINE;
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
    Ok(descriptor)
}

fn validate_descriptor(descriptor: &ControlDescriptor) -> Result<(), ControlError> {
    if descriptor.version == 0
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

#[cfg(unix)]
fn valid_endpoint(endpoint: &str) -> bool {
    Path::new(endpoint).is_absolute() && endpoint.ends_with(".sock")
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

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_guard_does_not_remove_replacement() {
        let temp = tempfile::tempdir().expect("temp directory");
        let socket_path = temp.path().join("replacement.sock");
        let descriptor_path = temp.path().join(DESCRIPTOR_FILE_NAME);
        let owner = ControlDescriptor {
            version: CONTROL_PROTOCOL_VERSION,
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
}
