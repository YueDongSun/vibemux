//! Bounded loopback gRPC transport using the official SDK handler and wire types.
//!
//! This module owns only sockets and streams. Authentication, authorization and
//! task state remain in the same TaskHandler/TaskBackend used by HTTP transports.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use a2a_grpc::{GrpcHandler, errors::status_to_a2a_error};
use a2a_pb::proto::a2a_service_server::A2aService;
use a2a_pb::{pbconv, proto, proto::a2a_service_client::A2aServiceClient};
use futures::{StreamExt, future};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    Code, Request,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint, Server, server::Connected},
};
use tonic_types::{ErrorDetails, StatusExt};

use crate::{
    task_contract::{
        MAX_TASK_WIRE_BYTES, PeerCredential, TASK_CALL_DEADLINE, TASK_SHUTDOWN_DEADLINE,
        TaskBackend, TaskEventStream, TaskGatewayError, TaskList, TaskListRequest, TaskReply,
        TaskRequest, TaskSnapshot, TaskState, validate_task_identifier,
    },
    task_server::{TaskHandler, TaskServerConfig},
    task_wire::{
        apply_stream_response, local_error, reply_from_wire, request_to_wire, snapshot_from_wire,
        state_to_wire,
    },
};

const MAX_GRPC_CONNECTIONS: usize = 32;
const MAX_GRPC_STREAMS_PER_CONNECTION: u32 = 32;
const MAX_GRPC_QUEUED_REQUESTS: usize = 32;

/// An owned, authenticated loopback-only gRPC listener.
pub struct GrpcTaskServer {
    address: SocketAddr,
    handler: Arc<TaskHandler>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: Option<JoinHandle<Result<(), tonic::transport::Error>>>,
}

impl GrpcTaskServer {
    pub async fn start(
        config: TaskServerConfig,
        backend: Arc<dyn TaskBackend>,
    ) -> Result<Self, TaskGatewayError> {
        let handler = Arc::new(TaskHandler::new(config, backend)?);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| TaskGatewayError::Server)?;
        let address = listener
            .local_addr()
            .map_err(|_| TaskGatewayError::Server)?;
        let connection_budget = Arc::new(Semaphore::new(MAX_GRPC_CONNECTIONS));
        let incoming = TcpListenerStream::new(listener).filter_map(move |result| {
            future::ready(match result {
                Ok(socket) => connection_budget
                    .clone()
                    .try_acquire_owned()
                    .ok()
                    .map(|permit| {
                        Ok(BoundedConnection {
                            socket,
                            _permit: permit,
                        })
                    }),
                Err(error) => Some(Err(error)),
            })
        });
        let service = proto::a2a_service_server::A2aServiceServer::new(GrpcStatusCompatibility {
            inner: GrpcHandler::new(handler.clone()),
        })
        .max_decoding_message_size(MAX_TASK_WIRE_BYTES)
        .max_encoding_message_size(MAX_TASK_WIRE_BYTES);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            Server::builder()
                .concurrency_limit_per_connection(MAX_GRPC_QUEUED_REQUESTS)
                .max_concurrent_streams(MAX_GRPC_STREAMS_PER_CONNECTION)
                .timeout(TASK_CALL_DEADLINE)
                .add_service(service)
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        Ok(Self {
            address,
            handler,
            shutdown_tx: Some(shutdown_tx),
            server_task: Some(server_task),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn shutdown(mut self) -> Result<(), TaskGatewayError> {
        self.handler.shutdown();
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        let Some(mut server_task) = self.server_task.take() else {
            return Ok(());
        };
        match timeout(TASK_SHUTDOWN_DEADLINE, &mut server_task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(_) => Err(TaskGatewayError::Server),
            Err(_) => {
                server_task.abort();
                let _ = server_task.await;
                Err(TaskGatewayError::Deadline)
            }
        }
    }
}

impl Drop for GrpcTaskServer {
    fn drop(&mut self) {
        self.handler.shutdown();
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(server_task) = self.server_task.take() {
            server_task.abort();
        }
    }
}

/// A connection holds its global budget until tonic drops the owned socket.
struct BoundedConnection {
    socket: TcpStream,
    _permit: OwnedSemaphorePermit,
}

impl Connected for BoundedConnection {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for BoundedConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_read(context, buffer)
    }
}

impl AsyncWrite for BoundedConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.socket).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(context)
    }
}

/// A bounded gRPC client for a caller-selected and policy-validated interface.
///
/// No discovery, implicit redirection, non-loopback connection or automatic retry
/// is performed here. The caller chooses gRPC from a validated Agent Card.
pub struct GrpcTaskClient {
    client: A2aServiceClient<Channel>,
    authorization: MetadataValue<Ascii>,
}

