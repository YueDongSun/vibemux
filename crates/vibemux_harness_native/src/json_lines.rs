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

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;

    use super::{Reader, replay};
    use crate::{config::Limits, contract::Protocol};

    async fn read_one(input: &[u8], maximum_bytes: usize) -> Result<Option<String>, &'static str> {
        let mut reader =
            Reader::new(BufReader::new(input), maximum_bytes).map_err(|error| error.code())?;
        reader.next().await.map_err(|error| error.code())
    }

    async fn replayed(stream: &str, limits: &Limits) -> Result<Vec<(u64, String)>, &'static str> {
        let records = replay(
            &mut BufReader::new(stream.as_bytes()),
            Protocol::Acp,
            limits,
        )
        .await
        .map_err(|error| error.code())?;
        Ok(records
            .iter()
            .map(|record| (record.sequence, record.raw_json().to_string()))
            .collect())
    }

    /// The frame bound counts the whole line, its line ending included, so
    /// the same JSON fits a bound with LF that it overruns with CRLF.
    #[tokio::test]
    async fn the_frame_bound_counts_the_line_ending() {
        let json = "{\"value\":1}";
        let read = Ok(Some(json.to_string()));
        let bound = json.len() + 1;
        assert_eq!(read_one(format!("{json}\n").as_bytes(), bound).await, read);
        assert_eq!(
            read_one(format!("{json}\r\n").as_bytes(), bound).await,
            Err("harness_frame_too_large")
        );
        assert_eq!(
            read_one(format!("{json}\r\n").as_bytes(), bound + 1).await,
            read
        );
        // A final line without a line ending may fill the whole bound.
        assert_eq!(read_one(json.as_bytes(), json.len()).await, read);
        assert_eq!(
            read_one(json.as_bytes(), json.len() - 1).await,
            Err("harness_frame_too_large")
        );
        assert_eq!(read_one(b"\r\n", 8).await, Err("harness_empty_record"));
        assert_eq!(read_one(b"", 8).await, Ok(None));
        assert_eq!(read_one(b"{}\n", 0).await, Err("harness_invalid_limits"));
        let guide = include_str!("../../../docs/guides/harness_core_guide.md");
        assert!(guide.contains("including its line ending"));
    }

    /// `capture_bytes` counts each record's JSON text without its line
    /// ending. Reaching it or `record_count` is allowed; passing either
    /// fails the whole replay instead of truncating it.
    #[tokio::test]
    async fn replay_bounds_capture_and_record_count_exactly() {
        let record = format!("{{\"value\":\"{}\"}}", "a".repeat(52));
        assert_eq!(record.len(), 64);
        let stream = format!("{record}\r\n{record}\n{record}");
        let limits = |capture_bytes, record_count| Limits {
            frame_bytes: 128,
            capture_bytes,
            record_count,
            ..Limits::default()
        };
        let all = (1..=3).map(|sequence| (sequence, record.clone())).collect();
        assert_eq!(replayed(&stream, &limits(3 * 64, 3)).await, Ok(all));
        for (capture_bytes, record_count) in [(3 * 64 - 1, 3), (3 * 64, 2)] {
            assert_eq!(
                replayed(&stream, &limits(capture_bytes, record_count)).await,
                Err("harness_capture_too_large"),
                "{capture_bytes} {record_count}"
            );
        }
        // Limits are validated before anything is read, even one that
        // replay does not otherwise use.
        let unused_invalid = Limits {
            shutdown_ms: 0,
            ..Limits::default()
        };
        assert_eq!(
            replayed(&stream, &unused_invalid).await,
            Err("harness_invalid_limits")
        );
    }
}
