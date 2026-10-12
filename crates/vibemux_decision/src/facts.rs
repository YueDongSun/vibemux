//! The enumerated facts a backend may see (ADR 034 §4).
//!
//! Only allowlisted labels, a coarse prompt size bucket, and the project
//! default harness. The prompt, its digest, paths, and the environment never
//! appear here.

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::identifiers::{Label, OptionId};

pub const MAX_REQUEST_LABELS: usize = 16;

const SMALL_PROMPT_BYTES: u64 = 1024;
const MEDIUM_PROMPT_BYTES: u64 = 4 * 1024;
const LARGE_PROMPT_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSizeBucket {
    /// Under 1 KiB.
    Tiny,
    /// 1 KiB to under 4 KiB.
    Small,
    /// 4 KiB to under 16 KiB.
    Medium,
    /// 16 KiB and more.
    Large,
}

impl PromptSizeBucket {
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        if bytes < SMALL_PROMPT_BYTES {
            Self::Tiny
        } else if bytes < MEDIUM_PROMPT_BYTES {
            Self::Small
        } else if bytes < LARGE_PROMPT_BYTES {
            Self::Medium
        } else {
            Self::Large
        }
    }
}

/// Facts for one `harness_route` question, serialized as Jev's `state`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteFacts {
    /// Sorted and unique.
    pub labels: Vec<Label>,
    pub prompt_size: PromptSizeBucket,
    /// The project default harness, only when it is among the options.
    pub project_default: Option<OptionId>,
}

impl RouteFacts {
    /// Labels must already be allowlisted by the caller (see
    /// `DecisionConfig::check_labels`); they are sorted and deduplicated
    /// here. A project default that is not eligible is left out.
    #[must_use]
    pub fn new(
        mut labels: Vec<Label>,
        prompt_bytes: u64,
        project_default: Option<AgentKind>,
        eligible: &[AgentKind],
    ) -> Self {
        labels.sort();
        labels.dedup();
        Self {
            labels,
            prompt_size: PromptSizeBucket::from_bytes(prompt_bytes),
            project_default: project_default
                .filter(|harness| eligible.contains(harness))
                .map(OptionId::harness),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_buckets_have_fixed_edges() {
        assert_eq!(PromptSizeBucket::from_bytes(0), PromptSizeBucket::Tiny);
        assert_eq!(PromptSizeBucket::from_bytes(1023), PromptSizeBucket::Tiny);
        assert_eq!(PromptSizeBucket::from_bytes(1024), PromptSizeBucket::Small);
        assert_eq!(PromptSizeBucket::from_bytes(4095), PromptSizeBucket::Small);
        assert_eq!(PromptSizeBucket::from_bytes(4096), PromptSizeBucket::Medium);
        assert_eq!(
            PromptSizeBucket::from_bytes(16 * 1024),
            PromptSizeBucket::Large
        );
    }

    #[test]
    fn facts_sort_labels_and_drop_an_ineligible_default() {
        let labels = vec![
            Label::new("rust").unwrap(),
            Label::new("refactor").unwrap(),
            Label::new("rust").unwrap(),
        ];
        let facts = RouteFacts::new(labels, 10, Some(AgentKind::Grok), &[AgentKind::Codex]);
        assert_eq!(
            facts.labels.iter().map(Label::as_str).collect::<Vec<_>>(),
            ["refactor", "rust"]
        );
        assert_eq!(facts.project_default, None);
        let kept = RouteFacts::new(
            Vec::new(),
            10,
            Some(AgentKind::OpenCode),
            &[AgentKind::OpenCode],
        );
        assert_eq!(kept.project_default.unwrap().as_str(), "opencode");
    }

    #[test]
    fn facts_serialize_without_free_text_fields() {
        let facts = RouteFacts::new(vec![Label::new("docs").unwrap()], 5000, None, &[]);
        let value = serde_json::to_value(&facts).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["labels", "project_default", "prompt_size"]);
        assert_eq!(value["prompt_size"], "medium");
    }
}
