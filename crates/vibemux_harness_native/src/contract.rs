use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use vibemux_harness::{AgentKind, dispatch::NativeProtocol};

use crate::Error;

/// Observation kinds are the shared ADR 029 vocabulary, so a record classified
/// here and one classified by the daemon mean the same thing.
pub use vibemux_harness::dispatch::ObservationKind as Kind;

pub const SCHEMA_VERSION: u32 = 1;
pub const EXECUTE_METHOD: &str = "harness:execute";
pub const PROBE_METHOD: &str = "harness:probe";
pub const RECORD_EVENT: &str = "harness:record";
pub const MAX_PROMPT_BYTES: usize = 128 * 1024;
/// The largest request JSON a request file or plugin payload may hold: a
/// maximum prompt with every byte written as a six-byte `\u00XX` escape,
/// plus room for the other fields.
pub const MAX_REQUEST_BYTES: usize = MAX_PROMPT_BYTES * 6 + 4096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Codex,
    Claude,
    Opencode,
    Copilot,
    Grok,
}

impl Harness {
    /// The shared registry identity of this harness.
    #[must_use]
    pub const fn agent_kind(self) -> AgentKind {
        match self {
            Self::Codex => AgentKind::Codex,
            Self::Claude => AgentKind::Claude,
            Self::Opencode => AgentKind::OpenCode,
            Self::Copilot => AgentKind::Copilot,
            Self::Grok => AgentKind::Grok,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Protocol {
    #[serde(rename = "codex_exec")]
    Codex,
    #[serde(rename = "codex_app_server")]
    Appserver,
    #[serde(rename = "claude_stream_json")]
    Claude,
    #[serde(rename = "opencode_json")]
    Opencode,
    #[serde(rename = "copilot_json")]
    Copilot,
    #[serde(rename = "acp")]
    Acp,
    #[serde(rename = "grok_stream_json")]
    Grok,
}

impl Protocol {
    pub fn accepts(self, harness: Harness) -> bool {
        matches!(
            (self, harness),
            (Self::Codex | Self::Appserver, Harness::Codex)
                | (Self::Claude, Harness::Claude)
                | (Self::Opencode | Self::Acp, Harness::Opencode)
                | (Self::Copilot | Self::Acp, Harness::Copilot)
                | (Self::Grok | Self::Acp, Harness::Grok)
        )
    }

    /// The shared protocol engine that drives this route, if any. Direct
    /// OpenCode, Copilot, and Grok JSON output is replay-only: no session
    /// adapter exists for it.
    #[must_use]
    pub const fn native_protocol(self) -> Option<NativeProtocol> {
        match self {
            Self::Codex => Some(NativeProtocol::CodexExec),
            Self::Appserver => Some(NativeProtocol::CodexAppServer),
            Self::Claude => Some(NativeProtocol::ClaudeStreamJson),
            Self::Acp => Some(NativeProtocol::Acp),
            Self::Opencode | Self::Copilot | Self::Grok => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Completed,
    Failed,
    Cancelled,
    Unverified,
    Probed,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub harness: Harness,
    pub prompt: String,
}

impl Request {
    pub fn validate(&self) -> Result<(), Error> {
        if self.schema_version != SCHEMA_VERSION || self.request_id.is_nil() {
            return Err(Error::new("harness_invalid_request"));
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(Error::new("harness_invalid_prompt"));
        }
        Ok(())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("request_id", &self.request_id)
            .field("harness", &self.harness)
            .field("prompt_bytes", &self.prompt.len())
            .finish_non_exhaustive()
    }
}

/// Complete vendor JSON bytes, including unknown fields, held in bounded memory.
/// Serialization is for the private local plugin pipe, never public reports.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub sequence: u64,
    pub kind: Kind,
    raw_json: String,
}

impl Record {
    pub fn parse(protocol: Protocol, sequence: u64, raw_json: String) -> Result<Self, Error> {
        let value: Value = serde_json::from_str(&raw_json)
            .map_err(|source| Error::with_source("harness_invalid_json", source))?;
        if !value.is_object() || sequence == 0 {
            return Err(Error::new("harness_invalid_record"));
        }
        let kind = crate::normalize::classify(protocol, &value);
        Ok(Self {
            sequence,
            kind,
            raw_json,
        })
    }

    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }

    pub fn value(&self) -> Result<Value, Error> {
        serde_json::from_str(&self.raw_json)
            .map_err(|source| Error::with_source("harness_invalid_json", source))
    }

    pub fn summary(&self) -> Observation {
        Observation {
            sequence: self.sequence,
            kind: self.kind,
            bytes: self.raw_json.len(),
        }
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.summary().fmt(formatter)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub sequence: u64,
    pub kind: Kind,
    pub bytes: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub schema_version: u32,
    pub status: Status,
    pub record_count: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub exit_code: Option<i32>,
    pub forced_termination: bool,
    pub error_code: Option<String>,
}

pub struct Outcome {
    pub request_id: Uuid,
    pub harness: Harness,
    pub records: Vec<Record>,
    pub completion: Completion,
}

impl Outcome {
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "request_id": self.request_id,
            "harness": self.harness,
            "completion": self.completion,
            "observations": self.records.iter().map(Record::summary).collect::<Vec<_>>()
        })
    }
}

impl fmt::Debug for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.summary().fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use vibemux_harness::{
        AgentKind,
        dispatch::{NativeProtocol, adapter_capability},
    };

