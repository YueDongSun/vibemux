//! Content-free canonical event drafts for dispatch attempts (ADR 029 §3).
//!
//! Payloads carry identities, hashes, sizes, counts, and fixed codes only.
//! The prompt, vendor records, stderr, paths, arguments, and environment
//! never appear here; `vibemux_events` additionally rejects forbidden keys.
//! Idempotency keys are deterministic per request and event kind, so a
//! retried writer command cannot append a second copy.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
use vibemux_types::{EventId, ProjectId, RunId, TaskId};

use crate::AgentKind;

use super::{
    AttemptOutcome, DispatchError, DispatchPhase, NativeProtocol, ProcessExit, PromptDigest,
    Sha256Digest, capture_budget::CaptureSummary,
};

pub const HARNESS_DISPATCH_ADMITTED_EVENT: &str = "harness_dispatch_admitted";
pub const HARNESS_DISPATCH_STARTED_EVENT: &str = "harness_dispatch_started";
pub const HARNESS_DISPATCH_CANCEL_REQUESTED_EVENT: &str = "harness_dispatch_cancel_requested";
pub const HARNESS_DISPATCH_FINISHED_EVENT: &str = "harness_dispatch_finished";
pub const HARNESS_DISPATCH_RECOVERED_EVENT: &str = "harness_dispatch_recovered";

const EVENT_ACTOR: &str = "vibemuxd";

/// Canonical identities every dispatch event is attached to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchEventContext {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub request_id: Uuid,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedPayload {
    pub request_id: Uuid,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub prompt_sha256: Sha256Digest,
    pub prompt_bytes: u64,
    pub config_sha256: Sha256Digest,
    pub base_commit: String,
}

impl AdmittedPayload {
    #[must_use]
    pub fn new(
        request_id: Uuid,
        harness: AgentKind,
        protocol: NativeProtocol,
        prompt: PromptDigest,
        config_sha256: Sha256Digest,
        base_commit: String,
    ) -> Self {
        Self {
            request_id,
            harness,
            protocol,
            prompt_sha256: prompt.sha256,
            prompt_bytes: prompt.byte_count,
            config_sha256,
            base_commit,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartedPayload {
    pub request_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequestedPayload {
    pub request_id: Uuid,
    pub from_phase: DispatchPhase,
}

/// Process facts at finish; stderr is counted, never retained.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSummary {
    pub exit_code: Option<i32>,
    pub forced_termination: bool,
    pub stderr_bytes: u64,
}

impl ProcessSummary {
    #[must_use]
    pub const fn new(exit: ProcessExit, stderr_bytes: u64) -> Self {
        Self {
            exit_code: exit.exit_code,
            forced_termination: exit.forced_termination,
            stderr_bytes,
        }
    }
}

/// Terminal transition. `outcome`, `process`, and `capture` are absent when
/// no process ran under this attempt (cancel before claim) or when an
/// operator closes a recovery-pending attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FinishedPayload {
    pub request_id: Uuid,
    pub from_phase: DispatchPhase,
    pub phase: DispatchPhase,
    pub outcome: Option<AttemptOutcome>,
    pub error_code: Option<DispatchError>,
    pub process: Option<ProcessSummary>,
    pub capture: Option<CaptureSummary>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredPayload {
    pub request_id: Uuid,
    pub from_phase: DispatchPhase,
    pub phase: DispatchPhase,
    pub error_code: DispatchError,
}

pub fn admitted(
    context: &DispatchEventContext,
    payload: &AdmittedPayload,
) -> Result<EventDraft, DispatchError> {
    draft(HARNESS_DISPATCH_ADMITTED_EVENT, context, payload)
}

pub fn started(context: &DispatchEventContext) -> Result<EventDraft, DispatchError> {
    draft(
        HARNESS_DISPATCH_STARTED_EVENT,
        context,
        &StartedPayload {
            request_id: context.request_id,
        },
    )
}

pub fn cancel_requested(
    context: &DispatchEventContext,
    from_phase: DispatchPhase,
) -> Result<EventDraft, DispatchError> {
    draft(
        HARNESS_DISPATCH_CANCEL_REQUESTED_EVENT,
        context,
        &CancelRequestedPayload {
            request_id: context.request_id,
            from_phase,
        },
    )
}

pub fn finished(
    context: &DispatchEventContext,
    payload: &FinishedPayload,
) -> Result<EventDraft, DispatchError> {
    draft(HARNESS_DISPATCH_FINISHED_EVENT, context, payload)
}

pub fn recovered(
    context: &DispatchEventContext,
    payload: &RecoveredPayload,
) -> Result<EventDraft, DispatchError> {
    draft(HARNESS_DISPATCH_RECOVERED_EVENT, context, payload)
}

/// `harness_dispatch:<request uuid>:<event suffix>`; at most one per kind.
#[must_use]
pub fn idempotency_key(request_id: Uuid, event_type: &str) -> String {
    let suffix = event_type
        .strip_prefix("harness_dispatch_")
        .unwrap_or(event_type);
    format!("harness_dispatch:{}:{suffix}", request_id.as_hyphenated())
}

fn draft(
    event_type: &str,
    context: &DispatchEventContext,
    payload: &impl Serialize,
) -> Result<EventDraft, DispatchError> {
    let payload = serde_json::to_value(payload).map_err(|_| DispatchError::Internal)?;
    let draft = EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new(event_type).map_err(|_| DispatchError::Internal)?,
        project_id: context.project_id,
        task_id: Some(context.task_id),
        run_id: Some(context.run_id),
        causation_id: None,
        actor: ActorName::new(EVENT_ACTOR).map_err(|_| DispatchError::Internal)?,
        timestamp: context.timestamp,
        idempotency_key: Some(idempotency_key(context.request_id, event_type)),
        payload: EventPayload::new(payload).map_err(|_| DispatchError::Internal)?,
    };
    draft.validate().map_err(|_| DispatchError::Internal)?;
    Ok(draft)
}
