use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::*, protocol::Protocol, server::serve, workspace::WorkerTools,
};
use vibemux_workflow::PathPattern;

fn fixture() -> (TempDir, WorkerTools) {
    let parent = TempDir::new().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(parent.path()).unwrap();
    let tools = WorkerTools::create(
        parent.path(),
        Bootstrap {
            schema_version: 1,
            attempt_id: Uuid::new_v4(),
            grants: WritableGrant {
                write_files: true,
                ..WritableGrant::NONE
            },
            owned_paths: vec![PathPattern::new("src/**").unwrap()],
            read_scopes: vec![],
            forbidden_paths: vec![],
            files: vec![],
        },
    )
    .unwrap();
    (parent, tools)
}
fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
fn initialize() -> Value {
    request(
        1,
        "initialize",
        json!({"protocolVersion":"2025-11-25","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"fixture","version":"1"}}),
    )
}
fn ready(protocol: &mut Protocol, tools: &mut WorkerTools) {
    assert_eq!(
        protocol.handle(initialize(), tools).unwrap().unwrap()["result"]["protocolVersion"],
        "2025-06-18"
    );
    protocol
        .handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            tools,
        )
        .unwrap();
}

#[test]
fn initialization_tools_and_duplicate_mutation_ids_are_closed() {
    let (_parent, mut tools) = fixture();
    let mut protocol = Protocol::default();
    assert_eq!(
        protocol
            .handle(request(0, "tools/list", json!({})), &mut tools)
            .unwrap()
            .unwrap()["error"]["message"],
        "not_initialized"
    );
    ready(&mut protocol, &mut tools);
    let listed = protocol
        .handle(request(2, "tools/list", json!({})), &mut tools)
        .unwrap()
        .unwrap();
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["create_file"]);
    let call = request(
        3,
        "tools/call",
        json!({"name":"create_file","arguments":{"path":"src/new.txt","content":"fixture"}}),
    );
    assert_eq!(
        protocol.handle(call.clone(), &mut tools).unwrap().unwrap()["result"]["isError"],
        false
    );
    assert_eq!(
        protocol.handle(call, &mut tools).unwrap().unwrap()["error"]["message"],
        "duplicate_request_id"
    );
    let denied = protocol
        .handle(
            request(
                4,
                "tools/call",
                json!({"name":"read_file","arguments":{"path":"src/new.txt"}}),
            ),
            &mut tools,
        )
        .unwrap()
        .unwrap();
    assert_eq!(denied["result"]["isError"], true);
    assert!(!denied.to_string().contains("fixture"));
    assert_eq!(tools.candidate().unwrap().changed_files.len(), 1);
}

#[test]
fn malformed_arguments_and_roots_cannot_add_authority() {
    let (_parent, mut tools) = fixture();
    let mut protocol = Protocol::default();
    ready(&mut protocol, &mut tools);
    for (index, arguments) in [
        json!(true),
        json!([]),
        json!(null),
        json!({"path":"src/new.txt","content":"x","operation":"read"}),
        json!({"path":"src/new.txt","content":"x","root":"C:/"}),
    ]
    .into_iter()
    .enumerate()
    {
        let result = protocol
            .handle(
                request(
                    index as u64 + 2,
                    "tools/call",
                    json!({"name":"create_file","arguments":arguments}),
                ),
                &mut tools,
            )
            .unwrap()
            .unwrap();
        assert_eq!(result["error"]["message"], "invalid_arguments");
    }
    protocol.handle(json!({"jsonrpc":"2.0","method":"notifications/roots/list_changed","params":{"roots":[{"uri":"file:///C:/"}]}}),&mut tools).unwrap();
    assert_eq!(
        protocol
            .handle(request(10, "roots/list", json!({})), &mut tools)
            .unwrap()
            .unwrap()["error"]["message"],
        "method_not_found"
    );
    assert!(tools.candidate().unwrap().changed_files.is_empty());
}

