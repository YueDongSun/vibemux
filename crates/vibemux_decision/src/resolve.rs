//! The deterministic fallback chain (ADR 034 §9).
//!
//! 1. An accepted external answer at or above τ: apply it (`escalate`
//!    escalates).
//! 2. The highest-priority matching local rule.
//! 3. The project default harness if eligible.
//! 4. Escalate to the operator.
//!
//! External evidence is kept separate from the applied choice.

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::answer::ValidatedAnswer;
use crate::error::ExternalError;
use crate::identifiers::Label;
use crate::question::DecisionQuestion;
use crate::rules::{LocalRule, match_rule};
use crate::wire::DecisionBackend;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// Names a harness the operator may dispatch to.
    Resolved,
    /// Names no harness; the operator chooses.
    Escalated,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    SingleOption,
    External,
    LocalRule,
    ProjectDefault,
    /// Nothing applied; the decision escalated.
    None,
}

/// Why no backend was called.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Exactly one harness is eligible.
    SingleOption,
    /// The request did not allow an external call.
    NotRequested,
    /// The decision config does not enable the backend.
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalStatus {
    SingleOption,
    NotRequested,
    Disabled,
    /// The call failed before an answer arrived.
    Failed,
    /// An answer arrived but failed validation.
    Rejected,
    /// A valid answer below τ.
    BelowThreshold,
    /// A valid answer at or above τ; it was applied.
    Accepted,
}

/// What the daemon obtained from the external backend, if anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalResult {
    Skipped(SkipReason),
    Failed(ExternalError),
    Answered(ValidatedAnswer),
}

/// Content-free external evidence, stored apart from the applied choice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEvidence {
    pub status: ExternalStatus,
    pub backend: Option<DecisionBackend>,
    pub error: Option<ExternalError>,
    pub answer: Option<ValidatedAnswer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteResolution {
    pub status: DecisionStatus,
    pub chosen_harness: Option<AgentKind>,
    pub source: DecisionSource,
    /// Config index of the applied local rule.
    pub rule_index: Option<u32>,
    pub external: ExternalEvidence,
}

/// Whether to call the backend. `None` means call it.
#[must_use]
pub const fn plan_external(
    eligible_count: usize,
    allow_external: bool,
    backend_enabled: bool,
) -> Option<SkipReason> {
    if eligible_count == 1 {
        Some(SkipReason::SingleOption)
    } else if !allow_external {
        Some(SkipReason::NotRequested)
    } else if !backend_enabled {
        Some(SkipReason::Disabled)
    } else {
        None
    }
}

