//! AAG model-gateway contract: route bindings, isolation evidence, request
//! and response mapping, usage truthfulness, and failure classification
//! (ADR 031 §3).
//!
//! AAG is the gateway, not a model. Each role (supervisor, compiler,
//! workers, reviewer, optimizer) binds to an immutable model alias on one
//! loopback endpoint; VibeMux never switches the gateway's global provider.
//! The inspected AAG proxy applies a global `pinned_provider` to every
//! request when set, so strict routing is blocked while `/health` reports a
//! pin. Requested aliases and reported models are recorded separately; the
//! effective upstream provider is unknown unless the gateway attests it,
//! and missing usage is unknown, never zero.
//!
//! Failures before any output may be retried once as a new recorded
//! attempt. After output was observed, or once a tool call may have taken
//! effect, nothing is retried or stitched automatically.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{Sha256Digest, canonical_json::canonical_digest};

pub const GATEWAY_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ROUTE_GENERATION_DOMAIN: &str = "vibemux.workflow.route_generation.v1";
pub const MAX_ALIAS_BYTES: usize = 128;
pub const MAX_COMPLETION_BYTES: usize = 256 * 1024;
pub const MIN_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;
pub const MAX_OUTPUT_TOKENS: u32 = 16_384;
pub const CORRELATION_HEADER: &str = "x-vibemux-correlation";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayProtocol {
    OpenaiChatCompletions,
    AnthropicMessages,
    /// Served by AAG, but this adapter has no certified mapping for it.
    OpenaiResponses,
}

impl GatewayProtocol {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "/v1/chat/completions",
            Self::AnthropicMessages => "/v1/messages",
            Self::OpenaiResponses => "/v1/responses",
        }
    }

    #[must_use]
    pub const fn adapter_supported(self) -> bool {
        !matches!(self, Self::OpenaiResponses)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRole {
    Supervisor,
    Compiler,
    WorkerA,
    WorkerB,
    Reviewer,
    Optimizer,
}

impl RouteRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Compiler => "compiler",
            Self::WorkerA => "worker_a",
            Self::WorkerB => "worker_b",
            Self::Reviewer => "reviewer",
            Self::Optimizer => "optimizer",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteBinding {
    pub role: RouteRole,
    pub model_alias: String,
    pub protocol: GatewayProtocol,
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
}

/// Operator-owned gateway configuration. It names the environment variable
/// holding the gateway token, never the token itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub schema_version: u32,
    pub endpoint: String,
    pub credential_env: String,
    pub bindings: Vec<RouteBinding>,
    /// Refuse to route while the gateway reports a global provider pin.
    pub strict_routing: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayConfigError {
    #[error("gateway configuration schema is unsupported")]
    SchemaUnsupported,
    #[error("gateway endpoint must be a loopback http origin")]
    EndpointNotLoopback,
    #[error("credential reference must be an environment variable name")]
    InvalidCredentialReference,
    #[error("a role is bound more than once")]
    DuplicateRole,
    #[error("model alias is empty, too long, or contains whitespace")]
    InvalidAlias,
    #[error("timeout or output token bound is out of range")]
    InvalidBounds,
    #[error("reviewer must use a model alias distinct from every worker")]
    ReviewerNotIndependent,
}

impl GatewayConfigError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SchemaUnsupported => "gateway_config_schema_unsupported",
            Self::EndpointNotLoopback => "gateway_endpoint_not_loopback",
            Self::InvalidCredentialReference => "gateway_invalid_credential_reference",
            Self::DuplicateRole => "gateway_duplicate_role",
            Self::InvalidAlias => "gateway_invalid_alias",
            Self::InvalidBounds => "gateway_invalid_bounds",
            Self::ReviewerNotIndependent => "gateway_reviewer_not_independent",
        }
    }
}

