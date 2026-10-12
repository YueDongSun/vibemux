//! Backend answer validation and the confidence gate (ADR 034 §8).
//!
//! A backend answer is untrusted. It is accepted only when it names a pinned
//! model, chooses an option of the question, covers exactly the options with
//! finite probabilities that sum to one, chooses the unique argmax, and
//! reports a confidence in [0, 1]. The provider does not publish its
//! confidence formula, so the gate uses the lower of the reported confidence
//! and the normalized peak computed here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ExternalError;
use crate::identifiers::OptionId;
use crate::probability::{Millionths, normalized_peak};
use crate::question::DecisionQuestion;

pub const MAX_MODEL_ID_BYTES: usize = 64;
/// Allowed distance of the probability sum from 1, for transport rounding.
pub const PROBABILITY_SUM_TOLERANCE: f64 = 0.01;
/// Another option this close to the maximum makes the argmax ambiguous.
pub const ARGMAX_TIE_TOLERANCE: f64 = 1e-9;

/// One answer as a backend reported it, before validation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendAnswer {
    /// The model the backend says produced the answer.
    pub model: String,
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionProbability {
    pub option: OptionId,
    pub probability_millionths: Millionths,
}

/// A validated answer. Probabilities follow the question's option order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedAnswer {
    pub model: String,
    pub choice: OptionId,
    pub probabilities: Vec<OptionProbability>,
    pub reported_confidence_millionths: Millionths,
    pub normalized_peak_millionths: Millionths,
    /// The lower of the two confidences; the only value the gate reads.
    pub gated_confidence_millionths: Millionths,
}

