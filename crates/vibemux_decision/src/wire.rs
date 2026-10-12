//! The one-shot line protocol between `vibemuxd` and
//! `vibemux_decision_peer` (ADR 034 §6).
//!
//! The daemon writes one [`PeerRequest`] line to the peer's stdin and reads
//! one [`PeerResult`] line from its stdout. The request names the credential
//! variable; its value reaches the peer only through the peer's environment.

use serde::{Deserialize, Serialize};

use crate::answer::BackendAnswer;
use crate::config::{JevConfig, is_valid_credential_env, is_valid_model};
use crate::error::ExternalError;
use crate::facts::RouteFacts;
use crate::kind::DecisionKind;
use crate::question::{
    DecisionQuestion, MAX_QUESTION_OPTIONS, MIN_QUESTION_OPTIONS, QuestionOption,
    is_valid_description,
};

pub const PEER_WIRE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PEER_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_PEER_RESULT_BYTES: usize = 16 * 1024;
/// Peer exit code for a failure it could not report on stdout.
pub const PEER_FAILURE_EXIT_CODE: i32 = 4;
pub const MAX_INSTRUCTIONS_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionBackend {
    /// TypeSafe Jev System One choice questions.
    Jev,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRequest {
    pub schema_version: u32,
    pub backend: DecisionBackend,
    pub request_model: String,
    /// The variable's name; never its value.
    pub credential_env: String,
    /// The question key in the provider request; the decision kind.
    pub question_key: DecisionKind,
    pub instructions: String,
    pub options: Vec<QuestionOption>,
    pub state: RouteFacts,
}

impl PeerRequest {
    #[must_use]
    pub fn for_route(question: &DecisionQuestion, facts: &RouteFacts, jev: &JevConfig) -> Self {
        Self {
            schema_version: PEER_WIRE_SCHEMA_VERSION,
            backend: DecisionBackend::Jev,
            request_model: jev.request_model.clone(),
            credential_env: jev.credential_env.clone(),
            question_key: question.kind(),
            instructions: question.instructions().to_string(),
            options: question.options().to_vec(),
            state: facts.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), ExternalError> {
        let options_valid = (MIN_QUESTION_OPTIONS..=MAX_QUESTION_OPTIONS)
            .contains(&self.options.len())
            && self
                .options
                .iter()
                .all(|option| is_valid_description(&option.description));
        let mut ids: Vec<_> = self.options.iter().map(|option| &option.id).collect();
        ids.sort();
        ids.dedup();
        if self.schema_version != PEER_WIRE_SCHEMA_VERSION
            || !is_valid_model(&self.request_model)
            || !is_valid_credential_env(&self.credential_env)
            || self.instructions.trim().is_empty()
            || self.instructions.len() > MAX_INSTRUCTIONS_BYTES
            || !options_valid
            || ids.len() != self.options.len()
        {
            return Err(ExternalError::RequestInvalid);
        }
        Ok(())
    }

    /// One JSON line, newline-terminated, within the size limit.
    pub fn encode_line(&self) -> Result<Vec<u8>, ExternalError> {
        self.validate()?;
        encode_line(self, MAX_PEER_REQUEST_BYTES, ExternalError::RequestInvalid)
    }

    pub fn decode_line(line: &[u8]) -> Result<Self, ExternalError> {
        let request: Self =
            decode_line(line, MAX_PEER_REQUEST_BYTES).map_err(|_| ExternalError::RequestInvalid)?;
        request.validate()?;
        Ok(request)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResultStatus {
    Answered,
    Failed,
}

/// Exactly one of `answer` (when answered) and `error` (when failed).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerResult {
    pub schema_version: u32,
    pub status: PeerResultStatus,
    pub answer: Option<BackendAnswer>,
    pub error: Option<ExternalError>,
}

impl PeerResult {
    #[must_use]
    pub const fn answered(answer: BackendAnswer) -> Self {
        Self {
            schema_version: PEER_WIRE_SCHEMA_VERSION,
            status: PeerResultStatus::Answered,
            answer: Some(answer),
            error: None,
        }
    }

    #[must_use]
    pub const fn failed(error: ExternalError) -> Self {
        Self {
            schema_version: PEER_WIRE_SCHEMA_VERSION,
            status: PeerResultStatus::Failed,
            answer: None,
            error: Some(error),
        }
    }

    pub fn encode_line(&self) -> Result<Vec<u8>, ExternalError> {
        encode_line(self, MAX_PEER_RESULT_BYTES, ExternalError::PeerProtocol)
    }

    /// Decodes and checks the shape; the answer itself is validated later
    /// against the question.
    pub fn decode_line(line: &[u8]) -> Result<Result<BackendAnswer, ExternalError>, ExternalError> {
        let result: Self =
            decode_line(line, MAX_PEER_RESULT_BYTES).map_err(|_| ExternalError::PeerProtocol)?;
        if result.schema_version != PEER_WIRE_SCHEMA_VERSION {
            return Err(ExternalError::PeerProtocol);
        }
        match (result.status, result.answer, result.error) {
            (PeerResultStatus::Answered, Some(answer), None) => Ok(Ok(answer)),
            (PeerResultStatus::Failed, None, Some(error)) => Ok(Err(error)),
            _ => Err(ExternalError::PeerProtocol),
        }
    }
}

fn encode_line(
    value: &impl Serialize,
    limit: usize,
    error: ExternalError,
) -> Result<Vec<u8>, ExternalError> {
    let mut line = serde_json::to_vec(value).map_err(|_| error)?;
    line.push(b'\n');
    if line.len() > limit {
        return Err(error);
    }
    Ok(line)
}

fn decode_line<T: for<'de> Deserialize<'de>>(line: &[u8], limit: usize) -> Result<T, ()> {
    if line.len() > limit {
        return Err(());
    }
    let trimmed = line
        .strip_suffix(b"\n")
        .map_or(line, |rest| rest.strip_suffix(b"\r").unwrap_or(rest));
    if trimmed.contains(&b'\n') {
        return Err(());
    }
    serde_json::from_slice(trimmed).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use vibemux_harness::AgentKind;

    use super::*;
    use crate::identifiers::Label;
    use crate::probability::Threshold;

    fn request() -> PeerRequest {
        let eligible = [AgentKind::Claude, AgentKind::Codex];
        let question = DecisionQuestion::harness_route(
            &eligible,
            &BTreeMap::new(),
            Threshold::from_unit(0.9).unwrap(),
        )
        .unwrap();
        let facts = RouteFacts::new(
            vec![Label::new("rust").unwrap()],
            2048,
            Some(AgentKind::Codex),
            &eligible,
        );
        PeerRequest::for_route(&question, &facts, &JevConfig::default())
    }

    #[test]
    fn a_request_round_trips_as_one_line_and_carries_no_credential_value() {
        let request = request();
        let line = request.encode_line().unwrap();
        assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(PeerRequest::decode_line(&line).unwrap(), request);
        let text = String::from_utf8(line).unwrap();
        assert!(text.contains("\"credential_env\":\"TYPESAFE_API_KEY\""));
        assert!(!text.contains("prompt\""), "no prompt field");
    }

    #[test]
    fn invalid_requests_are_refused() {
        let mut bad_env = request();
        bad_env.credential_env = "sk-live".into();
        assert_eq!(bad_env.encode_line(), Err(ExternalError::RequestInvalid));
        let mut one_option = request();
        one_option.options.truncate(1);
        assert_eq!(one_option.validate(), Err(ExternalError::RequestInvalid));
        let mut duplicate = request();
        duplicate.options[1] = duplicate.options[0].clone();
        assert_eq!(duplicate.validate(), Err(ExternalError::RequestInvalid));
        assert_eq!(
            PeerRequest::decode_line(b"{\"schema_version\":1}\n"),
            Err(ExternalError::RequestInvalid)
        );
        let oversized = vec![b' '; MAX_PEER_REQUEST_BYTES + 1];
        assert_eq!(
            PeerRequest::decode_line(&oversized),
            Err(ExternalError::RequestInvalid)
        );
    }

    #[test]
    fn results_decode_only_in_their_two_shapes() {
        let answer = BackendAnswer {
            model: "jev-1.13.0".into(),
            choice: "codex".into(),
            probabilities: BTreeMap::from([("codex".to_string(), 1.0)]),
            confidence: 1.0,
        };
        let answered = PeerResult::answered(answer.clone()).encode_line().unwrap();
        assert_eq!(PeerResult::decode_line(&answered), Ok(Ok(answer)));
        let failed = PeerResult::failed(ExternalError::RateLimited)
            .encode_line()
            .unwrap();
        assert_eq!(
            PeerResult::decode_line(&failed),
            Ok(Err(ExternalError::RateLimited))
        );
        for bad in [
            &br#"{"schema_version":1,"status":"answered","answer":null,"error":null}"#[..],
            br#"{"schema_version":1,"status":"failed","answer":null,"error":"transport","x":1}"#,
            br#"{"schema_version":2,"status":"failed","answer":null,"error":"transport"}"#,
            b"{}\n{}\n",
            b"not json",
        ] {
            assert_eq!(
                PeerResult::decode_line(bad),
                Err(ExternalError::PeerProtocol)
            );
        }
    }
}
