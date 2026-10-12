//! The persisted, content-free decision record and its dispatch binding
//! rules (ADR 034 §10–§11).

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vibemux_harness::AgentKind;
use vibemux_harness::dispatch::{PromptDigest, Sha256Digest};

use crate::error::DecisionError;
use crate::facts::PromptSizeBucket;
use crate::identifiers::{Label, OptionId};
use crate::kind::DecisionKind;
use crate::probability::Threshold;
use crate::resolve::{DecisionSource, DecisionStatus, ExternalEvidence, RouteResolution};

pub const DECISION_RECORD_SCHEMA_VERSION: u32 = 1;
/// A decision older than this can no longer start a dispatch.
pub const DECISION_TTL: Duration = Duration::minutes(15);

/// Inputs fixed when the decision is made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDecision {
    pub decision_id: Uuid,
    pub kind: DecisionKind,
    pub options: Vec<OptionId>,
    pub labels: Vec<Label>,
    pub prompt: PromptDigest,
    pub project_default: Option<AgentKind>,
    pub threshold: Threshold,
    pub dispatch_config_sha256: Sha256Digest,
    pub decision_config_sha256: Sha256Digest,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConsumption {
    pub request_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub consumed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
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
    pub consumption: Option<DecisionConsumption>,
}

/// How a decided dispatch request relates to the decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchBinding {
    /// First use: admit the dispatch and mark the decision consumed.
    Fresh(AgentKind),
    /// The same request already consumed it: admission replays idempotently.
    Replay(AgentKind),
}

impl DecisionRecord {
    #[must_use]
    pub fn new(input: NewDecision, resolution: RouteResolution) -> Self {
        Self {
            schema_version: DECISION_RECORD_SCHEMA_VERSION,
            decision_id: input.decision_id,
            kind: input.kind,
            status: resolution.status,
            chosen_harness: resolution.chosen_harness,
            source: resolution.source,
            rule_index: resolution.rule_index,
            options: input.options,
            labels: input.labels,
            prompt_size: PromptSizeBucket::from_bytes(input.prompt.byte_count),
            prompt_sha256: input.prompt.sha256,
            prompt_bytes: input.prompt.byte_count,
            project_default: input.project_default,
            threshold_millionths: input.threshold,
            dispatch_config_sha256: input.dispatch_config_sha256,
            decision_config_sha256: input.decision_config_sha256,
            external: resolution.external,
            created_at: input.created_at,
            expires_at: input.created_at + DECISION_TTL,
            consumption: None,
        }
    }

    /// Internal consistency, checked before every write and after every read.
    pub fn validate(&self) -> Result<(), DecisionError> {
        let consistent = self.schema_version == DECISION_RECORD_SCHEMA_VERSION
            && self.expires_at == self.created_at + DECISION_TTL
            && match self.status {
                DecisionStatus::Resolved => self
                    .chosen_harness
                    .is_some_and(|harness| self.options.contains(&OptionId::harness(harness))),
                DecisionStatus::Escalated => {
                    self.chosen_harness.is_none() && self.consumption.is_none()
                }
            }
            && (self.source == DecisionSource::LocalRule) == self.rule_index.is_some()
            && self.options.iter().any(OptionId::is_escalate);
        if consistent {
            Ok(())
        } else {
            Err(DecisionError::Internal)
        }
    }