impl GrpcTaskClient {
    pub async fn connect(
        endpoint: &str,
        credential: PeerCredential,
    ) -> Result<Self, TaskGatewayError> {
        let endpoint = validate_grpc_endpoint(endpoint)?;
        credential.validate()?;
        let authorization = format!("Bearer {}", credential.bearer_token)
            .parse()
            .map_err(|_| TaskGatewayError::InvalidRequest)?;
        let channel_endpoint = Endpoint::from_shared(endpoint.to_string())
            .map_err(|_| TaskGatewayError::InvalidRequest)?
            .connect_timeout(TASK_CALL_DEADLINE)
            .timeout(TASK_CALL_DEADLINE)
            .buffer_size(MAX_GRPC_QUEUED_REQUESTS)
            .concurrency_limit(MAX_GRPC_QUEUED_REQUESTS);
        let channel = timeout(TASK_CALL_DEADLINE, channel_endpoint.connect())
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(|_| TaskGatewayError::Transport)?;
        Ok(Self {
            client: A2aServiceClient::new(channel)
                .max_decoding_message_size(MAX_TASK_WIRE_BYTES)
                .max_encoding_message_size(MAX_TASK_WIRE_BYTES),
            authorization,
        })
    }

    fn request<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        request
            .metadata_mut()
            .insert("authorization", self.authorization.clone());
        request
            .metadata_mut()
            .insert("a2a-version", MetadataValue::from_static("1.0"));
        request.set_timeout(TASK_CALL_DEADLINE);
        request
    }

    pub async fn send(&self, request: &TaskRequest) -> Result<TaskReply, TaskGatewayError> {
        let native = request_to_wire(request)?;
        let grpc_request = self.request(pbconv::to_proto_send_message_request(&native));
        let mut client = self.client.clone();
        let response = timeout(TASK_CALL_DEADLINE, client.send_message(grpc_request))
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(grpc_error)?;
        let native = pbconv::from_proto_send_message_response(response.get_ref())
            .ok_or(TaskGatewayError::Protocol)?;
        let reply = reply_from_wire(native)?;
        let context = match &reply {
            TaskReply::Task(task) => &task.context_id,
            TaskReply::Message(message) => &message.context_id,
        };
        if context != &request.context_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(reply)
    }

    pub async fn get(&self, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let native = a2a::GetTaskRequest {
            id: task_id.to_string(),
            history_length: None,
            tenant: None,
        };
        let request = self.request(pbconv::to_proto_get_task_request(&native));
        let mut client = self.client.clone();
        let response = timeout(TASK_CALL_DEADLINE, client.get_task(request))
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(grpc_error)?;
        let snapshot = snapshot_from_wire(pbconv::from_proto_task(response.get_ref()))?;
        if snapshot.task_id != task_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(snapshot)
    }

    pub async fn list(&self, request: &TaskListRequest) -> Result<TaskList, TaskGatewayError> {
        request.validate()?;
        let native = a2a::ListTasksRequest {
            context_id: request.context_id.clone(),
            status: request.state.map(state_to_wire),
            page_size: Some(
                i32::try_from(request.page_size).map_err(|_| TaskGatewayError::InvalidRequest)?,
            ),
            page_token: request.page_token.clone(),
            history_length: Some(0),
            status_timestamp_after: None,
            include_artifacts: Some(request.include_artifacts),
            tenant: None,
        };
        let mut client = self.client.clone();
        let response = timeout(
            TASK_CALL_DEADLINE,
            client.list_tasks(self.request(pbconv::to_proto_list_tasks_request(&native))),
        )
        .await
        .map_err(|_| TaskGatewayError::Deadline)?
        .map_err(grpc_error)?;
        let native = pbconv::from_proto_list_tasks_response(response.get_ref());
        let page = TaskList {
            tasks: native
                .tasks
                .into_iter()
                .map(snapshot_from_wire)
                .collect::<Result<Vec<_>, _>>()?,
            next_page_token: (!native.next_page_token.is_empty()).then_some(native.next_page_token),
            total_size: u32::try_from(native.total_size).map_err(|_| TaskGatewayError::Protocol)?,
        };
        page.validate()?;
        if page.tasks.len() > request.page_size as usize {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(page)
    }

    pub async fn cancel(&self, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let native = a2a::CancelTaskRequest {
            id: task_id.to_string(),
            metadata: None,
            tenant: None,
        };
        let request = self.request(pbconv::to_proto_cancel_task_request(&native));
        let mut client = self.client.clone();
        let response = timeout(TASK_CALL_DEADLINE, client.cancel_task(request))
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(grpc_error)?;
        let snapshot = snapshot_from_wire(pbconv::from_proto_task(response.get_ref()))?;
        if snapshot.task_id != task_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(snapshot)
    }
    pub async fn send_stream(
        &self,
        request: &TaskRequest,
    ) -> Result<TaskEventStream, TaskGatewayError> {
        let native = request_to_wire(request)?;
        let grpc_request = self.request(pbconv::to_proto_send_message_request(&native));
        let mut client = self.client.clone();
        let response = timeout(
            TASK_CALL_DEADLINE,
            client.send_streaming_message(grpc_request),
        )
        .await
        .map_err(|_| TaskGatewayError::Deadline)?
        .map_err(grpc_error)?;
        Ok(snapshot_stream(
            response.into_inner(),
            request.task_id.clone(),
            Some(request.context_id.clone()),
            true,
        ))
    }

    pub async fn subscribe(&self, task_id: &str) -> Result<TaskEventStream, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let native = a2a::SubscribeToTaskRequest {
            id: task_id.to_string(),
            tenant: None,
        };
        let request = self.request(pbconv::to_proto_subscribe_to_task_request(&native));
        let mut client = self.client.clone();
        let response = timeout(TASK_CALL_DEADLINE, client.subscribe_to_task(request))
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(grpc_error)?;
        Ok(snapshot_stream(
            response.into_inner(),
            Some(task_id.to_string()),
            None,
            false,
        ))
    }
}

