//! Local-only AAG transport checks. No vendor or model is contacted.

#[allow(dead_code)]
#[path = "../src/workflow/aag_gateway.rs"]
mod aag_gateway;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use aag_gateway::{
    Aag_Call_Class, Aag_Call_Request, Aag_Credential, Aag_Fixture_Permit, Aag_Gateway_Error,
    Aag_Model_Gateway,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest,
    gateway::{
        CallCorrelation, CallOutcome, GATEWAY_CONFIG_SCHEMA_VERSION, GatewayConfig,
        GatewayProtocol, ProviderAttribution, RetryDecision, RouteBinding, RouteRole,
    },
};

const TEST_TOKEN: &str = "synthetic_fixture_token";
const MAX_REQUEST_HEAD: usize = 16 * 1024;
const MAX_REQUEST_BODY: usize = 256 * 1024;

#[derive(Clone, Copy)]
enum FakeMode {
    Normal,
    Status503,
    Oversized,
    PartialText,
    PartialTool,
    Delay,
}

#[derive(Clone, Debug)]
struct SeenRequest {
    path: String,
    model: Option<String>,
    correlation: Option<String>,
    authorization_ok: bool,
    anthropic_headers_ok: bool,
    stream: bool,
}

struct FakeServer {
    origin: String,
    permit: Aag_Fixture_Permit,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    pinned: Arc<AtomicBool>,
    mode: Arc<Mutex<FakeMode>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeServer {
    async fn start() -> Self {
        let listener = Arc::new(
            TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback"),
        );
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let permit = Aag_Fixture_Permit::from_owned_listener(Arc::clone(&listener))
            .expect("bound fake server owns loopback listener");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let pinned = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(Mutex::new(FakeMode::Normal));
        let server_seen = Arc::clone(&seen);
        let server_pinned = Arc::clone(&pinned);
        let server_mode = Arc::clone(&mode);
        let server_listener = Arc::clone(&listener);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = server_listener.accept().await else {
                    break;
                };
                let seen = Arc::clone(&server_seen);
                let pinned = Arc::clone(&server_pinned);
                let mode = Arc::clone(&server_mode);
                tokio::spawn(async move {
                    handle_connection(socket, seen, pinned, mode).await;
                });
            }
        });
        Self {
            origin,
            permit,
            seen,
            pinned,
            mode,
            task,
        }
    }

    fn set_mode(&self, mode: FakeMode) {
        *self.mode.lock().expect("mode") = mode;
    }

    fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().expect("seen").clone()
    }

    fn fixture_permit(&self) -> Aag_Fixture_Permit {
        self.permit.clone()
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    pinned: Arc<AtomicBool>,
    mode: Arc<Mutex<FakeMode>>,
) {
    let Some((head, body)) = read_request(&mut socket).await else {
        return;
    };
    let start = head.lines().next().unwrap_or_default();
    let path = start
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let headers = head.to_ascii_lowercase();
    let parsed_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let model = parsed_body["model"].as_str().map(str::to_string);
    let correlation = head.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .strip_prefix("x-vibemux-correlation:")
            .map(|value| value.trim().to_string())
    });
    seen.lock().expect("seen").push(SeenRequest {
        path: path.clone(),
        model: model.clone(),
        correlation,
        authorization_ok: headers.contains(&format!("authorization: bearer {TEST_TOKEN}")),
        anthropic_headers_ok: headers.contains(&format!("x-api-key: {TEST_TOKEN}"))
            && headers.contains("anthropic-version: 2023-06-01"),
        stream: parsed_body["stream"] == true,
    });
    match path.as_str() {
        "/health" => {
            let pin = pinned
                .load(Ordering::SeqCst)
                .then_some("fixture_global_pin");
            send_json(
                &mut socket,
                200,
                &json!({"status":"ok","pinnedProvider":pin}),
            )
            .await;
        }
        "/v1/models" => {
            send_json(
                &mut socket,
                200,
                &json!({"data":[
                    {"id":"alias_a"},{"id":"alias_b"},{"id":"alias_anthropic"}
                ]}),
            )
            .await;
        }
        "/v1/chat/completions" | "/v1/messages" => {
            let selected = *mode.lock().expect("mode");
            match selected {
                FakeMode::Status503 => {
                    send_bytes(&mut socket, 503, "application/json", b"{}", true).await
                }
                FakeMode::Oversized => {
                    send_bytes(
                        &mut socket,
                        200,
                        "application/json",
                        &vec![b'x'; 256 * 1024 + 1],
                        true,
                    )
                    .await
                }
                FakeMode::PartialText => {
                    send_bytes(&mut socket, 200, "text/event-stream",
                        b"data: {\"model\":\"reported_fixture\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n", false).await;
                }
                FakeMode::PartialTool => {
                    send_bytes(&mut socket, 200, "text/event-stream",
                        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"tool_1\"}]}}]}\n\n", false).await;
                }
                FakeMode::Delay => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    send_json(&mut socket, 200, &chat_response(model.as_deref())).await;
                }
                FakeMode::Normal if path == "/v1/messages" => {
                    send_json(
                        &mut socket,
                        200,
                        &json!({"content":[{"type":"text","text":"fixture answer"}],
                        "stop_reason":"end_turn"}),
                    )
                    .await;
                }
                FakeMode::Normal if parsed_body["stream"] == true => {
                    send_bytes(&mut socket, 200, "text/event-stream",
                        b"data: {\"model\":\"reported_fixture\",\"choices\":[{\"delta\":{\"content\":\"fixture answer\"}}]}\n\ndata: [DONE]\n\n", true).await;
                }
                FakeMode::Normal => {
                    send_json(&mut socket, 200, &chat_response(model.as_deref())).await;
                }
            }
        }
        _ => send_bytes(&mut socket, 404, "application/json", b"{}", true).await,
    }
}

