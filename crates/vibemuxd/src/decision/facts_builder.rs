//! Which harnesses a `harness_route` decision may choose (ADR 034 §3).
//!
//! A harness is eligible only if the pinned dispatch config has an enabled
//! route for it that allows execution under a protocol ADR 029 dispatch can
//! run (the same gates as `DispatchConfig::execution_route`), and the
//! trusted probe verified it. A decision therefore never offers a harness
//! that the decided dispatch would refuse at admission.

use vibemux_harness::{AgentKind, HarnessRow, dispatch::route_config::DispatchCatalogEntry};

/// Eligible harnesses in canonical `AgentKind` order.
#[must_use]
pub fn eligible_harnesses(catalog: &[DispatchCatalogEntry], rows: &[HarnessRow]) -> Vec<AgentKind> {
    AgentKind::all()
        .into_iter()
        .filter(|harness| {
            catalog.iter().any(|entry| {
                entry.harness == *harness
                    && entry.enabled
                    && entry.allow_execution
                    && entry.protocol.supports_read_only_dispatch()
            }) && is_detected(*harness, rows)
        })
        .collect()
}

/// The project default harness (`vibemuxctl switch`), if one is set.
#[must_use]
pub fn project_default(rows: &[HarnessRow]) -> Option<AgentKind> {
    let row = rows.iter().find(|row| row.default)?;
    AgentKind::all()
        .into_iter()
        .find(|harness| harness.command_name() == row.name)
}

fn is_detected(harness: AgentKind, rows: &[HarnessRow]) -> bool {
    rows.iter()
        .any(|row| row.name == harness.command_name() && row.available)
}

#[cfg(test)]
mod tests {
    use vibemux_harness::{HarnessProtocol, dispatch::NativeProtocol};

    use super::*;

    fn entry(
        harness: AgentKind,
        protocol: NativeProtocol,
        enabled: bool,
        allow_execution: bool,
    ) -> DispatchCatalogEntry {
        DispatchCatalogEntry {
            harness,
            protocol,
            enabled,
            allow_execution,
            probe_supported: true,
        }
    }

    fn row(name: &str, available: bool, default: bool) -> HarnessRow {
        HarnessRow {
            name: name.to_owned(),
            command: vec![name.to_owned()],
            protocol: HarnessProtocol::Pty,
            provider: "fixture".to_owned(),
            available,
            path: None,
            roles: Vec::new(),
            default,
            launcher: None,
            version: None,
        }
    }

    #[test]
    fn only_enabled_executable_and_detected_routes_are_eligible() {
        let catalog = [
            entry(AgentKind::Codex, NativeProtocol::CodexExec, true, true),
            entry(
                AgentKind::Claude,
                NativeProtocol::ClaudeStreamJson,
                true,
                true,
            ),
            entry(AgentKind::OpenCode, NativeProtocol::Acp, true, true),
            entry(AgentKind::Grok, NativeProtocol::Acp, false, true),
        ];
        let rows = [
            row("codex", true, false),
            row("claude", false, false),
            row("opencode", true, true),
            row("grok", true, false),
        ];
        assert_eq!(eligible_harnesses(&catalog, &rows), [AgentKind::Codex]);
        let mut no_execution = catalog;
        no_execution[0].allow_execution = false;
        assert_eq!(eligible_harnesses(&no_execution, &rows), []);
    }

    #[test]
    fn the_project_default_comes_from_the_default_row() {
        let rows = [row("codex", true, false), row("opencode", true, true)];
        assert_eq!(project_default(&rows), Some(AgentKind::OpenCode));
        assert_eq!(project_default(&rows[..1]), None);
    }
}
