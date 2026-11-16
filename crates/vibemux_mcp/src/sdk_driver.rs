//! ADR 075 Scope A contract core (slice 2): the SDK host's initialization
//! parsing, admission gate, runtime-gated tool surface and tool dispatch.
//!
//! Feature-gated (`rmcp`); the hand-written session remains the shipping
//! path. Every behavior here is a pure function or a state object the
//! future session driver composes, and the initialize parsing is pinned to
//! the hand-written `protocol::Protocol` by parity tests.

use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rmcp::{
    ErrorData as McpError, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
        Implementation, InitializeResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerConfig, TextContent,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};

use crate::{
    config::{MAX_RESTARTS, PROTOCOL_VERSION},
    tools::{READ_TOOLS, ReadOperation, parse_operation},
};

/// The single advertised protocol revision (ADR 075 Decision 5).
pub const ADVERTISED_PROTOCOL_VERSION: &str = "2025-06-18";
/// The client capability that requests metadata reads (a JSON boolean).
pub const GRANT_KEY: &str = "vibemux_metadata_read";

/// The pinned server identity and capabilities; `get_info` and the
/// initialize reply must agree.
#[must_use]
pub fn server_info() -> ServerConfig {
    let mut capabilities = ServerCapabilities::default();
    let mut tools = rmcp::model::ToolsCapability::default();
    tools.list_changed = Some(true);
    capabilities.tools = Some(tools);
    let mut info = InitializeResult::new(capabilities);
    info.protocol_version = ProtocolVersion::V_2025_06_18;
    info.server_info = Implementation::new("vibemux_mcp", env!("CARGO_PKG_VERSION"));
    info
}

