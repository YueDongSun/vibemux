//! Control v6 dual-track workflow operations (ADR 031 §8).
//!
//! Every argument is a strict JSON document; a malformed one is
//! `control_invalid_request`, and a service refusal passes its
//! `workflow_*`, `writer_*`, `store_*`, or `harness_dispatch_*` code
//! through. Answers are content-free except the session prompt operation,
//! which returns the one prompt the operator explicitly asked for. A
//! prepare whose compiled contract is rejected answers with the
//! content-free violation codes instead of a bare error, so the operator
//! learns why.
use serde_json::{Value, json};
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_workflow::SpecIdentifier;

use super::*;
use crate::workflow::error::WorkflowError;

/// Deadline of the operations that compile, read Git, or create
/// worktrees.
pub const WORKFLOW_SLOW_DEADLINE: Duration = Duration::from_secs(120);
/// Deadline of a prompt evaluation, which runs whole fixture workflows.
pub const WORKFLOW_EVALUATION_DEADLINE: Duration = Duration::from_secs(3600);
/// Bound on every argument except a prepare's, which the frame bounds.
const MAX_LOOKUP_ARGUMENT_BYTES: usize = 1024;
/// Headroom for the response envelope around a payload.
const RESPONSE_ENVELOPE_BYTES: usize = 1024;
/// Why a prompt is answered without its text.
const PROMPT_EXCEEDS_FRAME_CODE: &str = "prompt_exceeds_control_frame";

/// Argument of the prepare operation: the raw request and policy
/// documents, validated by the service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPrepareArgument {
    pub request: Value,
    pub policy: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStartArgument {
    /// The `start_contract` digest `prepare` returned, as hex.
    pub contract: String,
    pub request_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLookup {
    pub workflow_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowExportQuery {
    pub workflow_id: Uuid,
    /// The previous page's `next`, or zero.
    pub cursor: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowShareArgument {
    pub bundle_id: Uuid,
    pub to_session: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSessionLookup {
    pub session_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPromptQuery {
    pub session_id: Uuid,
    pub index: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEvaluateArgument {
    /// Candidate set ID: `prompt_candidates/<candidate>.json` in the state
    /// directory.
    pub candidate: String,
    /// Suite ID: `prompt_suites/<suite>.json` in the state directory.
    pub suite: String,
}

impl ControlClient {
    /// Validates, compiles, and admits a workflow request. A rejected
    /// contract answers `{"outcome": "rejected", "violation_codes": ...}`.
    pub async fn workflow_prepare(
        &self,
        request: Value,
        policy: Value,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowPrepare,
            Some(&WorkflowPrepareArgument { request, policy }),
            WORKFLOW_SLOW_DEADLINE,
        )
        .await
    }

    pub async fn workflow_start(
        &self,
        contract: &str,
        request_id: Uuid,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowStart,
            Some(&WorkflowStartArgument {
                contract: contract.to_string(),
                request_id,
            }),
            WORKFLOW_SLOW_DEADLINE,
        )
        .await
    }

    pub async fn workflow_status(&self, workflow_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_lookup(ControlOperation::WorkflowStatus, workflow_id)
            .await
    }

    pub async fn workflow_pause(&self, workflow_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_lookup(ControlOperation::WorkflowPause, workflow_id)
            .await
    }

    pub async fn workflow_cancel(&self, workflow_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_lookup(ControlOperation::WorkflowCancel, workflow_id)
            .await
    }

    /// Deletes the workflow's opt-in retained texts.
    pub async fn workflow_purge_content(&self, workflow_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowPurgeContent,
            Some(&WorkflowLookup { workflow_id }),
            WORKFLOW_SLOW_DEADLINE,
        )
        .await
    }

    /// One page of the content-free export; pass the previous page's
    /// `next` as the cursor.
    pub async fn workflow_export(
        &self,
        workflow_id: Uuid,
        cursor: usize,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowExport,
            Some(&WorkflowExportQuery {
                workflow_id,
                cursor,
            }),
            CONTROL_DEADLINE,
        )
        .await
    }

    pub async fn workflow_slots(&self) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowSlots,
            None::<&()>,
            CONTROL_DEADLINE,
        )
        .await
    }

    pub async fn workflow_share(
        &self,
        bundle_id: Uuid,
        to_session: Uuid,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowShare,
            Some(&WorkflowShareArgument {
                bundle_id,
                to_session,
            }),
            CONTROL_DEADLINE,
        )
        .await
    }

    pub async fn workflow_session_inspect(&self, session_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowSessionInspect,
            Some(&WorkflowSessionLookup { session_id }),
            CONTROL_DEADLINE,
        )
        .await
    }

    /// Prompt `index` of a session; the last index is the next prompt.
    pub async fn workflow_session_prompt(
        &self,
        session_id: Uuid,
        index: usize,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowSessionPrompt,
            Some(&WorkflowPromptQuery { session_id, index }),
            CONTROL_DEADLINE,
        )
        .await
    }

    /// Always refused with `workflow_native_tui_unavailable` for a known
    /// session: sessions run as structured turns.
    pub async fn workflow_session_attach(&self, session_id: Uuid) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowSessionAttach,
            Some(&WorkflowSessionLookup { session_id }),
            CONTROL_DEADLINE,
        )
        .await
    }

    pub async fn workflow_policy_versions(&self) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowPolicyVersions,
            None::<&()>,
            CONTROL_DEADLINE,
        )
        .await
    }

    /// Runs one bounded optimizer cycle; waits up to
    /// [`WORKFLOW_EVALUATION_DEADLINE`].
    pub async fn workflow_policy_evaluate(
        &self,
        candidate: &str,
        suite: &str,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowPolicyEvaluate,
            Some(&WorkflowEvaluateArgument {
                candidate: candidate.to_string(),
                suite: suite.to_string(),
            }),
            WORKFLOW_EVALUATION_DEADLINE,
        )
        .await
    }

    pub async fn workflow_policy_rollback(&self) -> Result<Value, ControlError> {
        self.workflow_request(
            ControlOperation::WorkflowPolicyRollback,
            None::<&()>,
            CONTROL_DEADLINE,
        )
        .await
    }

    async fn workflow_lookup(
        &self,
        operation: ControlOperation,
        workflow_id: Uuid,
    ) -> Result<Value, ControlError> {
        self.workflow_request(
            operation,
            Some(&WorkflowLookup { workflow_id }),
            CONTROL_DEADLINE,
        )
        .await
    }

    async fn workflow_request<T: Serialize>(
        &self,
        operation: ControlOperation,
        argument: Option<&T>,
        deadline: Duration,
    ) -> Result<Value, ControlError> {
        self.require_operation(operation.clone())?;
        let encoded = argument
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| ControlError::InvalidRequest)?;
        match self
            .request_with_argument(operation, encoded, deadline)
            .await?
        {
            ControlPayload::Workflow(value) => Ok(*value),
            _ => Err(ControlError::InvalidFrame),
        }
    }
}

