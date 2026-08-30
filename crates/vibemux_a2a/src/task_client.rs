//! Bounded network client. Reconnect/resubscribe is an explicit caller operation.
use crate::task_contract::*;
use crate::task_wire::*;
use a2a::{
    AgentCard, CancelTaskRequest, GetTaskRequest, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    SendMessageResponse, SubscribeToTaskRequest,
};
use a2a_pb::protojson_conv::{self, ProtoJsonPayload};
use futures::stream;
use reqwest::{Client, Method, Response};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};
use tokio::time::timeout;

#[derive(Clone, Debug)]
pub struct TaskClientConfig {
    pub allowed_origins: Vec<String>,
    pub credential: PeerCredential,
    pub preferred_bindings: Vec<TaskBinding>,
}
#[derive(Clone)]
pub struct TaskClient {
    http: Client,
    base_url: String,
    credential: PeerCredential,
    binding: TaskBinding,
}
impl TaskClient {
    pub async fn connect(
        base_url: &str,
        config: TaskClientConfig,
    ) -> Result<Self, TaskGatewayError> {
        config.credential.validate()?;
        if config.allowed_origins.is_empty()
            || config.allowed_origins.len() > MAX_TASK_IDENTITIES
            || config.preferred_bindings.is_empty()
            || config.preferred_bindings.len() > 2
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let allowed = config
            .allowed_origins
            .iter()
            .map(|url| loopback_origin(url))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let base_url = loopback_origin(base_url)?;
        if !allowed.contains(&base_url) {
            return Err(TaskGatewayError::Forbidden);
        }
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(TASK_CALL_DEADLINE)
            .build()
            .map_err(|_| TaskGatewayError::Transport)?;
        let response = http
            .get(format!("{base_url}/.well-known/agent-card.json"))
            .send()
            .await
            .map_err(|_| TaskGatewayError::Transport)?;
        if !response.status().is_success() {
            return Err(TaskGatewayError::Transport);
        }
        let bytes = read_bounded(response).await?;
        let card: AgentCard =
            serde_json::from_slice(&bytes).map_err(|_| TaskGatewayError::Protocol)?;
        if card.supported_interfaces.is_empty() || card.supported_interfaces.len() > 8 {
            return Err(TaskGatewayError::Protocol);
        }
        // Validate every advertised destination before considering a preferred one.
        for interface in &card.supported_interfaces {
            let interface_url =
                if interface.protocol_binding == "GRPC" && !interface.url.contains("://") {
                    format!("http://{}", interface.url)
                } else {
                    interface.url.clone()
                };
            let origin = loopback_origin(&interface_url)?;
            if !allowed.contains(&origin)
                || interface.protocol_version != "1.0"
                || interface.tenant.is_some()
            {
                return Err(TaskGatewayError::Forbidden);
            }
        }
        for binding in config.preferred_bindings {
            if let Some(interface) = card
                .supported_interfaces
                .iter()
                .find(|i| i.protocol_binding == binding.protocol())
            {
                return Ok(Self {
                    http,
                    base_url: loopback_origin(&interface.url)?,
                    credential: config.credential,
                    binding,
                });
            }
        }
        Err(TaskGatewayError::Unsupported)
    }
    pub const fn binding(&self) -> TaskBinding {
        self.binding
    }
    pub async fn send(&self, request: &TaskRequest) -> Result<TaskSnapshot, TaskGatewayError> {
        match self.send_reply(request).await? {
            TaskReply::Task(snapshot) => Ok(snapshot),
            TaskReply::Message(_) => Err(TaskGatewayError::Protocol),
        }
    }
    pub async fn send_reply(&self, request: &TaskRequest) -> Result<TaskReply, TaskGatewayError> {
        let wire = request_to_wire(request)?;
        let response: SendMessageResponse = self
            .call(
                a2a::jsonrpc::methods::SEND_MESSAGE,
                "/message:send",
                Method::POST,
                &wire,
            )
            .await?;
        let reply = reply_from_wire(response)?;
        let context = match &reply {
            TaskReply::Task(snapshot) => &snapshot.context_id,
            TaskReply::Message(message) => &message.context_id,
        };
        if context != &request.context_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(reply)
    }
    pub async fn get(&self, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let wire: GetTaskRequest = GetTaskRequest {
            id: task_id.to_string(),
            history_length: Some(0),
            tenant: None,
        };
        let response = self
            .call(
                a2a::jsonrpc::methods::GET_TASK,
                &format!("/tasks/{task_id}"),
                Method::GET,
                &wire,
            )
            .await?;
        let snapshot = snapshot_from_wire(response)?;
        if snapshot.task_id != task_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(snapshot)
    }
    pub async fn list(&self, request: &TaskListRequest) -> Result<TaskList, TaskGatewayError> {
        request.validate()?;
        let wire = a2a::ListTasksRequest {
            context_id: request.context_id.clone(),
            status: request.state.map(state_to_wire),
            page_size: Some(request.page_size as i32),
            page_token: request.page_token.clone(),
            history_length: Some(0),
            status_timestamp_after: None,
            include_artifacts: Some(request.include_artifacts),
            tenant: None,
        };
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("pageSize", &request.page_size.to_string())
            .append_pair(
                "includeArtifacts",
                if request.include_artifacts {
                    "true"
                } else {
                    "false"
                },
            );
        if let Some(context) = &request.context_id {
            query.append_pair("contextId", context);
        }
        if let Some(state) = request.state {
            let value = serde_json::to_value(state_to_wire(state))
                .map_err(|_| TaskGatewayError::InvalidRequest)?;
            query.append_pair(
                "status",
                value.as_str().ok_or(TaskGatewayError::InvalidRequest)?,
            );
        }
        if let Some(token) = &request.page_token {
            query.append_pair("pageToken", token);
        }
        let path = format!("/tasks?{}", query.finish());
        let response: a2a::ListTasksResponse = self
            .call(a2a::jsonrpc::methods::LIST_TASKS, &path, Method::GET, &wire)
            .await?;
        let result = TaskList {
            tasks: response
                .tasks
                .into_iter()
                .map(snapshot_from_wire)
                .collect::<Result<Vec<_>, _>>()?,
            next_page_token: (!response.next_page_token.is_empty())
                .then_some(response.next_page_token),
            total_size: u32::try_from(response.total_size)
                .map_err(|_| TaskGatewayError::Protocol)?,
        };
        result.validate()?;
        if result.tasks.len() > request.page_size as usize {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(result)
    }
    pub async fn cancel(&self, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let wire = CancelTaskRequest {
            id: task_id.to_string(),
            metadata: None,
            tenant: None,
        };
        let response = self
            .call(
                a2a::jsonrpc::methods::CANCEL_TASK,
                &format!("/tasks/{task_id}:cancel"),
                Method::POST,
                &wire,
            )
            .await?;
        let snapshot = snapshot_from_wire(response)?;
        if snapshot.task_id != task_id {
            return Err(TaskGatewayError::Protocol);
        }
        Ok(snapshot)
    }
    /// Opens one subscription. Disconnects/lag/timeouts are explicit errors; this
    /// method never retries a send or silently starts a replacement task.
    pub async fn subscribe(&self, task_id: &str) -> Result<TaskEventStream, TaskGatewayError> {
        validate_task_identifier(task_id)?;
        let wire = SubscribeToTaskRequest {
            id: task_id.to_string(),
            tenant: None,
        };
        let (body, rpc_id) = self.payload(a2a::jsonrpc::methods::SUBSCRIBE_TO_TASK, &wire)?;
        let mut request = match self.binding {
            TaskBinding::HttpJson => self
                .http
                .get(format!("{}/tasks/{task_id}:subscribe", self.base_url)),
            TaskBinding::JsonRpc => self.http.post(&self.base_url).json(&body),
        };
        request = request.header("Accept", "text/event-stream");
        let response = self
            .authorize(request)
            .send()
            .await
            .map_err(|_| TaskGatewayError::Transport)?;
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"))
        {
            let bytes = read_bounded(response).await?;
            if self.binding == TaskBinding::JsonRpc {
                let value =
                    serde_json::from_slice(&bytes).map_err(|_| TaskGatewayError::Protocol)?;
                return Err(rpc_result(value, rpc_id.as_ref())
                    .err()
                    .unwrap_or(TaskGatewayError::Protocol));
            }
            return Err(TaskGatewayError::Protocol);
        }
        let state = SseState {
            response,
            buffer: Vec::new(),
            previous: None,
            finished: false,
            expected_task_id: task_id.to_string(),
            binding: self.binding,
            rpc_id,
        };
        Ok(Box::pin(stream::unfold(state, |mut state| async move {
            if state.finished {
                return None;
            }
            let result = next_snapshot(&mut state).await;
            if result.is_err() {
                state.finished = true;
            }
            Some((result, state))
        })))
    }
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("A2A-Version", "1.0")
            .bearer_auth(&self.credential.bearer_token)
    }
    fn payload<Req: ProtoJsonPayload>(
        &self,
        method: &str,
        request: &Req,
    ) -> Result<(Value, Option<JsonRpcId>), TaskGatewayError> {
        let body =
            protojson_conv::to_value(request).map_err(|_| TaskGatewayError::InvalidRequest)?;
        let result = match self.binding {
            TaskBinding::HttpJson => (body, None),
            TaskBinding::JsonRpc => {
                let id = JsonRpcId::String(uuid::Uuid::new_v4().to_string());
                (
                    serde_json::to_value(JsonRpcRequest::new(id.clone(), method, Some(body)))
                        .map_err(|_| TaskGatewayError::InvalidRequest)?,
                    Some(id),
                )
            }
        };
        bounded_json(&result.0, MAX_TASK_WIRE_BYTES)?;
        Ok(result)
    }
    async fn call<Req: ProtoJsonPayload, Resp: ProtoJsonPayload>(
        &self,
        method: &str,
        path: &str,
        http_method: Method,
        request: &Req,
    ) -> Result<Resp, TaskGatewayError> {
        let (body, rpc_id) = self.payload(method, request)?;
        let request = match self.binding {
            TaskBinding::HttpJson => {
                let request = self
                    .http
                    .request(http_method.clone(), format!("{}{path}", self.base_url));
                if http_method == Method::GET {
                    request
                } else {
                    request.json(&body)
                }
            }
            TaskBinding::JsonRpc => self.http.post(&self.base_url).json(&body),
        };
        let response = self
            .authorize(request)
            .send()
            .await
            .map_err(|_| TaskGatewayError::Transport)?;
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }
        let bytes = read_bounded(response).await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| TaskGatewayError::Protocol)?;
        let payload = if self.binding == TaskBinding::JsonRpc {
            rpc_result(value, rpc_id.as_ref())?
        } else {
            value
        };
        protojson_conv::from_value(payload).map_err(|_| TaskGatewayError::Protocol)
    }
}
pub(crate) fn loopback_origin(value: &str) -> Result<String, TaskGatewayError> {
    let url = url::Url::parse(value).map_err(|_| TaskGatewayError::InvalidRequest)?;
    let numeric_loopback = match url.host() {
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        _ => false,
    };
    if !numeric_loopback
        || url.scheme() != "http"
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(TaskGatewayError::Forbidden);
    }
    Ok(url.origin().ascii_serialization())
}
async fn read_bounded(mut response: Response) -> Result<Vec<u8>, TaskGatewayError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TASK_WIRE_BYTES as u64)
    {
        return Err(TaskGatewayError::Protocol);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = timeout(TASK_CALL_DEADLINE, response.chunk())
        .await
        .map_err(|_| TaskGatewayError::Deadline)?
        .map_err(|_| TaskGatewayError::Transport)?
    {
        if chunk.len() > MAX_TASK_WIRE_BYTES.saturating_sub(bytes.len()) {
            return Err(TaskGatewayError::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn response_error(response: Response) -> TaskGatewayError {
    let status = response.status();
    let Ok(bytes) = read_bounded(response).await else {
        return TaskGatewayError::Protocol;
    };
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
            let protocol_code = value
                .pointer("/error/details")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|detail| {
                    serde_json::from_value::<a2a::TypedDetail>(detail.clone()).ok()
                })
                .find_map(|detail| {
                    detail
                        .value
                        .get("reason")
                        .and_then(Value::as_str)
                        .and_then(a2a::reason_to_error_code)
                })
                .unwrap_or(a2a::error_code::INTERNAL_ERROR);
            let error = local_error(&a2a::A2AError::new(protocol_code, message));
            if error != TaskGatewayError::Protocol {
                return error;
            }
        }
    }
    match status.as_u16() {
        401 => TaskGatewayError::Unauthorized,
        403 => TaskGatewayError::Forbidden,
        404 => TaskGatewayError::NotFound,
        _ => TaskGatewayError::Transport,
    }
}
fn rpc_result(value: Value, expected_id: Option<&JsonRpcId>) -> Result<Value, TaskGatewayError> {
    let response: JsonRpcResponse =
        serde_json::from_value(value).map_err(|_| TaskGatewayError::Protocol)?;
    if response.jsonrpc != "2.0"
        || Some(&response.id) != expected_id
        || response.error.is_some() == response.result.is_some()
    {
        return Err(TaskGatewayError::Protocol);
    }
    if let Some(error) = response.error {
        return Err(local_error(&a2a::A2AError::new(error.code, error.message)));
    }
    response.result.ok_or(TaskGatewayError::Protocol)
}
struct SseState {
    response: Response,
    buffer: Vec<u8>,
    previous: Option<TaskSnapshot>,
    finished: bool,
    expected_task_id: String,
    binding: TaskBinding,
    rpc_id: Option<JsonRpcId>,
}
async fn next_snapshot(state: &mut SseState) -> Result<TaskSnapshot, TaskGatewayError> {
    loop {
        if let Some((data_end, frame_end)) = event_boundary(&state.buffer) {
            let event = state.buffer.drain(..frame_end).collect::<Vec<_>>();
            let text =
                std::str::from_utf8(&event[..data_end]).map_err(|_| TaskGatewayError::Protocol)?;
            let data = text
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|s| s.strip_prefix(' ').unwrap_or(s))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&data).map_err(|_| TaskGatewayError::Protocol)?;
            let value = if state.binding == TaskBinding::JsonRpc {
                rpc_result(value, state.rpc_id.as_ref())?
            } else {
                value
            };
            let event =
                protojson_conv::from_value(value).map_err(|_| TaskGatewayError::Protocol)?;
            let snapshot = apply_stream_response(&mut state.previous, event)?;
            if snapshot.task_id != state.expected_task_id {
                return Err(TaskGatewayError::Protocol);
            }
            state.finished = snapshot.state.is_terminal();
            return Ok(snapshot);
        }
        let chunk = timeout(TASK_CALL_DEADLINE, state.response.chunk())
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(|_| TaskGatewayError::Transport)?
            .ok_or(TaskGatewayError::Transport)?;
        if chunk.len() > MAX_TASK_WIRE_BYTES.saturating_sub(state.buffer.len()) {
            return Err(TaskGatewayError::Protocol);
        }
        state.buffer.extend_from_slice(&chunk);
    }
}
fn event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len() {
        let rest = &bytes[index..];
        if rest.starts_with(b"\r\n\r\n") {
            return Some((index, index + 4));
        }
        if rest.starts_with(b"\n\n") || rest.starts_with(b"\r\r") {
            return Some((index, index + 2));
        }
    }
    None
}
