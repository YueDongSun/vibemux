//! Content-free canonical event drafts for decisions (ADR 034 §10).
//!
//! Payloads carry identities, digests, sizes, counts, option ids,
//! millionths, and fixed codes only. Idempotency keys are deterministic per
//! decision and event kind, so a retried writer command cannot append a
//! second copy.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
use vibemux_harness::AgentKind;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_types::{EventId, ProjectId, RunId, TaskId};

use crate::error::{DecisionError, ExternalError};
use crate::identifiers::OptionId;
use crate::kind::DecisionKind;
use crate::probability::{Millionths, Threshold};
use crate::record::DecisionRecord;
use crate::resolve::{DecisionSource, DecisionStatus, ExternalStatus};

pub const DECISION_RECORDED_EVENT: &str = "decision_recorded";
pub const DECISION_CONSUMED_EVENT: &str = "decision_consumed";

const EVENT_ACTOR: &str = "vibemuxd";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecordedPayload {
    pub decision_id: Uuid,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub chosen_harness: Option<AgentKind>,
    pub source: DecisionSource,
    pub rule_index: Option<u32>,
    pub option_count: u32,
    pub label_count: u32,
    pub prompt_sha256: Sha256Digest,
    pub prompt_bytes: u64,
    pub threshold_millionths: Threshold,
    pub external_status: ExternalStatus,
    pub external_error: Option<ExternalError>,
    pub external_choice: Option<OptionId>,
    pub gated_confidence_millionths: Option<Millionths>,
    pub dispatch_config_sha256: Sha256Digest,
    pub decision_config_sha256: Sha256Digest,
}

impl DecisionRecordedPayload {
    pub fn from_record(record: &DecisionRecord) -> Result<Self, DecisionError> {
        let answer = record.external.answer.as_ref();
        Ok(Self {
            decision_id: record.decision_id,
            kind: record.kind,
            status: record.status,
            chosen_harness: record.chosen_harness,
            source: record.source,
            rule_index: record.rule_index,
            option_count: u32::try_from(record.options.len())
                .map_err(|_| DecisionError::Internal)?,
            label_count: u32::try_from(record.labels.len()).map_err(|_| DecisionError::Internal)?,
            prompt_sha256: record.prompt_sha256,
            prompt_bytes: record.prompt_bytes,
            threshold_millionths: record.threshold_millionths,
            external_status: record.external.status,
            external_error: record.external.error,
            external_choice: answer.map(|answer| answer.choice.clone()),
            gated_confidence_millionths: answer.map(|answer| answer.gated_confidence_millionths),
            dispatch_config_sha256: record.dispatch_config_sha256,
            decision_config_sha256: record.decision_config_sha256,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConsumedPayload {
    pub decision_id: Uuid,
    pub request_id: Uuid,
    pub harness: AgentKind,
}

/// Identities of the dispatch a decision was consumed by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConsumedContext {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub timestamp: OffsetDateTime,
}

/// Project-level event; a decision has no task or run of its own.
pub fn recorded(
    project_id: ProjectId,
    record: &DecisionRecord,
) -> Result<EventDraft, DecisionError> {
    draft(
        DECISION_RECORDED_EVENT,
        record.decision_id,
        project_id,
        None,
        record.created_at,
        &DecisionRecordedPayload::from_record(record)?,
    )
}

/// Attached to the dispatch's task and run.
pub fn consumed(
    context: &ConsumedContext,
    payload: &DecisionConsumedPayload,
) -> Result<EventDraft, DecisionError> {
    draft(
        DECISION_CONSUMED_EVENT,
        payload.decision_id,
        context.project_id,
        Some((context.task_id, context.run_id)),
        context.timestamp,
        payload,
    )
}

/// `decision:<decision uuid>:<event suffix>`; at most one per kind.
#[must_use]
pub fn idempotency_key(decision_id: Uuid, event_type: &str) -> String {
    let suffix = event_type.strip_prefix("decision_").unwrap_or(event_type);
    format!("decision:{}:{suffix}", decision_id.as_hyphenated())
}

fn draft(
    event_type: &str,
    decision_id: Uuid,
    project_id: ProjectId,
    run: Option<(TaskId, RunId)>,
    timestamp: OffsetDateTime,
    payload: &impl Serialize,
) -> Result<EventDraft, DecisionError> {
    let payload = serde_json::to_value(payload).map_err(|_| DecisionError::Internal)?;
    let draft = EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new(event_type).map_err(|_| DecisionError::Internal)?,
        project_id,
        task_id: run.map(|(task_id, _)| task_id),
        run_id: run.map(|(_, run_id)| run_id),
        causation_id: None,
        actor: ActorName::new(EVENT_ACTOR).map_err(|_| DecisionError::Internal)?,
        timestamp,
        idempotency_key: Some(idempotency_key(decision_id, event_type)),
        payload: EventPayload::new(payload).map_err(|_| DecisionError::Internal)?,
    };
    draft.validate().map_err(|_| DecisionError::Internal)?;
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::sample;

    #[test]
    fn recorded_event_is_valid_project_level_and_idempotent() {
        let record = sample(Some(AgentKind::Codex));
        let project_id = ProjectId::new();
        let draft = recorded(project_id, &record).unwrap();
        assert_eq!(draft.task_id, None);
        assert_eq!(draft.run_id, None);
        assert_eq!(
            draft.idempotency_key.as_deref(),
            Some(idempotency_key(record.decision_id, DECISION_RECORDED_EVENT).as_str())
        );
        let again = recorded(project_id, &record).unwrap();
        assert_eq!(draft.idempotency_key, again.idempotency_key);
        let payload: DecisionRecordedPayload =
            serde_json::from_value(draft.payload.value().clone()).unwrap();
        assert_eq!(payload.chosen_harness, Some(AgentKind::Codex));
        assert_eq!(payload.option_count, 3);
    }

    #[test]
    fn consumed_event_attaches_to_the_dispatch_run() {
        let record = sample(Some(AgentKind::Codex));
        let context = ConsumedContext {
            project_id: ProjectId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
            timestamp: record.created_at,
        };
        let draft = consumed(
            &context,
            &DecisionConsumedPayload {
                decision_id: record.decision_id,
                request_id: Uuid::new_v4(),
                harness: AgentKind::Codex,
            },
        )
        .unwrap();
        assert_eq!(draft.task_id, Some(context.task_id));
        assert_eq!(draft.run_id, Some(context.run_id));
        assert!(draft.idempotency_key.unwrap().ends_with(":consumed"));
    }

    #[test]
    fn the_full_record_also_passes_the_forbidden_key_check() {
        // The projection row stores the whole record; keep it event-clean too.
        let record = sample(Some(AgentKind::Codex));
        let value = serde_json::to_value(&record).unwrap();
        assert!(EventPayload::new(value).is_ok());
    }
}
