//! Operator local rules: label → harness with a priority.

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::identifiers::{Label, OptionId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRule {
    pub label: Label,
    /// A harness command name; validated by `DecisionConfig::validate`.
    pub harness: OptionId,
    /// Higher wins; equal priorities keep config order.
    pub priority: u32,
}

/// The highest-priority rule whose label was requested and whose harness is
/// eligible. Returns the rule's config index with its harness.
#[must_use]
pub fn match_rule(
    rules: &[LocalRule],
    labels: &[Label],
    eligible: &[AgentKind],
) -> Option<(u32, AgentKind)> {
    let mut best: Option<(u32, u32, AgentKind)> = None;
    for (index, rule) in rules.iter().enumerate() {
        if !labels.contains(&rule.label) {
            continue;
        }
        let Some(harness) = rule.harness.to_harness() else {
            continue;
        };
        if !eligible.contains(&harness) {
            continue;
        }
        let Ok(index) = u32::try_from(index) else {
            break;
        };
        if best.is_none_or(|(_, priority, _)| rule.priority > priority) {
            best = Some((index, rule.priority, harness));
        }
    }
    best.map(|(index, _, harness)| (index, harness))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(label: &str, harness: &str, priority: u32) -> LocalRule {
        LocalRule {
            label: Label::new(label).unwrap(),
            harness: OptionId::new(harness).unwrap(),
            priority,
        }
    }

    fn labels(values: &[&str]) -> Vec<Label> {
        values
            .iter()
            .map(|value| Label::new(*value).unwrap())
            .collect()
    }

    #[test]
    fn highest_priority_eligible_rule_wins_and_ties_keep_config_order() {
        let rules = vec![
            rule("rust", "codex", 1),
            rule("refactor", "claude", 5),
            rule("refactor", "codex", 5),
            rule("rust", "grok", 9),
        ];
        let eligible = [AgentKind::Claude, AgentKind::Codex];
        assert_eq!(
            match_rule(&rules, &labels(&["rust", "refactor"]), &eligible),
            Some((1, AgentKind::Claude)),
            "grok is not eligible; claude wins the tie by order"
        );
        assert_eq!(
            match_rule(&rules, &labels(&["rust"]), &eligible),
            Some((0, AgentKind::Codex))
        );
        assert_eq!(match_rule(&rules, &labels(&["docs"]), &eligible), None);
        assert_eq!(match_rule(&rules, &labels(&["rust"]), &[]), None);
    }

    #[test]
    fn a_rule_naming_escalate_never_matches() {
        let rules = vec![rule("docs", "escalate", 10)];
        assert_eq!(
            match_rule(&rules, &labels(&["docs"]), &[AgentKind::Codex]),
            None
        );
    }
}