fn validate_grpc_endpoint(endpoint: &str) -> Result<String, TaskGatewayError> {
    if endpoint.len() > 2048 {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let normalized = if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    };
    let parsed = url::Url::parse(&normalized).map_err(|_| TaskGatewayError::InvalidRequest)?;
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if !loopback
        || parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
        || parsed.port_or_known_default().is_none_or(|port| port == 0)
    {
        return Err(TaskGatewayError::InvalidRequest);
    }
    Ok(normalized)
}

fn grpc_error(status: tonic::Status) -> TaskGatewayError {
    match status.code() {
        Code::DeadlineExceeded => TaskGatewayError::Deadline,
        Code::Unauthenticated => TaskGatewayError::Unauthorized,
        Code::PermissionDenied => TaskGatewayError::Forbidden,
        Code::ResourceExhausted => TaskGatewayError::Busy,
        Code::Unavailable | Code::Cancelled => TaskGatewayError::Transport,
        _ => local_error(&status_to_a2a_error(&status)),
    }
}

struct GrpcStreamState {
    stream: tonic::Streaming<proto::StreamResponse>,
    previous: Option<TaskSnapshot>,
    expected_task_id: Option<String>,
    expected_context_id: Option<String>,
    ended: bool,
    end_on_interrupted: bool,
}

fn snapshot_stream(
    stream: tonic::Streaming<proto::StreamResponse>,
    expected_task_id: Option<String>,
    expected_context_id: Option<String>,
    end_on_interrupted: bool,
) -> TaskEventStream {
    let state = GrpcStreamState {
        stream,
        previous: None,
        expected_task_id,
        expected_context_id,
        ended: false,
        end_on_interrupted,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        if state.ended {
            return None;
        }
        let result = match timeout(TASK_CALL_DEADLINE, state.stream.message()).await {
            Err(_) => Err(TaskGatewayError::Deadline),
            Ok(Err(error)) => Err(grpc_error(error)),
            Ok(Ok(None)) => Err(TaskGatewayError::Protocol),
            Ok(Ok(Some(event))) => match pbconv::from_proto_stream_response(&event) {
                None => Err(TaskGatewayError::Protocol),
                Some(event) => apply_stream_response(&mut state.previous, event),
            },
        };
        let result = result.and_then(|snapshot| {
            if state
                .expected_task_id
                .as_ref()
                .is_some_and(|id| *id != snapshot.task_id)
                || state
                    .expected_context_id
                    .as_ref()
                    .is_some_and(|id| *id != snapshot.context_id)
            {
                return Err(TaskGatewayError::Protocol);
            }
            state.expected_task_id = Some(snapshot.task_id.clone());
            state.expected_context_id = Some(snapshot.context_id.clone());
            Ok(snapshot)
        });
        state.ended = match &result {
            Ok(snapshot) => {
                snapshot.state.is_terminal()
                    || (state.end_on_interrupted
                        && matches!(
                            snapshot.state,
                            TaskState::InputRequired | TaskState::AuthRequired
                        ))
            }
            Err(_) => true,
        };
        Some((result, state))
    }))
}