fn chat_response(model: Option<&str>) -> Value {
    json!({"model":model.unwrap_or("reported_fixture"),
        "choices":[{"message":{"content":"fixture answer"},"finish_reason":"stop"}]})
}

async fn read_request(socket: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 || bytes.len().saturating_add(read) > MAX_REQUEST_HEAD + MAX_REQUEST_BODY {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if bytes.len() > MAX_REQUEST_HEAD {
            return None;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).ok()?;
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > MAX_REQUEST_BODY {
        return None;
    }
    while bytes.len() - header_end < length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Some((head, bytes[header_end..header_end + length].to_vec()))
}

async fn send_json(socket: &mut TcpStream, status: u16, value: &Value) {
    let body = serde_json::to_vec(value).expect("fixture JSON");
    send_bytes(socket, status, "application/json", &body, true).await;
}

async fn send_bytes(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    length: bool,
) {
    let reason = if status == 200 {
        "OK"
    } else {
        "Service Unavailable"
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nConnection: close\r\n"
    );
    if length {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}

fn config(origin: &str) -> GatewayConfig {
    GatewayConfig {
        schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
        endpoint: origin.to_string(),
        credential_env: "AAG_FIXTURE_TOKEN".to_string(),
        bindings: vec![
            binding(
                RouteRole::WorkerA,
                "alias_a",
                GatewayProtocol::OpenaiChatCompletions,
            ),
            binding(
                RouteRole::WorkerB,
                "alias_b",
                GatewayProtocol::OpenaiChatCompletions,
            ),
            binding(
                RouteRole::Compiler,
                "alias_anthropic",
                GatewayProtocol::AnthropicMessages,
            ),
        ],
        strict_routing: true,
    }
}

fn binding(role: RouteRole, alias: &str, protocol: GatewayProtocol) -> RouteBinding {
    RouteBinding {
        role,
        model_alias: alias.to_string(),
        protocol,
        max_output_tokens: 64,
        timeout_ms: 1000,
    }
}

fn digest() -> Sha256Digest {
    Sha256Digest::of(b"fixture-config")
}

fn gateway(server: &FakeServer) -> Aag_Model_Gateway {
    Aag_Model_Gateway::new(
        config(&server.origin),
        digest(),
        Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("synthetic token"),
    )
    .expect("fixture gateway")
    .with_fixture_permit(server.fixture_permit())
    .expect("matching owned listener")
}

fn request(role: RouteRole) -> Aag_Call_Request {
    Aag_Call_Request {
        class: Aag_Call_Class::Fixture,
        role,
        correlation: CallCorrelation {
            workflow_id: Uuid::from_u128(1),
            call_id: Uuid::new_v4(),
            attempt: 1,
        },
        expected_config_digest: digest(),
        expected_route_generation: None,
        system: "fixture system".to_string(),
        user: "fixture user".to_string(),
        stream: false,
    }
}

fn cancel_receiver() -> watch::Receiver<bool> {
    watch::channel(false).1
}

#[tokio::test]
async fn a_fixture_label_requires_an_owned_listener_for_the_exact_origin() {
    let server = FakeServer::start().await;
    let unpermitted = Aag_Model_Gateway::new(
        config(&server.origin),
        digest(),
        Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("synthetic token"),
    )
    .expect("gateway without fixture permit");
    let denied = unpermitted
        .call(request(RouteRole::WorkerA), cancel_receiver())
        .await;
    assert_eq!(denied.error, Some(Aag_Gateway_Error::FixturePermitRequired));
    assert!(server.requests().is_empty());

    let other = FakeServer::start().await;
    let mismatched = Aag_Model_Gateway::new(
        config(&other.origin),
        digest(),
        Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("synthetic token"),
    )
    .expect("second gateway");
    assert_eq!(
        mismatched
            .with_fixture_permit(server.fixture_permit())
            .unwrap_err(),
        Aag_Gateway_Error::FixturePermitMismatch
    );
    assert!(other.requests().is_empty());
}

#[tokio::test]
async fn concurrent_aliases_keep_correlation_and_unknown_attribution() {
    let server = FakeServer::start().await;
    let gateway = gateway(&server);
    let request_a = request(RouteRole::WorkerA);
    let request_b = request(RouteRole::WorkerB);
    let correlation_a = request_a.correlation.header_value();
    let correlation_b = request_b.correlation.header_value();
    let (a, b) = tokio::join!(
        gateway.call(request_a, cancel_receiver()),
        gateway.call(request_b, cancel_receiver())
    );
    for result in [&a, &b] {
        assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
        assert!(
            result
                .completion
                .as_ref()
                .expect("completion")
                .usage
                .is_unknown()
        );
        assert_eq!(
            result.receipt.as_ref().expect("receipt").effective_provider,
            ProviderAttribution::Unknown
        );
    }
    let calls: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|seen| seen.path == "/v1/chat/completions")
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|call| call.authorization_ok));
    assert!(
        calls
            .iter()
            .any(|call| call.model.as_deref() == Some("alias_a")
                && call.correlation.as_deref() == Some(correlation_a.as_str()))
    );
    assert!(
        calls
            .iter()
            .any(|call| call.model.as_deref() == Some("alias_b")
                && call.correlation.as_deref() == Some(correlation_b.as_str()))
    );
}

