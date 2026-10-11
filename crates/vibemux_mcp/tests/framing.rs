use proptest::prelude::*;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use vibemux_mcp::{
    config::{HostConfig, LaunchMode, MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, parse_arguments},
    framing::{LineReader, write_json},
};

#[derive(Default)]
struct Output(Vec<u8>);
impl AsyncWrite for Output {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().0.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

proptest! {
    #[test]
    fn framing_obeys_the_payload_bound_independent_of_read_chunking(
        payload in prop::collection::vec(any::<u8>().prop_filter("not a newline", |byte| *byte != b'\n'), 0..200),
        limit in 0usize..128,
        chunk in 1usize..33,
    ) {
        let mut framed = payload.clone(); framed.push(b'\n');
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = runtime.block_on(async {
            LineReader::new(BufReader::with_capacity(chunk, framed.as_slice()), limit).next().await
        });
        if payload.len() <= limit { prop_assert_eq!(result, Ok(Some(payload))); }
        else { prop_assert_eq!(result, Err("frame_too_large")); }
    }
}

#[tokio::test]
async fn cancelled_line_read_retains_its_partial_frame() {
    let (mut writer, stream) = tokio::io::duplex(64);
    let mut reader = LineReader::new(BufReader::with_capacity(3, stream), 32);
    writer.write_all(b"{\"value\"").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), reader.next())
            .await
            .is_err()
    );
    writer.write_all(b":1}\n").await.unwrap();
    assert_eq!(
        reader.next().await.unwrap(),
        Some(b"{\"value\":1}".to_vec())
    );
    writer.shutdown().await.unwrap();
    assert_eq!(reader.next().await.unwrap(), None);
}

#[tokio::test]
async fn input_and_output_limits_exclude_exactly_one_newline_delimiter() {
    let mut bytes = vec![b'x'; MAX_INPUT_BYTES];
    bytes.push(b'\n');
    let read = LineReader::new(BufReader::new(bytes.as_slice()), MAX_INPUT_BYTES)
        .next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.len(), MAX_INPUT_BYTES);
    let mut output = Output::default();
    write_json(&mut output, &json!("x".repeat(MAX_OUTPUT_BYTES - 2)))
        .await
        .unwrap();
    assert_eq!(output.0.len(), MAX_OUTPUT_BYTES + 1);
    assert_eq!(output.0.last(), Some(&b'\n'));
    let mut rejected = Output::default();
    assert_eq!(
        write_json(&mut rejected, &json!("x".repeat(MAX_OUTPUT_BYTES - 1))).await,
        Err("output_too_large")
    );
    assert!(
        rejected.0.is_empty(),
        "oversized output must be rejected before writing"
    );
}

#[tokio::test]
async fn malformed_json_utf8_and_batches_never_become_tool_calls() {
    let input = b"\xff\n{broken\n[]\n";
    let mut output = Output::default();
    vibemux_mcp::session::run(
        BufReader::new(input.as_slice()),
        &mut output,
        HostConfig {
            descriptor: "never_opened".into(),
            allow_metadata_read: true,
        },
    )
    .await
    .unwrap();
    let replies: Vec<Value> = output
        .0
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["error"]["code"], -32700);
    assert_eq!(replies[2]["error"]["code"], -32600);
    assert!(replies.iter().all(|reply| reply["id"].is_null()));
}

#[test]
fn launch_arguments_are_strict_and_descriptor_path_is_bounded() {
    for arguments in [
        vec![],
        vec!["--descriptor"],
        vec![
            "--descriptor",
            "missing",
            "--allow_metadata_read",
            "--allow_metadata_read",
        ],
        vec!["--descriptor", "missing", "--executable", "other"],
        vec!["--ipc_reader", "extra"],
        vec!["--help", "extra"],
    ] {
        assert!(parse_arguments(arguments.into_iter().map(OsString::from)).is_err());
    }
    assert!(matches!(
        parse_arguments([OsString::from("--help")]).unwrap(),
        LaunchMode::Help
    ));
    assert!(
        parse_arguments([
            OsString::from("--descriptor"),
            OsString::from("x".repeat(4097))
        ])
        .is_err(),
        "descriptor path must have an explicit bound"
    );
}
