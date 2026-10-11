#![cfg(feature = "test_endpoint")]
//! Spawns the compiled peer binary against a loopback fixture server and
//! checks its exit-code/stdout/stderr contract (ADR 034 §6).

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::process::{Command, Output, Stdio};

use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use vibemux_decision::{
    AgentKind, DecisionQuestion, JevConfig, Label, PeerRequest, PeerResult, RouteFacts, Threshold,
};

const CREDENTIAL_SENTINEL: &str = "sk_test_sentinel_value_123";
const CREDENTIAL_ENV_VAR: &str = "TYPESAFE_API_KEY";
/// Mirrors `SYSTEM_ENVIRONMENT_NAMES` in `vibemuxd::decision::peer_process`.
const DAEMON_SYSTEM_ENVIRONMENT_NAMES: [&str; 4] = ["SystemRoot", "WINDIR", "TEMP", "TMP"];

fn request_line() -> Vec<u8> {
    let eligible = [AgentKind::Claude, AgentKind::Codex];
    let question = DecisionQuestion::harness_route(
        &eligible,
        &BTreeMap::new(),
        Threshold::from_unit(0.9).unwrap(),
    )
    .unwrap();
    let facts = RouteFacts::new(
        vec![Label::new("rust").unwrap()],
        2048,
        Some(AgentKind::Codex),
        &eligible,
    );
    PeerRequest::for_route(&question, &facts, &JevConfig::default())
        .encode_line()
        .expect("fixture request encodes")
}

fn success_body() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "harness_route": {
                "type": "choice",
                "choice": "codex",
                "probabilities": {"claude": 0.1, "codex": 0.8, "escalate": 0.1},
                "confidence": 0.81,
            }
        }
    })
}

struct RunningServer {
    address: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    async fn shutdown(self) {
        let _ = self.stop.send(());
        self.handle.await.expect("server task joins");
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1/systemone", self.address)
    }
}

async fn spawn(app: Router) -> RunningServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("local address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .expect("server runs");
    });
    RunningServer {
        address,
        stop,
        handle,
    }
}

/// Runs the compiled peer binary with a cleared environment plus the
/// platform's minimum system variables, the credential sentinel, and the
/// loopback test endpoint override. Spawning and waiting happen on tokio's
/// blocking pool so a concurrently running loopback server keeps making
/// progress on the same runtime.
async fn run_peer_binary(endpoint: Option<&str>, stdin_bytes: Vec<u8>) -> Output {
    let endpoint = endpoint.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        let exe = env!("CARGO_BIN_EXE_vibemux_decision_peer");
        let mut command = Command::new(exe);
        command.env_clear();
        // Exactly the system variables `vibemuxd` passes to the peer: no
        // `PATH`, so the binary must start without it.
        for name in DAEMON_SYSTEM_ENVIRONMENT_NAMES {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env(CREDENTIAL_ENV_VAR, CREDENTIAL_SENTINEL);
        if let Some(endpoint) = endpoint {
            command.env("VIBEMUX_DECISION_PEER_TEST_ENDPOINT", endpoint);
        }
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn the peer binary");
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(&stdin_bytes)
            .expect("write stdin");
        child.wait_with_output().expect("wait for the peer binary")
    })
    .await
    .expect("blocking task joins")
}

fn assert_no_sentinel(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(CREDENTIAL_SENTINEL),
        "stdout leaked the credential"
    );
    assert!(
        !stderr.contains(CREDENTIAL_SENTINEL),
        "stderr leaked the credential"
    );
}

#[tokio::test]
async fn a_successful_answer_is_the_only_stdout_line_with_exit_zero() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|headers: HeaderMap, Json(_body): Json<Value>| async move {
            assert_eq!(
                headers.get("authorization").expect("authorization header"),
                &format!("Bearer {CREDENTIAL_SENTINEL}")
            );
            Json(success_body())
        }),
    );
    let server = spawn(app).await;
    let output = run_peer_binary(Some(&server.endpoint()), request_line()).await;
    assert_eq!(output.status.code(), Some(0));
    assert_no_sentinel(&output);
    let decoded =
        PeerResult::decode_line(&output.stdout).expect("exactly one decodable result line");
    let answer = decoded.expect("answered");
    assert_eq!(answer.choice, "codex");
    server.shutdown().await;
}

#[tokio::test]
async fn a_malformed_body_that_echoes_the_auth_header_never_leaks_it() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|headers: HeaderMap| async move {
            let auth = headers
                .get("authorization")
                .expect("authorization header")
                .to_str()
                .expect("ascii header")
                .to_string();
            // Deliberately echoes the credential back; the peer must still
            // reject the body and never forward it to stdout/stderr.
            json!({"leaked": auth}).to_string()
        }),
    );
    let server = spawn(app).await;
    let output = run_peer_binary(Some(&server.endpoint()), request_line()).await;
    assert_eq!(output.status.code(), Some(0));
    assert_no_sentinel(&output);
    let decoded =
        PeerResult::decode_line(&output.stdout).expect("exactly one decodable result line");
    assert!(decoded.is_err(), "the malformed body must not be accepted");
    server.shutdown().await;
}

#[tokio::test]
async fn an_invalid_stdin_line_fails_cleanly_without_a_server() {
    let output = run_peer_binary(None, b"not json\n".to_vec()).await;
    assert_eq!(output.status.code(), Some(0));
    assert_no_sentinel(&output);
    let decoded =
        PeerResult::decode_line(&output.stdout).expect("exactly one decodable result line");
    assert!(decoded.is_err());
}

#[tokio::test]
async fn oversized_stdin_without_a_newline_fails_cleanly_without_a_server() {
    let oversized = vec![b'x'; vibemux_decision::wire::MAX_PEER_REQUEST_BYTES + 256];
    let output = run_peer_binary(None, oversized).await;
    assert_eq!(output.status.code(), Some(0));
    assert_no_sentinel(&output);
    let decoded =
        PeerResult::decode_line(&output.stdout).expect("exactly one decodable result line");
    assert!(decoded.is_err());
}