    /// A wall clock behind the creation time counts as expired too, so a
    /// backward clock step cannot stretch the window past [`DECISION_TTL`].
    #[must_use]
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        now < self.created_at || now >= self.expires_at
    }

    /// Checks whether `request_id` may dispatch `prompt` under this decision
    /// with the currently pinned dispatch config.
    pub fn check_dispatchable(
        &self,
        request_id: Uuid,
        prompt: PromptDigest,
        dispatch_config_sha256: Sha256Digest,
        now: OffsetDateTime,
    ) -> Result<DispatchBinding, DecisionError> {
        self.validate()?;
        if let Some(consumption) = self.consumption {
            if consumption.request_id != request_id {
                return Err(DecisionError::AlreadyConsumed);
            }
        }
        let harness = match (self.status, self.chosen_harness) {
            (DecisionStatus::Escalated, _) => return Err(DecisionError::Escalated),
            (DecisionStatus::Resolved, Some(harness)) => harness,
            (DecisionStatus::Resolved, None) => return Err(DecisionError::Internal),
        };
        if self.prompt_sha256 != prompt.sha256 || self.prompt_bytes != prompt.byte_count {
            return Err(DecisionError::PromptMismatch);
        }
        if self.consumption.is_some() {
            // The decision side lets a replay through after expiry or a
            // re-pin; the dispatch admission's own idempotency and request
            // fingerprint then decide whether the original receipt returns.
            return Ok(DispatchBinding::Replay(harness));
        }
        if self.is_expired(now) {
            return Err(DecisionError::Expired);
        }
        if self.dispatch_config_sha256 != dispatch_config_sha256 {
            return Err(DecisionError::ConfigChanged);
        }
        Ok(DispatchBinding::Fresh(harness))
    }

    pub fn mark_consumed(
        &mut self,
        request_id: Uuid,
        consumed_at: OffsetDateTime,
    ) -> Result<(), DecisionError> {
        if self.status != DecisionStatus::Resolved {
            return Err(DecisionError::Escalated);
        }
        if self.consumption.is_some() {
            return Err(DecisionError::AlreadyConsumed);
        }
        self.consumption = Some(DecisionConsumption {
            request_id,
            consumed_at,
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::question::DecisionQuestion;
    use crate::resolve::{ExternalResult, SkipReason, resolve_route};

    pub(crate) fn digest(byte: u8) -> Sha256Digest {
        Sha256Digest::of(&[byte])
    }

    pub(crate) fn prompt() -> PromptDigest {
        PromptDigest {
            sha256: digest(1),
            byte_count: 42,
        }
    }

    pub(crate) fn sample(default: Option<AgentKind>) -> DecisionRecord {
        let eligible = [AgentKind::Claude, AgentKind::Codex];
        let threshold = Threshold::from_unit(0.9).unwrap();
        let question =
            DecisionQuestion::harness_route(&eligible, &BTreeMap::new(), threshold).unwrap();
        let resolution = resolve_route(
            &question,
            &eligible,
            &[],
            default,
            &[],
            ExternalResult::Skipped(SkipReason::NotRequested),
        );
        DecisionRecord::new(
            NewDecision {
                decision_id: Uuid::new_v4(),
                kind: DecisionKind::HarnessRoute,
                options: question.option_ids(),
                labels: Vec::new(),
                prompt: prompt(),
                project_default: default,
                threshold,
                dispatch_config_sha256: digest(2),
                decision_config_sha256: digest(3),
                created_at: OffsetDateTime::UNIX_EPOCH + Duration::days(20_000),
            },
            resolution,
        )
    }

    #[test]
    fn a_new_record_is_consistent_and_round_trips() {
        let record = sample(Some(AgentKind::Codex));
        record.validate().unwrap();
        assert_eq!(record.expires_at - record.created_at, DECISION_TTL);
        let json = serde_json::to_string(&record).unwrap();
        let back: DecisionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn binding_rules() {
        let record = sample(Some(AgentKind::Codex));
        let now = record.created_at + Duration::minutes(1);
        let request = Uuid::new_v4();
        assert_eq!(
            record.check_dispatchable(request, prompt(), digest(2), now),
            Ok(DispatchBinding::Fresh(AgentKind::Codex))
        );
        let other_prompt = PromptDigest {
            sha256: digest(9),
            byte_count: 42,
        };
        assert_eq!(
            record.check_dispatchable(request, other_prompt, digest(2), now),
            Err(DecisionError::PromptMismatch)
        );
        assert_eq!(
            record.check_dispatchable(request, prompt(), digest(8), now),
            Err(DecisionError::ConfigChanged)
        );
        assert_eq!(
            record.check_dispatchable(request, prompt(), digest(2), record.expires_at),
            Err(DecisionError::Expired)
        );
        assert_eq!(
            record.check_dispatchable(
                request,
                prompt(),
                digest(2),
                record.created_at - Duration::seconds(1)
            ),
            Err(DecisionError::Expired),
            "a clock stepped behind the creation time does not extend the window"
        );

        let mut consumed = record.clone();
        consumed.mark_consumed(request, now).unwrap();
        assert_eq!(
            consumed.check_dispatchable(request, prompt(), digest(8), record.expires_at),
            Ok(DispatchBinding::Replay(AgentKind::Codex)),
            "the decision side passes a replay after expiry and a re-pin"
        );
        assert_eq!(
            consumed.check_dispatchable(Uuid::new_v4(), prompt(), digest(2), now),
            Err(DecisionError::AlreadyConsumed)
        );
        assert_eq!(
            consumed.mark_consumed(Uuid::new_v4(), now),
            Err(DecisionError::AlreadyConsumed)
        );
    }

    #[test]
    fn an_escalated_decision_never_dispatches() {
        let record = sample(None);
        assert_eq!(record.status, DecisionStatus::Escalated);
        let now = record.created_at;
        assert_eq!(
            record.check_dispatchable(Uuid::new_v4(), prompt(), digest(2), now),
            Err(DecisionError::Escalated)
        );
        let mut escalated = record;
        assert_eq!(
            escalated.mark_consumed(Uuid::new_v4(), now),
            Err(DecisionError::Escalated)
        );
    }

    #[test]
    fn tampered_records_fail_validation() {
        let mut no_harness = sample(Some(AgentKind::Codex));
        no_harness.chosen_harness = None;
        assert_eq!(no_harness.validate(), Err(DecisionError::Internal));
        let mut foreign = sample(Some(AgentKind::Codex));
        foreign.chosen_harness = Some(AgentKind::Grok);
        assert_eq!(foreign.validate(), Err(DecisionError::Internal));
        let mut ttl = sample(Some(AgentKind::Codex));
        ttl.expires_at += Duration::hours(1);
        assert_eq!(ttl.validate(), Err(DecisionError::Internal));
    }
}
