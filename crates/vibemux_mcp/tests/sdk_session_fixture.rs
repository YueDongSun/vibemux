#![cfg(all(feature = "test_helpers", feature = "rmcp"))]
//! ADR 075 Scope A: the SDK session driver over the real fixture reader
//! process, through the production supervisor adapter.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex},
    time::timeout,
};
use vibemux_mcp::{
    config::HostConfig, fixture_support::FixtureMode, sdk_session::run_session, sdk_wiring,
    supervisor::Supervisor,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_mcp_fixture");
const CALL_DEADLINE: Duration = Duration::from_secs(3);

struct Harness {
    stdin: DuplexStream,
    stdout: BufReader<DuplexStream>,
    session: tokio::task::JoinHandle<Result<(), &'static str>>,
}

fn start() -> Harness {
    let config = HostConfig {
        descriptor: PathBuf::from("unused_fixture_descriptor"),
        allow_metadata_read: true,
    };
    let supervisor = Supervisor::for_fixture(
        config.clone(),
        PathBuf::from(FIXTURE),
        FixtureMode::Healthy,
        CALL_DEADLINE,
    );
    let wiring = sdk_wiring::start(config, supervisor);
    let cancel = {
        let executor = wiring.executor.clone();
        Arc::new(move || executor.cancel()) as Arc<dyn Fn() + Send + Sync>
    };
    let sdk_wiring::Wiring {
        host,
        events,
        finalize,
        ..
    } = wiring;
    let (client_writer, server_reader) = duplex(1 << 20);
    let (server_writer, client_reader) = duplex(1 << 20);
    let session = tokio::spawn(run_session(
        BufReader::new(server_reader),
        server_writer,
        host,
        true,
        Some(cancel),
        Some(events),
        finalize,
    ));
    Harness {
        stdin: client_writer,
        stdout: BufReader::new(client_reader),
        session,
    }
}

impl Harness {
    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).expect("encode");
        line.push(b'\n');
        self.stdin.write_all(&line).await.expect("write frame");
    }

    async fn read(&mut self) -> Value {
        let mut line = String::new();
        tokio::select! {
            result = timeout(Duration::from_secs(5), self.stdout.read_line(&mut line)) => {
                result.expect("frame within deadline").expect("read frame");
            }
            result = &mut self.session => {
                panic!("session ended before replying: {result:?}");
            }
        }
        assert!(!line.is_empty(), "expected a frame");
        serde_json::from_str(&line).expect("JSON frame")
    }

    async fn initialize(&mut self) {
        self.send(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18",
            "capabilities":{"experimental":{"vibemux_metadata_read":true}},
            "clientInfo":{"name":"fixture","version":"1.0"}}}),
        )
        .await;
        let reply = self.read().await;
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
            .await;
    }

    async fn finish(mut self) -> Result<(), &'static str> {
        drop(self.stdin);
        timeout(Duration::from_secs(10), &mut self.session)
            .await
            .expect("session ends")
            .expect("session task joins")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sdk_session_reads_through_the_fixture_reader_process() {
    let mut harness = start();
    harness.initialize().await;
    harness
        .send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .await;
    let listed = harness.read().await;
    assert_eq!(
        listed["result"]["tools"].as_array().map(Vec::len),
        Some(5),
        "the granted surface lists five tools"
    );

    harness
        .send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"health","arguments":{}}}))
        .await;
    let health = harness.read().await;
    assert_eq!(health["id"], 3);
    assert_eq!(health["result"]["isError"], false);
    // The fixture reader answers with its synthetic identity payload; the
    // supervisor validates the envelope, not daemon semantics.
    let structured = &health["result"]["structuredContent"];
    assert!(
        structured["fixture_pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0)
    );
    assert!(structured["sequence"].as_u64().is_some_and(|seq| seq >= 1));

    // The local tool answers while the reader frame path stays alive.
    harness
        .send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"host_status","arguments":{}}}))
        .await;
    let status = harness.read().await;
    assert_eq!(status["result"]["isError"], false);
    assert_eq!(status["result"]["structuredContent"]["metadata_read"], true);

    harness
        .finish()
        .await
        .expect("clean shutdown with the finalizer");
}