/// The initialize result exactly as the hand-written protocol emits it.
#[must_use]
pub fn initialize_reply_value() -> Value {
    json!({"protocolVersion":PROTOCOL_VERSION,
        "capabilities":{"tools":{"listChanged":true}},
        "serverInfo":{"name":"vibemux_mcp","version":env!("CARGO_PKG_VERSION")}})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializeFault {
    InvalidArguments,
}

/// Parses the raw initialize params first-party, before any typed decode:
/// the experimental capability values are JSON objects to the SDK but the
/// grant is a boolean on this wire (ADR 075 Decision 5).
pub fn parse_initialize(params: &Value, host_grant: bool) -> Result<bool, InitializeFault> {
    let valid = params.as_object().is_some_and(|object| {
        object.keys().all(|key| {
            ["protocolVersion", "capabilities", "clientInfo", "_meta"].contains(&key.as_str())
        }) && params["protocolVersion"]
            .as_str()
            .is_some_and(|value| !value.is_empty() && value.len() <= 32)
            && params["capabilities"].is_object()
            && params["capabilities"]
                .get("experimental")
                .is_none_or(Value::is_object)
            && params.get("_meta").is_none_or(Value::is_object)
            && params["clientInfo"]["name"]
                .as_str()
                .is_some_and(|value| !value.is_empty() && value.len() <= 128)
            && params["clientInfo"]["version"]
                .as_str()
                .is_some_and(|value| !value.is_empty() && value.len() <= 64)
    });
    let requested = params.pointer("/capabilities/experimental/vibemux_metadata_read");
    if !valid || requested.is_some_and(|value| !value.is_boolean()) {
        return Err(InitializeFault::InvalidArguments);
    }
    // Absent means granted-to-host-only; the operator grant always caps it,
    // and an unreadable value failed closed above.
    Ok(host_grant && requested.and_then(Value::as_bool).unwrap_or(true))
}

/// One producer call may be active (ADR 035 contract): later requests are
/// busy, the active id is a duplicate, and a cancelled id gets no reply.
#[derive(Clone, Debug, Default)]
pub struct Admission(Arc<Mutex<AdmissionState>>);

#[derive(Debug, Default)]
struct AdmissionState {
    active: Option<String>,
    cancelled: HashSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmitError {
    Busy,
    Duplicate,
}

/// Releases the active slot on drop, including through a cancelled future.
pub struct AdmissionGuard {
    admission: Admission,
    id: String,
}

impl Admission {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_admit(&self, id: &str) -> Result<AdmissionGuard, AdmitError> {
        let mut state = self.0.lock().expect("admission lock");
        if state.active.as_deref() == Some(id) {
            return Err(AdmitError::Duplicate);
        }
        if state.active.is_some() {
            return Err(AdmitError::Busy);
        }
        state.active = Some(id.to_string());
        Ok(AdmissionGuard {
            admission: self.clone(),
            id: id.to_string(),
        })
    }

    /// Marks a cancelled id and reports whether it was the active call, so
    /// the driver knows to unload the reader.
    pub fn cancel(&self, id: &str) -> bool {
        let mut state = self.0.lock().expect("admission lock");
        state.cancelled.insert(id.to_string());
        state.active.as_deref() == Some(id)
    }

    /// True while the response for this id must be suppressed on the wire.
    #[must_use]
    pub fn should_suppress(&self, id: &str) -> bool {
        self.0
            .lock()
            .expect("admission lock")
            .cancelled
            .contains(id)
    }

    /// Clears the suppression record after the driver dropped the reply.
    pub fn clear_cancelled(&self, id: &str) {
        self.0.lock().expect("admission lock").cancelled.remove(id);
    }

    #[must_use]
    pub fn active(&self) -> Option<String> {
        self.0.lock().expect("admission lock").active.clone()
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        let mut state = self.admission.0.lock().expect("admission lock");
        if state.active.as_deref() == Some(self.id.as_str()) {
            state.active = None;
        }
    }
}

/// Stable wire key for a request id.
#[must_use]
pub fn wire_id(id: &rmcp::model::RequestId) -> String {
    match id {
        rmcp::model::NumberOrString::Number(number) => format!("n:{number}"),
        rmcp::model::NumberOrString::String(text) => format!("s:{text}"),
    }
}

/// Runtime state the tool surface is gated on.
#[derive(Debug)]
pub struct WireState {
    metadata_read: AtomicBool,
    initialized: AtomicBool,
    backend: Mutex<BackendSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendSnapshot {
    pub available: bool,
    pub state: &'static str,
    pub launches: u32,
    pub restarts: u32,
    pub last_error: Option<&'static str>,
}

impl Default for BackendSnapshot {
    fn default() -> Self {
        Self {
            available: true,
            state: "not_started",
            launches: 0,
            restarts: 0,
            last_error: None,
        }
    }
}

impl WireState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            metadata_read: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            backend: Mutex::new(BackendSnapshot::default()),
        }
    }

    pub fn set_grant(&self, granted: bool) {
        self.metadata_read.store(granted, Ordering::SeqCst);
    }

    #[must_use]
    pub fn metadata_read(&self) -> bool {
        self.metadata_read.load(Ordering::SeqCst)
    }

    pub fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    pub fn set_backend(&self, snapshot: BackendSnapshot) {
        *self.backend.lock().expect("backend lock") = snapshot;
    }

    #[must_use]
    pub fn backend(&self) -> BackendSnapshot {
        self.backend.lock().expect("backend lock").clone()
    }

    /// Metadata tools appear only when the grant is on and the reader is
    /// available, exactly as the hand-written session decides it.
    #[must_use]
    pub fn metadata_available(&self) -> bool {
        self.metadata_read() && self.backend().available
    }
}

impl Default for WireState {
    fn default() -> Self {
        Self::new()
    }
}

/// The read seam the future session actor implements; boxed per call so
/// the host stays object-safe and the future stays `Send`.
pub trait MetadataExecutor: Send + Sync + 'static {
    fn read<'a>(
        &'a self,
        operation: ReadOperation,
    ) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + 'a>>;
}

/// The SDK host: the `ServerHandler` surface backed by the contract above.
pub struct SdkHost<E> {
    state: Arc<WireState>,
    admission: Admission,
    executor: E,
}

