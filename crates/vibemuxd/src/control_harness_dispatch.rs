//! Control v5 harness request dispatch operations (ADR 029 §7).
//!
//! Every argument is a strict JSON document; a malformed one is
//! `control_invalid_request`, and a service refusal passes its
//! `harness_dispatch_*`, `writer_*`, or `store_*` code through. Status views
//! are allowlisted: they carry no digest that identifies the prompt, the
//! config, or the project root. Raw vendor records leave the daemon only
//! through the output operation, split into pages that fit the frame.
use time::OffsetDateTime;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchPhase, DispatchRequest, NativeProtocol,
        capture_budget::CaptureSummary, events::ProcessSummary,
        route_config::MAX_SHUTDOWN_GRACE_MS,
    },
};
use vibemux_store::HarnessDispatchRecord;
use vibemux_types::RunId;

use super::*;
use crate::harness_dispatch::{
    DispatchReceipt, DispatchServiceError, LiveCapture, OutputCursor, OutputPageLimits,
    PROBE_DEADLINE_MS,
};

/// Client deadline of a probe: the capped probe deadline, the cancel grace,
/// and time to kill and reap the tree.
pub const HARNESS_DISPATCH_PROBE_DEADLINE: Duration =
    Duration::from_millis(PROBE_DEADLINE_MS + MAX_SHUTDOWN_GRACE_MS + 20_000);
/// Bound on every argument except a submit's, which the frame bounds.
const MAX_LOOKUP_ARGUMENT_BYTES: usize = 1024;
/// Headroom for the response envelope around a payload.
const RESPONSE_ENVELOPE_BYTES: usize = 1024;
/// Encoded budget of one output page, below the payload bound.
const OUTPUT_PAGE_ENCODED_BYTES: usize = MAX_CONTROL_FRAME_BYTES - 2 * RESPONSE_ENVELOPE_BYTES;

/// Argument of the probe operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchTarget {
    pub harness: AgentKind,
}

/// Argument of the status and cancel operations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchLookup {
    pub request_id: Uuid,
}

/// Argument of the output operation. `cursor` is the previous page's `next`,
/// or zero for the first page.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchOutputQuery {
    pub request_id: Uuid,
    pub cursor: OutputCursor,
}

/// Answer to a submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchReceipt {
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub phase: DispatchPhase,
    /// The request id was already admitted with the same content; nothing
    /// new was written or launched.
    pub duplicate: bool,
    /// Sequence of the event that last changed the attempt.
    pub sequence: u64,
}

impl From<DispatchReceipt> for HarnessDispatchReceipt {
    fn from(receipt: DispatchReceipt) -> Self {
        let record = receipt.record;
        Self {
            request_id: record.request_id,
            task_id: record.task_id,
            run_id: record.run_id,
            harness: record.harness,
            protocol: record.protocol,
            phase: record.phase,
            duplicate: receipt.duplicate,
            sequence: receipt.sequence,
        }
    }
}