impl GatewayConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self, GatewayConfigError> {
        let config: Self =
            serde_json::from_slice(bytes).map_err(|_| GatewayConfigError::SchemaUnsupported)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), GatewayConfigError> {
        if self.schema_version != GATEWAY_CONFIG_SCHEMA_VERSION {
            return Err(GatewayConfigError::SchemaUnsupported);
        }
        if !is_loopback_origin(&self.endpoint) {
            return Err(GatewayConfigError::EndpointNotLoopback);
        }
        let name = &self.credential_env;
        let valid_name = !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            && !name.starts_with("VIBEMUX_");
        if !valid_name {
            return Err(GatewayConfigError::InvalidCredentialReference);
        }
        let mut roles: Vec<RouteRole> = self.bindings.iter().map(|binding| binding.role).collect();
        roles.sort();
        let count = roles.len();
        roles.dedup();
        if roles.len() != count {
            return Err(GatewayConfigError::DuplicateRole);
        }
        for binding in &self.bindings {
            let alias = &binding.model_alias;
            if alias.is_empty()
                || alias.len() > MAX_ALIAS_BYTES
                || alias
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(GatewayConfigError::InvalidAlias);
            }
            if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&binding.timeout_ms)
                || binding.max_output_tokens == 0
                || binding.max_output_tokens > MAX_OUTPUT_TOKENS
            {
                return Err(GatewayConfigError::InvalidBounds);
            }
        }
        if let Some(reviewer) = self.binding(RouteRole::Reviewer) {
            let collides = [RouteRole::WorkerA, RouteRole::WorkerB]
                .into_iter()
                .filter_map(|role| self.binding(role))
                .any(|worker| worker.model_alias == reviewer.model_alias);
            if collides {
                return Err(GatewayConfigError::ReviewerNotIndependent);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn binding(&self, role: RouteRole) -> Option<&RouteBinding> {
        self.bindings.iter().find(|binding| binding.role == role)
    }
}

/// `http://` plus `127.0.0.1`, `localhost`, or `[::1]`, an explicit port,
/// and no path, query, or user information.
#[must_use]
pub fn is_loopback_origin(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("http://") else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = if let Some(after) = rest.strip_prefix("[::1]:") {
        ("[::1]", after)
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => return false,
        }
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
        && !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port > 0)
}

/// What the gateway's authenticated `/health` reported, reduced to the
/// facts routing needs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GatewayHealth {
    pub status_ok: bool,
    pub pinned_provider_present: bool,
    pub models: Vec<String>,
    pub route_generation: Sha256Digest,
}

impl GatewayHealth {
    /// Parses AAG `/health` and `/v1/models` bodies.
    pub fn from_responses(health: &Value, models: &Value) -> Result<Self, ResponseError> {
        let status_ok = health.get("status").and_then(Value::as_str) == Some("ok");
        let pinned_provider_present = match health.get("pinnedProvider") {
            None | Some(Value::Null) => false,
            Some(Value::String(text)) => !text.is_empty(),
            Some(_) => true,
        };
        let mut aliases: Vec<String> = models
            .get("data")
            .and_then(Value::as_array)
            .ok_or(ResponseError::Malformed)?
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        aliases.sort();
        aliases.dedup();
        let generation_input = json!({"pinned": pinned_provider_present, "models": aliases});
        let route_generation = canonical_digest(ROUTE_GENERATION_DOMAIN, &generation_input)
            .map_err(|_| ResponseError::Malformed)?;
        Ok(Self {
            status_ok,
            pinned_provider_present,
            models: aliases,
            route_generation,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteBlock {
    GatewayUnhealthy,
    GlobalProviderPinActive,
    AliasNotAdvertised,
    ProtocolUnsupported,
    RoleUnbound,
}

impl RouteBlock {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::GatewayUnhealthy => "route_gateway_unhealthy",
            Self::GlobalProviderPinActive => "route_global_provider_pin_active",
            Self::AliasNotAdvertised => "route_alias_not_advertised",
            Self::ProtocolUnsupported => "route_protocol_unsupported",
            Self::RoleUnbound => "route_role_unbound",
        }
    }
}

/// Whether `role` may be called now, given the latest health observation.
pub fn evaluate_route<'a>(
    config: &'a GatewayConfig,
    health: &GatewayHealth,
    role: RouteRole,
) -> Result<&'a RouteBinding, Vec<RouteBlock>> {
    let mut blocks = Vec::new();
    let binding = config.binding(role);
    if binding.is_none() {
        blocks.push(RouteBlock::RoleUnbound);
    }
    if !health.status_ok {
        blocks.push(RouteBlock::GatewayUnhealthy);
    }
    if config.strict_routing && health.pinned_provider_present {
        blocks.push(RouteBlock::GlobalProviderPinActive);
    }
    if let Some(binding) = binding {
        if !binding.protocol.adapter_supported() {
            blocks.push(RouteBlock::ProtocolUnsupported);
        }
        if !health
            .models
            .iter()
            .any(|model| model == &binding.model_alias)
        {
            blocks.push(RouteBlock::AliasNotAdvertised);
        }
    }
    match binding {
        Some(binding) if blocks.is_empty() => Ok(binding),
        _ => {
            blocks.sort();
            Err(blocks)
        }
    }
}

