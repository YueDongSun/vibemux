//! Bounded HTTPS adapter to the TypeSafe Jev System One API (ADR 034 §6).
//!
//! `JevClient` sends exactly the fields ADR 034 §4 allows, refuses redirects
//! and proxies, caps the response body, and retries 429/529 at most once
//! inside a fixed total time budget. It performs no answer validation beyond
//! shape: the confidence gate and model pin (§8) are the daemon's job.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use vibemux_decision::{BackendAnswer, DecisionKind, ExternalError, PeerRequest};

/// Fixed production endpoint; the only one a release build ever contacts.
pub const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
pub const MAX_JEV_RESPONSE_BYTES: usize = 16 * 1024;
pub const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(20);
pub const TOTAL_BUDGET: Duration = Duration::from_secs(45);
pub const RETRY_BACKOFF: Duration = Duration::from_secs(2);
/// One attempt plus one retry, and only for 429/529 (ADR 034 §6).
pub const MAX_ATTEMPTS: u32 = 2;

const JEV_QUESTION_TYPE: &str = "choice";

/// Tunable request limits. Tests shrink these; production uses [`Default`].
#[derive(Clone, Copy, Debug)]
pub struct ClientLimits {
    pub per_attempt_timeout: Duration,
    pub total_budget: Duration,
    pub retry_backoff: Duration,
    pub max_attempts: u32,
    pub max_response_bytes: usize,
}

impl Default for ClientLimits {
    fn default() -> Self {
        Self {
            per_attempt_timeout: PER_ATTEMPT_TIMEOUT,
            total_budget: TOTAL_BUDGET,
            retry_backoff: RETRY_BACKOFF,
            max_attempts: MAX_ATTEMPTS,
            max_response_bytes: MAX_JEV_RESPONSE_BYTES,
        }
    }
}

/// The credential value read from the configured environment variable.
/// `Debug` is redacted; there is no `Display` and it never appears in an
/// error.
pub struct Credential(String);

impl Credential {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Credential(<redacted>)")
    }
}

/// Reads the credential named by `name` through an injectable lookup so
/// callers (and tests) never need to mutate the process environment.
/// Missing, empty, or non-UTF-8 (filtered out by the lookup itself) yields
/// [`ExternalError::CredentialUnavailable`]; no request is attempted.
pub fn read_credential_env(
    name: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Credential, ExternalError> {
    let value = lookup(name).ok_or(ExternalError::CredentialUnavailable)?;
    if value.is_empty() {
        return Err(ExternalError::CredentialUnavailable);
    }
    Ok(Credential::new(value))
}

pub struct JevClient {
    endpoint: reqwest::Url,
    http: reqwest::Client,
    limits: ClientLimits,
}

impl JevClient {
    /// The production client: always the fixed Jev endpoint.
    pub fn new(limits: ClientLimits) -> Result<Self, ExternalError> {
        let endpoint = reqwest::Url::parse(JEV_ENDPOINT).map_err(|_| ExternalError::Transport)?;
        Self::build(endpoint, limits)
    }

    /// Test-only client bound to a loopback endpoint. Accepts only
    /// `http://127.0.0.1:<port>/...`, `http://[::1]:<port>/...`, or
    /// `http://localhost:<port>/...`.
    #[cfg(feature = "test_endpoint")]
    pub fn with_test_endpoint(limits: ClientLimits, endpoint: &str) -> Result<Self, ExternalError> {
        let endpoint = validate_loopback_endpoint(endpoint)?;
        Self::build(endpoint, limits)
    }

    fn build(endpoint: reqwest::Url, limits: ClientLimits) -> Result<Self, ExternalError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(limits.per_attempt_timeout)
            .build()
            .map_err(|_| ExternalError::Transport)?;
        Ok(Self {
            endpoint,
            http,
            limits,
        })
    }

    /// Asks one `harness_route` (or other `question_key`) choice question.
    /// Retries once on 429/529 within the total budget; everything else
    /// (including transport errors and timeouts) returns immediately. Every
    /// attempt is also cut off when the total budget runs out.
    pub async fn ask(
        &self,
        request: &PeerRequest,
        credential: &Credential,
    ) -> Result<BackendAnswer, ExternalError> {
        let body = build_body(request);
        let question_key = request.question_key;
        let started = Instant::now();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let remaining = self.limits.total_budget.saturating_sub(started.elapsed());
            let outcome =
                tokio::time::timeout(remaining, self.send_once(&body, credential, question_key))
                    .await
                    .unwrap_or(Err(AttemptOutcome::Terminal(ExternalError::BackendTimeout)));
            match outcome {
                Ok(answer) => return Ok(answer),
                Err(AttemptOutcome::Terminal(error)) => return Err(error),
                Err(AttemptOutcome::Retryable) => {
                    if attempt >= self.limits.max_attempts {
                        return Err(ExternalError::RateLimited);
                    }
                    let remaining = self.limits.total_budget.saturating_sub(started.elapsed());
                    if self.limits.retry_backoff > remaining {
                        return Err(ExternalError::RateLimited);
                    }
                    tokio::time::sleep(self.limits.retry_backoff).await;
                }
            }
        }
    }

    async fn send_once(
        &self,
        body: &Value,
        credential: &Credential,
        question_key: DecisionKind,
    ) -> Result<BackendAnswer, AttemptOutcome> {
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(credential.expose())
            .json(body)
            .send()
            .await
            .map_err(|error| AttemptOutcome::Terminal(transport_error(&error)))?;
        match classify_status(response.status()) {
            StatusOutcome::Retryable => return Err(AttemptOutcome::Retryable),
            StatusOutcome::Terminal(error) => return Err(AttemptOutcome::Terminal(error)),
            StatusOutcome::Success => {}
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.limits.max_response_bytes as u64)
        {
            return Err(AttemptOutcome::Terminal(ExternalError::ResponseTooLarge));
        }
        let mut stream = response.bytes_stream();
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| AttemptOutcome::Terminal(transport_error(&error)))?;
            if collected.len().saturating_add(chunk.len()) > self.limits.max_response_bytes {
                return Err(AttemptOutcome::Terminal(ExternalError::ResponseTooLarge));
            }
            collected.extend_from_slice(&chunk);
        }
        parse_response(&collected, question_key).map_err(AttemptOutcome::Terminal)
    }
}

