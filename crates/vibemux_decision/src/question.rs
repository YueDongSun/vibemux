//! A typed decision question: a closed option set plus a threshold.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::error::DecisionError;
use crate::identifiers::OptionId;
use crate::kind::DecisionKind;
use crate::probability::Threshold;

pub const MIN_QUESTION_OPTIONS: usize = 2;
/// Jev's per-question option limit.
pub const MAX_QUESTION_OPTIONS: usize = 255;
pub const MAX_OPTION_DESCRIPTION_BYTES: usize = 256;

/// Fixed instructions for `harness_route`. No caller text is ever added.
pub const HARNESS_ROUTE_INSTRUCTIONS: &str = "Choose the coding harness that should handle \
     this task. The state lists operator labels for the task, a coarse prompt size, and the \
     project default harness. Choose escalate when no harness clearly fits or the information \
     is insufficient.";
const ESCALATE_DESCRIPTION: &str =
    "No harness clearly fits, or the information is insufficient; a human chooses.";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionOption {
    pub id: OptionId,
    /// Sent to the backend as the option's criterion.
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionQuestion {
    kind: DecisionKind,
    options: Vec<QuestionOption>,
    threshold: Threshold,
}

impl DecisionQuestion {
    /// Options must be unique, between 2 and 255, and include `escalate`;
    /// descriptions must be non-empty, short, and free of control characters.
    pub fn new(
        kind: DecisionKind,
        options: Vec<QuestionOption>,
        threshold: Threshold,
    ) -> Result<Self, DecisionError> {
        if !(MIN_QUESTION_OPTIONS..=MAX_QUESTION_OPTIONS).contains(&options.len()) {
            return Err(DecisionError::InvalidRequest);
        }
        let mut seen = BTreeSet::new();
        for option in &options {
            if !seen.insert(&option.id) || !is_valid_description(&option.description) {
                return Err(DecisionError::InvalidRequest);
            }
        }
        if !options.iter().any(|option| option.id.is_escalate()) {
            return Err(DecisionError::InvalidRequest);
        }
        Ok(Self {
            kind,
            options,
            threshold,
        })
    }

    /// One option per eligible harness, in canonical `AgentKind` order, then
    /// `escalate`. Operator descriptions replace the built-in ones.
    pub fn harness_route(
        eligible: &[AgentKind],
        descriptions: &BTreeMap<OptionId, String>,
        threshold: Threshold,
    ) -> Result<Self, DecisionError> {
        let mut options: Vec<QuestionOption> = AgentKind::all()
            .into_iter()
            .filter(|harness| eligible.contains(harness))
            .map(|harness| {
                let id = OptionId::harness(harness);
                let description = descriptions.get(&id).cloned().unwrap_or_else(|| {
                    format!("Route the task to the {} harness.", harness.display_name())
                });
                QuestionOption { id, description }
            })
            .collect();
        if options.is_empty() {
            return Err(DecisionError::NoEligibleHarness);
        }
        let escalate = OptionId::escalate();
        let description = descriptions
            .get(&escalate)
            .cloned()
            .unwrap_or_else(|| ESCALATE_DESCRIPTION.to_string());
        options.push(QuestionOption {
            id: escalate,
            description,
        });
        Self::new(DecisionKind::HarnessRoute, options, threshold)
    }

    #[must_use]
    pub const fn kind(&self) -> DecisionKind {
        self.kind
    }

    #[must_use]
    pub fn options(&self) -> &[QuestionOption] {
        &self.options
    }

    #[must_use]
    pub const fn threshold(&self) -> Threshold {
        self.threshold
    }

    #[must_use]
    pub fn option_ids(&self) -> Vec<OptionId> {
        self.options
            .iter()
            .map(|option| option.id.clone())
            .collect()
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.options.iter().any(|option| option.id.as_str() == id)
    }

    #[must_use]
    pub const fn instructions(&self) -> &'static str {
        match self.kind {
            DecisionKind::HarnessRoute => HARNESS_ROUTE_INSTRUCTIONS,
        }
    }
}

/// Non-empty, at most 256 bytes, no control characters.
#[must_use]
pub fn is_valid_description(description: &str) -> bool {
    !description.trim().is_empty()
        && description.len() <= MAX_OPTION_DESCRIPTION_BYTES
        && !description.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threshold() -> Threshold {
        Threshold::from_unit(0.9).unwrap()
    }

    fn option(id: &str) -> QuestionOption {
        QuestionOption {
            id: OptionId::new(id).unwrap(),
            description: "d".into(),
        }
    }

    #[test]
    fn harness_route_lists_eligible_harnesses_in_canonical_order_then_escalate() {
        let question = DecisionQuestion::harness_route(
            &[AgentKind::OpenCode, AgentKind::Claude],
            &BTreeMap::new(),
            threshold(),
        )
        .unwrap();
        let ids: Vec<_> = question
            .options()
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(ids, ["claude", "opencode", "escalate"]);
        assert!(question.contains("escalate"));
        assert!(!question.contains("codex"));
    }

    #[test]
    fn operator_descriptions_replace_builtin_ones() {
        let mut descriptions = BTreeMap::new();
        descriptions.insert(
            OptionId::harness(AgentKind::Codex),
            "Rust refactors".to_string(),
        );
        let question =
            DecisionQuestion::harness_route(&[AgentKind::Codex], &descriptions, threshold())
                .unwrap();
        assert_eq!(question.options()[0].description, "Rust refactors");
        assert_eq!(question.options()[1].description, ESCALATE_DESCRIPTION);
    }

    #[test]
    fn no_eligible_harness_is_rejected() {
        assert_eq!(
            DecisionQuestion::harness_route(&[], &BTreeMap::new(), threshold()),
            Err(DecisionError::NoEligibleHarness)
        );
    }

    #[test]
    fn question_shape_rules_are_enforced() {
        let kind = DecisionKind::HarnessRoute;
        assert!(DecisionQuestion::new(kind, vec![option("escalate")], threshold()).is_err());
        assert!(
            DecisionQuestion::new(kind, vec![option("codex"), option("claude")], threshold())
                .is_err(),
            "escalate is mandatory"
        );
        assert!(
            DecisionQuestion::new(
                kind,
                vec![option("codex"), option("codex"), option("escalate")],
                threshold()
            )
            .is_err(),
            "duplicates are rejected"
        );
        let mut bad_description = option("codex");
        bad_description.description = "line\nbreak".into();
        assert!(
            DecisionQuestion::new(kind, vec![bad_description, option("escalate")], threshold())
                .is_err()
        );
        let many: Vec<_> = (0..MAX_QUESTION_OPTIONS)
            .map(|index| option(&format!("option{index}")))
            .chain(std::iter::once(option("escalate")))
            .collect();
        assert_eq!(many.len(), MAX_QUESTION_OPTIONS + 1);
        assert!(DecisionQuestion::new(kind, many, threshold()).is_err());
    }
}
