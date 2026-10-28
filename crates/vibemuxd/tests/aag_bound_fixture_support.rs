//! Owned loopback AAG v2 fixture. No vendor, credential discovery or model call.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, watch},
    task::{JoinHandle, JoinSet},
};

pub const FIXTURE_TOKEN: &str = "aag_v2_synthetic_fixture_token";
pub const FIXTURE_INSTANCE: &str = "11111111111111111111111111111111";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 256 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FixtureMode {
    #[default]
    Normal,
    CapabilitiesV1,
    CapabilitiesMalformed,
    CapabilitiesPinned,
    PreconditionChanged,
    ReceiptMissing,
    ReceiptWrongNonce,
    ReceiptWrongInstance,
    ReceiptWrongGeneration,
    ReceiptWrongSelector,
    ReceiptWrongSurface,
    ReceiptWrongRequestId,
    ReceiptDropped,
    ReceiptTruncated,
    ReceiptIncomplete,
    ReceiptExpired,
    ReceiptOld,
    ReceiptDelay,
    ResponseMissingVersion,
    ResponseDuplicateNonce,
    ResponseWrongRequestId,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixtureCounts {
    pub requests: usize,
    pub inference_requests: usize,
    pub upstream_calls: usize,
    pub receipt_reads: usize,
}

#[derive(Clone, Debug)]
pub struct RequestSummary {
    pub path: String,
    pub method: String,
    pub gateway_auth_valid: bool,
    pub receipt_opt_in: bool,
    pub route_precondition: bool,
    pub lookup_capability: bool,
    pub selector: Option<String>,
}

struct StoredReceipt {
    capability: String,
    value: Value,
}

struct State {
    aliases: Vec<String>,
    completion: String,
    mode: FixtureMode,
    generation: u64,
    pinned: bool,
    counts: FixtureCounts,
    summaries: Vec<RequestSummary>,
    receipts: HashMap<String, StoredReceipt>,
}

pub struct BoundFixtureServer {
    origin: String,
    listener: Arc<TcpListener>,
    state: Arc<Mutex<State>>,
    receipt_seen: Arc<Notify>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl BoundFixtureServer {
    pub async fn start(aliases: &[&str], completion: &str) -> Self {
        let listener = Arc::new(
            TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture listener"),
        );
        let origin = format!("http://{}", listener.local_addr().expect("fixture address"));
        let state = Arc::new(Mutex::new(State {
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            completion: completion.to_string(),
            mode: FixtureMode::Normal,
            generation: 7,
            pinned: false,
            counts: FixtureCounts::default(),
            summaries: Vec::new(),
            receipts: HashMap::new(),
        }));
        let receipt_seen = Arc::new(Notify::new());
        let (stop, mut stopped) = watch::channel(false);
        let owned_listener = Arc::clone(&listener);
        let owned_state = Arc::clone(&state);
        let owned_seen = Arc::clone(&receipt_seen);
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    accepted = owned_listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let state = Arc::clone(&owned_state);
                        let seen = Arc::clone(&owned_seen);
                        children.spawn(async move {
                            let _ = tokio::time::timeout(IO_TIMEOUT, handle(socket, state, seen)).await;
                        });
                    }
                    _ = children.join_next(), if !children.is_empty() => {}
                }
            }
            children.abort_all();
            while children.join_next().await.is_some() {}
        });
        Self {
            origin,
            listener,
            state,
            receipt_seen,
            stop,
            task: Some(task),
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn listener(&self) -> Arc<TcpListener> {
        Arc::clone(&self.listener)
    }
    pub fn counts(&self) -> FixtureCounts {
        self.state.lock().expect("fixture state").counts
    }
    pub fn summaries(&self) -> Vec<RequestSummary> {
        self.state.lock().expect("fixture state").summaries.clone()
    }
    pub fn set_mode(&self, mode: FixtureMode) {
        self.state.lock().expect("fixture state").mode = mode;
    }
    pub fn set_generation(&self, generation: u64) {
        self.state.lock().expect("fixture state").generation = generation;
    }
    pub fn set_pin(&self, pinned: bool) {
        self.state.lock().expect("fixture state").pinned = pinned;
    }
    pub async fn wait_for_receipt_read(&self) {
        tokio::time::timeout(IO_TIMEOUT, self.receipt_seen.notified())
            .await
            .expect("receipt request observed");
    }
    pub async fn close(mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            tokio::time::timeout(IO_TIMEOUT, task)
                .await
                .expect("fixture shutdown deadline")
                .expect("fixture listener joined");
        }
    }
}

