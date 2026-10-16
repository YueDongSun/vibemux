use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use vibemuxd::workflow::aag_gateway::Aag_Fixture_Permit;

const FIXTURE_TOKEN: &str = "aag_workflow_fixture_token";
const MAX_REQUEST_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedCall {
    pub alias: String,
    pub correlation: String,
    pub authorization_ok: bool,
}

pub struct FakeAagServer {
    origin: String,
    permit: Aag_Fixture_Permit,
    calls: Arc<Mutex<Vec<ObservedCall>>>,
    task: JoinHandle<()>,
}

impl FakeAagServer {
    pub async fn start(aliases: Vec<String>, compiler_output: String) -> Self {
        let listener = Arc::new(
            TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local fixture gateway"),
        );
        let address = listener.local_addr().expect("fixture gateway address");
        let origin = format!("http://{address}");
        let permit = Aag_Fixture_Permit::from_owned_listener(Arc::clone(&listener))
            .expect("retained local listener proves the fixture origin");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(serve(
            listener,
            Arc::clone(&calls),
            aliases.clone(),
            compiler_output,
        ));
        Self {
            origin,
            permit,
            calls,
            task,
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn permit(&self) -> Aag_Fixture_Permit {
        self.permit.clone()
    }

    pub fn calls(&self) -> Vec<ObservedCall> {
        self.calls.lock().expect("fixture calls").clone()
    }
}

impl Drop for FakeAagServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    listener: Arc<TcpListener>,
    calls: Arc<Mutex<Vec<ObservedCall>>>,
    aliases: Vec<String>,
    compiler_output: String,
) {
    while let Ok((socket, _)) = listener.accept().await {
        let calls = Arc::clone(&calls);
        let aliases = aliases.clone();
        let compiler_output = compiler_output.clone();
        tokio::spawn(async move {
            handle_connection(socket, calls, aliases, compiler_output).await;
        });
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    calls: Arc<Mutex<Vec<ObservedCall>>>,
    aliases: Vec<String>,
    compiler_output: String,
) {
    let Some((head, body)) = read_request(&mut socket).await else {
        return;
    };
    let request_line = head.lines().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let headers = head.to_ascii_lowercase();
    match path {
        "/health" => {
            send_json(
                &mut socket,
                &json!({"status": "ok", "pinnedProvider": null}),
            )
            .await;
        }
        "/v1/models" => {
            let models: Vec<Value> = aliases
                .into_iter()
                .map(|alias| json!({"id": alias}))
                .collect();
            send_json(&mut socket, &json!({"data": models})).await;
        }
        "/v1/chat/completions" => {
            let request: Value = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(_) => {
                    send_json(&mut socket, &json!({"error": "invalid_fixture_request"})).await;
                    return;
                }
            };
            let alias = request["model"].as_str().unwrap_or_default().to_string();
            let correlation = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("x-vibemux-correlation:")
                        .map(|value| value.trim().to_string())
                })
                .unwrap_or_default();
            calls.lock().expect("fixture calls").push(ObservedCall {
                alias: alias.clone(),
                correlation,
                authorization_ok: headers
                    .contains(&format!("authorization: bearer {FIXTURE_TOKEN}")),
            });
            send_json(
                &mut socket,
                &json!({
                    "model": alias,
                    "choices": [{
                        "message": {"content": compiler_output},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}
                }),
            )
            .await;
        }
        _ => send_json(&mut socket, &json!({"error": "not_found"})).await,
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 || bytes.len().saturating_add(read) > MAX_REQUEST_BYTES {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).ok()?;
    let content_length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES {
        return None;
    }
    while bytes.len().saturating_sub(header_end) < content_length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Some((
        head,
        bytes[header_end..header_end + content_length].to_vec(),
    ))
}

async fn send_json(socket: &mut TcpStream, value: &Value) {
    let body = serde_json::to_vec(value).expect("fixture response JSON");
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&body).await;
    let _ = socket.shutdown().await;
}

pub fn fixture_token() -> String {
    FIXTURE_TOKEN.to_string()
}
