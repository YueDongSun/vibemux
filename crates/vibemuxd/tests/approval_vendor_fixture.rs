#![cfg(all(windows, feature = "test_helpers"))]
//! The synthetic peer must wait for the exact one-use permission response.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(100);
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_chat_fixture");
const CONTENT: &str = "approved fixture\n";

struct Peer {
    _temp: tempfile::TempDir,
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    session_id: Uuid,
    effect: PathBuf,
}

impl Peer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("fixture directory");
        let vendor = temp.path().join("vendor space_中文");
        std::fs::create_dir(&vendor).expect("vendor directory");
        let image = vendor.join("claude.exe");
        std::fs::copy(FIXTURE, &image).expect("trusted built fixture");
        let effect = temp.path().join("candidate.txt");
        std::fs::write(
            vendor.join("chat_fixture.json"),
            serde_json::to_vec(&json!({"mode":"claude_approval","effect_path":effect}))
                .expect("config JSON"),
        )
        .expect("fixture config");
        let mut command = Command::new(image);
        command.current_dir(temp.path()).env_clear();
        for name in ["SystemRoot", "WINDIR", "PATH", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(0x0800_0000);
        let mut child = command.spawn().expect("synthetic peer");
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let mut peer = Self {
            _temp: temp,
            child,
            input,
            output,
            session_id: Uuid::new_v4(),
            effect,
        };
        peer.send(json!({"type":"control_request","request_id":"initialize",
            "request":{"subtype":"initialize"}}))
            .await;
        assert_eq!(peer.next().await["response"]["request_id"], "initialize");
        peer
    }

    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).expect("frame JSON");
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.expect("frame write");
        self.input.flush().await.expect("frame flush");
    }

    async fn next(&mut self) -> Value {
        let line = timeout(WAIT, self.output.next_line())
            .await
            .expect("bounded frame")
            .expect("frame read")
            .expect("frame before EOF");
        serde_json::from_str(&line).expect("JSON frame")
    }

    async fn challenge(&mut self) -> Value {
        let user_id = Uuid::new_v4();
        self.send(
            json!({"type":"user","uuid":user_id,"session_id":self.session_id,
            "message":{"role":"user","content":"Create the test candidate after approval."}}),
        )
        .await;
        let mut tool = None;
        for _ in 0..6 {
            let value = self.next().await;
            match value["type"].as_str() {
                Some("system") => {
                    assert_eq!(value["permissionMode"], "manual");
                    assert_eq!(value["tools"], json!(["Write"]));
                    assert_eq!(value["session_id"], self.session_id.to_string());
                }
                Some("assistant") => {
                    assert_eq!(value["session_id"], self.session_id.to_string());
                    assert!(value["parent_tool_use_id"].is_null());
                    let block = value["message"]["content"]
                        .as_array()
                        .expect("assistant content")
                        .iter()
                        .find(|block| block["type"] == "tool_use")
                        .expect("complete tool use")
                        .clone();
                    tool = Some(block);
                }
                Some("control_request") => {
                    let block = tool.expect("root tool evidence before challenge");
                    let request = &value["request"];
                    assert_eq!(request["subtype"], "can_use_tool");
                    assert_eq!(request["tool_name"], "Write");
                    assert_eq!(request["tool_use_id"], block["id"]);
                    assert_eq!(request["input"], block["input"]);
                    assert_eq!(request["input"]["file_path"], json!(self.effect));
                    assert_eq!(request["input"]["content"], CONTENT);
                    return value;
                }
                _ => panic!("unexpected frame before permission request"),
            }
        }
        panic!("missing permission request")
    }

    async fn finished(&mut self) {
        for _ in 0..5 {
            if self.next().await["type"] == "result" {
                return;
            }
        }
        panic!("missing correlated result")
    }

    async fn quiet(&mut self) {
        assert!(timeout(QUIET, self.output.next_line()).await.is_err());
    }

    async fn close(mut self) -> bool {
        self.input.shutdown().await.expect("close input");
        drop(self.input);
        assert!(
            timeout(WAIT, self.child.wait())
                .await
                .expect("bounded exit")
                .expect("peer exit")
                .success()
        );
        self.effect.exists()
    }
}

fn allow(challenge: &Value) -> Value {
    json!({"type":"control_response","response":{"subtype":"success",
        "request_id":challenge["request_id"],
        "response":{"behavior":"allow","updatedInput":challenge["request"]["input"]}}})
}

#[tokio::test]
async fn an_exact_one_use_allow_is_required_before_the_fixture_effect() {
    let mut peer = Peer::start().await;
    let first = peer.challenge().await;
    assert!(!peer.effect.exists());
    peer.quiet().await;
    let mut foreign = allow(&first);
    foreign["response"]["request_id"] = json!("foreign");
    peer.send(foreign).await;
    peer.quiet().await;
    assert!(!peer.effect.exists());
    peer.send(allow(&first)).await;
    peer.finished().await;
    assert_eq!(
        std::fs::read_to_string(&peer.effect).expect("effect"),
        CONTENT
    );
    std::fs::write(&peer.effect, "retained after first effect\n").expect("retained sentinel");
    peer.send(allow(&first)).await;
    peer.quiet().await;
    assert_eq!(
        std::fs::read_to_string(&peer.effect).expect("retained"),
        "retained after first effect\n"
    );
    let second = peer.challenge().await;
    assert_ne!(first["request_id"], second["request_id"]);
    assert_ne!(
        first["request"]["tool_use_id"],
        second["request"]["tool_use_id"]
    );
    peer.send(allow(&first)).await;
    peer.quiet().await;
    assert_eq!(
        std::fs::read_to_string(&peer.effect).expect("retained"),
        "retained after first effect\n"
    );
    peer.send(allow(&second)).await;
    peer.finished().await;
    assert_eq!(
        std::fs::read_to_string(&peer.effect).expect("second effect"),
        CONTENT
    );
    assert!(peer.close().await);
}

#[tokio::test]
async fn denied_or_changed_input_never_creates_the_fixture_effect() {
    for changed_input in [false, true] {
        let mut peer = Peer::start().await;
        let challenge = peer.challenge().await;
        let response = if changed_input {
            let mut response = allow(&challenge);
            response["response"]["response"]["updatedInput"]["content"] = json!("changed");
            response
        } else {
            json!({"type":"control_response","response":{"subtype":"success",
                "request_id":challenge["request_id"],"response":{"behavior":"deny","message":"Denied"}}})
        };
        peer.send(response).await;
        if !changed_input {
            peer.finished().await;
        } else {
            peer.quiet().await;
        }
        assert!(!peer.effect.exists());
        assert!(!peer.close().await, "EOF must not release a pending effect");
    }
}

#[tokio::test]
async fn interrupt_retires_the_challenge_and_eof_cannot_approve_it() {
    let mut peer = Peer::start().await;
    let challenge = peer.challenge().await;
    peer.send(json!({"type":"control_request","request_id":"interrupt",
        "request":{"subtype":"interrupt"}}))
        .await;
    peer.finished().await;
    peer.send(allow(&challenge)).await;
    peer.quiet().await;
    assert!(!peer.effect.exists());
    assert!(!peer.close().await);
}