#[tokio::test]
async fn anthropic_headers_and_live_attestation_boundary() {
    let server = FakeServer::start().await;
    let gateway = gateway(&server);
    let mut live = request(RouteRole::Compiler);
    live.class = Aag_Call_Class::Live;
    let denied = gateway.call(live, cancel_receiver()).await;
    assert_eq!(
        denied.error,
        Some(Aag_Gateway_Error::LiveAttestationUnavailable)
    );
    assert!(server.requests().is_empty());
    let result = gateway
        .call(request(RouteRole::Compiler), cancel_receiver())
        .await;
    assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
    assert!(result.completion.expect("completion").usage.is_unknown());
    let call = server
        .requests()
        .into_iter()
        .find(|call| call.path == "/v1/messages")
        .expect("anthropic request");
    assert!(call.anthropic_headers_ok);
    assert_eq!(call.model.as_deref(), Some("alias_anthropic"));
}

#[tokio::test]
async fn configuration_pin_and_protocol_changes_block_before_model_send() {
    let server = FakeServer::start().await;
    let gateway = gateway(&server);
    let mut stale = request(RouteRole::WorkerA);
    stale.expected_config_digest = Sha256Digest::of(b"changed");
    assert_eq!(
        gateway.call(stale, cancel_receiver()).await.error,
        Some(Aag_Gateway_Error::ConfigChanged)
    );
    assert!(server.requests().is_empty());
    server.pinned.store(true, Ordering::SeqCst);
    let pinned = gateway
        .call(request(RouteRole::WorkerA), cancel_receiver())
        .await;
    assert_eq!(pinned.error, Some(Aag_Gateway_Error::RouteUnavailable));
    assert!(
        server
            .requests()
            .iter()
            .all(|seen| seen.path != "/v1/chat/completions")
    );
    server.pinned.store(false, Ordering::SeqCst);
    let mut stale_route = request(RouteRole::WorkerA);
    stale_route.expected_route_generation = Some(Sha256Digest::of(b"stale observation"));
    assert_eq!(
        gateway.call(stale_route, cancel_receiver()).await.error,
        Some(Aag_Gateway_Error::RouteChanged)
    );
    let mut unsupported_config = config(&server.origin);
    unsupported_config.bindings[0].protocol = GatewayProtocol::OpenaiResponses;
    let unsupported = Aag_Model_Gateway::new(
        unsupported_config,
        digest(),
        Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("synthetic token"),
    )
    .expect("valid config with unsupported adapter")
    .with_fixture_permit(server.fixture_permit())
    .expect("matching owned listener");
    assert_eq!(
        unsupported
            .call(request(RouteRole::WorkerA), cancel_receiver())
            .await
            .error,
        Some(Aag_Gateway_Error::UnsupportedProtocol)
    );
    assert!(
        server
            .requests()
            .iter()
            .all(|seen| seen.path != "/v1/responses")
    );
}