impl<E: MetadataExecutor> SdkHost<E> {
    #[must_use]
    pub fn new(state: Arc<WireState>, admission: Admission, executor: E) -> Self {
        Self {
            state,
            admission,
            executor,
        }
    }

    #[must_use]
    pub fn state(&self) -> Arc<WireState> {
        Arc::clone(&self.state)
    }

    #[must_use]
    pub fn admission(&self) -> Admission {
        self.admission.clone()
    }

    fn error(number: i32, code: &'static str) -> McpError {
        McpError::new(ErrorCode(number), code, Some(json!({"code": code})))
    }

    fn tool_result(value: Value) -> CallToolResult {
        let mut result = CallToolResult::success(vec![ContentBlock::Text(TextContent::new(
            value.to_string(),
        ))]);
        result.structured_content = Some(value);
        result
    }

    fn tool_error(code: &'static str) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::Text(TextContent::new(
            json!({"error": {"code": code}}).to_string(),
        ))])
    }

    fn host_status(&self) -> Value {
        let backend = self.state.backend();
        json!({
            "metadata_read": self.state.metadata_read(),
            "backend_state": backend.state,
            "worker_launches": backend.launches,
            "launch_count_kind": "attempts",
            "restart_limit": MAX_RESTARTS,
            "restarts_used": backend.restarts,
            "last_error": backend.last_error,
            "evidence": "host_configuration_only",
        })
    }

    /// Dispatch without the SDK context, so tests and the driver share one
    /// path.
    pub async fn dispatch_tool(
        &self,
        id: &str,
        request: &CallToolRequestParams,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.as_ref();
        let arguments = request
            .arguments
            .as_ref()
            .map_or_else(|| json!({}), |arguments| Value::Object(arguments.clone()));
        if name == "host_status" {
            return if arguments
                .as_object()
                .is_some_and(|object| object.is_empty())
            {
                Ok(Self::tool_result(self.host_status()).into())
            } else {
                Err(Self::error(-32602, "invalid_arguments"))
            };
        }
        if READ_TOOLS.contains(&name) && !self.state.metadata_read() {
            return Err(Self::error(-32001, "metadata_read_denied"));
        }
        let operation = match parse_operation(name, &arguments) {
            Ok(operation) => operation,
            Err(code) => return Err(Self::error(-32602, code)),
        };
        if !self.state.backend().available {
            return Ok(Self::tool_error("reader_unavailable").into());
        }
        let _guard = match self.admission.try_admit(id) {
            Ok(guard) => guard,
            Err(AdmitError::Duplicate) => return Err(Self::error(-32600, "duplicate_request_id")),
            Err(AdmitError::Busy) => return Err(Self::error(-32000, "request_busy")),
        };
        match self.executor.read(operation).await {
            Ok(value) => Ok(Self::tool_result(value).into()),
            Err(code) => Ok(Self::tool_error(code).into()),
        }
    }
}

