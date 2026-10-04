//! Control v6 DSH session operations (ADR 030 §9).
//!
//! Every argument is a strict JSON document; a malformed one is
//! `control_invalid_request`, and a service refusal passes its `dsh_session_*`
//! code through. Status views are allowlisted: they carry no path, no digest,
//! and no prompt text. `dsh_session_poll` is the only operation that returns
//! runtime content, and its page is sized to fit the control frame.

use serde::de::DeserializeOwned;

use super::*;
use crate::dsh_session::{DshServiceError, DshSessionService};

/// Bound on every DSH argument except a prompt, which the frame bounds.
const MAX_DSH_ARGUMENT_BYTES: usize = 1024;
/// Headroom for the response envelope around a payload.
const DSH_RESPONSE_ENVELOPE_BYTES: usize = 1024;

impl ControlClient {
    /// Opens a session. Refused with `dsh_session_busy` while one is live.
    pub async fn dsh_session_open(
        &self,
        request: &DshSessionOpenRequest,
    ) -> Result<DshSessionStatus, ControlError> {
        match self
            .dsh_request(
                ControlOperation::DshSessionOpen,
                request,
                DSH_SESSION_OPEN_DEADLINE,
            )
            .await?
        {
            ControlPayload::DshSessionStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// Sends one prompt. Refused while the session is not ready.
    pub async fn dsh_session_prompt(
        &self,
        request: &DshSessionPromptRequest,
    ) -> Result<DshSessionStatus, ControlError> {
        match self
            .dsh_request(
                ControlOperation::DshSessionPrompt,
                request,
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::DshSessionStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    /// One poll page; pass the previous page's `next` as the cursor.
    pub async fn dsh_session_poll(
        &self,
        query: DshSessionPollQuery,
    ) -> Result<DshSessionPollPage, ControlError> {
        match self
            .dsh_request(ControlOperation::DshSessionPoll, &query, CONTROL_DEADLINE)
            .await?
        {
            ControlPayload::DshSessionPoll(page) => Ok(page),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn dsh_session_status(&self) -> Result<DshSessionStatus, ControlError> {
        match self
            .dsh_request(
                ControlOperation::DshSessionStatus,
                &DshSessionPollQuery::default(),
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::DshSessionStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn dsh_session_cancel(&self) -> Result<DshSessionStatus, ControlError> {
        match self
            .dsh_request(
                ControlOperation::DshSessionCancel,
                &DshSessionPollQuery::default(),
                CONTROL_DEADLINE,
            )
            .await?
        {
            ControlPayload::DshSessionStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    pub async fn dsh_session_close(&self) -> Result<DshSessionStatus, ControlError> {
        match self
            .dsh_request(
                ControlOperation::DshSessionClose,
                &DshSessionPollQuery::default(),
                DSH_SESSION_CLOSE_DEADLINE,
            )
            .await?
        {
            ControlPayload::DshSessionStatus(status) => Ok(*status),
            _ => Err(ControlError::InvalidFrame),
        }
    }

    async fn dsh_request(
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
    service: &DshSessionService,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let result = match operation {
        ControlOperation::DshSessionOpen => {
            let request: DshSessionOpenRequest = parse_argument(argument)?;
            service
                .open(&request)
                .await
                .map(|status| ControlPayload::DshSessionStatus(Box::new(status)))
        }
        ControlOperation::DshSessionPrompt => {
            let argument = argument.ok_or(ControlError::InvalidRequest)?;
            let request: DshSessionPromptRequest =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            service
                .prompt(&request)
                .await
                .map(|status| ControlPayload::DshSessionStatus(Box::new(status)))
        }
        ControlOperation::DshSessionPoll => {
            let query: DshSessionPollQuery = parse_argument(argument)?;
            service
                .poll(query)
                .await
                .map(ControlPayload::DshSessionPoll)
        }
        // Status, cancel, and close take no argument beyond the empty object.
        ControlOperation::DshSessionStatus => service
            .status()
            .await
            .map(|status| ControlPayload::DshSessionStatus(Box::new(status))),
        ControlOperation::DshSessionCancel => service
            .cancel()
            .await
            .map(|status| ControlPayload::DshSessionStatus(Box::new(status))),
        ControlOperation::DshSessionClose => service
            .close()
            .await
            .map(|status| ControlPayload::DshSessionStatus(Box::new(status))),
        _ => return Err(ControlError::InvalidRequest),
    };
    let payload = result.map_err(|error: DshServiceError| ControlError::Remote {
        code: error.code().to_string(),
    })?;
    // Poll pages are sized to fit; this catches any other oversized payload
    // with a typed error instead of a dropped frame.
    if serde_json::to_vec(&payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len()
        > MAX_CONTROL_FRAME_BYTES - DSH_RESPONSE_ENVELOPE_BYTES
    {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(payload)
}

fn parse_argument<T: DeserializeOwned>(argument: Option<String>) -> Result<T, ControlError> {
    let argument = argument.unwrap_or_else(|| "{}".to_string());
    if argument.len() > MAX_DSH_ARGUMENT_BYTES {
        return Err(ControlError::InvalidRequest);
    }
    serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)
}

/// Client deadline of an open: the initialize deadline plus the shutdown
/// grace, since a runtime that fails its handshake is torn down before the
/// answer is written.
pub const DSH_SESSION_OPEN_DEADLINE: Duration = Duration::from_millis(
    vibemux_harness::dsh::MAX_INITIALIZE_DEADLINE_MS
        + vibemux_harness::dsh::MAX_SHUTDOWN_GRACE_MS
        + 20_000,
);

/// Client deadline of a close: the shutdown grace plus the kill and reap.
pub const DSH_SESSION_CLOSE_DEADLINE: Duration =
    Duration::from_millis(vibemux_harness::dsh::MAX_SHUTDOWN_GRACE_MS + 20_000);

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_harness::dsh::{DshPhase, DshRecordKind};
    use vibemux_types::ProjectId;

    fn status() -> DshSessionStatus {
        DshSessionStatus {
            session_id: "vibemux_test".to_string(),
            phase: DshPhase::Ready,
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            server_version: Some("0.0.1".to_string()),
            turns_started: 1,
            completed_turns: 1,
            failed_turns: 0,
            cancelled_turns: 0,
            unverified_turns: 0,
            last_outcome: None,
            records: 4,
            record_bytes: 512,
            foreign_notifications: 0,
            transport_open: true,
            prompt_bytes_total: 12,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            config: vibemux_dsh::DshSessionConfigSummary {
                profile: "sdk".to_string(),
                provider: "deepseek-official".to_string(),
                model: "deepseek-v4-flash".to_string(),
                has_reasoning_effort: false,
                max_tokens: None,
                environment_name_count: 1,
                limits: vibemux_harness::dsh::DshSessionLimits::default(),
            },
        }
    }

    fn encoded_response(payload: ControlPayload) -> Vec<u8> {
        let response = ControlResponse::success("r".repeat(128), payload);
        serde_json::to_vec(&response).expect("encode response")
    }

    #[test]
    fn a_maximal_status_fits_the_frame_and_carries_no_path() {
        let encoded = encoded_response(ControlPayload::DshSessionStatus(Box::new(status())));
        assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES);
        let text = String::from_utf8(encoded).expect("utf8");
        for hidden in ["executable", "entry_script", "bin.js", "node.exe"] {
            assert!(!text.contains(hidden), "{hidden}");
        }
        assert!(text.contains("\"prompt_bytes_total\":12"));
        let _ = ProjectId::new();
    }

    #[test]
    fn a_maximally_escaped_poll_page_fits_the_frame() {
        // Quotes and backslashes double in the record and again in the page.
        let heavy = "\"\\".repeat(6 * 1024);
        let records: Vec<DshSessionRecord> = (1..=4)
            .map(|sequence| DshSessionRecord {
                sequence,
                kind: DshRecordKind::Text,
                raw_json: heavy.clone(),
            })
            .collect();
        let page = DshSessionPollPage {
            next: 4,
            more: false,
            truncated: false,
            status: status(),
            records,
        };
        // The raw page is over the service budget, so the service must trim
        // it; the trimmed page then fits the control frame with room to spare.
        let raw = serde_json::to_vec(&page).expect("encode");
        assert!(raw.len() > crate::dsh_session::MAX_POLL_PAYLOAD_BYTES);
        let trimmed = serde_json::to_vec(&DshSessionPollPage {
            records: page.records[..1].to_vec(),
            ..page
        })
        .expect("encode");
        let encoded = encoded_response(ControlPayload::DshSessionPoll(
            serde_json::from_slice(&trimmed).expect("decode page"),
        ));
        assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES, "{}", encoded.len());
    }

    #[test]
    fn arguments_are_strict_and_bounded() {
        let open: DshSessionOpenRequest = parse_argument(None).expect("absent means empty");
        assert_eq!(open.session_id, None);
        let explicit: DshSessionOpenRequest =
            parse_argument(Some(r#"{"session_id":"vibemux_1"}"#.to_string())).expect("explicit");
        assert_eq!(explicit.session_id.as_deref(), Some("vibemux_1"));
        for argument in [
            Some(r#"{"session_id":"vibemux_1","extra":1}"#.to_string()),
            Some("{".to_string()),
            Some("x".repeat(MAX_DSH_ARGUMENT_BYTES + 1)),
        ] {
            assert_eq!(
                parse_argument::<DshSessionOpenRequest>(argument),
                Err(ControlError::InvalidRequest)
            );
        }
    }

    #[test]
    fn a_prompt_argument_never_reaches_debug() {
        let request = DshSessionPromptRequest {
            prompt: "a very secret prompt body".to_string(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("prompt_bytes"));
        // The encoded control request keeps the prompt out of `Debug` too.
        let control = ControlRequest {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            token: "t".repeat(64),
            operation: ControlOperation::DshSessionPrompt,
            argument: Some(serde_json::to_string(&request).expect("encode")),
        };
        assert!(!format!("{control:?}").contains("secret"));
        assert!(serde_json::to_vec(&control).expect("encode").len() < MAX_CONTROL_FRAME_BYTES);
    }

    #[test]
    fn the_dsh_operations_require_v6_only() {
        for operation in [
            ControlOperation::DshSessionOpen,
            ControlOperation::DshSessionPrompt,
            ControlOperation::DshSessionPoll,
            ControlOperation::DshSessionStatus,
            ControlOperation::DshSessionCancel,
            ControlOperation::DshSessionClose,
        ] {
            assert_eq!(operation.minimum_protocol_version(), CONTROL_PROTOCOL_V6);
            assert!(supported_control_version(CONTROL_PROTOCOL_V6));
        }
        // The current protocol is exactly v6, and v5 remains accepted so a
        // v5-only operation still works against a v6 daemon.
        assert_eq!(CONTROL_PROTOCOL_VERSION, CONTROL_PROTOCOL_V6);
        assert!(supported_control_version(vibemuxd_v5()));
        assert!(!supported_control_version(CONTROL_PROTOCOL_V6 + 1));
    }

    /// Read through a function so the assertion is a runtime check rather
    /// than a constant one the compiler folds away.
    fn vibemuxd_v5() -> u32 {
        CONTROL_PROTOCOL_V5
    }

    #[test]
    fn service_codes_pass_through_as_remote_errors() {
        assert_eq!(DshServiceError::Busy.code(), "dsh_session_busy");
        // A driver failure keeps its own actionable code rather than
        // collapsing into a generic refusal.
        assert_eq!(
            DshServiceError::from_driver(&vibemux_dsh::DshDriverError::ExecutableUntrusted),
            DshServiceError::ConfigInvalid
        );
        assert_eq!(
            DshServiceError::from_driver(&vibemux_dsh::DshDriverError::Unconfigured),
            DshServiceError::Unconfigured
        );
        assert_eq!(
            DshServiceError::from_driver(&vibemux_dsh::DshDriverError::InitializeTimeout),
            DshServiceError::Refused
        );
    }
}
