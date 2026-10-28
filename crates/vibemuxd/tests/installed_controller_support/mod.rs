//! Bounded synthetic Anthropic transport for explicit installed-client tests.
use serde_json::{Value, json};
use std::{
    net::{SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REQUESTS: usize = 48;
const CONNECTION_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Scenario {
    pub worker_calls: Vec<Value>,
    pub reviewer_calls: Vec<Value>,
    pub changed_files: Vec<String>,
    pub forbidden_marker: String,
    pub hold_after_call: bool,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub tools: Vec<String>,
    pub result_errors: Vec<bool>,
    pub denial_codes: Vec<String>,
    pub forbidden_context: bool,
    pub synthetic_auth: bool,
    pub model: String,
}
pub struct SyntheticApi {
    pub address: SocketAddrV4,
    observations: Arc<Mutex<Vec<Observation>>>,
    failures: Arc<Mutex<Vec<String>>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}
impl SyntheticApi {
    pub async fn start(scenario: Scenario) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            panic!("IPv4");
        };
        let observations = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let (stop, mut stopped) = oneshot::channel();
        let observed = Arc::clone(&observations);
        let failed = Arc::clone(&failures);
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let scenario = scenario.clone();
                        let observed = Arc::clone(&observed);
                        let failed = Arc::clone(&failed);
                        connections.spawn(async move {
                            let result = tokio::time::timeout(CONNECTION_DEADLINE,
                                handle(socket, scenario, observed)).await;
                            match result {
                                Ok(Ok(())) => (),
                                Ok(Err(code)) => failed.lock().unwrap().push(code.into()),
                                Err(_) => failed.lock().unwrap().push("connection_timeout".into()),
                            }
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            address,
            observations,
            failures,
            stop: Some(stop),
            task: Some(task),
        }
    }
    pub fn observations(&self) -> Vec<Observation> {
        self.observations.lock().unwrap().clone()
    }
    pub fn failures(&self) -> Vec<String> {
        self.failures.lock().unwrap().clone()
    }
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.unwrap();
        }
    }
}
impl Drop for SyntheticApi {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn handle(
    mut socket: TcpStream,
    scenario: Scenario,
    observed: Arc<Mutex<Vec<Observation>>>,
) -> Result<(), &'static str> {
    let (head, bytes) = read_request(&mut socket).await?;
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("request_line")?;
    let path = target.split('?').next().unwrap_or_default();
    if !head.starts_with("POST ") || !matches!(path, "/v1/messages" | "/v1/messages/count_tokens") {
        return Err("unexpected_endpoint");
    }
    let token_count = path == "/v1/messages/count_tokens";
    let request: Value = serde_json::from_slice(&bytes).map_err(|_| "request_json")?;
    let result_errors: Vec<bool> = request["messages"]
        .as_array()
        .ok_or("messages")?
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .map(|block| block["is_error"].as_bool().unwrap_or(false))
        .collect();
    let denial_codes = [
        "worker_tools_missing",
        "worker_tools_already_exists",
        "invalid_arguments",
    ]
    .into_iter()
    .filter(|code| request["messages"].to_string().contains(code))
    .map(str::to_owned)
    .collect();
    let tools = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    {
        let mut observed = observed.lock().unwrap();
        if observed.len() >= MAX_REQUESTS {
            return Err("request_limit");
        }
        observed.push(Observation {
            tools,
            result_errors: result_errors.clone(),
            denial_codes,
            forbidden_context: String::from_utf8_lossy(&bytes).contains(&scenario.forbidden_marker),
            synthetic_auth: head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
                .map(|(_, value)| value.trim())
                .eq(["synthetic-fixture-key"]),
            model: request["model"].as_str().unwrap_or_default().into(),
        });
    }
    if token_count {
        return send(&mut socket, "application/json", r#"{"input_tokens":1}"#).await;
    }
    let prompt = request["messages"]
        .as_array()
        .ok_or("messages")?
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| text_blocks(&message["content"]))
        .find(|text| text.contains("TaskSpec digest: "))
        .ok_or("rendered_prompt")?;
    let digest = prompt
        .lines()
        .find_map(|line| line.strip_prefix("TaskSpec digest: "))
        .ok_or("digest")?
        .trim();
    let review = prompt
        .lines()
        .find(|line| line.starts_with("turn "))
        .is_some_and(|line| line.ends_with(": review"));
    let calls = if review {
        &scenario.reviewer_calls
    } else {
        &scenario.worker_calls
    };
    if scenario.hold_after_call && !result_errors.is_empty() {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    let block = if let Some(call) = calls.get(result_errors.len()) {
        json!({"type":"tool_use","id":format!("tool_fixture_{}",result_errors.len()),
            "name":call["name"],"input":call["input"]})
    } else {
        let (fence, report) = if review {
            (
                "vibemux_review",
                json!({"schema_version":1,"task_spec_digest":digest,
                "verdict":"pass","findings":[],"checks_executed":["read the candidate"]}),
            )
        } else {
            (
                "vibemux_checkpoint",
                json!({"schema_version":1,"task_spec_digest":digest,
                "changed_files":scenario.changed_files,"development_tests":[],"unresolved_issues":[],
                "context_refs_consumed":[],"messages":[],"summary":"Installed controller synthetic qualification"}),
            )
        };
        let delimiter = char::from(96).to_string().repeat(3);
        json!({"type":"text","text":format!("Fixture complete.\n{delimiter}{fence}\n{report}\n{delimiter}\n")})
    };
    let stop = if block["type"] == "tool_use" {
        "tool_use"
    } else {
        "end_turn"
    };
    if request["stream"] == true {
        let empty = if stop == "tool_use" {
            json!({"type":"tool_use","id":block["id"],"name":block["name"],"input":{}})
        } else {
            json!({"type":"text","text":""})
        };
        let delta = if stop == "tool_use" {
            json!({"type":"input_json_delta","partial_json":block["input"].to_string()})
        } else {
            json!({"type":"text_delta","text":block["text"]})
        };
        let events = [
            (
                "message_start",
                json!({"type":"message_start","message":{"id":"msg_fixture","type":"message",
                "role":"assistant","model":"fixture-model","content":[],"stop_reason":null,
                "stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}),
            ),
            (
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":empty}),
            ),
            (
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":delta}),
            ),
            (
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            (
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":1}}),
            ),
            ("message_stop", json!({"type":"message_stop"})),
        ];
        let body = events
            .into_iter()
            .map(|(name, value)| format!("event: {name}\ndata: {value}\n\n"))
            .collect::<String>();
        send(&mut socket, "text/event-stream", &body).await
    } else {
        let body = json!({"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model",
            "content":[block],"stop_reason":stop,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}).to_string();
        send(&mut socket, "application/json", &body).await
    }
}
fn text_blocks(value: &Value) -> Vec<&str> {
    if let Some(text) = value.as_str() {
        return vec![text];
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect()
}
async fn read_request(socket: &mut TcpStream) -> Result<(String, Vec<u8>), &'static str> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = socket.read(&mut chunk).await.map_err(|_| "request_read")?;
        if count == 0 || bytes.len() + count > MAX_REQUEST_BYTES {
            return Err("request_bound");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).map_err(|_| "header_encoding")?;
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .ok_or("content_length")?;
    if length > MAX_REQUEST_BYTES - header_end {
        return Err("request_bound");
    }
    while bytes.len() - header_end < length {
        let count = socket.read(&mut chunk).await.map_err(|_| "request_read")?;
        if count == 0 || bytes.len() + count > MAX_REQUEST_BYTES {
            return Err("request_bound");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok((head, bytes[header_end..header_end + length].to_vec()))
}
async fn send(socket: &mut TcpStream, content_type: &str, body: &str) -> Result<(), &'static str> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    socket
        .write_all(head.as_bytes())
        .await
        .map_err(|_| "response_write")?;
    socket
        .write_all(body.as_bytes())
        .await
        .map_err(|_| "response_write")?;
    socket.shutdown().await.map_err(|_| "response_close")
}
