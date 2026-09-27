//! Fixed, content-free failure codes for harness dispatch.
//!
//! Each variant maps to one `harness_`-prefixed string. The strings are wire
//! contracts: they appear in Control IPC errors (where the `harness_` prefix
//! round-trips as `ControlError::Remote`), canonical event payloads, and the
//! persisted dispatch record. The enum is also the allowlist: vendor text,
//! paths, and parser messages never become an error code.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

macro_rules! dispatch_errors {
    ($($variant:ident => $code:literal,)+) => {
        /// Stable dispatch failure. `Display` and serialization emit only the code.
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum DispatchError {
            $($variant,)+
        }

        impl DispatchError {
            /// Every code, for exhaustive contract tests and code lookup.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            #[must_use]
            pub const fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }
    };
}

dispatch_errors! {
    // Admission, configuration, and lifecycle.
    Unconfigured => "harness_dispatch_unconfigured",
    ConfigInvalid => "harness_dispatch_config_invalid",
    ConfigTooLarge => "harness_dispatch_config_too_large",
    ConfigRouteInvalid => "harness_dispatch_config_route_invalid",
    ConfigExecutableInvalid => "harness_dispatch_config_executable_invalid",
    ConfigEnvironmentInvalid => "harness_dispatch_config_environment_invalid",
    RouteUnavailable => "harness_dispatch_route_unavailable",
    ExecutionDisabled => "harness_dispatch_execution_disabled",
    NotDetected => "harness_dispatch_not_detected",
    ProbeUnsupported => "harness_dispatch_probe_unsupported",
    InvalidRequest => "harness_dispatch_invalid_request",
    InvalidPrompt => "harness_dispatch_invalid_prompt",
    Busy => "harness_dispatch_busy",
    Conflict => "harness_dispatch_conflict",
    NotFound => "harness_dispatch_not_found",
    OutputUnavailable => "harness_dispatch_output_unavailable",
    Terminal => "harness_dispatch_terminal",
    InvalidTransition => "harness_dispatch_invalid_transition",
    StaleFence => "harness_dispatch_stale_fence",
    Interrupted => "harness_dispatch_interrupted",
    Cancelled => "harness_dispatch_cancelled",
    DeadlineExceeded => "harness_dispatch_deadline_exceeded",
    Internal => "harness_dispatch_internal",
    // Vendor protocol evidence.
    FrameTooLarge => "harness_protocol_frame_too_large",
    EmptyRecord => "harness_protocol_empty_record",
    InvalidUtf8 => "harness_protocol_invalid_utf8",
    InvalidJson => "harness_protocol_invalid_json",
    InvalidRecord => "harness_protocol_invalid_record",
    InvalidJsonRpc => "harness_protocol_invalid_jsonrpc",
    InvalidRpcId => "harness_protocol_invalid_rpc_id",
    InvalidRpcResult => "harness_protocol_invalid_rpc_result",
    RpcError => "harness_protocol_rpc_error",
    InvalidIdentifier => "harness_protocol_invalid_identifier",
    InvalidInitialize => "harness_protocol_invalid_initialize",
    UnsupportedProtocolVersion => "harness_protocol_unsupported_version",
    UncorrelatedResponse => "harness_protocol_uncorrelated_response",
    UnexpectedResponse => "harness_protocol_unexpected_response",
    UncorrelatedSession => "harness_protocol_uncorrelated_session",
    UncorrelatedTurn => "harness_protocol_uncorrelated_turn",
    UnexpectedTerminal => "harness_protocol_unexpected_terminal",
    InvalidTerminal => "harness_protocol_invalid_terminal",
    TurnFailed => "harness_protocol_turn_failed",
    EofWithoutTerminal => "harness_protocol_eof_without_terminal",
    CaptureTooLarge => "harness_capture_too_large",
    // Native process ownership (produced by the daemon, fixed here).
    ExecutableUnavailable => "harness_process_executable_unavailable",
    WorkingDirectoryInvalid => "harness_process_invalid_cwd",
    EnvironmentUnavailable => "harness_process_environment_unavailable",
    SpawnFailed => "harness_process_spawn_failed",
    ContainmentFailed => "harness_process_containment_failed",
    PipeFailed => "harness_process_pipe_failed",
    ProcessFailed => "harness_process_failed",
}

impl DispatchError {
    /// Reverse lookup for persisted or received codes; unknown text is `None`.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|error| error.code() == code)
    }
}

impl fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for DispatchError {}

impl Serialize for DispatchError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for DispatchError {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::from_code(&code).ok_or_else(|| de::Error::custom("unknown dispatch error code"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn codes_are_unique_snake_case_and_round_trip_through_the_harness_namespace() {
        let mut seen = BTreeSet::new();
        for error in DispatchError::ALL {
            let code = error.code();
            assert!(seen.insert(code), "duplicate code {code}");
            assert!(code.starts_with("harness_"));
            assert!(
                code.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            );
            assert_eq!(DispatchError::from_code(code), Some(*error));
            let encoded = serde_json::to_string(error).expect("encode code");
            assert_eq!(encoded, format!("\"{code}\""));
            let decoded: DispatchError = serde_json::from_str(&encoded).expect("decode code");
            assert_eq!(decoded, *error);
            assert_eq!(error.to_string(), code);
        }
        assert!(serde_json::from_str::<DispatchError>("\"harness_vendor_text\"").is_err());
    }
}
