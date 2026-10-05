//! Content-free execution authority derived from an admitted, narrowed TaskSpec.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    Sha256Digest,
    task_spec::{TaskSpec, ToolName},
};

pub const EXECUTION_AUTHORITY_SCHEMA_VERSION: u32 = 1;

/// What the dispatcher can prove about one effective route. An unknown
/// endpoint is kept unknown; a model label is never gateway attestation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveRouteIdentity {
    pub harness: vibemux_harness::AgentKind,
    pub protocol: vibemux_harness::dispatch::NativeProtocol,
    pub model_alias: Option<String>,
    pub endpoint: Option<String>,
    pub config_digest: Sha256Digest,
    pub profile: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthority {
    pub schema_version: u32,
    pub task_spec_digest: Sha256Digest,
    pub admitted_tools: BTreeSet<ToolName>,
}

impl ExecutionAuthority {
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, crate::canonical_json::CanonicalError> {
        Ok(Self {
            schema_version: EXECUTION_AUTHORITY_SCHEMA_VERSION,
            task_spec_digest: spec.digest()?,
            admitted_tools: spec.permitted_tools.iter().copied().collect(),
        })
    }

    /// Answer and review cannot add read/search authority or retain mutation tools.
    #[must_use]
    pub fn effective_tools(&self, read_only: bool) -> BTreeSet<ToolName> {
        self.admitted_tools
            .iter()
            .copied()
            .filter(|tool| {
                !read_only || matches!(tool, ToolName::ReadFiles | ToolName::SearchFiles)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grant_removal_survives_purpose_intersection() {
        let mut spec = crate::task_spec::fixtures::track_a_spec();
        let all = [
            ToolName::ReadFiles,
            ToolName::SearchFiles,
            ToolName::EditFiles,
            ToolName::WriteFiles,
            ToolName::RunDevTests,
        ];
        for removed in all {
            spec.permitted_tools = all.into_iter().filter(|tool| *tool != removed).collect();
            let authority = ExecutionAuthority::from_spec(&spec).expect("authority");
            assert!(!authority.effective_tools(false).contains(&removed));
            assert!(!authority.effective_tools(true).contains(&removed));
            assert!(
                authority
                    .effective_tools(true)
                    .iter()
                    .all(|tool| matches!(tool, ToolName::ReadFiles | ToolName::SearchFiles))
            );
        }
        spec.permitted_tools.clear();
        assert!(
            ExecutionAuthority::from_spec(&spec)
                .expect("authority")
                .effective_tools(true)
                .is_empty()
        );
    }
}
