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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use uuid::Uuid;
    use vibemux_types::TaskId;

    use super::*;

    const ID: Uuid = Uuid::from_u128(0x5f0c_6a52_8f3b_4c1e_9a7d_2b1e_3c4d_5e6f);

    fn request(
        generation: u64,
        sequence: u64,
        operation: ReadOperation,
        descriptor: &Path,
    ) -> ReaderRequest {
        ReaderRequest {
            generation,
            sequence,
            descriptor: descriptor.to_path_buf(),
            operation,
        }
    }

    fn page(limit: u32) -> ReadOperation {
        ReadOperation::FrontendTasks { after: None, limit }
    }

    async fn outcome(operation: ReadOperation, descriptor: &Path) -> Option<&'static str> {
        read_metadata(&request(1, 1, operation, descriptor))
            .await
            .err()
            .map(ReaderError::code)
    }

    /// A request the reader answers `reader_request_invalid` without
    /// reading its descriptor, so a run needs no daemon.
    fn probe(generation: u64, sequence: u64) -> Value {
        serde_json::to_value(request(
            generation,
            sequence,
            page(0),
            Path::new("control.json"),
        ))
        .expect("encode request")
    }

    /// The reader's result over `lines`, and the replies it wrote.
    async fn run_lines(lines: &[Value]) -> (Result<(), &'static str>, Vec<ReaderReply>) {
        let mut input = Vec::new();
        for line in lines {
            input.extend(serde_json::to_vec(line).expect("encode line"));
            input.push(b'\n');
        }
        let mut output = Vec::new();
        let result = run(input.as_slice(), &mut output).await;
        let replies = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("reply"))
            .collect();
        (result, replies)
    }

    #[tokio::test]
    async fn invalid_operations_are_refused_before_the_descriptor_is_read() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        // Nothing exists here, so any read that reaches the descriptor is
        // `producer_unavailable` rather than `reader_request_invalid`.
        let descriptor = temporary.path().join("control.json");
        for refused in [
            page(0),
            page(MAX_FRONTEND_TASKS + 1),
            ReadOperation::HarnessDispatchStatus {
                request_id: Uuid::nil(),
            },
        ] {
            assert_eq!(
                outcome(refused, &descriptor).await,
                Some("reader_request_invalid")
            );
        }
        for admitted in [
            ReadOperation::Health,
            page(1),
            page(MAX_FRONTEND_TASKS),
            ReadOperation::FrontendTask {
                task_id: TaskId::from_uuid(ID).expect("task id"),
            },
            ReadOperation::HarnessDispatchStatus { request_id: ID },
        ] {
            assert_eq!(
                outcome(admitted, &descriptor).await,
                Some("producer_unavailable")
            );
        }
    }

    #[test]
    fn a_request_with_a_nil_task_id_or_an_extra_key_does_not_decode() {
        let task_id = TaskId::from_uuid(ID).expect("task id");
        let detail = serde_json::to_value(request(
            1,
            1,
            ReadOperation::FrontendTask { task_id },
            Path::new("control.json"),
        ))
        .expect("encode request");
        let after = serde_json::to_value(request(
            1,
            1,
            ReadOperation::FrontendTasks {
                after: Some(task_id),
                limit: 1,
            },
            Path::new("control.json"),
        ))
        .expect("encode request");
        let decodes =
            |value: &Value| serde_json::from_value::<ReaderRequest>(value.clone()).is_ok();
        assert!(decodes(&detail) && decodes(&after));
        let nil = json!(Uuid::nil().to_string());
        for (mut value, field) in [(detail.clone(), "task_id"), (after, "after")] {
            value["operation"][field] = nil.clone();
            assert!(!decodes(&value), "nil {field}");
        }
        let mut extra = detail.clone();
        extra["operation"]["descriptor"] = json!("other.json");
        assert!(!decodes(&extra));
        let mut extra = detail;
        extra["deadline_ms"] = json!(1);
        assert!(!decodes(&extra));
    }

    #[tokio::test]
    async fn each_request_is_answered_in_order_until_the_input_ends() {
        let (result, replies) = run_lines(&[probe(7, 1), probe(7, 2), probe(7, 3)]).await;
        assert_eq!(result, Ok(()));
        let order: Vec<(u64, u64)> = replies
            .iter()
            .map(|reply| (reply.generation, reply.sequence))
            .collect();
        assert_eq!(order, [(7, 1), (7, 2), (7, 3)]);
        assert!(
            replies
                .iter()
                .all(|reply| matches!(reply.result, Err(ReaderError::InvalidRequest)))
        );
        let (result, replies) = run_lines(&[]).await;
        assert_eq!(result, Ok(()));
        assert!(replies.is_empty());
    }

    #[tokio::test]
    async fn a_request_out_of_generation_or_sequence_stops_the_reader() {
        for (lines, answered) in [
            (vec![probe(0, 1)], 0),
            (vec![probe(7, 2)], 0),
            (vec![probe(7, 1), probe(7, 1)], 1),
            (vec![probe(7, 1), probe(7, 3)], 1),
            (vec![probe(7, 1), probe(8, 2)], 1),
            (
                vec![probe(7, 1), json!({"generation": 7, "sequence": 2})],
                1,
            ),
        ] {
            let (result, replies) = run_lines(&lines).await;
            assert_eq!(result, Err("reader_request_invalid"), "{lines:?}");
            assert_eq!(replies.len(), answered, "{lines:?}");
        }
    }
}
