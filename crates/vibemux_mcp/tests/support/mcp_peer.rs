use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use vibemux_mcp::{config::MAX_OUTPUT_BYTES, framing::LineReader};

const DEADLINE: Duration = Duration::from_secs(15);

pub struct McpPeer {
    child: Child,
    input: Option<ChildStdin>,
    output: LineReader<BufReader<ChildStdout>>,
    pub notifications: usize,
}

impl McpPeer {
    pub async fn start(descriptor: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vibemux_mcp"));
        command
            .arg("--descriptor")
            .arg(descriptor)
            .arg("--allow_metadata_read")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command.spawn().expect("MCP peer spawn");
        let input = child.stdin.take();
        let output = LineReader::new(
            BufReader::new(child.stdout.take().unwrap()),
            MAX_OUTPUT_BYTES,
        );
        let mut peer = Self {
            child,
            input,
            output,
            notifications: 0,
        };
        let initialized = peer.call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).await;
        assert!(initialized.get("result").is_some(), "initialize response");
        peer.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        peer
    }

    pub async fn send(&mut self, value: Value) {
        let bytes = format!("{value}\n");
        self.input
            .as_mut()
            .expect("open stdin")
            .write_all(bytes.as_bytes())
            .await
            .expect("send request");
    }

    pub async fn receive(&mut self) -> Value {
        let bytes = timeout(DEADLINE, self.output.next())
            .await
            .expect("response deadline")
            .expect("bounded response")
            .expect("response before EOF");
        serde_json::from_slice(&bytes).expect("JSON-RPC response")
    }

    pub async fn call(&mut self, request: Value) -> Value {
        let id = request["id"].clone();
        self.send(request).await;
        for _ in 0..4 {
            let reply = self.receive().await;
            if reply.get("id") == Some(&id) {
                return reply;
            }
            assert_eq!(reply["method"], "notifications/tools/list_changed");
            self.notifications += 1;
        }
        panic!("too many notifications before response");
    }

    pub async fn finish(mut self) {
        self.input.take();
        let status = timeout(DEADLINE, self.child.wait())
            .await
            .expect("MCP EOF exit deadline")
            .expect("MCP exit");
        assert!(
            status.success(),
            "MCP should cleanly terminate owned resources on EOF"
        );
    }
}