impl Drop for BoundFixtureServer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("fixture clock")
        .as_millis()
        .try_into()
        .expect("clock range")
}

async fn read_request(
    socket: &mut TcpStream,
) -> Option<(String, String, HashMap<String, String>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 || bytes.len().saturating_add(read) > MAX_HEADER_BYTES + MAX_BODY_BYTES {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break end + 4;
        }
        if bytes.len() > MAX_HEADER_BYTES {
            return None;
        }
    };
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_string())
            .is_some()
        {
            return None;
        }
    }
    let length = headers
        .get("content-length")
        .map_or(Some(0), |value| value.parse::<usize>().ok())?;
    if length > MAX_BODY_BYTES {
        return None;
    }
    while bytes.len().saturating_sub(end) < length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 || bytes.len().saturating_add(read) > MAX_HEADER_BYTES + MAX_BODY_BYTES {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Some((method, path, headers, bytes[end..end + length].to_vec()))
}

async fn send(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra: &[(String, String)],
    body: &[u8],
) {
    let mut head = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}
async fn json_response(socket: &mut TcpStream, status: u16, value: &Value) {
    send(
        socket,
        status,
        "application/json",
        &[],
        &serde_json::to_vec(value).expect("fixture JSON"),
    )
    .await;
}

fn capabilities(state: &State) -> Value {
    let mut value = json!({
        "schema_version":2,"issuer":"aag","supported_versions":[2],"server_instance_id":FIXTURE_INSTANCE,
        "generation":state.generation,"pinned_provider_present":state.pinned || state.mode == FixtureMode::CapabilitiesPinned,
        "supported_surfaces":["chat","messages"],"receipt_capacity":128,"max_retained_wire_attempts":32,
        "terminal_retention_seconds":600,"upstream_model_attestation":"unavailable","messages_credential_scope":"stored_key_only_snapshot"
    });
    if state.mode == FixtureMode::CapabilitiesV1 {
        value["schema_version"] = json!(1);
    }
    if state.mode == FixtureMode::CapabilitiesMalformed {
        value["unexpected"] = json!(true);
    }
    value
}

fn routing_receipt(
    nonce: &str,
    request_id: &str,
    selector: &str,
    surface: &str,
    generation: u64,
    pinned: bool,
    stream: bool,
) -> Value {
    let issued = now_ms();
    json!({"schema_version":2,"issuer":"aag","admission":{"server_instance_id":FIXTURE_INSTANCE,"generation":generation,"pinned_provider_present":pinned},
        "routing_receipt":{"schema_version":1,"issuer":"aag","evidence_kind":"gateway_observed","server_instance_id":FIXTURE_INSTANCE,
        "request_nonce":nonce,"request_id":request_id,"surface":surface,"generation":generation,"requested_selector":selector,"selector_state":"reported",
        "phase":"settled","http_status":200,"body_state":"exhausted","selected_attempt":1,
        "attempts":[{"index":1,"hop_index":0,"subattempt":0,"identity":{"provider_ref":"provider_fixture","supplier_ref":"supplier_fixture","endpoint_ref":"endpoint_fixture","association":"supplier_endpoint"},
        "protocol":if surface=="messages" {"anthropic"} else {"openai_chat"},"credential_mode":"stored_key","wire_model":"wire_fixture_model","wire_model_state":"reported",
        "provider_model":"upstream_claim","provider_model_state":"reported","provider_model_source":if surface=="messages" {"message_model"} else {"model"},
        "http_status":200,"send_started":true,"transport_state":if stream {"stream_ended"} else {"response_parsed"},
        "terminal_signals":if stream {vec!["openai_finish","openai_done"]} else {Vec::<&str>::new()},"error_code":null}],
        "capture_complete":true,"capture_truncated":false,"upstream_model_attestation":"unavailable","issued_at_ms":issued,"expires_at_ms":issued+600_000}}
    )
}