/// Allowlisted view of a dispatch attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDispatchStatus {
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub phase: DispatchPhase,
    pub version: u64,
    pub outcome: Option<AttemptOutcome>,
    pub error_code: Option<DispatchError>,
    pub process: Option<ProcessSummary>,
    /// Final capture summary, once the attempt finished.
    pub capture: Option<CaptureSummary>,
    /// Size of the capture so far, while the attempt runs in this daemon.
    pub live_capture: Option<LiveCapture>,
    pub prompt_bytes: u64,
    /// Project `HEAD` observed at admission.
    pub base_commit: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl HarnessDispatchStatus {
    fn new(record: HarnessDispatchRecord, live_capture: Option<LiveCapture>) -> Self {
        Self {
            request_id: record.request_id,
            task_id: record.task_id,
            run_id: record.run_id,
            harness: record.harness,
            protocol: record.protocol,
            phase: record.phase,
            version: record.version,
            outcome: record.outcome,
            error_code: record.error_code,
            process: record.process,
            capture: record.capture,
            live_capture,
            prompt_bytes: record.prompt.byte_count,
            base_commit: record.base_commit,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

impl ControlClient {
    /// Content-free projection of the daemon's pinned routes.
    pub async fn harness_dispatch_catalog(
        &self,
    ) -> Result<Vec<DispatchCatalogEntry>, ControlError> {
        self.require_operation(ControlOperation::HarnessDispatchCatalog)?;
        match self
            .request(ControlOperation::HarnessDispatchCatalog, CONTROL_DEADLINE)
            .await?
        {
            ControlPayload::HarnessDispatchCatalog(routes) => Ok(routes),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// Initialize-only handshake; waits up to
    /// [`HARNESS_DISPATCH_PROBE_DEADLINE`].
    pub async fn harness_dispatch_probe(
        &self,
        harness: AgentKind,
    ) -> Result<ProbeReport, ControlError> {
        match self
            .dispatch_request(
                ControlOperation::HarnessDispatchProbe,
                &HarnessDispatchTarget { harness },
                HARNESS_DISPATCH_PROBE_DEADLINE,
            )
            .await?
        {
            ControlPayload::HarnessDispatchProbe(report) => Ok(report),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// Admits a request; returns once the attempt is admitted, not finished.
    /// A prompt whose encoded request exceeds the frame is
    /// `control_frame_too_large`.
    pub async fn harness_dispatch_submit(
        &self,
        request: &DispatchRequest,
    ) -> Result<HarnessDispatchReceipt, ControlError> {
        match self
            .dispatch_request(
                ControlOperation::HarnessDispatchSubmit,
                request,
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::HarnessDispatchReceipt(receipt) => Ok(receipt),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn harness_dispatch_status(
        &self,
        request_id: Uuid,
    ) -> Result<HarnessDispatchStatus, ControlError> {
        self.dispatch_lookup(ControlOperation::HarnessDispatchStatus, request_id)
            .await
    }

    /// One output page; pass the previous page's `next` as the cursor.
    pub async fn harness_dispatch_output(
        &self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, ControlError> {
        match self
            .dispatch_request(
                ControlOperation::HarnessDispatchOutput,
                &query,
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::HarnessDispatchOutput(page) => Ok(page),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// Commits the cancellation and returns the attempt's status.
    pub async fn harness_dispatch_cancel(
        &self,
        request_id: Uuid,
    ) -> Result<HarnessDispatchStatus, ControlError> {
        self.dispatch_lookup(ControlOperation::HarnessDispatchCancel, request_id)
            .await
    }

    async fn dispatch_lookup(
        &self,
        operation: ControlOperation,
        request_id: Uuid,
    ) -> Result<HarnessDispatchStatus, ControlError> {
        match self
            .dispatch_request(
                operation,
                &HarnessDispatchLookup { request_id },
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::HarnessDispatchStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    async fn dispatch_request(
        &self,
        operation: ControlOperation,
        value: &impl Serialize,
        deadline: Duration,
    ) -> Result<ControlPayload, ControlError> {
        self.require_operation(operation.clone())?;
        let encoded = serde_json::to_string(value).map_err(|_| ControlError::InvalidRequest)?;
        self.request_with_argument(operation, Some(encoded), deadline)
            .await
    }
}

pub(super) async fn dispatch(
    state: &ServerState,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let service = &state.dispatch;
    let result = match operation {
        ControlOperation::HarnessDispatchCatalog => {
            if argument.is_some() {
                return Err(ControlError::InvalidRequest);
            }
            service
                .catalog()
                .map(ControlPayload::HarnessDispatchCatalog)
        }
        ControlOperation::HarnessDispatchProbe => {
            let target: HarnessDispatchTarget = parse_lookup(argument)?;
            service
                .probe(target.harness)
                .await
                .map(ControlPayload::HarnessDispatchProbe)
        }
        ControlOperation::HarnessDispatchSubmit => {
            // The frame already bounds the argument; the service bounds the
            // prompt.
            let argument = argument.ok_or(ControlError::InvalidRequest)?;
            let request: DispatchRequest =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            service
                .submit(request)
                .await
                .map(|receipt| ControlPayload::HarnessDispatchReceipt(receipt.into()))
        }
        ControlOperation::HarnessDispatchStatus => {
            let lookup: HarnessDispatchLookup = parse_lookup(argument)?;
            service
                .status(lookup.request_id)
                .await
                .map(|record| status_payload(service, record))
        }
        ControlOperation::HarnessDispatchOutput => {
            let query: HarnessDispatchOutputQuery = parse_lookup(argument)?;
            service
                .output(
                    query.request_id,
                    query.cursor,
                    OutputPageLimits::within(OUTPUT_PAGE_ENCODED_BYTES),
                )
                .await
                .map(ControlPayload::HarnessDispatchOutput)
        }
        ControlOperation::HarnessDispatchCancel => {
            let lookup: HarnessDispatchLookup = parse_lookup(argument)?;
            service
                .cancel(lookup.request_id)
                .await
                .map(|record| status_payload(service, record))
        }
        _ => return Err(ControlError::InvalidRequest),
    };
    let payload = result.map_err(|error: DispatchServiceError| ControlError::Remote {
        code: error.code().to_string(),
    })?;
    // Output pages are sized to fit; this catches any other oversized
    // payload with a typed error instead of a dropped frame.
    if serde_json::to_vec(&payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len()
        > MAX_CONTROL_FRAME_BYTES - RESPONSE_ENVELOPE_BYTES
    {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(payload)
}

fn parse_lookup<T: DeserializeOwned>(argument: Option<String>) -> Result<T, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > MAX_LOOKUP_ARGUMENT_BYTES {
        return Err(ControlError::InvalidRequest);
    }
    serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)
}

fn status_payload(
    service: &HarnessDispatchService,
    record: HarnessDispatchRecord,
) -> ControlPayload {
    let live_capture = service.live_capture(record.request_id);
    ControlPayload::HarnessDispatchStatus(Box::new(HarnessDispatchStatus::new(
        record,
        live_capture,
    )))
}

#[cfg(test)]
mod tests {
    use vibemux_harness::dispatch::{
        ObservedRecord, PromptDigest, Sha256Digest,
        capture_budget::KindCounts,
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
    };
    use vibemux_types::ProjectId;

    use super::*;
    use crate::harness_dispatch::{OutputReassembler, build_output_page};

    fn encoded_response(payload: ControlPayload) -> Vec<u8> {
        let response = ControlResponse::success("r".repeat(128), payload);
        serde_json::to_vec(&response).expect("encode response")
    }

    fn record(sequence: u64, text: &str) -> ObservedRecord {
        let raw = serde_json::json!({"type": "item.updated", "text": text}).to_string();
        ObservedRecord::parse(NativeProtocol::CodexExec, sequence, raw)
            .expect("record")
            .0
    }

    fn dispatch_record() -> HarnessDispatchRecord {
        let digest = |label: &str| Sha256Digest::of(label.as_bytes());
        HarnessDispatchRecord {
            schema_version: 1,
            request_id: Uuid::new_v4(),
            project_id: ProjectId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
            harness: AgentKind::Codex,
            protocol: NativeProtocol::CodexExec,
            fingerprint: digest("fingerprint"),
            resource_key: digest("resource_key"),
            prompt: PromptDigest {
                sha256: digest("prompt"),
                byte_count: 12,
            },
            config_sha256: digest("config"),
            base_commit: "a".repeat(64),
            phase: DispatchPhase::Completed,
            version: u64::MAX,
            outcome: Some(AttemptOutcome::Completed),
            error_code: Some(DispatchError::Internal),
            process: Some(ProcessSummary {
                exit_code: Some(i32::MIN),
                forced_termination: true,
                stderr_bytes: u64::MAX,
            }),
            capture: Some(CaptureSummary {
                record_count: u64::MAX,
                record_bytes: u64::MAX,
                kinds: KindCounts::default(),
                transcript_sha256: digest("transcript"),
            }),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_maximally_escaped_output_page_fits_the_frame_and_reassembles() {
        // Quotes and backslashes double in the record and again in the page.
        let heavy = "\"\\".repeat(24 * 1024);
        let records = [
            record(1, &heavy),
            record(2, &"\u{1}".repeat(8 * 1024)),
            record(3, &"é😀".repeat(4 * 1024)),
        ];
        let mut reassembler = OutputReassembler::starting_after(0);
        let mut rebuilt = Vec::new();
        for _ in 0..64 {
            let page = build_output_page(
                &records,
                true,
                reassembler.cursor(),
                OutputPageLimits::within(OUTPUT_PAGE_ENCODED_BYTES),
            )
            .expect("page");
            let encoded = encoded_response(ControlPayload::HarnessDispatchOutput(page));
            assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES, "{}", encoded.len());
            let decoded: ControlResponse = serde_json::from_slice(&encoded).expect("decode");
            let Some(ControlPayload::HarnessDispatchOutput(page)) = decoded.payload else {
                panic!("not an output page");
            };
            let complete = page.complete;
            rebuilt.extend(reassembler.accept(page).expect("consistent page"));
            if complete {
                break;
            }
        }
        assert_eq!(rebuilt.len(), records.len());
        for (rebuilt, original) in rebuilt.iter().zip(&records) {
            assert_eq!(rebuilt.raw_json, original.raw_json());
        }
    }

    #[test]
    fn a_maximal_status_fits_the_frame_and_omits_identifying_digests() {
        let record = dispatch_record();
        let status = HarnessDispatchStatus::new(
            record.clone(),
            Some(LiveCapture {
                record_count: u64::MAX,
                record_bytes: u64::MAX,
            }),
        );
        let encoded = encoded_response(ControlPayload::HarnessDispatchStatus(Box::new(status)));
        assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES);
        let text = String::from_utf8(encoded).expect("utf8");
        for hidden in ["fingerprint", "resource_key", "config_sha256", "project_id"] {
            assert!(!text.contains(hidden), "{hidden}");
        }
        for digest in [
            record.fingerprint,
            record.resource_key,
            record.prompt.sha256,
            record.config_sha256,
        ] {
            assert!(!text.contains(&digest.to_hex()));
        }
        assert!(!text.contains(&record.project_id.to_string()));
        assert!(text.contains("\"prompt_bytes\":12"));
    }

    #[test]
    fn a_maximal_ordinary_prompt_fits_the_request_frame() {
        for unit in ["a", "é", "😀", "plain text, with punctuation. "] {
            let prompt = unit.repeat(MAX_PROMPT_BYTES / unit.len());
            let request = DispatchRequest {
                schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
                request_id: Uuid::new_v4(),
                harness: AgentKind::Codex,
                prompt,
            };
            request.validate().expect("valid prompt");
            let control = ControlRequest {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: "r".repeat(128),
                token: "t".repeat(64),
                operation: ControlOperation::HarnessDispatchSubmit,
                argument: Some(serde_json::to_string(&request).expect("encode")),
            };
            let encoded = serde_json::to_vec(&control).expect("encode request");
            assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES, "{unit:?}");
            // The prompt never reaches `Debug`.
            assert!(format!("{control:?}").len() < 512);
        }
    }

    #[test]
    fn lookup_arguments_are_strict_and_bounded() {
        let request_id = Uuid::new_v4();
        let valid = serde_json::to_string(&HarnessDispatchLookup { request_id }).expect("json");
        let parsed: HarnessDispatchLookup = parse_lookup(Some(valid)).expect("valid lookup");
        assert_eq!(parsed.request_id, request_id);
        let unknown = format!(r#"{{"request_id":"{request_id}","extra":1}}"#);
        let oversized = format!(
            r#"{{"request_id":"{request_id}"{}}}"#,
            " ".repeat(MAX_LOOKUP_ARGUMENT_BYTES)
        );
        for argument in [None, Some(unknown), Some(oversized), Some("{}".to_string())] {
            assert_eq!(
                parse_lookup::<HarnessDispatchLookup>(argument),
                Err(ControlError::InvalidRequest)
            );
        }
    }
}
