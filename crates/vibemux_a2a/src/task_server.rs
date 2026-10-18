//! Authenticated loopback transports over one daemon-owned backend.
use crate::task_contract::*;
use crate::task_wire::*;
use a2a::*;
use a2a_server::{
    RequestHandler, ServiceParams, StaticAgentCard, agent_card::agent_card_router,
    jsonrpc::jsonrpc_router, rest::rest_router,
};
use async_trait::async_trait;
use axum::{Router, extract::DefaultBodyLimit};
use futures::{
    StreamExt,
    stream::{self, BoxStream},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    sync::Arc,
};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

#[derive(Clone, Debug)]
pub struct TaskServerConfig {
    pub peer_id: String,
    pub credentials: Vec<PeerCredential>,
}
impl TaskServerConfig {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        validate_task_identifier(&self.peer_id)?;
        if self.credentials.is_empty() || self.credentials.len() > MAX_TASK_IDENTITIES {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let mut subjects = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        for credential in &self.credentials {
            credential.validate()?;
            if !subjects.insert(&credential.subject) || !tokens.insert(&credential.bearer_token) {
                return Err(TaskGatewayError::InvalidRequest);
            }
        }
        Ok(())
    }
}
pub(crate) struct TaskHandler {
    backend: Arc<dyn TaskBackend>,
    config: TaskServerConfig,
    capacity: Arc<Semaphore>,
    stopped: watch::Sender<bool>,
}
impl TaskHandler {
    pub(crate) fn new(
        config: TaskServerConfig,
        backend: Arc<dyn TaskBackend>,
    ) -> Result<Self, TaskGatewayError> {
        config.validate()?;
        let (stopped, _) = watch::channel(false);
        Ok(Self {
            backend,
            config,
            capacity: Arc::new(Semaphore::new(32)),
            stopped,
        })
    }
    pub(crate) fn shutdown(&self) {
        self.stopped.send_replace(true);
    }
    fn begin(&self, params: &ServiceParams) -> Result<(String, OwnedSemaphorePermit), A2AError> {
        if *self.stopped.borrow() {
            return Err(sdk_error(TaskGatewayError::Server));
        }
        if let Some(version) = params.get("a2a-version")
            && (version.len() != 1 || version[0] != "1.0")
        {
            return Err(A2AError::version_not_supported("unsupported"));
        }
        let tokens = params
            .get("authorization")
            .filter(|values| values.len() == 1)
            .ok_or_else(|| sdk_error(TaskGatewayError::Unauthorized))?;
        let token = tokens[0]
            .strip_prefix("Bearer ")
            .ok_or_else(|| sdk_error(TaskGatewayError::Unauthorized))?;
        let subject = self
            .config
            .credentials
            .iter()
            .find(|credential| {
                bool::from(credential.bearer_token.as_bytes().ct_eq(token.as_bytes()))
            })
            .map(|credential| credential.subject.clone())
            .ok_or_else(|| sdk_error(TaskGatewayError::Unauthorized))?;
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| sdk_error(TaskGatewayError::Busy))?;
        Ok((subject, permit))
    }
    async fn infer_context(
        &self,
        subject: &str,
        request: &mut SendMessageRequest,
    ) -> Result<(), A2AError> {
        if let Some(task_id) = &request.message.task_id {
            validate_task_identifier(task_id).map_err(sdk_error)?;
            let snapshot = call(self.backend.get(subject, task_id)).await?;
            if &snapshot.task_id != task_id {
                return Err(sdk_error(TaskGatewayError::Protocol));
            }
            if request
                .message
                .context_id
                .as_ref()
                .is_some_and(|context| context != &snapshot.context_id)
            {
                return Err(sdk_error(TaskGatewayError::InvalidRequest));
            }
            request.message.context_id = Some(snapshot.context_id);
        }
        Ok(())
    }
    fn stream(
        &self,
        stream: TaskEventStream,
        snapshot: TaskSnapshot,
        permit: OwnedSemaphorePermit,
        stop_on_interrupted: bool,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let state = SubscriptionState {
            stream,
            task_id: snapshot.task_id,
            context_id: snapshot.context_id,
            previous: None,
            pending: std::collections::VecDeque::new(),
            done: false,
            stop: self.stopped.subscribe(),
            _permit: permit,
            stop_on_interrupted,
        };
        Box::pin(stream::unfold(state, |mut state| async move {
            loop {
                if *state.stop.borrow() {
                    return None;
                }
                if let Some(event) = state.pending.pop_front() {
                    return Some((event, state));
                }
                if state.done {
                    return None;
                }
                let received = tokio::select! {biased;_=state.stop.changed()=>return None,result=timeout(TASK_CALL_DEADLINE,state.stream.next())=>result};
                let result = match received {
                    Ok(Some(Ok(snapshot))) => snapshot_events(&mut state, snapshot),
                    Ok(Some(Err(error))) => Err(error),
                    Err(_) => Err(TaskGatewayError::Deadline),
                    _ => Err(TaskGatewayError::Protocol),
                };
                if let Err(error) = result {
                    state.done = true;
                    state.pending.push_back(Err(sdk_error(error)));
                }
                tokio::task::yield_now().await;
            }
        }))
    }
}
struct SubscriptionState {
    stream: TaskEventStream,
    task_id: String,
    context_id: String,
    previous: Option<TaskSnapshot>,
    pending: std::collections::VecDeque<Result<StreamResponse, A2AError>>,
    done: bool,
    stop: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
    stop_on_interrupted: bool,
}
fn snapshot_events(
    state: &mut SubscriptionState,
    snapshot: TaskSnapshot,
) -> Result<(), TaskGatewayError> {
    snapshot.validate()?;
    if snapshot.task_id != state.task_id || snapshot.context_id != state.context_id {
        return Err(TaskGatewayError::Protocol);
    }
    if let Some(previous) = &state.previous {
        if previous.artifacts.iter().any(|artifact| {
            !snapshot
                .artifacts
                .iter()
                .any(|a| a.artifact_id == artifact.artifact_id)
        }) {
            return Err(TaskGatewayError::Protocol);
        }
        for artifact in &snapshot.artifacts {
            if !previous.artifacts.contains(artifact) {
                state.pending.push_back(Ok(StreamResponse::ArtifactUpdate(
                    TaskArtifactUpdateEvent {
                        task_id: snapshot.task_id.clone(),
                        context_id: snapshot.context_id.clone(),
                        artifact: artifact_to_wire(artifact),
                        append: Some(false),
                        last_chunk: Some(true),
                        metadata: None,
                    },
                )));
            }
        }
        if previous.state != snapshot.state || previous.error_code != snapshot.error_code {
            state
                .pending
                .push_back(Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                    task_id: snapshot.task_id.clone(),
                    context_id: snapshot.context_id.clone(),
                    status: TaskStatus {
                        state: state_to_wire(snapshot.state),
                        message: None,
                        timestamp: None,
                    },
                    metadata: snapshot.error_code.clone().map(|code| {
                        HashMap::from([("error_code".to_string(), serde_json::Value::String(code))])
                    }),
                })));
        }
    } else {
        state
            .pending
            .push_back(Ok(StreamResponse::Task(snapshot_to_wire(
                snapshot.clone(),
            )?)));
    }
    state.done = snapshot.state.is_terminal()
        || (state.stop_on_interrupted && stream_finished(snapshot.state));
    state.previous = Some(snapshot);
    Ok(())
}
fn stream_finished(state: crate::task_contract::TaskState) -> bool {
    state.is_terminal()
        || matches!(
            state,
            crate::task_contract::TaskState::InputRequired
                | crate::task_contract::TaskState::AuthRequired
        )
}
async fn call<T>(
    future: impl std::future::Future<Output = Result<T, TaskGatewayError>>,
) -> Result<T, A2AError> {
    timeout(TASK_CALL_DEADLINE, future)
        .await
        .map_err(|_| sdk_error(TaskGatewayError::Deadline))?
        .map_err(sdk_error)
}
fn validate_task_query(id: &str, tenant: &Option<String>) -> Result<(), A2AError> {
    validate_task_identifier(id).map_err(sdk_error)?;
    if tenant.is_some() {
        return Err(sdk_error(TaskGatewayError::Unsupported));
    }
    Ok(())
}
#[async_trait]
impl RequestHandler for TaskHandler {
    async fn send_message(
        &self,
        params: &ServiceParams,
        mut request: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        let (subject, _permit) = self.begin(params)?;
        let return_immediately = request
            .configuration
            .as_ref()
            .and_then(|config| config.return_immediately)
            .unwrap_or(false);
        self.infer_context(&subject, &mut request).await?;
        let request = request_from_wire(request).map_err(sdk_error)?;
        let context_id = request.context_id.clone();
        let mut reply = call(self.backend.send_reply(&subject, request)).await?;
        let returned_context = match &reply {
            TaskReply::Task(t) => &t.context_id,
            TaskReply::Message(m) => &m.context_id,
        };
        if returned_context != &context_id {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        if let TaskReply::Task(snapshot) = &reply {
            snapshot.validate().map_err(sdk_error)?;
            if !return_immediately && !stream_finished(snapshot.state) {
                let task_id = snapshot.task_id.clone();
                let mut updates = call(self.backend.subscribe(&subject, &task_id)).await?;
                let final_snapshot = call(async {
                    while let Some(update) = updates.next().await {
                        let update = update?;
                        update.validate()?;
                        if update.task_id != task_id || update.context_id != context_id {
                            return Err(TaskGatewayError::Protocol);
                        }
                        if stream_finished(update.state) {
                            return Ok(update);
                        }
                    }
                    Err(TaskGatewayError::Protocol)
                })
                .await?;
                reply = TaskReply::Task(final_snapshot);
            }
        }
        reply_to_wire(reply).map_err(sdk_error)
    }
    async fn send_streaming_message(
        &self,
        params: &ServiceParams,
        mut request: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let (subject, permit) = self.begin(params)?;
        self.infer_context(&subject, &mut request).await?;
        let request = request_from_wire(request).map_err(sdk_error)?;
        let context_id = request.context_id.clone();
        match call(self.backend.send_reply(&subject, request)).await? {
            TaskReply::Message(message) => {
                if message.context_id != context_id {
                    return Err(sdk_error(TaskGatewayError::Protocol));
                }
                let SendMessageResponse::Message(wire) =
                    reply_to_wire(TaskReply::Message(message)).map_err(sdk_error)?
                else {
                    return Err(sdk_error(TaskGatewayError::Protocol));
                };
                Ok(Box::pin(stream::once(async move {
                    let _permit = permit;
                    Ok(StreamResponse::Message(wire))
                })))
            }
            TaskReply::Task(snapshot) => {
                if snapshot.context_id != context_id {
                    return Err(sdk_error(TaskGatewayError::Protocol));
                }
                snapshot.validate().map_err(sdk_error)?;
                if snapshot.state.is_terminal()
                    || matches!(
                        snapshot.state,
                        crate::task_contract::TaskState::InputRequired
                            | crate::task_contract::TaskState::AuthRequired
                    )
                {
                    let wire = snapshot_to_wire(snapshot).map_err(sdk_error)?;
                    return Ok(Box::pin(stream::once(async move {
                        let _permit = permit;
                        Ok(StreamResponse::Task(wire))
                    })));
                }
                let stream = call(self.backend.subscribe(&subject, &snapshot.task_id)).await?;
                Ok(self.stream(stream, snapshot, permit, true))
            }
        }
    }
    async fn get_task(
        &self,
        params: &ServiceParams,
        request: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        let (subject, _permit) = self.begin(params)?;
        validate_task_query(&request.id, &request.tenant)?;
        if request.history_length.is_some_and(|n| n < 0) {
            return Err(sdk_error(TaskGatewayError::InvalidRequest));
        }
        let snapshot = call(self.backend.get(&subject, &request.id)).await?;
        if snapshot.task_id != request.id {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        snapshot_to_wire(snapshot).map_err(sdk_error)
    }
    async fn cancel_task(
        &self,
        params: &ServiceParams,
        request: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        let (subject, _permit) = self.begin(params)?;
        validate_task_query(&request.id, &request.tenant)?;
        let before = call(self.backend.get(&subject, &request.id)).await?;
        if before.task_id != request.id {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        if before.state.is_terminal() {
            return Err(A2AError::task_not_cancelable(&request.id));
        }
        let snapshot = call(self.backend.cancel(&subject, &request.id)).await?;
        if snapshot.task_id != request.id || snapshot.context_id != before.context_id {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        snapshot_to_wire(snapshot).map_err(sdk_error)
    }
    async fn subscribe_to_task(
        &self,
        params: &ServiceParams,
        request: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let (subject, permit) = self.begin(params)?;
        validate_task_query(&request.id, &request.tenant)?;
        let snapshot = call(self.backend.get(&subject, &request.id)).await?;
        snapshot.validate().map_err(sdk_error)?;
        if snapshot.task_id != request.id {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        if snapshot.state.is_terminal() {
            return Err(sdk_error(TaskGatewayError::Unsupported));
        }
        let stream = call(self.backend.subscribe(&subject, &request.id)).await?;
        Ok(self.stream(stream, snapshot, permit, false))
    }
    async fn list_tasks(
        &self,
        params: &ServiceParams,
        request: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        let (subject, _permit) = self.begin(params)?;
        if request.tenant.is_some() || request.status_timestamp_after.is_some() {
            return Err(sdk_error(TaskGatewayError::Unsupported));
        }
        if request.history_length.is_some_and(|length| length < 0) {
            return Err(sdk_error(TaskGatewayError::InvalidRequest));
        }
        let page_size = u32::try_from(request.page_size.unwrap_or(32))
            .map_err(|_| sdk_error(TaskGatewayError::InvalidRequest))?;
        if page_size == 0 || page_size > 100 {
            return Err(sdk_error(TaskGatewayError::InvalidRequest));
        }
        let page_size = page_size.min(32);
        let mapped = TaskListRequest {
            context_id: request.context_id,
            state: request
                .status
                .map(state_from_wire)
                .transpose()
                .map_err(sdk_error)?,
            page_size,
            page_token: request.page_token.filter(|token| !token.is_empty()),
            include_artifacts: request.include_artifacts.unwrap_or(false),
        };
        mapped.validate().map_err(sdk_error)?;
        let page = call(self.backend.list(&subject, mapped)).await?;
        page.validate().map_err(sdk_error)?;
        if page.tasks.len() > page_size as usize {
            return Err(sdk_error(TaskGatewayError::Protocol));
        }
        Ok(ListTasksResponse {
            tasks: page
                .tasks
                .into_iter()
                .map(snapshot_to_wire)
                .collect::<Result<Vec<_>, _>>()
                .map_err(sdk_error)?,
            next_page_token: page.next_page_token.unwrap_or_default(),
            page_size: i32::try_from(page_size)
                .map_err(|_| sdk_error(TaskGatewayError::Protocol))?,
            total_size: i32::try_from(page.total_size)
                .map_err(|_| sdk_error(TaskGatewayError::Protocol))?,
        })
    }
    async fn create_push_config(
        &self,
        params: &ServiceParams,
        _request: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        let _ = self.begin(params)?;
        Err(A2AError::push_notification_not_supported())
    }
    async fn get_push_config(
        &self,
        params: &ServiceParams,
        _request: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        let _ = self.begin(params)?;
        Err(A2AError::push_notification_not_supported())
    }
    async fn list_push_configs(
        &self,
        params: &ServiceParams,
        _request: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        let _ = self.begin(params)?;
        Err(A2AError::push_notification_not_supported())
    }
    async fn delete_push_config(
        &self,
        params: &ServiceParams,
        _request: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        let _ = self.begin(params)?;
        Err(A2AError::push_notification_not_supported())
    }
    async fn get_extended_agent_card(
        &self,
        params: &ServiceParams,
        _request: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        let _ = self.begin(params)?;
        Err(sdk_error(TaskGatewayError::Unsupported))
    }
}

pub struct TaskServer {
    address: SocketAddr,
    base_url: String,
    handler: Arc<TaskHandler>,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<Result<(), TaskGatewayError>>>,
}
impl TaskServer {
    pub async fn start(
        config: TaskServerConfig,
        backend: Arc<dyn TaskBackend>,
    ) -> Result<Self, TaskGatewayError> {
        Self::start_inner(config, backend, None).await
    }
    pub async fn start_with_grpc(
        config: TaskServerConfig,
        backend: Arc<dyn TaskBackend>,
        grpc_base_url: String,
    ) -> Result<Self, TaskGatewayError> {
        let grpc_base_url = crate::task_client::loopback_origin(&grpc_base_url)?;
        Self::start_inner(config, backend, Some(grpc_base_url)).await
    }
    async fn start_inner(
        config: TaskServerConfig,
        backend: Arc<dyn TaskBackend>,
        grpc_base_url: Option<String>,
    ) -> Result<Self, TaskGatewayError> {
        let handler = Arc::new(TaskHandler::new(config.clone(), backend)?);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| TaskGatewayError::Server)?;
        let address = listener
            .local_addr()
            .map_err(|_| TaskGatewayError::Server)?;
        let base_url = format!("http://{address}");
        let mut card = task_agent_card(
            &config.peer_id,
            vec![
                AgentInterface::new(&base_url, TRANSPORT_PROTOCOL_HTTP_JSON),
                AgentInterface::new(&base_url, TRANSPORT_PROTOCOL_JSONRPC),
            ],
        );
        if let Some(url) = grpc_base_url {
            card.supported_interfaces
                .push(AgentInterface::new(url, TRANSPORT_PROTOCOL_GRPC));
        }
        let card_bytes = serde_json::to_vec(&card).map_err(|_| TaskGatewayError::Protocol)?;
        let card_etag = format!(
            "\"{}\"",
            Sha256::digest(&card_bytes)
                .iter()
                .flat_map(|byte| {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    [
                        char::from(HEX[usize::from(*byte >> 4)]),
                        char::from(HEX[usize::from(*byte & 15)]),
                    ]
                })
                .collect::<String>()
        );
        let modified = httpdate::fmt_http_date(std::time::SystemTime::now());
        let card_cache = std::sync::Arc::new((card_etag, modified));
        let app = Router::new()
            .merge(rest_router(handler.clone()))
            .merge(jsonrpc_router(handler.clone()))
            .merge(agent_card_router(Arc::new(StaticAgentCard::new(card))))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let cache = card_cache.clone();
                    async move {
                        let is_card = request.method() == axum::http::Method::GET
                            && request.uri().path() == "/.well-known/agent-card.json";
                        let unchanged = is_card
                            && request
                                .headers()
                                .get(axum::http::header::IF_NONE_MATCH)
                                .and_then(|value| value.to_str().ok())
                                == Some(cache.0.as_str());
                        let mut response = if unchanged {
                            axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::NOT_MODIFIED,
                            )
                        } else {
                            next.run(request).await
                        };
                        if is_card {
                            response.headers_mut().insert(
                                axum::http::header::CACHE_CONTROL,
                                axum::http::HeaderValue::from_static(
                                    "public, max-age=60, must-revalidate",
                                ),
                            );
                            if let Ok(value) = axum::http::HeaderValue::from_str(&cache.0) {
                                response
                                    .headers_mut()
                                    .insert(axum::http::header::ETAG, value);
                            }
                            if let Ok(value) = axum::http::HeaderValue::from_str(&cache.1) {
                                response
                                    .headers_mut()
                                    .insert(axum::http::header::LAST_MODIFIED, value);
                            }
                        }
                        response
                    }
                },
            ))
            .layer(DefaultBodyLimit::max(MAX_TASK_WIRE_BYTES));
        let (shutdown, receiver) = oneshot::channel();
        let worker = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
                .map_err(|_| TaskGatewayError::Server)
        });
        Ok(Self {
            address,
            base_url,
            handler,
            shutdown: Some(shutdown),
            worker: Some(worker),
        })
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub const fn address(&self) -> SocketAddr {
        self.address
    }
    pub async fn shutdown(mut self) -> Result<(), TaskGatewayError> {
        self.handler.shutdown();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let worker = self.worker.as_mut().ok_or(TaskGatewayError::Server)?;
        let result = match timeout(TASK_SHUTDOWN_DEADLINE, &mut *worker).await {
            Ok(result) => result.map_err(|_| TaskGatewayError::Server)?,
            Err(_) => {
                worker.abort();
                let _ = worker.await;
                Err(TaskGatewayError::Deadline)
            }
        };
        self.worker.take();
        result
    }
}
impl Drop for TaskServer {
    fn drop(&mut self) {
        self.handler.shutdown();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

pub(crate) fn task_agent_card(peer_id: &str, interfaces: Vec<AgentInterface>) -> AgentCard {
    AgentCard {
        name: format!("VibeMux {peer_id}"),
        description: "Authenticated local task gateway; pre-alpha validation endpoint".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: interfaces,
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            ..AgentCapabilities::default()
        },
        default_input_modes: vec!["application/json".to_string(), "text/plain".to_string()],
        default_output_modes: vec!["application/json".to_string(), "text/plain".to_string()],
        skills: vec![AgentSkill {
            id: "vibemux_task_gateway".to_string(),
            name: "Task gateway".to_string(),
            description: "Bounded task operations delegated to the authoritative backend"
                .to_string(),
            tags: vec!["task".to_string()],
            examples: None,
            input_modes: None,
            output_modes: None,
            security_requirements: None,
        }],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: Some(HashMap::from([(
            "peer_bearer".to_string(),
            SecurityScheme::HttpAuth(HttpAuthSecurityScheme {
                scheme: "bearer".to_string(),
                description: None,
                bearer_format: None,
            }),
        )])),
        security_requirements: Some(vec![HashMap::from([("peer_bearer".to_string(), vec![])])]),
        signatures: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_contract::TaskState;
    use serde_json::json;

    fn subscription(stop_on_interrupted: bool) -> SubscriptionState {
        let (_stopper, stop) = watch::channel(false);
        let permit = Arc::new(Semaphore::new(1))
            .try_acquire_owned()
            .expect("permit");
        SubscriptionState {
            stream: Box::pin(stream::empty()),
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            previous: None,
            pending: std::collections::VecDeque::new(),
            done: false,
            stop,
            _permit: permit,
            stop_on_interrupted,
        }
    }

    fn snapshot(
        state: TaskState,
        artifacts: Vec<TaskArtifact>,
        error_code: Option<&str>,
    ) -> TaskSnapshot {
        TaskSnapshot {
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            state,
            artifacts,
            error_code: error_code.map(str::to_string),
        }
    }

    fn artifact(artifact_id: &str, value: u32) -> TaskArtifact {
        TaskArtifact::data(artifact_id, json!(value))
    }

    fn drain(state: &mut SubscriptionState) -> Vec<StreamResponse> {
        state
            .pending
            .drain(..)
            .map(|event| event.expect("an event, not an error"))
            .collect()
    }

    fn artifact_update(artifact: &TaskArtifact) -> StreamResponse {
        StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            artifact: artifact_to_wire(artifact),
            append: Some(false),
            last_chunk: Some(true),
            metadata: None,
        })
    }