/// Resolves a `harness_route` question. `eligible` must be the harnesses the
/// question was built from.
#[must_use]
pub fn resolve_route(
    question: &DecisionQuestion,
    eligible: &[AgentKind],
    labels: &[Label],
    project_default: Option<AgentKind>,
    rules: &[LocalRule],
    external: ExternalResult,
) -> RouteResolution {
    let backend = Some(DecisionBackend::Jev);
    let evidence = match external {
        ExternalResult::Skipped(reason) => ExternalEvidence {
            status: match reason {
                SkipReason::SingleOption => ExternalStatus::SingleOption,
                SkipReason::NotRequested => ExternalStatus::NotRequested,
                SkipReason::Disabled => ExternalStatus::Disabled,
            },
            backend: None,
            error: None,
            answer: None,
        },
        ExternalResult::Failed(error) => ExternalEvidence {
            status: if error.is_answer_rejection() {
                ExternalStatus::Rejected
            } else {
                ExternalStatus::Failed
            },
            backend,
            error: Some(error),
            answer: None,
        },
        ExternalResult::Answered(answer) => {
            let in_question = question.contains(answer.choice.as_str());
            let met = question
                .threshold()
                .is_met_by(answer.gated_confidence_millionths);
            ExternalEvidence {
                status: if !in_question {
                    ExternalStatus::Rejected
                } else if met {
                    ExternalStatus::Accepted
                } else {
                    ExternalStatus::BelowThreshold
                },
                backend,
                error: (!in_question).then_some(ExternalError::AnswerUnknownChoice),
                answer: Some(answer),
            }
        }
    };

    if let [only] = eligible {
        return applied(*only, DecisionSource::SingleOption, None, evidence);
    }
    if evidence.status == ExternalStatus::Accepted {
        let choice = evidence.answer.as_ref().map(|answer| answer.choice.clone());
        match choice.as_ref().and_then(|choice| choice.to_harness()) {
            Some(harness) if eligible.contains(&harness) => {
                return applied(harness, DecisionSource::External, None, evidence);
            }
            _ if choice.as_ref().is_some_and(|choice| choice.is_escalate()) => {
                return RouteResolution {
                    status: DecisionStatus::Escalated,
                    chosen_harness: None,
                    source: DecisionSource::External,
                    rule_index: None,
                    external: evidence,
                };
            }
            // An accepted option outside `eligible` cannot happen when the
            // question was built from `eligible`; fall through defensively.
            _ => {}
        }
    }
    if let Some((index, harness)) = match_rule(rules, labels, eligible) {
        return applied(harness, DecisionSource::LocalRule, Some(index), evidence);
    }
    if let Some(harness) = project_default.filter(|harness| eligible.contains(harness)) {
        return applied(harness, DecisionSource::ProjectDefault, None, evidence);
    }
    RouteResolution {
        status: DecisionStatus::Escalated,
        chosen_harness: None,
        source: DecisionSource::None,
        rule_index: None,
        external: evidence,
    }
}