impl BackendAnswer {
    pub fn validate(
        &self,
        question: &DecisionQuestion,
        pinned_model: &str,
    ) -> Result<ValidatedAnswer, ExternalError> {
        if self.model != pinned_model || self.model.len() > MAX_MODEL_ID_BYTES {
            return Err(ExternalError::AnswerModelMismatch);
        }
        if !question.contains(&self.choice) {
            return Err(ExternalError::AnswerUnknownChoice);
        }
        let options = question.options();
        if self.probabilities.len() != options.len()
            || options
                .iter()
                .any(|option| !self.probabilities.contains_key(option.id.as_str()))
        {
            return Err(ExternalError::AnswerProbabilityCoverage);
        }
        if self
            .probabilities
            .values()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(ExternalError::AnswerProbabilityRange);
        }
        let sum: f64 = self.probabilities.values().sum();
        if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
            return Err(ExternalError::AnswerProbabilitySum);
        }
        let chosen = self.probabilities[&self.choice];
        let ambiguous = self.probabilities.iter().any(|(option, value)| {
            option != &self.choice && *value >= chosen - ARGMAX_TIE_TOLERANCE
        });
        if ambiguous {
            return Err(ExternalError::AnswerNotArgmax);
        }
        let reported =
            Millionths::from_unit(self.confidence).ok_or(ExternalError::AnswerConfidenceRange)?;
        let peak = Millionths::from_unit(normalized_peak(chosen, options.len()))
            .ok_or(ExternalError::AnswerConfidenceRange)?;
        let probabilities = options
            .iter()
            .map(|option| {
                Millionths::from_unit(self.probabilities[option.id.as_str()])
                    .map(|probability_millionths| OptionProbability {
                        option: option.id.clone(),
                        probability_millionths,
                    })
                    .ok_or(ExternalError::AnswerProbabilityRange)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let choice =
            OptionId::new(self.choice.clone()).map_err(|_| ExternalError::AnswerUnknownChoice)?;
        Ok(ValidatedAnswer {
            model: self.model.clone(),
            choice,
            probabilities,
            reported_confidence_millionths: reported,
            normalized_peak_millionths: peak,
            gated_confidence_millionths: reported.min(peak),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use vibemux_harness::AgentKind;

    use super::*;
    use crate::probability::Threshold;

    const PINNED: &str = "jev-1.13.0";

    fn question() -> DecisionQuestion {
        DecisionQuestion::harness_route(
            &[AgentKind::Claude, AgentKind::Codex],
            &BTreeMap::new(),
            Threshold::from_unit(0.9).unwrap(),
        )
        .unwrap()
    }

    fn answer(choice: &str, probabilities: &[(&str, f64)], confidence: f64) -> BackendAnswer {
        BackendAnswer {
            model: PINNED.into(),
            choice: choice.into(),
            probabilities: probabilities
                .iter()
                .map(|(option, value)| ((*option).to_string(), *value))
                .collect(),
            confidence,
        }
    }

    #[test]
    fn a_clean_answer_validates_and_gates_on_the_lower_confidence() {
        let valid = answer(
            "codex",
            &[("claude", 0.04), ("codex", 0.94), ("escalate", 0.02)],
            0.95,
        )
        .validate(&question(), PINNED)
        .unwrap();
        assert_eq!(valid.choice.as_str(), "codex");
        assert_eq!(valid.reported_confidence_millionths.get(), 950_000);
        assert_eq!(valid.normalized_peak_millionths.get(), 910_000);
        assert_eq!(valid.gated_confidence_millionths.get(), 910_000);
        let order: Vec<_> = valid
            .probabilities
            .iter()
            .map(|entry| entry.option.as_str())
            .collect();
        assert_eq!(order, ["claude", "codex", "escalate"]);
    }

    #[test]
    fn rejection_table() {
        let cases: Vec<(BackendAnswer, ExternalError)> = vec![
            (
                BackendAnswer {
                    model: "jev-latest".into(),
                    ..answer(
                        "codex",
                        &[("claude", 0.1), ("codex", 0.8), ("escalate", 0.1)],
                        0.7,
                    )
                },
                ExternalError::AnswerModelMismatch,
            ),
            (
                answer(
                    "grok",
                    &[("claude", 0.1), ("codex", 0.8), ("escalate", 0.1)],
                    0.7,
                ),
                ExternalError::AnswerUnknownChoice,
            ),
            (
                answer("codex", &[("claude", 0.2), ("codex", 0.8)], 0.7),
                ExternalError::AnswerProbabilityCoverage,
            ),
            (
                answer(
                    "codex",
                    &[
                        ("claude", 0.1),
                        ("codex", 0.8),
                        ("escalate", 0.05),
                        ("grok", 0.05),
                    ],
                    0.7,
                ),
                ExternalError::AnswerProbabilityCoverage,
            ),
            (
                answer(
                    "codex",
                    &[("claude", -0.1), ("codex", 1.0), ("escalate", 0.1)],
                    0.7,
                ),
                ExternalError::AnswerProbabilityRange,
            ),
            (
                answer(
                    "codex",
                    &[("claude", f64::NAN), ("codex", 0.9), ("escalate", 0.1)],
                    0.7,
                ),
                ExternalError::AnswerProbabilityRange,
            ),
            (
                answer(
                    "codex",
                    &[("claude", 0.3), ("codex", 0.8), ("escalate", 0.1)],
                    0.7,
                ),
                ExternalError::AnswerProbabilitySum,
            ),
            (
                answer(
                    "claude",
                    &[("claude", 0.1), ("codex", 0.8), ("escalate", 0.1)],
                    0.7,
                ),
                ExternalError::AnswerNotArgmax,
            ),
            (
                answer(
                    "codex",
                    &[("claude", 0.45), ("codex", 0.45), ("escalate", 0.1)],
                    0.2,
                ),
                ExternalError::AnswerNotArgmax,
            ),
            (
                answer(
                    "codex",
                    &[("claude", 0.1), ("codex", 0.8), ("escalate", 0.1)],
                    1.5,
                ),
                ExternalError::AnswerConfidenceRange,
            ),
            (
                answer(
                    "codex",
                    &[("claude", 0.1), ("codex", 0.8), ("escalate", 0.1)],
                    f64::NAN,
                ),
                ExternalError::AnswerConfidenceRange,
            ),
        ];
        for (candidate, expected) in cases {
            assert_eq!(
                candidate.validate(&question(), PINNED),
                Err(expected),
                "{candidate:?}"
            );
        }
    }

    #[test]
    fn provider_rounding_within_tolerance_is_accepted() {
        // The provider's documented example: three options, rounded values.
        let valid = answer(
            "codex",
            &[("claude", 0.12), ("codex", 0.88), ("escalate", 0.0)],
            0.81,
        )
        .validate(&question(), PINNED)
        .unwrap();
        assert_eq!(valid.gated_confidence_millionths.get(), 810_000);
        assert_eq!(valid.normalized_peak_millionths.get(), 820_000);
    }

    #[test]
    fn unknown_fields_in_an_answer_are_rejected() {
        let raw = r#"{"model":"jev-1.13.0","choice":"codex","probabilities":{},"confidence":0.5,"extra":1}"#;
        assert!(serde_json::from_str::<BackendAnswer>(raw).is_err());
    }

    proptest! {
        #[test]
        fn validation_never_panics_and_accepted_answers_hold_the_invariants(
            weights in proptest::collection::vec(0.0f64..1.0, 3),
            choice_index in 0usize..4,
            confidence in -0.5f64..1.5,
        ) {
            let ids = ["claude", "codex", "escalate", "grok"];
            let total: f64 = weights.iter().sum();
            let probabilities: Vec<(&str, f64)> = ids
                .iter()
                .take(3)
                .zip(&weights)
                .map(|(id, weight)| (*id, if total > 0.0 { weight / total } else { 0.0 }))
                .collect();
            let candidate = answer(ids[choice_index], &probabilities, confidence);
            if let Ok(valid) = candidate.validate(&question(), PINNED) {
                let chosen = candidate.probabilities[valid.choice.as_str()];
                for (option, value) in &candidate.probabilities {
                    if option != valid.choice.as_str() {
                        prop_assert!(*value < chosen);
                    }
                }
                prop_assert!(valid.gated_confidence_millionths <= valid.reported_confidence_millionths);
                prop_assert!(valid.gated_confidence_millionths <= valid.normalized_peak_millionths);
                prop_assert_eq!(valid.probabilities.len(), 3);
            }
        }
    }
}
