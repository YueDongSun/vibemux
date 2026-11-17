#![cfg(all(feature = "test_helpers", feature = "rmcp"))]
//! ADR 075 parity matrix: one wire script through the hand-written session
//! and the SDK session; frames must match after documented normalization.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex},
    time::timeout,
};
use vibemux_mcp::{
    config::HostConfig, fixture_support::FixtureMode, sdk_session::run_session, sdk_wiring,
    session, supervisor::Supervisor,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_mcp_fixture");
const DEADLINE: Duration = Duration::from_secs(3);

fn script(granted: bool) -> Vec<Value> {
    let mut lines = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{
            "protocolVersion":"2025-06-18",
            "capabilities":{"experimental":{"vibemux_metadata_read":granted}},
            "clientInfo":{"name":"parity","version":"1.0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"health","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"host_status","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"bogus","arguments":{}}}),
    ];
    lines.push(
        json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"parity","version":"1.0"}}}),
    );
    lines
}

async fn raw_lines(
    mut client: DuplexStream,
    client_reader: DuplexStream,
    lines: Vec<Value>,
) -> Vec<Value> {
    let mut reader = BufReader::new(client_reader);
    let mut frames = Vec::new();
    async fn read_one(reader: &mut BufReader<DuplexStream>) -> Value {
        let mut line = String::new();
        timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("reply within deadline")
            .expect("read");
        assert!(!line.is_empty(), "session ended before the reply arrived");
        serde_json::from_str(&line).expect("frame")
    }
    for line in lines {
        let mut bytes = serde_json::to_vec(&line).expect("encode");
        bytes.push(b'\n');
        client.write_all(&bytes).await.expect("write");
        if line.get("id").is_some() {
            frames.push(read_one(&mut reader).await);
        }
    }
    client.write_all(b"{not json\n").await.expect("bad line");
    frames.push(read_one(&mut reader).await);
    let mut after = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":8,
        "method":"tools/call","params":{"name":"frontend_tasks","arguments":{}}}))
    .expect("encode");
    after.push(b'\n');
    client.write_all(&after).await.expect("after");
    frames.push(read_one(&mut reader).await);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\",\"params\":{}}\n")
        .await
        .expect("ping");
    frames.push(read_one(&mut reader).await);
    drop(client);
    let mut extra = String::new();
    let read = timeout(Duration::from_secs(2), reader.read_line(&mut extra)).await;
    assert!(
        matches!(read, Ok(Ok(0)) | Err(_)),
        "unexpected extra frame: {extra}"
    );
    frames
}

async fn handwritten(granted: bool) -> Vec<Value> {
    let config = HostConfig {
        descriptor: PathBuf::from("unused"),
        allow_metadata_read: granted,
    };
    let supervisor = Supervisor::for_fixture(
        config.clone(),
        PathBuf::from(FIXTURE),
        FixtureMode::Healthy,
        DEADLINE,
    );
    let (client, server_rx) = duplex(1 << 20);
    let (server_tx, client_rx) = duplex(1 << 20);
    let handle = tokio::spawn(session::run_fixture(
        BufReader::new(server_rx),
        server_tx,
        config,
        supervisor,
    ));
    let frames = raw_lines(client, client_rx, script(granted)).await;
    let _ = timeout(Duration::from_secs(5), handle).await;
    frames
}

async fn sdk(granted: bool) -> Vec<Value> {
    let config = HostConfig {
        descriptor: PathBuf::from("unused"),
        allow_metadata_read: granted,
    };
    let supervisor = Supervisor::for_fixture(
        config.clone(),
        PathBuf::from(FIXTURE),
        FixtureMode::Healthy,
        DEADLINE,
    );
    let wiring = sdk_wiring::start(config, supervisor);
    let sdk_wiring::Wiring {
        host,
        events,
        finalize,
        ..
    } = wiring;
    let (client, server_rx) = duplex(1 << 20);
    let (server_tx, client_rx) = duplex(1 << 20);
    let handle = tokio::spawn(run_session(
        BufReader::new(server_rx),
        server_tx,
        host,
        granted,
        None,
        Some(events),
        finalize,
    ));
    let frames = raw_lines(client, client_rx, script(granted)).await;
    let _ = timeout(Duration::from_secs(5), handle).await;
    frames
}

fn normalize(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("fixture_pid");
            map.remove("resultType");
            map.remove("structuredContent");
            for child in map.values_mut() {
                normalize(child);
            }
            // The hand-written loop answers a cancelled/duplicate id with a
            // null id; the SDK path echoes the request id. Neither runs here,
            // but normalization keeps the comparison honest if they appear.
            if map
                .get("error")
                .and_then(|error| error.get("data"))
                .and_then(|data| data.get("code"))
                == Some(&json!("request_cancelled"))
            {
                map.insert("id".into(), Value::Null);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        Value::String(text) => {
            // Tool results embed the JSON as text; the fixture identity
            // differs between the two sessions' reader processes.
            if let Some(start) = text.find("\"fixture_pid\":") {
                let start = start + "\"fixture_pid\":".len();
                let end = text[start..]
                    .find(|c: char| !c.is_ascii_digit())
                    .map_or(text.len(), |offset| start + offset);
                text.replace_range(start..end, "0");
            }
        }
        _ => {}
    }
}

/// JSON-RPC permits out-of-order replies; the hand-written loop answers
/// strictly in order while the SDK session may complete a routed request
/// after a later direct reply. Parity therefore compares per request id,
/// and the anonymous frames (parse errors) in order.
fn indexed(frames: Vec<Value>) -> (BTreeMap<String, Value>, Vec<Value>) {
    let mut by_id = BTreeMap::new();
    let mut anonymous = Vec::new();
    for mut frame in frames {
        normalize(&mut frame);
        match frame.get("id") {
            Some(Value::Null) | None => anonymous.push(frame),
            Some(id) => {
                assert!(
                    by_id.insert(id.to_string(), frame).is_none(),
                    "duplicate reply id"
                );
            }
        }
    }
    (by_id, anonymous)
}

async fn compare(granted: bool) {
    let manual = handwritten(granted).await;
    let sdk = sdk(granted).await;
    assert!(!manual.is_empty(), "hand-written session produced frames");
    let (manual_by_id, manual_anonymous) = indexed(manual);
    let (sdk_by_id, sdk_anonymous) = indexed(sdk);
    assert_eq!(
        manual_by_id.keys().collect::<Vec<_>>(),
        sdk_by_id.keys().collect::<Vec<_>>(),
        "reply ids"
    );
    for (id, left) in &manual_by_id {
        assert_eq!(
            left,
            sdk_by_id.get(id).expect("same ids"),
            "reply {id} differs"
        );
    }
    assert_eq!(manual_anonymous, sdk_anonymous, "anonymous frames");
}

#[tokio::test(flavor = "multi_thread")]
async fn granted_scripts_match_frame_by_frame() {
    compare(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_scripts_match_frame_by_frame() {
    compare(false).await;
}
