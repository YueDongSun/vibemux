use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::{
    Error,
    config::Limits,
    contract::{Protocol, Record},
};

/// Partial frames remain owned by the reader when a select branch is cancelled.
pub struct Reader<R> {
    input: R,
    pending: Vec<u8>,
    maximum_bytes: usize,
}

impl<R: AsyncBufRead + Unpin> Reader<R> {
    pub fn new(input: R, maximum_bytes: usize) -> Result<Self, Error> {
        if maximum_bytes == 0 {
            return Err(Error::new("harness_invalid_limits"));
        }
        Ok(Self {
            input,
            pending: Vec::with_capacity(maximum_bytes.min(4096)),
            maximum_bytes,
        })
    }

    /// CRLF and a final line without LF are accepted; blank/invalid records fail explicitly.
    pub async fn next(&mut self) -> Result<Option<String>, Error> {
        loop {
            let available = self
                .input
                .fill_buf()
                .await
                .map_err(|source| Error::with_source("harness_read_failed", source))?;
            if available.is_empty() {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if self.pending.len().saturating_add(count) > self.maximum_bytes {
                return Err(Error::new("harness_frame_too_large"));
            }
            self.pending.extend_from_slice(&available[..count]);
            self.input.consume(count);
            if newline.is_some() {
                break;
            }
        }
        let mut line = std::mem::take(&mut self.pending);
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            return Err(Error::new("harness_empty_record"));
        }
        String::from_utf8(line)
            .map(Some)
            .map_err(|source| Error::with_source("harness_invalid_utf8", source))
    }
}

pub async fn replay<R: AsyncBufRead + Unpin>(
    input: &mut R,
    protocol: Protocol,
    limits: &Limits,
) -> Result<Vec<Record>, Error> {
    limits.validate()?;
    let mut reader = Reader::new(input, limits.frame_bytes)?;
    let mut records = Vec::new();
    let mut captured = 0_usize;
    while let Some(raw_json) = reader.next().await? {
        captured = captured
            .checked_add(raw_json.len())
            .ok_or_else(|| Error::new("harness_capture_too_large"))?;
        if captured > limits.capture_bytes || records.len() >= limits.record_count {
            return Err(Error::new("harness_capture_too_large"));
        }
        records.push(Record::parse(protocol, records.len() as u64 + 1, raw_json)?);
    }
    Ok(records)
}
