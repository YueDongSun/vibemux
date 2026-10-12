//! Decision kinds. Milestone 1 implements `harness_route` only; the roadmap
//! (`docs/decision_orchestration_roadmap.md`) reserves judge, review triage,
//! approval, governor, and handoff kinds, which are not implemented.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Which eligible coding harness should take one task.
    HarnessRoute,
}

impl DecisionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HarnessRoute => "harness_route",
        }
    }
}
