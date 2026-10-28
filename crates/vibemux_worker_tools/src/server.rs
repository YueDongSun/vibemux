//! Bounded stdio transport. The process owner supplies the lifetime/cancellation fence.

use crate::{contract::MAX_FRAME_BYTES, protocol::Protocol, workspace::WorkerTools};
use std::{num::NonZeroUsize, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, timeout_at},
};
use vibemux_harness::dispatch::json_line_framer::JsonLineFramer;

pub(crate) const SESSION_DEADLINE: Duration = Duration::from_secs(300);
pub(crate) const OUTPUT_DEADLINE: Duration = Duration::from_secs(5);
const CHUNK_BYTES: usize = 4096;

pub async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    tools: &mut WorkerTools,
) -> Result<(), &'static str> {
    let limit = NonZeroUsize::new(MAX_FRAME_BYTES).ok_or("invalid_limit")?;
    let mut framer = JsonLineFramer::new(limit);
    let mut protocol = Protocol::default();
    let mut buffer = [0u8; CHUNK_BYTES];
    let deadline = Instant::now() + SESSION_DEADLINE;
    loop {
        let count = timeout_at(deadline, reader.read(&mut buffer))
            .await
            .map_err(|_| "input_timeout")?
            .map_err(|_| "input_unavailable")?;
        if count == 0 {
            return if framer.pending_bytes() == 0 {
                Ok(())
            } else {
                Err("incomplete_frame")
            };
        }
        let mut frames = Vec::new();
        framer
            .push(&buffer[..count], &mut frames)
            .map_err(|_| "invalid_frame")?;
        for frame in frames {
            if Instant::now() >= deadline {
                return Err("session_timeout");
            }
            let message = serde_json::from_str(&frame).map_err(|_| "invalid_json")?;
            if let Some(reply) = protocol.handle(message, tools)? {
                let mut bytes = serde_json::to_vec(&reply).map_err(|_| "invalid_output")?;
                if bytes.len() > MAX_FRAME_BYTES {
                    return Err("output_limit");
                }
                bytes.push(b'\n');
                timeout_at(deadline.min(Instant::now() + OUTPUT_DEADLINE), async {
                    writer.write_all(&bytes).await?;
                    writer.flush().await
                })
                .await
                .map_err(|_| "output_timeout")?
                .map_err(|_| "output_unavailable")?;
            }
        }
    }
}