/// Compatibility boundary for demonstrated a2a-grpc 0.3.1 error mapping gaps.
/// All request, response and event conversions still run through GrpcHandler.
/// Match only known local/pinned-SDK errors; preserve unrelated transport errors.
struct GrpcStatusCompatibility {
    inner: GrpcHandler<TaskHandler>,
}

fn compatible_grpc_status(status: tonic::Status) -> tonic::Status {
    let selected = match (status.code(), status.message()) {
        (Code::NotFound, "a2a_task_not_found") => Some((Code::NotFound, "TASK_NOT_FOUND")),
        (Code::FailedPrecondition, "push notification not supported") => {
            Some((Code::Unimplemented, "PUSH_NOTIFICATION_NOT_SUPPORTED"))
        }
        (Code::FailedPrecondition, "a2a_task_unsupported") => {
            Some((Code::Unimplemented, "UNSUPPORTED_OPERATION"))
        }
        (Code::FailedPrecondition, message) if message.starts_with("version not supported:") => {
            Some((Code::Unimplemented, "VERSION_NOT_SUPPORTED"))
        }
        (Code::FailedPrecondition, message) if message.starts_with("task cannot be canceled:") => {
            Some((Code::FailedPrecondition, "TASK_NOT_CANCELABLE"))
        }
        (Code::Unknown, "a2a_task_unauthorized") => {
            return tonic::Status::unauthenticated("a2a_task_unauthorized");
        }
        (Code::Unknown, "a2a_task_forbidden") => {
            return tonic::Status::permission_denied("a2a_task_forbidden");
        }
        _ => None,
    };
    let Some((code, reason)) = selected else {
        return status;
    };
    tonic::Status::with_error_details(
        code,
        status.message(),
        ErrorDetails::with_error_info(
            reason,
            "a2a-protocol.org",
            std::collections::HashMap::<String, String>::new(),
        ),
    )
}

macro_rules! grpc_status_methods {
    ($($method:ident($request_type:ty) -> $response_type:ty;)*) => {
        #[tonic::async_trait]
        impl A2aService for GrpcStatusCompatibility {
            $(async fn $method(&self, request: Request<$request_type>) -> Result<tonic::Response<$response_type>, tonic::Status> {
                self.inner.$method(request).await.map_err(compatible_grpc_status)
            })*
            type SendStreamingMessageStream = Pin<Box<dyn futures::Stream<Item=Result<proto::StreamResponse,tonic::Status>>+Send+'static>>;
            async fn send_streaming_message(&self, request: Request<proto::SendMessageRequest>) -> Result<tonic::Response<Self::SendStreamingMessageStream>,tonic::Status> {
                let response=self.inner.send_streaming_message(request).await.map_err(compatible_grpc_status)?;
                Ok(tonic::Response::new(Box::pin(response.into_inner().map(|event|event.map_err(compatible_grpc_status)))))
            }
            type SubscribeToTaskStream = Pin<Box<dyn futures::Stream<Item=Result<proto::StreamResponse,tonic::Status>>+Send+'static>>;
            async fn subscribe_to_task(&self, request: Request<proto::SubscribeToTaskRequest>) -> Result<tonic::Response<Self::SubscribeToTaskStream>,tonic::Status> {
                let response=self.inner.subscribe_to_task(request).await.map_err(compatible_grpc_status)?;
                Ok(tonic::Response::new(Box::pin(response.into_inner().map(|event|event.map_err(compatible_grpc_status)))))
            }
        }
    }
}
grpc_status_methods! {
    send_message(proto::SendMessageRequest) -> proto::SendMessageResponse;
    get_task(proto::GetTaskRequest) -> proto::Task;
    list_tasks(proto::ListTasksRequest) -> proto::ListTasksResponse;
    cancel_task(proto::CancelTaskRequest) -> proto::Task;
    create_task_push_notification_config(proto::TaskPushNotificationConfig) -> proto::TaskPushNotificationConfig;
    get_task_push_notification_config(proto::GetTaskPushNotificationConfigRequest) -> proto::TaskPushNotificationConfig;
    list_task_push_notification_configs(proto::ListTaskPushNotificationConfigsRequest) -> proto::ListTaskPushNotificationConfigsResponse;
    get_extended_agent_card(proto::GetExtendedAgentCardRequest) -> proto::AgentCard;
    delete_task_push_notification_config(proto::DeleteTaskPushNotificationConfigRequest) -> ();
}