fn alter_receipt(mut value: Value, mode: FixtureMode) -> Value {
    match mode {
        FixtureMode::ReceiptWrongNonce => {
            value["routing_receipt"]["request_nonce"] = json!("2".repeat(32))
        }
        FixtureMode::ReceiptWrongInstance => {
            value["admission"]["server_instance_id"] = json!("2".repeat(32))
        }
        FixtureMode::ReceiptWrongGeneration => value["admission"]["generation"] = json!(8),
        FixtureMode::ReceiptWrongSelector => {
            value["routing_receipt"]["requested_selector"] = json!("different_alias")
        }
        FixtureMode::ReceiptWrongSurface => value["routing_receipt"]["surface"] = json!("messages"),
        FixtureMode::ReceiptWrongRequestId => {
            value["routing_receipt"]["request_id"] = json!("another_request")
        }
        FixtureMode::ReceiptDropped => value["routing_receipt"]["body_state"] = json!("dropped"),
        FixtureMode::ReceiptTruncated => {
            value["routing_receipt"]["capture_truncated"] = json!(true)
        }
        FixtureMode::ReceiptIncomplete => {
            value["routing_receipt"]["capture_complete"] = json!(false)
        }
        FixtureMode::ReceiptExpired => {
            value["routing_receipt"]["issued_at_ms"] = json!(now_ms() - 20_000);
            value["routing_receipt"]["expires_at_ms"] = json!(now_ms() - 1);
        }
        FixtureMode::ReceiptOld => {
            value["routing_receipt"]["issued_at_ms"] = json!(now_ms() - 20_000)
        }
        _ => {}
    }
    value
}