#[tokio::test]
async fn completed_sse_uses_the_stream_accumulator_without_inventing_usage() {
    let server = FakeServer::start().await;
    let gateway = gateway(&server);
    let mut streamed = request(RouteRole::WorkerA);
    streamed.stream = true;
    let result = gateway.call(streamed, cancel_receiver()).await;
    assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
    let completion = result.completion.expect("stream completion");
    assert_eq!(completion.text, "fixture answer");
    assert!(completion.usage.is_unknown());
    assert_eq!(
        completion.reported_model.as_deref(),
        Some("reported_fixture")
    );
    let receipt = result.receipt.expect("receipt");
    assert_eq!(receipt.effective_provider, ProviderAttribution::Unknown);
    assert!(receipt.response_sha256.is_some());
}

#[tokio::test]
async fn response_bounds_status_and_partial_streams_are_not_replayed() {
    let server = FakeServer::start().await;
    let gateway = gateway(&server);
    let mut too_large_request = request(RouteRole::WorkerA);
    too_large_request.user = "x".repeat(256 * 1024 + 1);
    assert_eq!(
        gateway
            .call(too_large_request, cancel_receiver())
            .await
            .error,
        Some(Aag_Gateway_Error::RequestTooLarge)
    );
    assert!(server.requests().is_empty());
    server.set_mode(FakeMode::Status503);
    let status = gateway
        .call(request(RouteRole::WorkerA), cancel_receiver())
        .await;
    assert_eq!(status.outcome, CallOutcome::FailedBeforeOutput);
    assert_eq!(status.error, Some(Aag_Gateway_Error::HttpStatus));
    assert_eq!(status.receipt.expect("receipt").http_status, Some(503));
    assert!(matches!(status.retry, RetryDecision::NoAutomaticRetry(_)));
    server.set_mode(FakeMode::Oversized);
    let oversized = gateway
        .call(request(RouteRole::WorkerA), cancel_receiver())
        .await;
    assert_eq!(oversized.error, Some(Aag_Gateway_Error::ResponseTooLarge));
    server.set_mode(FakeMode::PartialText);
    let mut streamed = request(RouteRole::WorkerA);
    streamed.stream = true;
    let partial = gateway.call(streamed.clone(), cancel_receiver()).await;
    assert_eq!(partial.outcome, CallOutcome::FailedAfterOutput);
    assert!(partial.completion.is_none());
    assert!(matches!(partial.retry, RetryDecision::NoAutomaticRetry(_)));
    server.set_mode(FakeMode::PartialTool);
    let tool = gateway.call(streamed, cancel_receiver()).await;
    assert_eq!(tool.outcome, CallOutcome::FailedAfterOutput);
    assert!(tool.completion.is_none());
    assert!(matches!(tool.retry, RetryDecision::NoAutomaticRetry(_)));
    assert!(server.requests().iter().filter(|seen| seen.stream).count() >= 2);
}

#[tokio::test]
async fn cancellation_and_deadline_never_replay_an_ambiguous_request() {
    let server = FakeServer::start().await;
    server.set_mode(FakeMode::Delay);
    let gateway = gateway(&server);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let call = gateway.call(request(RouteRole::WorkerA), cancel_rx);
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_tx.send(true).expect("cancel receiver alive");
    };
    let (cancelled, ()) = tokio::join!(call, canceller);
    assert_eq!(cancelled.error, Some(Aag_Gateway_Error::Cancelled));
    assert!(matches!(
        cancelled.retry,
        RetryDecision::NoAutomaticRetry(_)
    ));
    let timed_out = gateway
        .call(request(RouteRole::WorkerA), cancel_receiver())
        .await;
    assert_eq!(timed_out.error, Some(Aag_Gateway_Error::Deadline));
    assert!(matches!(
        timed_out.retry,
        RetryDecision::NoAutomaticRetry(_)
    ));
}

#[test]
fn numeric_origin_and_redacted_debug() {
    let base = config("http://localhost:8989");
    let token = Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("token");
    assert_eq!(
        Aag_Model_Gateway::new(base, digest(), token).unwrap_err(),
        Aag_Gateway_Error::OriginNotNumericLoopback
    );
    let request = request(RouteRole::WorkerA);
    assert!(!format!("{request:?}").contains("fixture system"));
    assert!(
        !format!(
            "{:?}",
            Aag_Credential::explicit(TEST_TOKEN.to_string()).expect("token")
        )
        .contains(TEST_TOKEN)
    );
}