/// Content-free correlation carried in [`CORRELATION_HEADER`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CallCorrelation {
    pub workflow_id: Uuid,
    pub call_id: Uuid,
    pub attempt: u32,
}

impl CallCorrelation {
    #[must_use]
    pub fn header_value(&self) -> String {
        format!(
            "{}/{}/{}",
            self.workflow_id.simple(),
            self.call_id.simple(),
            self.attempt
        )
    }
}

/// Non-streaming request body. Temperature zero is requested but is not a
/// determinism claim; the deterministic validator decides admission.
#[must_use]
pub fn request_body(binding: &RouteBinding, system: &str, user: &str) -> Value {
    match binding.protocol {
        GatewayProtocol::AnthropicMessages => json!({
            "model": binding.model_alias,
            "system": system,
            "messages": [{"role": "user", "content": user}],
            "max_tokens": binding.max_output_tokens,
            "temperature": 0,
            "stream": false,
        }),
        GatewayProtocol::OpenaiChatCompletions | GatewayProtocol::OpenaiResponses => json!({
            "model": binding.model_alias,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "max_tokens": binding.max_output_tokens,
            "temperature": 0,
            "stream": false,
        }),
    }
}

/// Reported usage. `None` means the gateway did not report it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportedUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
}

impl ReportedUsage {
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        self.input_tokens.is_none() && self.output_tokens.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCompletion {
    pub text: String,
    pub reported_model: Option<String>,
    pub usage: ReportedUsage,
    pub finish_reason: Option<String>,
    pub tool_call_requested: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseError {
    #[error("response is not the expected JSON shape")]
    Malformed,
    #[error("response exceeds its size limit")]
    TooLarge,
    #[error("response carried no text")]
    Empty,
}

impl ResponseError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "gateway_response_malformed",
            Self::TooLarge => "gateway_response_too_large",
            Self::Empty => "gateway_response_empty",
        }
    }
}

pub fn parse_completion(
    protocol: GatewayProtocol,
    body: &[u8],
) -> Result<ParsedCompletion, ResponseError> {
    if body.len() > MAX_COMPLETION_BYTES {
        return Err(ResponseError::TooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| ResponseError::Malformed)?;
    let reported_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64);
    let parsed = match protocol {
        GatewayProtocol::AnthropicMessages => {
            let content = value
                .get("content")
                .and_then(Value::as_array)
                .ok_or(ResponseError::Malformed)?;
            let text: String = content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect();
            let tool_call_requested = content
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
            let usage = value
                .get("usage")
                .map_or_else(ReportedUsage::default, |usage| ReportedUsage {
                    input_tokens: number(usage, "input_tokens"),
                    output_tokens: number(usage, "output_tokens"),
                    cached_tokens: number(usage, "cache_read_input_tokens"),
                });
            ParsedCompletion {
                text,
                reported_model,
                usage,
                finish_reason: value
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tool_call_requested,
            }
        }
        GatewayProtocol::OpenaiChatCompletions => {
            let choice = value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .ok_or(ResponseError::Malformed)?;
            let message = choice.get("message").ok_or(ResponseError::Malformed)?;
            let text = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool_call_requested = message
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty());
            let usage = value
                .get("usage")
                .map_or_else(ReportedUsage::default, |usage| ReportedUsage {
                    input_tokens: number(usage, "prompt_tokens"),
                    output_tokens: number(usage, "completion_tokens"),
                    cached_tokens: usage
                        .get("prompt_tokens_details")
                        .and_then(|details| number(details, "cached_tokens")),
                });
            ParsedCompletion {
                text,
                reported_model,
                usage,
                finish_reason: choice
                    .get("finish_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tool_call_requested,
            }
        }
        GatewayProtocol::OpenaiResponses => return Err(ResponseError::Malformed),
    };
    if parsed.text.trim().is_empty() && !parsed.tool_call_requested {
        return Err(ResponseError::Empty);
    }
    Ok(parsed)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "stage", content = "status")]
pub enum FailureStage {
    /// The connection or request write failed; no response bytes arrived.
    Connect,
    /// The gateway answered with an error status before any output.
    Status(u16),
    /// The deadline passed; upstream generation may still be running.
    Timeout,
    /// The stream broke or the body failed to parse after output began.
    BrokenAfterOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "reason")]