fn applied(
    harness: AgentKind,
    source: DecisionSource,
    rule_index: Option<u32>,
    external: ExternalEvidence,
) -> RouteResolution {
    RouteResolution {
        status: DecisionStatus::Resolved,
        chosen_harness: Some(harness),
        source,
        rule_index,
        external,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::answer::BackendAnswer;
    use crate::identifiers::OptionId;
    use crate::probability::Threshold;

    const PINNED: &str = "jev-1.13.0";
    const ELIGIBLE: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

    fn question() -> DecisionQuestion {
        DecisionQuestion::harness_route(
            &ELIGIBLE,
            &BTreeMap::new(),
            Threshold::from_unit(0.8).unwrap(),
        )
        .unwrap()
    }

    fn answered(choice: &str, peak: f64) -> ExternalResult {
        let rest = (1.0 - peak) / 2.0;
        let probabilities = ["claude", "codex", "escalate"]
            .into_iter()
            .map(|id| (id.to_string(), if id == choice { peak } else { rest }))
            .collect();
        let answer = BackendAnswer {
            model: PINNED.into(),
            choice: choice.into(),
            probabilities,
            confidence: 1.0,
        };
        ExternalResult::Answered(answer.validate(&question(), PINNED).unwrap())
    }

    fn rules() -> Vec<LocalRule> {
        vec![LocalRule {
            label: Label::new("rust").unwrap(),
            harness: OptionId::new("codex").unwrap(),
            priority: 1,
        }]
    }

    fn labels(values: &[&str]) -> Vec<Label> {
        values
            .iter()
            .map(|value| Label::new(*value).unwrap())
            .collect()
    }

    fn resolve(
        labels: &[Label],
        project_default: Option<AgentKind>,
        external: ExternalResult,
    ) -> RouteResolution {
        resolve_route(
            &question(),
            &ELIGIBLE,
            labels,
            project_default,
            &rules(),
            external,
        )
    }

    #[test]
    fn plan_external_order() {
        assert_eq!(plan_external(1, true, true), Some(SkipReason::SingleOption));
        assert_eq!(
            plan_external(2, false, true),
            Some(SkipReason::NotRequested)
        );
        assert_eq!(plan_external(2, true, false), Some(SkipReason::Disabled));
        assert_eq!(plan_external(2, true, true), None);
    }

    #[test]
    fn an_accepted_answer_beats_rules_and_default() {
        // Peak 0.9 over 3 options → normalized peak 0.85 ≥ 0.8.
        let resolution = resolve(
            &labels(&["rust"]),
            Some(AgentKind::Codex),
            answered("claude", 0.9),
        );
        assert_eq!(resolution.status, DecisionStatus::Resolved);
        assert_eq!(resolution.chosen_harness, Some(AgentKind::Claude));
        assert_eq!(resolution.source, DecisionSource::External);
        assert_eq!(resolution.external.status, ExternalStatus::Accepted);
    }

    #[test]
    fn an_accepted_escalate_escalates_even_with_a_matching_rule() {
        let resolution = resolve(&labels(&["rust"]), None, answered("escalate", 0.95));
        assert_eq!(resolution.status, DecisionStatus::Escalated);
        assert_eq!(resolution.chosen_harness, None);
        assert_eq!(resolution.source, DecisionSource::External);
    }

    #[test]
    fn a_low_confidence_answer_falls_back_and_keeps_its_evidence() {
        // Peak 0.7 → normalized peak 0.55 < 0.8.
        let resolution = resolve(&labels(&["rust"]), None, answered("claude", 0.7));
        assert_eq!(resolution.chosen_harness, Some(AgentKind::Codex));
        assert_eq!(resolution.source, DecisionSource::LocalRule);
        assert_eq!(resolution.rule_index, Some(0));
        assert_eq!(resolution.external.status, ExternalStatus::BelowThreshold);
        assert_eq!(
            resolution.external.answer.unwrap().choice.as_str(),
            "claude",
            "the original external choice is kept as evidence"
        );
    }

    #[test]
    fn fallback_order_rule_then_default_then_escalate() {
        let skipped = || ExternalResult::Skipped(SkipReason::NotRequested);
        let by_rule = resolve(&labels(&["rust"]), Some(AgentKind::Claude), skipped());
        assert_eq!(by_rule.source, DecisionSource::LocalRule);
        let by_default = resolve(&labels(&["docs"]), Some(AgentKind::Claude), skipped());
        assert_eq!(by_default.source, DecisionSource::ProjectDefault);
        assert_eq!(by_default.chosen_harness, Some(AgentKind::Claude));
        let ineligible_default = resolve(&[], Some(AgentKind::Grok), skipped());
        assert_eq!(ineligible_default.status, DecisionStatus::Escalated);
        assert_eq!(ineligible_default.source, DecisionSource::None);
        assert_eq!(
            ineligible_default.external.status,
            ExternalStatus::NotRequested
        );
    }

    #[test]
    fn failures_and_rejections_are_recorded_and_fall_back() {
        let failed = resolve(
            &labels(&["rust"]),
            None,
            ExternalResult::Failed(ExternalError::CredentialUnavailable),
        );
        assert_eq!(failed.external.status, ExternalStatus::Failed);
        assert_eq!(
            failed.external.error,
            Some(ExternalError::CredentialUnavailable)
        );
        assert_eq!(failed.source, DecisionSource::LocalRule);
        let rejected = resolve(
            &[],
            None,
            ExternalResult::Failed(ExternalError::AnswerNotArgmax),
        );
        assert_eq!(rejected.external.status, ExternalStatus::Rejected);
        assert_eq!(rejected.status, DecisionStatus::Escalated);
    }

    #[test]
    fn a_single_eligible_harness_is_applied_without_external_evidence() {
        let question = DecisionQuestion::harness_route(
            &[AgentKind::Codex],
            &BTreeMap::new(),
            Threshold::from_unit(0.8).unwrap(),
        )
        .unwrap();
        let resolution = resolve_route(
            &question,
            &[AgentKind::Codex],
            &[],
            None,
            &[],
            ExternalResult::Skipped(SkipReason::SingleOption),
        );
        assert_eq!(resolution.chosen_harness, Some(AgentKind::Codex));
        assert_eq!(resolution.source, DecisionSource::SingleOption);
        assert_eq!(resolution.external.status, ExternalStatus::SingleOption);
        assert_eq!(resolution.external.backend, None);
    }
}