#[tokio::test]
async fn actual_framing_refuses_partial_invalid_and_oversized_frames() {
    let (_parent, mut tools) = fixture();
    for (bytes, expected) in [
        (b"{}".to_vec(), "incomplete_frame"),
        (b"not-json\n".to_vec(), "invalid_json"),
        (vec![b'x'; MAX_FRAME_BYTES + 1], "invalid_frame"),
    ] {
        let mut reader = bytes.as_slice();
        let mut writer = Vec::new();
        assert_eq!(
            serve(&mut reader, &mut writer, &mut tools).await,
            Err(expected)
        );
        assert!(writer.is_empty());
    }
}

#[tokio::test]
async fn stdio_session_performs_a_correlated_create_and_exports_to_owner_only() {
    let (_parent, mut tools) = fixture();
    let messages = [
        initialize(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        request(
            2,
            "tools/call",
            json!({"name":"create_file","arguments":{"path":"src/utf8.txt","content":"bounded unicode: λ"}}),
        ),
    ];
    let input = messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    let mut reader = input.as_bytes();
    let mut writer = Vec::new();
    serve(&mut reader, &mut writer, &mut tools).await.unwrap();
    let replies: Vec<Value> = String::from_utf8(writer)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["result"]["isError"], false);
    assert!(!replies[1].to_string().contains("bounded unicode"));
    assert_eq!(
        tools.candidate().unwrap().changed_files[0].content,
        "bounded unicode: λ"
    );
}

#[tokio::test]
async fn maximally_escaped_admitted_text_still_fits_the_reply_frame() {
    let parent = TempDir::new().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(parent.path()).unwrap();
    let content = "\u{1}".repeat(MAX_FILE_BYTES);
    let mut tools = WorkerTools::create(
        parent.path(),
        Bootstrap {
            schema_version: 1,
            attempt_id: Uuid::new_v4(),
            grants: WritableGrant::READ_ONLY,
            owned_paths: vec![],
            read_scopes: vec![PathPattern::new("data.txt").unwrap()],
            forbidden_paths: vec![],
            files: vec![TextFile {
                path: "data.txt".into(),
                content: content.clone(),
            }],
        },
    )
    .unwrap();
    let messages = [
        initialize(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        request(
            2,
            "tools/call",
            json!({"name":"read_file","arguments":{"path":"data.txt"}}),
        ),
    ];
    let input = messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    let mut reader = input.as_bytes();
    let mut writer = Vec::new();
    serve(&mut reader, &mut writer, &mut tools).await.unwrap();
    let lines = String::from_utf8(writer).unwrap();
    let last = lines.lines().last().unwrap();
    assert!(last.len() < MAX_FRAME_BYTES);
    let reply: Value = serde_json::from_str(last).unwrap();
    let value: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(value["content"], content);
}

#[tokio::test(start_paused = true)]
async fn session_deadline_fences_buffered_calls_after_a_slow_reply() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::io::{AsyncWrite, AsyncWriteExt};
    struct SlowReply {
        delay: Pin<Box<tokio::time::Sleep>>,
        done: bool,
    }
    impl AsyncWrite for SlowReply {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if !self.done {
                std::task::ready!(self.delay.as_mut().poll(cx));
                self.done = true;
            }
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    let (_parent, mut tools) = fixture();
    let messages = [
        initialize(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        request(
            2,
            "tools/call",
            json!({"name":"create_file","arguments":{"path":"src/late.txt","content":"too late"}}),
        ),
    ];
    let input = messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    let (mut sender, mut reader) = tokio::io::duplex(4096);
    let producer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(299)).await;
        sender.write_all(input.as_bytes()).await.unwrap();
    });
    let mut writer = SlowReply {
        delay: Box::pin(tokio::time::sleep(Duration::from_secs(301))),
        done: false,
    };
    assert_eq!(
        serve(&mut reader, &mut writer, &mut tools).await,
        Err("output_timeout")
    );
    producer.await.unwrap();
    assert!(tools.candidate().unwrap().changed_files.is_empty());
}
