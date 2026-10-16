//! Cancellation-safe newline framing with a limit checked before allocation.

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::config::{MAX_OUTPUT_BYTES, OUTPUT_DEADLINE};

pub struct LineReader<R> {
    reader: R,
    bytes: Vec<u8>,
    limit: usize,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    pub fn new(reader: R, limit: usize) -> Self {
        Self {
            reader,
            bytes: Vec::new(),
            limit,
        }
    }

    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, &'static str> {
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .map_err(|_| "input_unavailable")?;
            if available.is_empty() {
                return if self.bytes.is_empty() {
                    Ok(None)
                } else {
                    Err("incomplete_frame")
                };
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(available.len());
            if count > self.limit.saturating_sub(self.bytes.len()) {
                return Err("frame_too_large");
            }
            self.bytes.extend_from_slice(&available[..count]);
            self.reader.consume(count + usize::from(newline.is_some()));
            if newline.is_some() {
                return Ok(Some(std::mem::take(&mut self.bytes)));
            }
        }
    }
}

pub async fn write_json<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), &'static str> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| "output_invalid")?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err("output_too_large");
    }
    bytes.push(b'\n');
    timeout(OUTPUT_DEADLINE, async {
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| "output_timeout")?
    .map_err(|_| "output_unavailable")
}
