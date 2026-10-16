//! Private contained child role. Only the four existing read methods are reachable.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    time::timeout,
};
use vibemux_types::frontend::{FrontendTasksQuery, MAX_FRONTEND_TASKS};
use vibemuxd::control::{ControlClient, ControlError};

use crate::{
    config::{MAX_INPUT_BYTES, READ_DEADLINE, WORKER_IDLE_DEADLINE},
    framing::{LineReader, write_json},
    projection,
    tools::ReadOperation,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderRequest {
    pub generation: u64,
    pub sequence: u64,
    pub descriptor: PathBuf,
    pub operation: ReadOperation,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderReply {
    pub generation: u64,
    pub sequence: u64,
    pub result: Result<Value, ReaderError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderError {
    Unavailable,
    UnsupportedVersion,
    InvalidRequest,
    Deadline,
}

impl ReaderError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "producer_unavailable",
            Self::UnsupportedVersion => "control_version_unsupported",
            Self::InvalidRequest => "reader_request_invalid",
            Self::Deadline => "producer_timeout",
        }
    }
}

fn safe_control_error(error: ControlError) -> ReaderError {
    match error {
        ControlError::UnsupportedVersion => ReaderError::UnsupportedVersion,
        ControlError::Deadline => ReaderError::Deadline,
        _ => ReaderError::Unavailable,
    }
}

async fn read_metadata(request: &ReaderRequest) -> Result<Value, ReaderError> {
    match &request.operation {
        ReadOperation::FrontendTasks { limit, .. } if !(1..=MAX_FRONTEND_TASKS).contains(limit) => {
            return Err(ReaderError::InvalidRequest);
        }
        ReadOperation::HarnessDispatchStatus { request_id } if request_id.is_nil() => {
            return Err(ReaderError::InvalidRequest);
        }
        _ => {}
    }
    // Descriptor I/O is confined to this contained process after a validated call.
    let client = ControlClient::from_descriptor(&request.descriptor).map_err(safe_control_error)?;
    match request.operation {
        ReadOperation::Health => client.health().await.map(projection::health),
        ReadOperation::FrontendTasks { after, limit } => client
            .frontend_tasks(FrontendTasksQuery { after, limit })
            .await
            .map(|page| projection::tasks(page, limit)),
        ReadOperation::FrontendTask { task_id } => client
            .frontend_task(task_id)
            .await
            .map(projection::task_detail),
        ReadOperation::HarnessDispatchStatus { request_id } => client
            .harness_dispatch_status(request_id)
            .await
            .map(projection::dispatch),
    }
    .map_err(safe_control_error)
}

pub async fn run<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: R,
    mut writer: W,
) -> Result<(), &'static str> {
    let mut reader = LineReader::new(reader, MAX_INPUT_BYTES);
    let mut generation = None;
    let mut last_sequence = 0_u64;
    loop {
        let Some(bytes) = timeout(WORKER_IDLE_DEADLINE, reader.next())
            .await
            .map_err(|_| "reader_idle_timeout")??
        else {
            return Ok(());
        };
        let request: ReaderRequest =
            serde_json::from_slice(&bytes).map_err(|_| "reader_request_invalid")?;
        if request.generation == 0
            || generation.is_some_and(|value| value != request.generation)
            || last_sequence.checked_add(1) != Some(request.sequence)
        {
            return Err("reader_request_invalid");
        }
        generation = Some(request.generation);
        last_sequence = request.sequence;
        let result = timeout(READ_DEADLINE, read_metadata(&request))
            .await
            .unwrap_or(Err(ReaderError::Deadline));
        let reply = ReaderReply {
            generation: request.generation,
            sequence: request.sequence,
            result,
        };
        let value = serde_json::to_value(reply).map_err(|_| "reader_output_invalid")?;
        // A private reply has a tighter ceiling than the outer MCP envelope,
        // which includes both structuredContent and its JSON text representation.
        if value.to_string().len() > MAX_INPUT_BYTES {
            return Err("reader_output_too_large");
        }
        write_json(&mut writer, &value).await?;
    }
}
