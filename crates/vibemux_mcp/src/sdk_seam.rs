//! ADR 075 Scope A seam: official Rust MCP SDK (`rmcp`) types plus the
//! VibeMux-owned transport, framing and tool-mapping helpers the future
//! SDK-based session composes.
//!
//! Inert unless the `rmcp` feature is on. The hand-written session remains
//! the shipping path; nothing here changes protocol behavior. The seam keeps
//! ADR 075's ownership split: VibeMux owns framing limits, raw parse, error
//! synthesis and the fatal exit; the SDK owns typed messages and future
//! protocol surface.

use std::{fmt, sync::Arc};

use rmcp::model::Tool;
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    sync::mpsc,
};

use crate::{
    config::MAX_OUTPUT_BYTES,
    framing::{LineReader, write_json},
};

/// Capacity of both seam channels. Bounded on purpose: a slow consumer
/// applies backpressure instead of accumulating memory (AGENTS §7.4).
pub const SEAM_CHANNEL_CAPACITY: usize = 16;

/// A fatal fault the session driver must surface as a nonzero exit with the
/// fixed stderr code and no protocol bytes on stdout.
#[derive(Clone, Debug, Default)]
pub struct FatalStatus(Arc<std::sync::Mutex<Option<&'static str>>>);

impl FatalStatus {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, code: &'static str) {
        let mut slot = self.0.lock().expect("fatal status lock");
        if slot.is_none() {
            *slot = Some(code);
        }
    }

    #[must_use]
    pub fn get(&self) -> Option<&'static str> {
        *self.0.lock().expect("fatal status lock")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeamError {
    OutputTooLarge,
    OutputInvalid,
    OutputFailed,
    TransportClosed,
}

impl fmt::Display for SeamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OutputTooLarge => "output_too_large",
            Self::OutputInvalid => "output_invalid",
            Self::OutputFailed => "output_unavailable",
            Self::TransportClosed => "transport_closed",
        })
    }
}

impl std::error::Error for SeamError {}

/// Why one inbound line could not become a typed message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFault {
    /// The line exceeded the 64 KiB bound before allocation completed.
    TooLarge,
    /// EOF arrived mid-line.
    Incomplete,
    /// The reader failed at the byte level.
    Unavailable,
    /// The line was not a valid JSON-RPC message (the driver answers
    /// `-32700 parse_error`; shape errors are a separate first-party
    /// classification).
    Malformed,
}

/// A bounded, in-process transport: the future session driver parses raw
/// lines first-party, synthesizes protocol errors itself, and hands only
/// typed messages to the SDK through these channels.
pub struct ChannelTransport {
    inbound: mpsc::Receiver<RxJsonRpcMessage<RoleServer>>,
    outbound: mpsc::Sender<TxJsonRpcMessage<RoleServer>>,
    closed: bool,
}

/// The driver-side endpoints of [`channel_transport`].
pub struct ChannelPeer {
    pub inbound: mpsc::Sender<RxJsonRpcMessage<RoleServer>>,
    pub outbound: mpsc::Receiver<TxJsonRpcMessage<RoleServer>>,
}

/// Creates the two endpoints of one bounded seam.
#[must_use]
pub fn channel_transport() -> (ChannelTransport, ChannelPeer) {
    let (inbound_tx, inbound_rx) = mpsc::channel(SEAM_CHANNEL_CAPACITY);
    let (outbound_tx, outbound_rx) = mpsc::channel(SEAM_CHANNEL_CAPACITY);
    (
        ChannelTransport {
            inbound: inbound_rx,
            outbound: outbound_tx,
            closed: false,
        },
        ChannelPeer {
            inbound: inbound_tx,
            outbound: outbound_rx,
        },
    )
}

impl Transport<RoleServer> for ChannelTransport {
    type Error = SeamError;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let outbound = self.outbound.clone();
        async move {
            outbound
                .send(item)
                .await
                .map_err(|_| SeamError::TransportClosed)
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        if self.closed {
            return None;
        }
        self.inbound.recv().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.closed = true;
        self.inbound.close();
        Ok(())
    }
}

