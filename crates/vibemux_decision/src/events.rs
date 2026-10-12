//! Content-free canonical event drafts for decisions (ADR 034 §10).
//!
//! The two events hold the complete sanitized decision state, so the
//! `decision_records` projection can be rebuilt from them or verified
//! against them with [`rebuild_record`]. Payloads carry identities,
//! digests, sizes, option ids, allowlisted labels, millionths, model ids,
//! and fixed codes; the prompt appears only as its digest and size.
//! Idempotency keys are deterministic per decision and event kind, so a
//! retried writer command cannot append a second copy.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
use vibemux_harness::AgentKind;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_types::{EventId, ProjectId, RunId, TaskId};

use crate::error::DecisionError;
use crate::facts::PromptSizeBucket;
use crate::identifiers::{Label, OptionId};
use crate::kind::DecisionKind;
use crate::probability::Threshold;
use crate::record::DecisionRecord;
use crate::resolve::{DecisionSource, DecisionStatus, ExternalEvidence};

pub const DECISION_RECORDED_EVENT: &str = "decision_recorded";
pub const DECISION_CONSUMED_EVENT: &str = "decision_consumed";

const EVENT_ACTOR: &str = "vibemuxd";

/// Every field of a new, unconsumed [`DecisionRecord`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecordedPayload {
    pub schema_version: u32,
    pub decision_id: Uuid,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub chosen_harness: Option<AgentKind>,
    pub source: DecisionSource,
    pub rule_index: Option<u32>,
    pub options: Vec<OptionId>,
    pub labels: Vec<Label>,
    pub prompt_size: PromptSizeBucket,
    pub prompt_sha256: Sha256Digest,
    pub prompt_bytes: u64,
    pub project_default: Option<AgentKind>,
    pub threshold_millionths: Threshold,
    pub dispatch_config_sha256: Sha256Digest,
    pub decision_config_sha256: Sha256Digest,
    pub external: ExternalEvidence,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl DecisionRecordedPayload {
    /// Only an unconsumed record is recorded; its consumption arrives in a
    /// later [`DecisionConsumedPayload`].
    pub fn from_record(record: &DecisionRecord) -> Result<Self, DecisionError> {
        // Destructured so a new record field cannot be left out of the event.
        let DecisionRecord {
            schema_version,
            decision_id,
            kind,
            status,
            chosen_harness,
            source,
            rule_index,
            options,
            labels,
            prompt_size,
            prompt_sha256,
            prompt_bytes,
            project_default,
            threshold_millionths,
            dispatch_config_sha256,
            decision_config_sha256,
            external,
            created_at,
            expires_at,
            consumption,
        } = record.clone();
        if consumption.is_some() {
            return Err(DecisionError::Internal);
        }
        Ok(Self {
            schema_version,
            decision_id,
            kind,
            status,
            chosen_harness,
            source,
            rule_index,
            options,
            labels,
            prompt_size,
            prompt_sha256,
            prompt_bytes,
            project_default,
            threshold_millionths,
            dispatch_config_sha256,
            decision_config_sha256,
            external,
            created_at,
            expires_at,
        })
    }

    /// The validated, unconsumed record this payload describes.
    pub fn into_record(self) -> Result<DecisionRecord, DecisionError> {
        let record = DecisionRecord {
            schema_version: self.schema_version,
            decision_id: self.decision_id,
            kind: self.kind,
            status: self.status,
            chosen_harness: self.chosen_harness,
            source: self.source,
            rule_index: self.rule_index,
            options: self.options,
            labels: self.labels,
            prompt_size: self.prompt_size,
            prompt_sha256: self.prompt_sha256,
            prompt_bytes: self.prompt_bytes,
            project_default: self.project_default,
            threshold_millionths: self.threshold_millionths,
            dispatch_config_sha256: self.dispatch_config_sha256,
            decision_config_sha256: self.decision_config_sha256,
            external: self.external,
            created_at: self.created_at,
            expires_at: self.expires_at,
            consumption: None,
        };
        record.validate()?;
        Ok(record)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConsumedPayload {
    pub decision_id: Uuid,
    pub request_id: Uuid,
    pub harness: AgentKind,
    #[serde(with = "time::serde::rfc3339")]
    pub consumed_at: OffsetDateTime,
}

/// Rebuilds a decision record from its `decision_recorded` payload and, when
/// a dispatch consumed it, its `decision_consumed` payload. Payloads that
/// disagree with each other or with the record rules are `decision_internal`.
pub fn rebuild_record(
    recorded: DecisionRecordedPayload,
    consumed: Option<&DecisionConsumedPayload>,
) -> Result<DecisionRecord, DecisionError> {
    let mut record = recorded.into_record()?;
    if let Some(consumed) = consumed {
        if consumed.decision_id != record.decision_id
            || Some(consumed.harness) != record.chosen_harness
        {
            return Err(DecisionError::Internal);
        }
        record
            .mark_consumed(consumed.request_id, consumed.consumed_at)
            .map_err(|_| DecisionError::Internal)?;
        record.validate()?;
    }
    Ok(record)
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
    use std::collections::BTreeMap;

    use time::Duration;

    use super::*;
    use crate::answer::BackendAnswer;
    use crate::question::DecisionQuestion;
    use crate::record::NewDecision;
    use crate::record::tests::{digest, prompt, sample};
    use crate::resolve::{ExternalResult, resolve_route};

    const PINNED: &str = "jev-1.13.0";

    /// A decision with full external evidence: the backend answered below
    /// the threshold, so the project default applied.
    fn answered_sample() -> DecisionRecord {
        let eligible = [AgentKind::Claude, AgentKind::Codex];
        let threshold = Threshold::from_unit(0.9).unwrap();
        let question =
            DecisionQuestion::harness_route(&eligible, &BTreeMap::new(), threshold).unwrap();
        let answer = BackendAnswer {
            model: PINNED.to_owned(),
            choice: "codex".to_owned(),
            probabilities: [("claude", 0.05), ("codex", 0.9), ("escalate", 0.05)]
                .into_iter()
                .map(|(option, probability)| (option.to_owned(), probability))
                .collect(),
            confidence: 0.85,
        }
        .validate(&question, PINNED)
        .unwrap();
        let labels = vec![Label::new("rust").unwrap()];
        let resolution = resolve_route(
            &question,
            &eligible,
            &labels,
            Some(AgentKind::Claude),
            &[],
            ExternalResult::Answered(answer),
        );
        let record = DecisionRecord::new(
            NewDecision {
                decision_id: Uuid::new_v4(),
                kind: DecisionKind::HarnessRoute,
                options: question.option_ids(),
                labels,
                prompt: prompt(),
                project_default: Some(AgentKind::Claude),
                threshold,
                dispatch_config_sha256: digest(2),
                decision_config_sha256: digest(3),
                created_at: OffsetDateTime::UNIX_EPOCH + Duration::days(20_000),
            },
            resolution,
        );
        assert!(record.external.answer.is_some());
        record
    }

    fn consumption_of(record: &DecisionRecord) -> DecisionConsumedPayload {
        DecisionConsumedPayload {
            decision_id: record.decision_id,
            request_id: Uuid::new_v4(),
            harness: record.chosen_harness.expect("a resolved decision"),
            consumed_at: record.created_at + Duration::minutes(1),
        }
    }

    #[test]
    fn the_events_rebuild_the_projection_record() {
        for mut record in [
            sample(Some(AgentKind::Codex)),
            sample(None),
            answered_sample(),
        ] {
            let draft = recorded(ProjectId::new(), &record).unwrap();
            let payload: DecisionRecordedPayload =
                serde_json::from_value(draft.payload.value().clone()).unwrap();
            assert_eq!(rebuild_record(payload.clone(), None).unwrap(), record);
            if record.chosen_harness.is_some() {
                let consumption = consumption_of(&record);
                let context = ConsumedContext {
                    project_id: draft.project_id,
                    task_id: TaskId::new(),
                    run_id: RunId::new(),
                    timestamp: consumption.consumed_at,
                };
                let draft = consumed(&context, &consumption).unwrap();
                let consumption: DecisionConsumedPayload =
                    serde_json::from_value(draft.payload.value().clone()).unwrap();
                record
                    .mark_consumed(consumption.request_id, consumption.consumed_at)
                    .unwrap();
                assert_eq!(rebuild_record(payload, Some(&consumption)).unwrap(), record);
            }
        }
    }

    #[test]
    fn inconsistent_events_do_not_rebuild() {
        let record = answered_sample();
        let payload = DecisionRecordedPayload::from_record(&record).unwrap();

        let mut other_decision = consumption_of(&record);
        other_decision.decision_id = Uuid::new_v4();
        let mut other_harness = consumption_of(&record);
        other_harness.harness = AgentKind::Codex;
        for consumption in [other_decision, other_harness] {
            assert_eq!(
                rebuild_record(payload.clone(), Some(&consumption)),
                Err(DecisionError::Internal)
            );
        }

        let escalated = sample(None);
        let mut escalated_consumption = consumption_of(&record);
        escalated_consumption.decision_id = escalated.decision_id;
        assert_eq!(
            rebuild_record(
                DecisionRecordedPayload::from_record(&escalated).unwrap(),
                Some(&escalated_consumption)
            ),
            Err(DecisionError::Internal)
        );

        let mut stretched = payload;
        stretched.expires_at += Duration::minutes(1);
        assert_eq!(
            rebuild_record(stretched, None),
            Err(DecisionError::Internal)
        );

        let mut consumed_record = record.clone();
        consumed_record
            .mark_consumed(Uuid::new_v4(), record.created_at)
            .unwrap();
        assert_eq!(
            DecisionRecordedPayload::from_record(&consumed_record),
            Err(DecisionError::Internal)
        );
    }

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
        assert_eq!(payload.options, record.options);
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
                consumed_at: context.timestamp,
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