    use uuid::Uuid;

    use super::{Harness, MAX_PROMPT_BYTES, Protocol, Request, SCHEMA_VERSION};

    const HARNESSES: [Harness; 5] = [
        Harness::Codex,
        Harness::Claude,
        Harness::Opencode,
        Harness::Copilot,
        Harness::Grok,
    ];
    const PROTOCOLS: [Protocol; 7] = [
        Protocol::Codex,
        Protocol::Appserver,
        Protocol::Claude,
        Protocol::Opencode,
        Protocol::Copilot,
        Protocol::Acp,
        Protocol::Grok,
    ];

    fn serde_name<T: serde::Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The native route table drives a harness through the shared engine
    /// exactly when the shared engine accepts that pair, and it names both
    /// with the shared vocabulary: the harness by its command name, the
    /// protocol by the engine's id. So the native configuration, the daemon
    /// configuration, and the capability table cannot drift apart.
    #[test]
    fn native_routes_agree_with_the_shared_engine() {
        for harness in HARNESSES {
            assert_eq!(serde_name(harness), harness.agent_kind().command_name());
        }
        for protocol in PROTOCOLS {
            let Some(shared) = protocol.native_protocol() else {
                continue;
            };
            assert_eq!(serde_name(protocol), shared.as_str());
            for harness in HARNESSES {
                assert_eq!(
                    protocol.accepts(harness),
                    shared.accepts(harness.agent_kind()),
                    "{protocol:?} {harness:?}"
                );
            }
        }
    }

    /// Every harness and protocol pair the shared engine can drive in the
    /// CLI process has a native route, so no harness that the capability
    /// table lists with a structured adapter is unreachable from
    /// `vibemuxctl harness`. DSH is driven by `vibemux_dsh` instead.
    #[test]
    fn every_shared_adapter_outside_dsh_has_a_native_route() {
        for kind in AgentKind::all() {
            for shared in adapter_capability(kind).protocols {
                if shared == NativeProtocol::DshSdkJsonRpc {
                    continue;
                }
                let routed = HARNESSES.into_iter().any(|harness| {
                    harness.agent_kind() == kind
                        && PROTOCOLS.into_iter().any(|protocol| {
                            protocol.native_protocol() == Some(shared) && protocol.accepts(harness)
                        })
                });
                assert!(
                    routed,
                    "{kind:?} via {} has no native route",
                    shared.as_str()
                );
            }
        }
    }

    fn request(prompt: String) -> Request {
        Request {
            schema_version: SCHEMA_VERSION,
            request_id: Uuid::new_v4(),
            harness: Harness::Codex,
            prompt,
        }
    }

    fn outcome(request: &Request) -> &'static str {
        request
            .validate()
            .map_or_else(|error| error.code(), |()| "ok")
    }

    #[test]
    fn requests_need_an_identity_and_a_bounded_non_blank_prompt() {
        // The bound counts UTF-8 bytes, not characters.
        let two_byte = "\u{e9}".repeat(MAX_PROMPT_BYTES / 2);
        for prompt in [
            "hello".to_string(),
            "x".repeat(MAX_PROMPT_BYTES),
            two_byte.clone(),
        ] {
            assert_eq!(outcome(&request(prompt)), "ok");
        }
        for prompt in [
            String::new(),
            " \t\r\n".to_string(),
            "x".repeat(MAX_PROMPT_BYTES + 1),
            two_byte + "x",
        ] {
            assert_eq!(
                outcome(&request(prompt.clone())),
                "harness_invalid_prompt",
                "{} bytes",
                prompt.len()
            );
        }
        let mut other_schema = request("hello".to_string());
        other_schema.schema_version = SCHEMA_VERSION + 1;
        assert_eq!(outcome(&other_schema), "harness_invalid_request");
        let mut nil_id = request("hello".to_string());
        nil_id.request_id = Uuid::nil();
        assert_eq!(outcome(&nil_id), "harness_invalid_request");
        // The guide states the same bound.
        let guide = include_str!("../../../docs/guides/harness_core_guide.md");
        let bound = format!("at most {} KiB", MAX_PROMPT_BYTES / 1024);
        assert!(guide.contains(&bound), "{bound}");
    }
}