pub enum RetryDecision {
    /// Nothing was produced; one new recorded attempt is allowed.
    MayRetryOnce,
    NoAutomaticRetry(&'static str),
}

/// Retry policy at the VibeMux client. `output_bytes` counts response
/// bytes the client observed for this attempt; `tool_effect_possible` is
/// true once a tool call was emitted.
#[must_use]
pub fn classify_failure(
    stage: FailureStage,
    output_bytes: u64,
    tool_effect_possible: bool,
) -> RetryDecision {
    if tool_effect_possible {
        return RetryDecision::NoAutomaticRetry("tool_effect_possible");
    }
    if output_bytes > 0 {
        return RetryDecision::NoAutomaticRetry("output_committed");
    }
    match stage {
        FailureStage::Connect => RetryDecision::MayRetryOnce,
        FailureStage::Status(status) if status == 429 || status >= 500 => {
            RetryDecision::MayRetryOnce
        }
        FailureStage::Status(_) => RetryDecision::NoAutomaticRetry("request_rejected"),
        FailureStage::Timeout => RetryDecision::NoAutomaticRetry("upstream_may_still_run"),
        FailureStage::BrokenAfterOutput => RetryDecision::NoAutomaticRetry("output_committed"),
    }
}

/// Accumulates an OpenAI-compatible SSE stream. Once any content or tool
/// delta was accepted the output is committed; a later failure ends the
/// attempt as failed and the partial text is reported only as discarded
/// size, never joined with another attempt's output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StreamAccumulator {
    text: String,
    committed_bytes: u64,
    tool_call_seen: bool,
    done: bool,
    reported_model: Option<String>,
    usage: ReportedUsage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamEnd {
    Completed(ParsedCompletion),
    /// The stream broke after output was committed.
    FailedAfterOutput {
        discarded_bytes: u64,
        tool_call_seen: bool,
    },
    /// The stream broke before any output.
    FailedBeforeOutput,
}

impl StreamAccumulator {
    /// Feeds one SSE `data:` payload (without the prefix).
    pub fn accept_data(&mut self, data: &str) -> Result<(), ResponseError> {
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_str(data).map_err(|_| ResponseError::Malformed)?;
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            self.reported_model = Some(model.to_string());
        }
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = ReportedUsage {
                input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
                output_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
                cached_tokens: usage
                    .get("prompt_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
                    .and_then(Value::as_u64),
            };
        }
        let delta = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"));
        if let Some(delta) = delta {
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if self.text.len() + content.len() > MAX_COMPLETION_BYTES {
                    return Err(ResponseError::TooLarge);
                }
                self.text.push_str(content);
                self.committed_bytes += content.len() as u64;
            }
            if delta
                .get("tool_calls")
                .is_some_and(|calls| !calls.is_null())
            {
                self.tool_call_seen = true;
                self.committed_bytes += 1;
            }
        }
        Ok(())
    }

    #[must_use]
    pub const fn committed_bytes(&self) -> u64 {
        self.committed_bytes
    }

    #[must_use]
    pub const fn tool_call_seen(&self) -> bool {
        self.tool_call_seen
    }

    /// Ends the stream; `broken` is true when the transport failed.
    #[must_use]
    pub fn finish(self, broken: bool) -> StreamEnd {
        if broken || !self.done {
            if self.committed_bytes == 0 {
                return StreamEnd::FailedBeforeOutput;
            }
            return StreamEnd::FailedAfterOutput {
                discarded_bytes: self.text.len() as u64,
                tool_call_seen: self.tool_call_seen,
            };
        }
        StreamEnd::Completed(ParsedCompletion {
            text: self.text,
            reported_model: self.reported_model,
            usage: self.usage,
            finish_reason: None,
            tool_call_requested: self.tool_call_seen,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Completed,
    FailedBeforeOutput,
    FailedAfterOutput,
    Blocked,
}

/// What the gateway would not tell us stays explicitly unknown.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttribution {
    Unknown,
}

/// The content-free record of one model call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallReceipt {
    pub call_id: Uuid,
    pub role: RouteRole,
    pub requested_alias: String,
    pub protocol: GatewayProtocol,
    pub reported_model: Option<String>,
    pub effective_provider: ProviderAttribution,
    pub usage: ReportedUsage,
    pub http_status: Option<u16>,
    pub outcome: CallOutcome,
    pub failure: Option<FailureStage>,
    pub elapsed_ms: u64,
    pub request_sha256: Sha256Digest,
    pub response_sha256: Option<Sha256Digest>,
    pub route_generation: Sha256Digest,
    pub attempt: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(role: RouteRole, alias: &str, protocol: GatewayProtocol) -> RouteBinding {
        RouteBinding {
            role,
            model_alias: alias.to_string(),
            protocol,
            max_output_tokens: 4096,
            timeout_ms: 120_000,
        }
    }

    fn config() -> GatewayConfig {
        GatewayConfig {
            schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
            endpoint: "http://127.0.0.1:15721".into(),
            credential_env: "AAG_GATEWAY_TOKEN".into(),
            bindings: vec![
                binding(
                    RouteRole::Supervisor,
                    "vibemux-supervisor",
                    GatewayProtocol::OpenaiChatCompletions,
                ),
                binding(
                    RouteRole::Compiler,
                    "vibemux-compiler",
                    GatewayProtocol::OpenaiChatCompletions,
                ),
                binding(
                    RouteRole::Reviewer,
                    "vibemux-reviewer",
                    GatewayProtocol::AnthropicMessages,
                ),
                binding(
                    RouteRole::WorkerA,
                    "vibemux-worker-a",
                    GatewayProtocol::AnthropicMessages,
                ),
            ],
            strict_routing: true,
        }
    }

    fn health(pinned: Value, models: &[&str]) -> GatewayHealth {
        let data: Vec<Value> = models.iter().map(|id| json!({"id": id})).collect();
        GatewayHealth::from_responses(
            &json!({"status": "ok", "pinnedProvider": pinned}),
            &json!({"object": "list", "data": data}),
        )
        .expect("health")
    }

    #[test]
    fn configs_are_loopback_only_and_reference_credentials_by_name() {
        config().validate().expect("valid");
        for endpoint in [
            "https://127.0.0.1:1",
            "http://10.0.0.5:8080",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:99999",
            "http://user@127.0.0.1:80",
            "http://127.0.0.1:80/v1",
            "http://localhost.evil.com:80",
        ] {
            let mut bad = config();
            bad.endpoint = endpoint.into();
            assert_eq!(
                bad.validate(),
                Err(GatewayConfigError::EndpointNotLoopback),
                "{endpoint}"
            );
        }
        for endpoint in [
            "http://localhost:8787",
            "http://[::1]:8787/",
            "http://127.0.0.1:15721/",
        ] {
            assert!(is_loopback_origin(endpoint), "{endpoint}");
        }
        let mut literal_token = config();
        literal_token.credential_env = "sk-live-abc".into();
        assert_eq!(
            literal_token.validate(),
            Err(GatewayConfigError::InvalidCredentialReference)
        );
        let mut duplicate = config();
        duplicate.bindings.push(binding(
            RouteRole::Compiler,
            "other",
            GatewayProtocol::OpenaiChatCompletions,
        ));
        assert_eq!(duplicate.validate(), Err(GatewayConfigError::DuplicateRole));
        let mut shared = config();
        shared.bindings[2].model_alias = "vibemux-worker-a".into();
        assert_eq!(
            shared.validate(),
            Err(GatewayConfigError::ReviewerNotIndependent)
        );
    }

    #[test]
    fn a_global_pin_blocks_strict_routing_and_changes_the_generation() {
        let models = [
            "vibemux-supervisor",
            "vibemux-compiler",
            "vibemux-reviewer",
            "vibemux-worker-a",
        ];
        let clear = health(Value::Null, &models);
        let pinned = health(json!("provider_x"), &models);
        assert_ne!(clear.route_generation, pinned.route_generation);
        evaluate_route(&config(), &clear, RouteRole::Supervisor).expect("routable");
        assert_eq!(
            evaluate_route(&config(), &pinned, RouteRole::Supervisor),
            Err(vec![RouteBlock::GlobalProviderPinActive])
        );
        let mut relaxed = config();
        relaxed.strict_routing = false;
        evaluate_route(&relaxed, &pinned, RouteRole::Supervisor).expect("operator opted out");
    }

    #[test]
    fn unsupported_protocols_unbound_roles_and_missing_aliases_are_refused_up_front() {
        let mut responses = config();
        responses.bindings[0].protocol = GatewayProtocol::OpenaiResponses;
        let healthy = health(Value::Null, &["vibemux-supervisor"]);
        assert_eq!(
            evaluate_route(&responses, &healthy, RouteRole::Supervisor),
            Err(vec![RouteBlock::ProtocolUnsupported])
        );
        assert_eq!(
            evaluate_route(&config(), &healthy, RouteRole::Optimizer),
            Err(vec![RouteBlock::RoleUnbound])
        );
        assert_eq!(
            evaluate_route(&config(), &healthy, RouteRole::Compiler),
            Err(vec![RouteBlock::AliasNotAdvertised])
        );
    }

    #[test]
    fn usage_is_parsed_when_reported_and_unknown_otherwise() {
        let openai = br#"{"model":"upstream-model-x","choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":4}}}"#;
        let parsed =
            parse_completion(GatewayProtocol::OpenaiChatCompletions, openai).expect("parse");
        assert_eq!(parsed.text, "ok");
        assert_eq!(parsed.reported_model.as_deref(), Some("upstream-model-x"));
        assert_eq!(
            parsed.usage,
            ReportedUsage {
                input_tokens: Some(12),
                output_tokens: Some(3),
                cached_tokens: Some(4)
            }
        );
        let no_usage = br#"{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn"}"#;
        let parsed = parse_completion(GatewayProtocol::AnthropicMessages, no_usage).expect("parse");
        assert!(parsed.usage.is_unknown());
        assert_eq!(parsed.reported_model, None);
        assert_eq!(
            parse_completion(GatewayProtocol::OpenaiChatCompletions, b"{}"),
            Err(ResponseError::Malformed)
        );
        let empty = br#"{"choices":[{"message":{"content":"  "}}]}"#;
        assert_eq!(
            parse_completion(GatewayProtocol::OpenaiChatCompletions, empty),
            Err(ResponseError::Empty)
        );
    }

    #[test]
    fn failures_after_output_or_tool_calls_are_never_retried_automatically() {
        assert_eq!(
            classify_failure(FailureStage::Connect, 0, false),
            RetryDecision::MayRetryOnce
        );
        assert_eq!(
            classify_failure(FailureStage::Status(503), 0, false),
            RetryDecision::MayRetryOnce
        );
        assert_eq!(
            classify_failure(FailureStage::Status(400), 0, false),
            RetryDecision::NoAutomaticRetry("request_rejected")
        );
        assert_eq!(
            classify_failure(FailureStage::Timeout, 0, false),
            RetryDecision::NoAutomaticRetry("upstream_may_still_run")
        );
        assert_eq!(
            classify_failure(FailureStage::Status(503), 10, false),
            RetryDecision::NoAutomaticRetry("output_committed")
        );
        assert_eq!(
            classify_failure(FailureStage::Connect, 0, true),
            RetryDecision::NoAutomaticRetry("tool_effect_possible")
        );
    }

    #[test]
    fn broken_streams_are_not_stitched() {
        let mut stream = StreamAccumulator::default();
        stream
            .accept_data(r#"{"model":"m","choices":[{"delta":{"content":"partial "}}]}"#)
            .expect("chunk");
        assert_eq!(stream.committed_bytes(), 8);
        assert_eq!(
            stream.finish(true),
            StreamEnd::FailedAfterOutput {
                discarded_bytes: 8,
                tool_call_seen: false
            }
        );
        let mut tool = StreamAccumulator::default();
        tool.accept_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1"}]}}]}"#)
            .expect("tool");
        assert!(matches!(
            tool.clone().finish(true),
            StreamEnd::FailedAfterOutput {
                tool_call_seen: true,
                ..
            }
        ));
        assert_eq!(
            StreamAccumulator::default().finish(true),
            StreamEnd::FailedBeforeOutput
        );
        let mut complete = StreamAccumulator::default();
        complete
            .accept_data(r#"{"model":"m","choices":[{"delta":{"content":"done"}}]}"#)
            .expect("chunk");
        complete.accept_data("[DONE]").expect("done");
        assert!(
            matches!(complete.finish(false), StreamEnd::Completed(parsed) if parsed.text == "done" && parsed.usage.is_unknown())
        );
    }

    #[test]
    fn correlation_header_is_content_free() {
        let correlation = CallCorrelation {
            workflow_id: Uuid::from_u128(1),
            call_id: Uuid::from_u128(2),
            attempt: 1,
        };
        let header = correlation.header_value();
        assert_eq!(header.split('/').count(), 3);
        assert!(
            header
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'/')
        );
    }
}