impl<E: MetadataExecutor> ServerHandler for SdkHost<E> {
    fn get_info(&self) -> ServerConfig {
        server_info()
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        static VERSIONS: [ProtocolVersion; 1] = [ProtocolVersion::V_2025_06_18];
        std::borrow::Cow::Borrowed(&VERSIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Single-flight parity: while one read is active every other
        // request is busy, as in the hand-written session.
        if self.admission.active().is_some() {
            return Err(Self::error(-32000, "request_busy"));
        }
        let tools = crate::sdk_seam::tools_as_rmcp(self.state.metadata_available())
            .map_err(|_| Self::error(-32603, "tools_contract_invalid"))?;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let id = wire_id(&context.id);
        let dispatch = self.dispatch_tool(&id, &request);
        tokio::pin!(dispatch);
        tokio::select! {
            result = &mut dispatch => result,
            _ = context.ct.cancelled() => {
                // No reply may reach the wire for a cancelled id; the
                // session driver drops outbound messages whose id the
                // admission recorded as cancelled.
                self.admission.cancel(&id);
                Err(Self::error(-32000, "request_cancelled"))
            }
        }
    }

    async fn on_initialized(&self, _context: rmcp::service::NotificationContext<RoleServer>) {
        self.state.mark_initialized();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use super::*;
    use crate::protocol::{BackendView, Protocol};

    struct StubExecutor {
        calls: Arc<AtomicUsize>,
        delay_ms: u64,
    }

    impl MetadataExecutor for StubExecutor {
        fn read<'a>(
            &'a self,
            operation: ReadOperation,
        ) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                if self.delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
                }
                match operation {
                    ReadOperation::Health => Ok(json!({"healthy": true})),
                    _ => Err("reader_failed"),
                }
            })
        }
    }

    fn initialize_params(grant: Option<Value>) -> Value {
        let mut capabilities = json!({});
        if let Some(value) = grant {
            capabilities = json!({"experimental": {GRANT_KEY: value}});
        }
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": capabilities,
            "clientInfo": {"name": "fixture", "version": "1.0"},
        })
    }

    fn call_request(name: &str, arguments: Value) -> CallToolRequestParams {
        let mut request = CallToolRequestParams::new(name.to_string());
        if let Some(object) = arguments.as_object() {
            request.arguments = Some(object.clone());
        }
        request
    }

    fn host(executor: StubExecutor) -> SdkHost<StubExecutor> {
        SdkHost::new(Arc::new(WireState::new()), Admission::new(), executor)
    }

    #[test]
    fn initialize_parsing_matches_the_hand_written_protocol() {
        let host_grant = true;
        for params in [
            initialize_params(None),
            initialize_params(Some(json!(true))),
            initialize_params(Some(json!(false))),
            initialize_params(Some(json!("yes"))),
            json!({"protocolVersion": "", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}}),
            json!({"protocolVersion": "2025-06-18", "capabilities": {"experimental": []}, "clientInfo": {"name": "x", "version": "1"}}),
        ] {
            let mut protocol = Protocol::default();
            let reply = protocol.handle(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":params}),
                host_grant,
                &BackendView::default(),
            );
            let reply = match reply {
                crate::protocol::Action::Reply(reply) => reply,
                _ => panic!("initialize must reply"),
            };
            match parse_initialize(&params, host_grant) {
                Ok(granted) => {
                    assert_eq!(reply["result"], initialize_reply_value(), "{params}");
                    assert_eq!(protocol.metadata_read, granted, "{params}");
                }
                Err(InitializeFault::InvalidArguments) => {
                    assert_eq!(
                        reply["error"]["data"]["code"], "invalid_arguments",
                        "{params}"
                    );
                }
            }
        }
        let denied = parse_initialize(&initialize_params(None), false).expect("absent is valid");
        assert!(!denied, "the operator grant always caps the client request");
    }

    #[test]
    fn the_server_identity_is_pinned() {
        let info = server_info();
        assert_eq!(info.protocol_version.as_str(), ADVERTISED_PROTOCOL_VERSION);
        assert_eq!(
            info.capabilities.tools.and_then(|tools| tools.list_changed),
            Some(true)
        );
        assert_eq!(info.server_info.name, "vibemux_mcp");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            serde_json::to_value(initialize_reply_value()).expect("reply"),
            json!({"protocolVersion": PROTOCOL_VERSION,
                "capabilities":{"tools":{"listChanged":true}},
                "serverInfo":{"name":"vibemux_mcp","version":env!("CARGO_PKG_VERSION")}})
        );
    }

    #[test]
    fn admission_allows_one_call_and_reports_busy_and_duplicate() {
        let admission = Admission::new();
        let guard = admission.try_admit("n:1").expect("first");
        assert_eq!(admission.try_admit("n:2").err(), Some(AdmitError::Busy));
        assert_eq!(
            admission.try_admit("n:1").err(),
            Some(AdmitError::Duplicate)
        );
        drop(guard);
        assert_eq!(admission.active(), None);
        let guard = admission.try_admit("n:4").expect("after release");
        assert!(
            admission.cancel("n:4"),
            "cancelling the active id reports it"
        );
        assert!(
            !admission.cancel("n:5"),
            "cancelling an idle id still records it"
        );
        assert!(admission.should_suppress("n:4"));
        assert!(admission.should_suppress("n:5"));
        admission.clear_cancelled("n:4");
        assert!(!admission.should_suppress("n:4"));
        drop(guard);
    }

    #[tokio::test]
    async fn dispatch_tool_mirrors_the_hand_written_contract() {
        let calls = Arc::new(AtomicUsize::new(0));
        let host = host(StubExecutor {
            calls: Arc::clone(&calls),
            delay_ms: 0,
        });
        let complete = |response: CallToolResponse| match response {
            CallToolResponse::Complete(result) => result,
            _ => panic!("the host only completes tool calls"),
        };
        let code_of = |error: McpError| {
            error.data.expect("error data")["code"]
                .as_str()
                .expect("code")
                .to_string()
        };

        let status = complete(
            host.dispatch_tool("n:1", &call_request("host_status", json!({})))
                .await
                .expect("host status"),
        );
        let value = status.structured_content.expect("structured content");
        assert_eq!(value["metadata_read"], false);
        assert_eq!(value["backend_state"], "not_started");
        assert_eq!(value["launch_count_kind"], "attempts");
        assert_eq!(value["evidence"], "host_configuration_only");

        let denied = host
            .dispatch_tool("n:2", &call_request("health", json!({})))
            .await
            .expect_err("metadata read denied");
        assert_eq!(denied.code.0, -32001);
        assert_eq!(code_of(denied), "metadata_read_denied");

        let invalid = host
            .dispatch_tool("n:3", &call_request("host_status", json!({"extra": 1})))
            .await
            .expect_err("host_status takes no arguments");
        assert_eq!(code_of(invalid), "invalid_arguments");

        host.state.set_grant(true);
        let health = complete(
            host.dispatch_tool("n:4", &call_request("health", json!({})))
                .await
                .expect("granted read"),
        );
        assert_eq!(health.is_error, Some(false));
        assert_eq!(
            health.structured_content.expect("structured")["healthy"],
            true
        );
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        let failed = complete(
            host.dispatch_tool("n:5", &call_request("frontend_tasks", json!({})))
                .await
                .expect("executor failure is a tool-level error"),
        );
        assert_eq!(failed.is_error, Some(true));

        let guard = host.admission.try_admit("n:9").expect("hold the call");
        let busy = host
            .dispatch_tool("n:10", &call_request("health", json!({})))
            .await
            .expect_err("one producer call only");
        assert_eq!(code_of(busy), "request_busy");
        let duplicate = host
            .dispatch_tool("n:9", &call_request("health", json!({})))
            .await
            .expect_err("duplicate active id");
        assert_eq!(code_of(duplicate), "duplicate_request_id");
        drop(guard);

        host.state.set_backend(BackendSnapshot {
            available: false,
            ..BackendSnapshot::default()
        });
        let unavailable = complete(
            host.dispatch_tool("n:11", &call_request("health", json!({})))
                .await
                .expect("reader unavailability is a tool-level error"),
        );
        assert_eq!(unavailable.is_error, Some(true));
        assert!(
            unavailable
                .content
                .first()
                .and_then(|content| content.as_text())
                .is_some_and(|text| text.text.contains("reader_unavailable"))
        );
    }

    #[tokio::test]
    async fn the_tool_surface_is_runtime_gated() {
        let host = host(StubExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            delay_ms: 0,
        });
        // The SDK RequestContext cannot be constructed outside the service,
        // so gating is asserted through `metadata_available` (the same
        // predicate `list_tools` reads) and the wire contract here.
        assert!(!host.state.metadata_available());
        host.state.set_grant(true);
        assert!(host.state.metadata_available());
        host.state.set_backend(BackendSnapshot {
            available: false,
            ..BackendSnapshot::default()
        });
        assert!(!host.state.metadata_available());
    }
}
