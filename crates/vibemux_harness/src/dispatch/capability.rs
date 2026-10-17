//! What VibeMux can do with each harness through a first-party structured
//! adapter.
//!
//! The registry row's `protocol` label describes the interactive launch
//! surface only, so it reads `pty` for every harness. This table answers the
//! harness-integration question instead: which structured adapters exist for
//! a harness, and whether any of them can run a prompt turn or only an
//! initialize-only probe. It is derived from [`NativeProtocol`], so admission
//! rules and every frontend read the same answer.
//!
//! A capability is what this build supports, never what the operator has
//! configured or authorized: a route still needs the operator's dispatch
//! configuration, detection, and `allow_execution` before anything runs.

use serde::{Deserialize, Serialize};

use crate::AgentKind;

use super::NativeProtocol;

/// The strongest thing any structured adapter can do for one harness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSupport {
    /// At least one adapter can execute a prompt turn.
    Execute,
    /// Adapters exist, but only for an initialize-only probe.
    ProbeOnly,
    /// No structured adapter; the harness is reachable through its terminal
    /// UI only.
    TerminalOnly,
}

impl AdapterSupport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::ProbeOnly => "probe_only",
            Self::TerminalOnly => "terminal_only",
        }
    }
}

/// Structured adapters for one harness, in [`NativeProtocol::ALL`] order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCapability {
    pub harness: AgentKind,
    pub protocols: Vec<NativeProtocol>,
    pub support: AdapterSupport,
}

impl AdapterCapability {
    /// Comma-separated protocol ids, or `-` when there is none.
    #[must_use]
    pub fn protocol_list(&self) -> String {
        if self.protocols.is_empty() {
            return "-".to_string();
        }
        self.protocols
            .iter()
            .map(|protocol| protocol.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// One display line for a frontend's detail view, such as
    /// `execute via codex_exec,codex_app_server`, `probe_only via acp`, or
    /// `terminal_only`. The protocol list has no space after its commas.
    #[must_use]
    pub fn summary(&self) -> String {
        match self.support {
            AdapterSupport::TerminalOnly => self.support.as_str().to_string(),
            support => format!("{} via {}", support.as_str(), self.protocol_list()),
        }
    }
}

#[must_use]
pub fn adapter_capability(harness: AgentKind) -> AdapterCapability {
    let protocols: Vec<NativeProtocol> = NativeProtocol::ALL
        .into_iter()
        .filter(|protocol| protocol.accepts(harness))
        .collect();
    let support = if protocols
        .iter()
        .any(|protocol| protocol.supports_execution())
    {
        AdapterSupport::Execute
    } else if protocols.iter().any(|protocol| protocol.supports_probe()) {
        AdapterSupport::ProbeOnly
    } else {
        AdapterSupport::TerminalOnly
    };
    AdapterCapability {
        harness,
        protocols,
        support,
    }
}

/// One entry per harness, in canonical probe/dashboard order.
#[must_use]
pub fn adapter_capabilities() -> Vec<AdapterCapability> {
    AgentKind::all().map(adapter_capability).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn support(harness: AgentKind) -> AdapterSupport {
        adapter_capability(harness).support
    }

    #[test]
    fn executing_adapters_cover_codex_claude_and_dsh() {
        assert_eq!(support(AgentKind::Codex), AdapterSupport::Execute);
        assert_eq!(support(AgentKind::Claude), AdapterSupport::Execute);
        assert_eq!(support(AgentKind::Dsh), AdapterSupport::Execute);
        assert_eq!(
            adapter_capability(AgentKind::Codex).protocol_list(),
            "codex_exec,codex_app_server"
        );
        assert_eq!(
            adapter_capability(AgentKind::Claude).protocols,
            [NativeProtocol::ClaudeStreamJson]
        );
    }

    #[test]
    fn acp_harnesses_are_probe_only_until_a_deny_posture_exists() {
        for harness in [AgentKind::OpenCode, AgentKind::Copilot, AgentKind::Grok] {
            let capability = adapter_capability(harness);
            assert_eq!(capability.support, AdapterSupport::ProbeOnly);
            assert_eq!(capability.protocols, [NativeProtocol::Acp]);
        }
    }

    #[test]
    fn harnesses_without_an_adapter_are_terminal_only() {
        for harness in [
            AgentKind::Qwen,
            AgentKind::Iflow,
            AgentKind::Trae,
            AgentKind::Codebuddy,
            AgentKind::Kimi,
        ] {
            let capability = adapter_capability(harness);
            assert_eq!(capability.support, AdapterSupport::TerminalOnly);
            assert_eq!(capability.protocol_list(), "-");
        }
    }

    #[test]
    fn table_follows_canonical_order_and_every_protocol_has_an_owner() {
        let table = adapter_capabilities();
        let order: Vec<AgentKind> = table.iter().map(|entry| entry.harness).collect();
        assert_eq!(order, AgentKind::all());
        for protocol in NativeProtocol::ALL {
            assert!(
                table
                    .iter()
                    .any(|entry| entry.protocols.contains(&protocol)),
                "{} has no harness",
                protocol.as_str()
            );
        }
        assert_eq!(
            serde_json::to_string(&AdapterSupport::ProbeOnly).expect("encode"),
            "\"probe_only\""
        );
    }

    #[test]
    fn summaries_name_the_support_level_and_its_protocols() {
        assert_eq!(
            adapter_capability(AgentKind::Codex).summary(),
            "execute via codex_exec,codex_app_server"
        );
        assert_eq!(
            adapter_capability(AgentKind::Grok).summary(),
            "probe_only via acp"
        );
        assert_eq!(
            adapter_capability(AgentKind::Kimi).summary(),
            "terminal_only"
        );
    }

    /// The operator documentation's capability table names every harness
    /// once, with the protocols and support derived here, so a new harness
    /// or protocol fails this test until the documentation follows.
    #[test]
    fn protocol_boundaries_document_every_harness_capability() {
        let documentation = include_str!("../../../../docs/protocol_boundaries.md");
        let section = documentation
            .split("## Harness adapter capability")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("capability section");
        let rows: Vec<Vec<&str>> = section
            .lines()
            .filter(|line| line.starts_with("| ") && !line.starts_with("| Harness |"))
            .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
            .collect();
        let documented = rows
            .iter()
            .map(|cells| cells[0].split(", ").count())
            .sum::<usize>();
        assert_eq!(documented, AgentKind::all().len(), "stale or extra harness");
        for capability in adapter_capabilities() {
            let name = capability.harness.display_name();
            let row = rows
                .iter()
                .find(|cells| cells[0].split(", ").any(|entry| entry == name))
                .unwrap_or_else(|| panic!("{name} is not documented"));
            let protocols = if capability.protocols.is_empty() {
                "none".to_string()
            } else {
                capability
                    .protocols
                    .iter()
                    .map(|protocol| format!("`{}`", protocol.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            assert_eq!(row[1], protocols, "{name} protocols");
            assert_eq!(
                row[2],
                format!("`{}`", capability.support.as_str()),
                "{name} support"
            );
        }
    }
}