/// Distinguishes a 429/529 attempt (may retry) from every other failure.
enum AttemptOutcome {
    Retryable,
    Terminal(ExternalError),
}

enum StatusOutcome {
    Success,
    Retryable,
    Terminal(ExternalError),
}

fn classify_status(status: reqwest::StatusCode) -> StatusOutcome {
    if status.is_success() {
        StatusOutcome::Success
    } else if status.is_redirection() {
        StatusOutcome::Terminal(ExternalError::RedirectRefused)
    } else if matches!(status.as_u16(), 401 | 403) {
        StatusOutcome::Terminal(ExternalError::Unauthorized)
    } else if matches!(status.as_u16(), 400 | 422) {
        StatusOutcome::Terminal(ExternalError::RequestInvalid)
    } else if matches!(status.as_u16(), 429 | 529) {
        StatusOutcome::Retryable
    } else {
        StatusOutcome::Terminal(ExternalError::HttpStatus)
    }
}

fn transport_error(error: &reqwest::Error) -> ExternalError {
    if error.is_timeout() {
        ExternalError::BackendTimeout
    } else {
        ExternalError::Transport
    }
}

/// Exactly the fields ADR 034 §4 allows: the request model, the enumerated
/// state, and one choice question built from the option ids/descriptions and
/// fixed instructions. Never the credential, `credential_env`, or
/// `schema_version`.
fn build_body(request: &PeerRequest) -> Value {
    let criteria: BTreeMap<&str, &str> = request
        .options
        .iter()
        .map(|option| (option.id.as_str(), option.description.as_str()))
        .collect();
    json!({
        "model": request.request_model,
        "state": request.state,
        "questions": {
            request.question_key.as_str(): {
                "type": JEV_QUESTION_TYPE,
                "instructions": request.instructions,
                "criteria": criteria,
            }
        }
    })
}

/// The answer object's shape is strict; unknown fields are malformed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JevAnswerWire {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    probabilities: BTreeMap<String, f64>,
    confidence: f64,
}

/// The top level is lenient (`usage` and other fields are ignored) but must
/// carry `model` and an `answers` object with exactly the question key.
fn parse_response(
    bytes: &[u8],
    question_key: DecisionKind,
) -> Result<BackendAnswer, ExternalError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| ExternalError::ResponseMalformed)?;
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .ok_or(ExternalError::ResponseMalformed)?;
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(ExternalError::ResponseMalformed)?;
    if answers.len() != 1 {
        return Err(ExternalError::ResponseMalformed);
    }
    let answer_value = answers
        .get(question_key.as_str())
        .ok_or(ExternalError::ResponseMalformed)?;
    let wire: JevAnswerWire = serde_json::from_value(answer_value.clone())
        .map_err(|_| ExternalError::ResponseMalformed)?;
    if wire.kind != JEV_QUESTION_TYPE {
        return Err(ExternalError::ResponseMalformed);
    }
    Ok(BackendAnswer {
        model: model.to_string(),
        choice: wire.choice,
        probabilities: wire.probabilities,
        confidence: wire.confidence,
    })
}

/// Loopback-only: exactly `127.0.0.1`, `::1`, or `localhost`, over `http`,
/// with an explicit port. Anything else is refused.
///
/// `Url::host_str` keeps the brackets on an IPv6 host (`"[::1]"`), unlike
/// the bracket-free form accepted in a request line.
#[cfg(feature = "test_endpoint")]
fn validate_loopback_endpoint(raw: &str) -> Result<reqwest::Url, ExternalError> {
    let url = reqwest::Url::parse(raw).map_err(|_| ExternalError::RequestInvalid)?;
    let allowed_host = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if url.scheme() != "http"
        || !allowed_host
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ExternalError::RequestInvalid);
    }
    Ok(url)
}

#[cfg(all(test, feature = "test_endpoint"))]
mod tests {
    use super::*;

    #[test]
    fn loopback_endpoints_are_accepted_and_others_refused() {
        for good in [
            "http://127.0.0.1:8080/v1/systemone",
            "http://[::1]:8080/v1/systemone",
            "http://localhost:8080/v1/systemone",
        ] {
            assert!(validate_loopback_endpoint(good).is_ok(), "{good}");
        }
        for bad in [
            "https://127.0.0.1:8080/v1/systemone",
            "http://example.invalid:8080/v1/systemone",
            "http://127.0.0.1/v1/systemone",
            "http://user:pass@127.0.0.1:8080/v1/systemone",
            "not a url",
        ] {
            assert_eq!(
                validate_loopback_endpoint(bad),
                Err(ExternalError::RequestInvalid),
                "{bad}"
            );
        }
    }

    #[test]
    fn credential_debug_is_redacted() {
        let credential = Credential::new("sk_live_super_secret");
        assert_eq!(format!("{credential:?}"), "Credential(<redacted>)");
    }
}