async fn handle(mut socket: TcpStream, state: Arc<Mutex<State>>, receipt_seen: Arc<Notify>) {
    let Some((method, path, headers, body)) = read_request(&mut socket).await else {
        return;
    };
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let authorized = headers
        .get("authorization")
        .is_some_and(|value| value == &format!("Bearer {FIXTURE_TOKEN}"))
        || headers
            .get("x-api-key")
            .is_some_and(|value| value == FIXTURE_TOKEN);
    {
        let mut state = state.lock().expect("fixture state");
        state.counts.requests += 1;
        state.summaries.push(RequestSummary {
            path: path.clone(),
            method: method.clone(),
            gateway_auth_valid: authorized,
            receipt_opt_in: headers.contains_key("x-aag-receipt"),
            route_precondition: headers.contains_key("x-aag-route-precondition"),
            lookup_capability: headers.contains_key("x-aag-receipt-capability"),
            selector: value["model"].as_str().map(str::to_string),
        });
    }
    if !authorized {
        json_response(&mut socket, 401, &json!({"error":"unauthorized"})).await;
        return;
    }
    if method == "GET" {
        let response = {
            let mut state = state.lock().expect("fixture state");
            match path.as_str() {
                "/health" => Some(
                    json!({"status":"ok","pinnedProvider":state.pinned.then_some("fixture_pin")}),
                ),
                "/v1/models" => Some(
                    json!({"data":state.aliases.iter().map(|alias|json!({"id":alias})).collect::<Vec<_>>()}),
                ),
                "/_aag/v2/receipt_capabilities" => Some(capabilities(&state)),
                path if path.starts_with("/_aag/v2/receipts/") => {
                    state.counts.receipt_reads += 1;
                    None
                }
                _ => None,
            }
        };
        if let Some(response) = response {
            json_response(&mut socket, 200, &response).await;
            return;
        }
        if let Some(nonce) = path.strip_prefix("/_aag/v2/receipts/") {
            receipt_seen.notify_one();
            let (mode, receipt) = {
                let state = state.lock().expect("fixture state");
                let receipt = state
                    .receipts
                    .get(nonce)
                    .filter(|record| {
                        headers.get("x-aag-receipt-capability") == Some(&record.capability)
                    })
                    .map(|record| record.value.clone());
                (state.mode, receipt)
            };
            if mode == FixtureMode::ReceiptDelay {
                tokio::time::sleep(IO_TIMEOUT).await;
                return;
            }
            if mode == FixtureMode::ReceiptMissing {
                json_response(&mut socket, 404, &json!({"error":"missing"})).await;
                return;
            }
            if let Some(receipt) = receipt {
                json_response(&mut socket, 200, &alter_receipt(receipt, mode)).await;
                return;
            }
        }
        json_response(&mut socket, 404, &json!({"error":"missing"})).await;
        return;
    }
    if method != "POST" || !["/v1/chat/completions", "/v1/messages"].contains(&path.as_str()) {
        json_response(&mut socket, 404, &json!({"error":"missing"})).await;
        return;
    }
    let (mode, generation, pinned, completion, sequence) = {
        let mut state = state.lock().expect("fixture state");
        state.counts.inference_requests += 1;
        (
            state.mode,
            state.generation,
            state.pinned,
            state.completion.clone(),
            state.counts.inference_requests,
        )
    };
    let actual_generation = if mode == FixtureMode::PreconditionChanged {
        generation + 1
    } else {
        generation
    };
    if headers.get("x-aag-route-precondition")
        != Some(&format!(
            "v2.{FIXTURE_INSTANCE}.{actual_generation}.{}",
            u8::from(pinned)
        ))
    {
        json_response(
            &mut socket,
            409,
            &json!({"error":{"code":"receipt_route_precondition_failed"}}),
        )
        .await;
        return;
    }
    let Some(opt_in) = headers.get("x-aag-receipt") else {
        json_response(&mut socket, 400, &json!({"error":"opt_in"})).await;
        return;
    };
    let fields: Vec<_> = opt_in.split('.').collect();
    if fields.len() != 3 || fields[0] != "v2" || fields[1].len() != 32 || fields[2].len() != 64 {
        json_response(&mut socket, 400, &json!({"error":"opt_in"})).await;
        return;
    }
    let Some(selector) = value["model"].as_str() else {
        json_response(&mut socket, 400, &json!({"error":"model"})).await;
        return;
    };
    let stream = value["stream"] == true;
    let surface = if path == "/v1/messages" {
        "messages"
    } else {
        "chat"
    };
    let request_id = format!("request_fixture_{sequence}");
    {
        let mut state = state.lock().expect("fixture state");
        state.counts.upstream_calls += 1;
        state.receipts.insert(
            fields[1].to_string(),
            StoredReceipt {
                capability: fields[2].to_string(),
                value: routing_receipt(
                    fields[1],
                    &request_id,
                    selector,
                    surface,
                    generation,
                    pinned,
                    stream,
                ),
            },
        );
    }
    let mut extra = vec![
        ("x-aag-receipt-version".into(), "2".into()),
        ("x-aag-receipt-nonce".into(), fields[1].into()),
        ("x-aag-request-id".into(), request_id),
    ];
    if mode == FixtureMode::ResponseMissingVersion {
        extra.remove(0);
    }
    if mode == FixtureMode::ResponseDuplicateNonce {
        extra.push(("x-aag-receipt-nonce".into(), fields[1].into()));
    }
    if mode == FixtureMode::ResponseWrongRequestId {
        extra
            .iter_mut()
            .find(|(name, _)| name == "x-aag-request-id")
            .expect("id header")
            .1 = "other_request".into();
    }
    if stream {
        let event = json!({"model":selector,"choices":[{"delta":{"content":completion},"finish_reason":"stop"}]});
        send(
            &mut socket,
            200,
            "text/event-stream",
            &extra,
            format!("data: {event}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await;
    } else {
        let output = if surface == "messages" {
            json!({"model":selector,"content":[{"type":"text","text":completion}],"stop_reason":"end_turn"})
        } else {
            json!({"model":selector,"choices":[{"message":{"content":completion},"finish_reason":"stop"}]})
        };
        send(
            &mut socket,
            200,
            "application/json",
            &extra,
            &serde_json::to_vec(&output).expect("fixture output"),
        )
        .await;
    }
}