/// Reads one typed message from the bounded framing. EOF is `Ok(None)`; a
/// fault names the first-party error the driver must synthesize.
pub async fn read_typed<R: AsyncBufRead + Unpin>(
    reader: &mut LineReader<R>,
) -> Result<Option<RxJsonRpcMessage<RoleServer>>, ReadFault> {
    match reader.next().await {
        Ok(None) => Ok(None),
        Ok(Some(line)) => serde_json::from_slice(&line)
            .map(Some)
            .map_err(|_| ReadFault::Malformed),
        Err("frame_too_large") => Err(ReadFault::TooLarge),
        Err("incomplete_frame") => Err(ReadFault::Incomplete),
        Err(_) => Err(ReadFault::Unavailable),
    }
}

/// Writes one typed message through the crate-owned framing: the 256 KiB
/// bound is checked before any byte reaches the writer and the 2 s write
/// deadline applies inside `write_json`.
pub async fn write_typed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &TxJsonRpcMessage<RoleServer>,
) -> Result<(), SeamError> {
    let value = serde_json::to_value(message).map_err(|_| SeamError::OutputInvalid)?;
    if serde_json::to_vec(&value)
        .map_err(|_| SeamError::OutputInvalid)?
        .len()
        > MAX_OUTPUT_BYTES
    {
        return Err(SeamError::OutputTooLarge);
    }
    write_json(writer, &value).await.map_err(|code| match code {
        "output_too_large" => SeamError::OutputTooLarge,
        "output_invalid" => SeamError::OutputInvalid,
        _ => SeamError::OutputFailed,
    })
}

