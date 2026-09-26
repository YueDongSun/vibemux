//! Incremental, bounded JSON Lines framing over arbitrary byte chunks.
//!
//! The daemon reads stdout in chunks of any size and feeds them here; frames
//! come out whole regardless of where chunk boundaries fall, including inside
//! a multi-byte UTF-8 sequence or between CR and LF. Memory is bounded by the
//! frame limit: an overlong frame fails as soon as the pending bytes exceed
//! it, without waiting for the newline.

use std::num::NonZeroUsize;

use super::DispatchError;

/// Splits a byte stream into LF-terminated frames. A trailing CR is removed.
/// A frame (the bytes before LF, including any CR) must not exceed the limit;
/// empty frames and invalid UTF-8 fail explicitly. After the first failure the
/// framer is poisoned and keeps returning that failure.
#[derive(Debug)]
pub struct JsonLineFramer {
    pending: Vec<u8>,
    max_frame_bytes: usize,
    failure: Option<DispatchError>,
}

impl JsonLineFramer {
    #[must_use]
    pub fn new(max_frame_bytes: NonZeroUsize) -> Self {
        Self {
            pending: Vec::with_capacity(max_frame_bytes.get().min(4096)),
            max_frame_bytes: max_frame_bytes.get(),
            failure: None,
        }
    }

    /// Consumes `chunk`, appending every completed frame to `frames` in order.
    /// On error, frames completed before the failing one are already in
    /// `frames`.
    pub fn push(&mut self, chunk: &[u8], frames: &mut Vec<String>) -> Result<(), DispatchError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let result = self.push_unpoisoned(chunk, frames);
        if let Err(failure) = result {
            self.failure = Some(failure);
            self.pending.clear();
        }
        result
    }

    /// Ends the stream. A non-empty final frame without LF is accepted.
    pub fn finish(&mut self) -> Result<Option<String>, DispatchError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        let frame = std::mem::take(&mut self.pending);
        let result = decode_frame(frame).map(Some);
        if let Err(failure) = result {
            self.failure = Some(failure);
        }
        result
    }

    /// Bytes of the current incomplete frame.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    fn push_unpoisoned(
        &mut self,
        mut chunk: &[u8],
        frames: &mut Vec<String>,
    ) -> Result<(), DispatchError> {
        while !chunk.is_empty() {
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let body = newline.map_or(chunk, |index| &chunk[..index]);
            if self.pending.len().saturating_add(body.len()) > self.max_frame_bytes {
                return Err(DispatchError::FrameTooLarge);
            }
            self.pending.extend_from_slice(body);
            let Some(index) = newline else {
                return Ok(());
            };
            chunk = &chunk[index + 1..];
            let frame = std::mem::replace(
                &mut self.pending,
                Vec::with_capacity(self.max_frame_bytes.min(4096)),
            );
            frames.push(decode_frame(frame)?);
        }
        Ok(())
    }
}

fn decode_frame(mut frame: Vec<u8>) -> Result<String, DispatchError> {
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    if frame.is_empty() {
        return Err(DispatchError::EmptyRecord);
    }
    String::from_utf8(frame).map_err(|_| DispatchError::InvalidUtf8)
}
