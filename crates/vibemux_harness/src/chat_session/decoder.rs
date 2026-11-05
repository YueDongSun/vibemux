//! The existing pure JSONL framer with a bounded chunk/output batch contract.

use std::num::NonZeroUsize;

use crate::dispatch::{DispatchError, json_line_framer::JsonLineFramer};

use super::{SessionError, SessionLimits};

pub const MAX_INPUT_CHUNK_BYTES: usize = 64 * 1024;
const MAX_FRAMES_PER_CHUNK: usize = 256;

pub struct SessionDecoder {
    framer: JsonLineFramer,
    failed: bool,
}

impl SessionDecoder {
    pub fn new(limits: &SessionLimits) -> Result<Self, SessionError> {
        limits.validate()?;
        let limit =
            NonZeroUsize::new(limits.max_frame_bytes).ok_or(SessionError::InvalidConfiguration)?;
        Ok(Self {
            framer: JsonLineFramer::new(limit),
            failed: false,
        })
    }

    /// Assign per-process read ordinals to returned frames before `receive`.
    /// A framing error invalidates the batch and fences this decoder.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, SessionError> {
        if self.failed {
            return Err(SessionError::InvalidFrame);
        }
        if chunk.len() > MAX_INPUT_CHUNK_BYTES {
            self.failed = true;
            return Err(SessionError::FrameTooLarge);
        }
        let mut frames = Vec::new();
        if let Err(error) = self.framer.push(chunk, &mut frames) {
            self.failed = true;
            return Err(framing_error(error));
        }
        if frames.len() > MAX_FRAMES_PER_CHUNK {
            self.failed = true;
            return Err(SessionError::RecordBudgetExhausted);
        }
        Ok(frames)
    }

    /// EOF is a transport fact, never turn completion. The caller feeds a
    /// returned final frame, then invokes the machine's `transport_lost`.
    pub fn finish(&mut self) -> Result<Option<String>, SessionError> {
        if self.failed {
            return Err(SessionError::InvalidFrame);
        }
        self.framer.finish().map_err(|error| {
            self.failed = true;
            framing_error(error)
        })
    }
}

fn framing_error(error: DispatchError) -> SessionError {
    if error == DispatchError::FrameTooLarge {
        SessionError::FrameTooLarge
    } else {
        SessionError::InvalidFrame
    }
}