/// Maps the hand-written `tools/list` contract onto SDK `Tool` values. The
/// mapping is a parse of the exact first-party JSON, so parity is by
/// construction and pinned by tests.
pub fn tools_as_rmcp(metadata_available: bool) -> Result<Vec<Tool>, &'static str> {
    let listed = crate::tools::list_tools(metadata_available);
    listed["tools"]
        .as_array()
        .ok_or("tools_contract_invalid")?
        .iter()
        .map(|tool| serde_json::from_value(tool.clone()).map_err(|_| "tools_contract_invalid"))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;
    use crate::config::MAX_INPUT_BYTES;

    fn request_line(id: u64, method: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}}))
            .expect("request JSON")
    }

    fn value_of<T: serde::Serialize>(message: &T) -> Value {
        serde_json::to_value(message).expect("serialize message")
    }

    #[tokio::test]
    async fn the_channel_transport_carries_typed_messages_both_ways() {
        let (
            mut transport,
            ChannelPeer {
                inbound,
                mut outbound,
            },
        ) = channel_transport();
        let request: RxJsonRpcMessage<RoleServer> = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {}
        }))
        .expect("typed request");
        let expected_request = value_of(&request);
        inbound.send(request).await.expect("send inbound");
        let received = transport.receive().await.expect("inbound");
        assert_eq!(value_of(&received), expected_request);

        let response: TxJsonRpcMessage<RoleServer> = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 7, "result": {"tools": []}
        }))
        .expect("typed response");
        let expected_response = value_of(&response);
        transport.send(response).await.expect("send outbound");
        let sent = outbound.recv().await.expect("outbound");
        assert_eq!(value_of(&sent), expected_response);

        transport.close().await.expect("close");
        assert!(transport.receive().await.is_none());
    }

    #[tokio::test]
    async fn the_seam_channels_are_bounded() {
        let (_transport, peer) = channel_transport();
        for index in 0..SEAM_CHANNEL_CAPACITY {
            let message: RxJsonRpcMessage<RoleServer> = serde_json::from_value(json!({
                "jsonrpc": "2.0", "id": index, "method": "ping", "params": {}
            }))
            .expect("typed request");
            peer.inbound
                .try_send(message)
                .expect("capacity holds exactly the bound");
        }
        let overflow: RxJsonRpcMessage<RoleServer> = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 999u64, "method": "ping", "params": {}
        }))
        .expect("typed request");
        assert!(matches!(
            peer.inbound.try_send(overflow),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[tokio::test]
    async fn read_typed_classifies_eof_malformed_and_oversize_lines() {
        let (mut input, mut writer) = tokio::io::duplex(4 * MAX_INPUT_BYTES);
        writer
            .write_all(&request_line(1, "tools/list"))
            .await
            .expect("valid line");
        writer
            .write_all(b"\n{\"jsonrpc\":\"2.0\",")
            .await
            .expect("malformed");
        writer.write_all(b"\n").await.expect("newline");
        let mut oversized = vec![b'a'; MAX_INPUT_BYTES + 1];
        oversized.push(b'\n');
        writer.write_all(&oversized).await.expect("oversize line");
        writer.shutdown().await.expect("eof");

        let mut reader = LineReader::new(BufReader::new(&mut input), MAX_INPUT_BYTES);
        let first = read_typed(&mut reader).await.expect("valid").expect("some");
        assert_eq!(
            serde_json::to_value(first).expect("serialize"),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
        );
        assert!(matches!(
            read_typed(&mut reader).await,
            Err(ReadFault::Malformed)
        ));
        assert!(matches!(
            read_typed(&mut reader).await,
            Err(ReadFault::TooLarge)
        ));
    }

    #[tokio::test]
    async fn read_typed_reports_clean_eof() {
        let (mut input, writer) = tokio::io::duplex(64);
        drop(writer);
        let mut reader = LineReader::new(BufReader::new(&mut input), MAX_INPUT_BYTES);
        assert!(matches!(read_typed(&mut reader).await, Ok(None)));
    }

    #[tokio::test]
    async fn write_typed_round_trips_and_refuses_oversize_before_writing() {
        let (mut output, mut reader) = tokio::io::duplex(1024 * 1024);
        let response: TxJsonRpcMessage<RoleServer> = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 3, "result": {"tools": []}
        }))
        .expect("typed response");
        write_typed(&mut output, &response).await.expect("write");
        let mut line = String::new();
        BufReader::new(&mut reader)
            .read_line(&mut line)
            .await
            .expect("read line");
        let parsed: Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(
            parsed,
            json!({"jsonrpc": "2.0", "id": 3, "result": {"tools": []}})
        );

        let huge = json!({
            "jsonrpc": "2.0", "method": "notifications/message",
            "params": {"level": "info", "data": "x".repeat(MAX_OUTPUT_BYTES)}
        });
        let oversized: TxJsonRpcMessage<RoleServer> =
            serde_json::from_value(huge).expect("typed notification");
        assert_eq!(
            write_typed(&mut output, &oversized).await,
            Err(SeamError::OutputTooLarge)
        );
    }

    #[test]
    fn the_tool_mapping_is_value_identical_to_the_hand_written_contract() {
        for granted in [true, false] {
            let listed = crate::tools::list_tools(granted);
            let expected = listed["tools"].as_array().expect("tools").clone();
            let mapped = tools_as_rmcp(granted).expect("mapping");
            assert_eq!(mapped.len(), expected.len());
            for (tool, original) in mapped.iter().zip(expected.iter()) {
                assert_eq!(&serde_json::to_value(tool).expect("serialize"), original);
            }
        }
        let names: Vec<String> = tools_as_rmcp(true)
            .expect("mapping")
            .into_iter()
            .map(|tool| tool.name.into_owned())
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
        assert_eq!(tools_as_rmcp(false).expect("denied mapping").len(), 1);
    }

    #[test]
    fn a_fatal_status_keeps_its_first_code() {
        let status = FatalStatus::new();
        assert_eq!(status.get(), None);
        status.set("frame_too_large");
        status.set("parse_error");
        assert_eq!(status.get(), Some("frame_too_large"));
    }
}
