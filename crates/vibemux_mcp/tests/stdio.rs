use std::{process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const BINARY: &str = env!("CARGO_BIN_EXE_vibemux_mcp");
const TEST_DEADLINE: Duration = Duration::from_secs(15);

async fn exchange(arguments: &[&str], messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(BINARY)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn MCP fixture");
    let mut input = child.stdin.take().expect("stdin");
    for message in messages {
        let line = format!("{message}\n");
        // A broken pipe is still an observable failed exchange below.
        if input.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }
    drop(input);
    let output = timeout(TEST_DEADLINE, child.wait_with_output())
        .await
        .expect("bounded host exit")
        .expect("host output");
    assert!(output.status.success(), "MCP host must exit cleanly on EOF");
    String::from_utf8(output.stdout)
        .expect("UTF-8 protocol")
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON-RPC only stdout"))
        .collect()
}

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"fixture","version":"1"}}})
}

#[tokio::test]
async fn default_denial_still_initializes_and_lists_without_descriptor_io() {
    let replies = exchange(
        &["--descriptor", "nonexistent_private_descriptor"],
        &[
            initialize(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"host_status","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"health","arguments":{}}}),
        ],
    )
    .await;
    assert_eq!(
        replies.len(),
        4,
        "initialization, discovery, status and denial respond"
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(replies[1]["result"]["tools"][0]["name"], "host_status");
    assert_eq!(
        replies[2]["result"]["structuredContent"]["worker_launches"],
        0
    );
    assert_eq!(
        replies[2]["result"]["structuredContent"]["metadata_read"],
        false
    );
    assert_eq!(replies[3]["error"]["data"]["code"], "metadata_read_denied");
}

#[tokio::test]
async fn granted_discovery_and_argument_rejection_do_not_start_a_reader() {
    let mut messages = vec![
        initialize(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    for (index, name, arguments) in [
        (3, "health", json!({"extra":true})),
        (4, "frontend_task", json!({"task_id":"not_a_uuid"})),
        (5, "frontend_tasks", json!({"limit":33})),
        (
            6,
            "frontend_tasks",
            json!({"after":"00000000-0000-0000-0000-000000000000"}),
        ),
        (
            7,
            "harness_dispatch_status",
            json!({"request_id":"urn:uuid:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}),
        ),
    ] {
        messages.push(json!({"jsonrpc":"2.0","id":index,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
    }
    messages.push(json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"host_status","arguments":{}}}));
    let replies = exchange(
        &[
            "--descriptor",
            "missing_descriptor",
            "--allow_metadata_read",
        ],
        &messages,
    )
    .await;
    let names: Vec<_> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "host_status",
            "health",
            "frontend_tasks",
            "frontend_task",
            "harness_dispatch_status"
        ]
    );
    for reply in &replies[2..7] {
        assert_eq!(reply["error"]["data"]["code"], "invalid_arguments");
    }
    assert_eq!(
        replies[7]["result"]["structuredContent"]["worker_launches"],
        0
    );
    assert_eq!(
        replies[7]["result"]["structuredContent"]["metadata_read"],
        true
    );
}

#[tokio::test]
async fn initialization_reduces_the_host_grant_and_negotiates_only_a_supported_version() {
    for host_grant in [false, true] {
        let mut initialize = initialize();
        initialize["params"]["protocolVersion"] = json!("2099-01-01");
        initialize["params"]["capabilities"] =
            json!({"experimental":{"vibemux_metadata_read":false}});
        let mut arguments = vec!["--descriptor", "missing_descriptor"];
        if host_grant {
            arguments.push("--allow_metadata_read");
        }
        let replies = exchange(
            &arguments,
            &[
                json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}),
                initialize,
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            ],
        )
        .await;
        assert_eq!(replies[0]["error"]["data"]["code"], "not_initialized");
        assert_eq!(replies[1]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(replies[2]["error"]["data"]["code"], "not_initialized");
        assert_eq!(replies[3]["result"]["tools"].as_array().unwrap().len(), 1);
    }
    let mut requested = initialize();
    requested["params"]["capabilities"] = json!({"experimental":{"vibemux_metadata_read":true}});
    let replies = exchange(&["--descriptor", "missing_descriptor"], &[
        requested,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"health","arguments":{}}}),
    ]).await;
    assert_eq!(replies[1]["error"]["data"]["code"], "metadata_read_denied");
}

#[tokio::test]
async fn standard_request_metadata_is_ignored_without_weakening_tool_argument_schemas() {
    let replies = exchange(&["--descriptor", "missing_descriptor"], &[
        initialize(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{"_meta":{"fixture":true}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"progressToken":uuid::Uuid::new_v4().to_string()}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{"_meta":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"host_status","_meta":{"allow_metadata_read":true},"arguments":{}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"host_status","arguments":{"_meta":{}}}}),
    ]).await;
    assert!(replies[1]["result"]["tools"].is_array());
    assert_eq!(replies[2]["result"], json!({}));
    assert_eq!(
        replies[3]["result"]["structuredContent"]["metadata_read"],
        false
    );
    assert_eq!(replies[4]["error"]["data"]["code"], "invalid_arguments");
}

#[tokio::test]
async fn oversized_unterminated_stdio_exits_even_when_client_keeps_stdin_open() {
    let mut child = Command::new(BINARY)
        .args(["--descriptor", "missing_descriptor"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let mut errors = child.stderr.take().unwrap();
    let _ = input
        .write_all(&vec![b'x'; vibemux_mcp::config::MAX_INPUT_BYTES + 1])
        .await;
    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("fatal framing must not wait for client EOF")
        .unwrap();
    drop(input);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    output.read_to_end(&mut stdout).await.unwrap();
    errors.read_to_end(&mut stderr).await.unwrap();
    assert!(!status.success());
    assert!(stdout.is_empty());
    assert_eq!(String::from_utf8(stderr).unwrap().trim(), "frame_too_large");
}