    fn status_update(wire_state: a2a::TaskState, error_code: Option<&str>) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            status: TaskStatus {
                state: wire_state,
                message: None,
                timestamp: None,
            },
            metadata: error_code
                .map(|code| HashMap::from([("error_code".to_string(), json!(code))])),
        })
    }

    #[test]
    fn the_first_snapshot_is_sent_whole_and_later_ones_only_as_changes() {
        let mut state = subscription(false);
        let first = snapshot(TaskState::Working, vec![artifact("artifact_1", 1)], None);
        assert_eq!(snapshot_events(&mut state, first.clone()), Ok(()));
        assert_eq!(
            drain(&mut state),
            vec![StreamResponse::Task(
                snapshot_to_wire(first.clone()).expect("first snapshot")
            )]
        );
        // An unchanged snapshot sends nothing.
        assert_eq!(snapshot_events(&mut state, first), Ok(()));
        assert!(state.pending.is_empty());

        // A changed or new artifact is sent whole as a replacing update, in
        // snapshot order; an unchanged state sends no status update.
        let artifacts = vec![artifact("artifact_1", 2), artifact("artifact_2", 3)];
        let second = snapshot(TaskState::Working, artifacts.clone(), None);
        assert_eq!(snapshot_events(&mut state, second), Ok(()));
        assert_eq!(
            drain(&mut state),
            vec![
                artifact_update(&artifacts[0]),
                artifact_update(&artifacts[1])
            ]
        );

        // A change of state, of error code alone, or a cleared error code
        // each send one status update and no artifact update.
        for (task_state, error_code, wire_state) in [
            (
                TaskState::InputRequired,
                None,
                a2a::TaskState::InputRequired,
            ),
            (
                TaskState::InputRequired,
                Some("needs_input"),
                a2a::TaskState::InputRequired,
            ),
            (
                TaskState::InputRequired,
                None,
                a2a::TaskState::InputRequired,
            ),
        ] {
            let next = snapshot(task_state, artifacts.clone(), error_code);
            assert_eq!(snapshot_events(&mut state, next), Ok(()));
            assert_eq!(
                drain(&mut state),
                vec![status_update(wire_state, error_code)]
            );
        }
    }

    #[test]
    fn applying_the_sent_events_reproduces_each_snapshot() {
        let mut state = subscription(false);
        let mut client = None;
        for expected in [
            snapshot(TaskState::Submitted, Vec::new(), None),
            snapshot(TaskState::Working, vec![artifact("artifact_1", 1)], None),
            snapshot(
                TaskState::Working,
                vec![artifact("artifact_1", 2), artifact("artifact_2", 3)],
                None,
            ),
            snapshot(
                TaskState::InputRequired,
                vec![artifact("artifact_1", 2), artifact("artifact_2", 3)],
                Some("needs_input"),
            ),
            snapshot(
                TaskState::Working,
                vec![artifact("artifact_1", 2), artifact("artifact_2", 4)],
                None,
            ),
            snapshot(
                TaskState::Completed,
                vec![artifact("artifact_1", 5), artifact("artifact_2", 4)],
                None,
            ),
        ] {
            assert!(!state.done);
            assert_eq!(snapshot_events(&mut state, expected.clone()), Ok(()));
            for event in drain(&mut state) {
                apply_stream_response(&mut client, event).expect("the client applies the event");
            }
            assert_eq!(client.as_ref(), Some(&expected));
        }
        assert!(state.done);
    }

    #[test]
    fn a_snapshot_of_another_task_an_invalid_one_or_a_shrinking_one_is_refused() {
        let mut state = subscription(false);
        let other_task = TaskSnapshot {
            task_id: "task_2".to_string(),
            ..snapshot(TaskState::Working, Vec::new(), None)
        };
        let other_context = TaskSnapshot {
            context_id: "context_2".to_string(),
            ..snapshot(TaskState::Working, Vec::new(), None)
        };
        for refused in [other_task.clone(), other_context.clone()] {
            assert_eq!(
                snapshot_events(&mut state, refused),
                Err(TaskGatewayError::Protocol)
            );
        }
        let invalid = snapshot(TaskState::Working, Vec::new(), Some("needs input"));
        assert_eq!(
            snapshot_events(&mut state, invalid),
            Err(TaskGatewayError::InvalidRequest)
        );
        assert_eq!((&state.previous, state.pending.len()), (&None, 0));

        let first = snapshot(
            TaskState::Working,
            vec![artifact("artifact_1", 1), artifact("artifact_2", 2)],
            None,
        );
        assert_eq!(snapshot_events(&mut state, first.clone()), Ok(()));
        drain(&mut state);
        let dropped = snapshot(TaskState::Working, vec![artifact("artifact_2", 2)], None);
        for refused in [other_task, other_context, dropped] {
            assert_eq!(
                snapshot_events(&mut state, refused),
                Err(TaskGatewayError::Protocol)
            );
        }
        // Once a snapshot was sent, an invalid one is refused before any
        // status update could carry its error code.
        let invalid = snapshot(
            TaskState::Failed,
            first.artifacts.clone(),
            Some("worker failed"),
        );
        assert_eq!(
            snapshot_events(&mut state, invalid),
            Err(TaskGatewayError::InvalidRequest)
        );
        assert_eq!((state.previous, state.pending.len()), (Some(first), 0));
    }

    #[test]
    fn a_terminal_state_ends_the_stream_and_an_interruption_ends_it_only_when_asked() {
        for (task_state, ends, ends_when_asked) in [
            (TaskState::Submitted, false, false),
            (TaskState::Working, false, false),
            (TaskState::InputRequired, false, true),
            (TaskState::AuthRequired, false, true),
            (TaskState::Completed, true, true),
            (TaskState::Failed, true, true),
            (TaskState::Canceled, true, true),
            (TaskState::Rejected, true, true),
        ] {
            assert_eq!(
                stream_finished(task_state),
                ends_when_asked,
                "{task_state:?}"
            );
            for (stop_on_interrupted, expected) in [(false, ends), (true, ends_when_asked)] {
                let mut state = subscription(stop_on_interrupted);
                let next = snapshot(task_state, Vec::new(), None);
                assert_eq!(snapshot_events(&mut state, next), Ok(()));
                assert_eq!(
                    state.done, expected,
                    "{task_state:?} stop_on_interrupted={stop_on_interrupted}"
                );
            }
        }
    }

    #[test]
    fn a_task_query_needs_a_valid_id_and_no_tenant() {
        assert!(validate_task_query("task_1", &None).is_ok());
        for (id, tenant, code) in [
            ("task 1", None, a2a::error_code::INVALID_PARAMS),
            ("", None, a2a::error_code::INVALID_PARAMS),
            (
                "task_1",
                Some("tenant_1".to_string()),
                a2a::error_code::UNSUPPORTED_OPERATION,
            ),
        ] {
            let error = validate_task_query(id, &tenant).expect_err("refused query");
            assert_eq!(error.code, code, "{id:?} {tenant:?}");
        }
    }
}