pub(super) async fn dispatch(
    state: &ServerState,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let service = &state.workflow;
    let value = match operation {
        ControlOperation::WorkflowPrepare => {
            // The frame already bounds the argument; the service bounds the
            // request and the policy.
            let argument = argument.ok_or(ControlError::InvalidRequest)?;
            let prepare: WorkflowPrepareArgument =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            match service.prepare(prepare.request, prepare.policy).await {
                Ok(response) => tagged("prepared", &response)?,
                Err(WorkflowError::ContractRejected { codes }) => json!({
                    "outcome": "rejected",
                    "error_code": WorkflowError::ContractRejected { codes: Vec::new() }.code(),
                    "violation_codes": codes,
                }),
                Err(error) => return Err(remote(&error)),
            }
        }
        ControlOperation::WorkflowStart => {
            let start: WorkflowStartArgument = parse_lookup(argument)?;
            let contract =
                Sha256Digest::from_hex(&start.contract).ok_or(ControlError::InvalidRequest)?;
            encode(service.start_workflow(contract, start.request_id).await)?
        }
        ControlOperation::WorkflowStatus => {
            let lookup: WorkflowLookup = parse_lookup(argument)?;
            encode(service.status(lookup.workflow_id).await)?
        }
        ControlOperation::WorkflowPause => {
            let lookup: WorkflowLookup = parse_lookup(argument)?;
            encode(service.pause(lookup.workflow_id).await)?
        }
        ControlOperation::WorkflowCancel => {
            let lookup: WorkflowLookup = parse_lookup(argument)?;
            encode(service.cancel(lookup.workflow_id).await)?
        }
        ControlOperation::WorkflowPurgeContent => {
            let lookup: WorkflowLookup = parse_lookup(argument)?;
            encode(service.purge_content(lookup.workflow_id).await)?
        }
        ControlOperation::WorkflowExport => {
            let query: WorkflowExportQuery = parse_lookup(argument)?;
            encode(service.export(query.workflow_id, query.cursor).await)?
        }
        ControlOperation::WorkflowSlots => {
            no_argument(argument)?;
            encode(service.slots().await)?
        }
        ControlOperation::WorkflowShare => {
            let share: WorkflowShareArgument = parse_lookup(argument)?;
            encode(service.share(share.bundle_id, share.to_session).await)?
        }
        ControlOperation::WorkflowSessionInspect => {
            let lookup: WorkflowSessionLookup = parse_lookup(argument)?;
            encode(service.inspect(lookup.session_id).await)?
        }
        ControlOperation::WorkflowSessionPrompt => {
            let query: WorkflowPromptQuery = parse_lookup(argument)?;
            let prompt = service
                .session_prompt(query.session_id, query.index)
                .await
                .map_err(|error| remote(&error))?;
            fit_prompt(serde_json::to_value(prompt).map_err(|_| ControlError::InvalidFrame)?)
        }
        ControlOperation::WorkflowSessionAttach => {
            let lookup: WorkflowSessionLookup = parse_lookup(argument)?;
            service
                .attach(lookup.session_id)
                .await
                .map_err(|error| remote(&error))?;
            // `attach` never succeeds today; a success would be a bug.
            return Err(ControlError::Remote {
                code: WorkflowError::Internal.code().to_string(),
            });
        }
        ControlOperation::WorkflowPolicyVersions => {
            no_argument(argument)?;
            encode(service.policy_versions().await.map(|records| {
                records
                    .into_iter()
                    .map(|record| record.version)
                    .collect::<Vec<_>>()
            }))?
        }
        ControlOperation::WorkflowPolicyRollback => {
            no_argument(argument)?;
            encode(service.rollback_policy().await.map(|record| record.version))?
        }
        ControlOperation::WorkflowPolicyEvaluate => {
            let evaluate: WorkflowEvaluateArgument = parse_lookup(argument)?;
            let identifier = |value: String| {
                SpecIdentifier::new(value).map_err(|_| ControlError::InvalidRequest)
            };
            let candidate = identifier(evaluate.candidate)?;
            let suite = identifier(evaluate.suite)?;
            encode(service.evaluate_prompt_policy(candidate, suite).await)?
        }
        _ => return Err(ControlError::InvalidRequest),
    };
    let payload = ControlPayload::Workflow(Box::new(value));
    // Export pages and prompts are sized to fit; this catches any other
    // oversized payload with a typed error instead of a dropped frame.
    if encoded_size(&payload)? > MAX_CONTROL_FRAME_BYTES - RESPONSE_ENVELOPE_BYTES {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(payload)
}

fn remote(error: &WorkflowError) -> ControlError {
    ControlError::Remote {
        code: error.code().to_string(),
    }
}

fn encode<T: Serialize>(result: Result<T, WorkflowError>) -> Result<Value, ControlError> {
    let value = result.map_err(|error| remote(&error))?;
    serde_json::to_value(value).map_err(|_| ControlError::InvalidFrame)
}

/// `value`'s fields with an `outcome` tag.
fn tagged<T: Serialize>(outcome: &str, value: &T) -> Result<Value, ControlError> {
    let mut value = serde_json::to_value(value).map_err(|_| ControlError::InvalidFrame)?;
    let object = value.as_object_mut().ok_or(ControlError::InvalidFrame)?;
    object.insert("outcome".to_string(), Value::from(outcome));
    Ok(value)
}

/// A prompt whose escaped encoding would not fit the frame is answered by
/// digest only.
fn fit_prompt(mut prompt: Value) -> Value {
    let size = encoded_size(&ControlPayload::Workflow(Box::new(prompt.clone())));
    let fits = size.is_ok_and(|size| size <= MAX_CONTROL_FRAME_BYTES - 2 * RESPONSE_ENVELOPE_BYTES);
    if !fits {
        if let Some(object) = prompt.as_object_mut() {
            object.insert("text".to_string(), Value::Null);
            object.insert(
                "unavailable_reason".to_string(),
                Value::from(PROMPT_EXCEEDS_FRAME_CODE),
            );
        }
    }
    prompt
}

fn encoded_size(payload: &ControlPayload) -> Result<usize, ControlError> {
    Ok(serde_json::to_vec(payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len())
}

fn no_argument(argument: Option<String>) -> Result<(), ControlError> {
    if argument.is_some() {
        return Err(ControlError::InvalidRequest);
    }
    Ok(())
}

fn parse_lookup<T: DeserializeOwned>(argument: Option<String>) -> Result<T, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > MAX_LOOKUP_ARGUMENT_BYTES {
        return Err(ControlError::InvalidRequest);
    }
    serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_arguments_are_strict_and_bounded() {
        let workflow_id = Uuid::new_v4();
        let valid = serde_json::to_string(&WorkflowLookup { workflow_id }).expect("json");
        let parsed: WorkflowLookup = parse_lookup(Some(valid)).expect("valid lookup");
        assert_eq!(parsed.workflow_id, workflow_id);
        let unknown = format!(r#"{{"workflow_id":"{workflow_id}","extra":1}}"#);
        let oversized = format!(
            r#"{{"workflow_id":"{workflow_id}"{}}}"#,
            " ".repeat(MAX_LOOKUP_ARGUMENT_BYTES)
        );
        for argument in [None, Some(unknown), Some(oversized), Some("{}".to_string())] {
            assert_eq!(
                parse_lookup::<WorkflowLookup>(argument),
                Err(ControlError::InvalidRequest)
            );
        }
    }

    #[test]
    fn an_oversized_prompt_is_answered_by_digest_only() {
        let fitting = json!({"text": "short", "unavailable_reason": null});
        assert_eq!(fit_prompt(fitting.clone()), fitting);
        // Control characters escape to six bytes each.
        let heavy = "\u{1}".repeat(16 * 1024);
        let fitted = fit_prompt(json!({"text": heavy, "unavailable_reason": null}));
        assert_eq!(fitted["text"], Value::Null);
        assert_eq!(fitted["unavailable_reason"], PROMPT_EXCEEDS_FRAME_CODE);
    }

    #[test]
    fn a_rejection_is_tagged_and_a_success_is_tagged() {
        let value = tagged("prepared", &json!({"workflow_id": "x"})).expect("tag");
        assert_eq!(value["outcome"], "prepared");
        assert!(tagged("prepared", &json!(1)).is_err());
    }
}
