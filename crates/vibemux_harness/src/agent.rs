//! Agent, launcher, and probe-state vocabulary shared by the harness registry
//! and the probe.
//!
//! These enums are pure data (no I/O) so they live in `vibemux_harness`, the
//! lowest crate that needs them; `vibemux_probe` re-exports them at their
//! historical paths. This keeps the crate graph acyclic per AGENTS.md §4.2:
//! the harness crate must not depend on the probe implementation.
//!
//! The serde variant names below are wire contracts: the probe report JSON,
//! the persisted schema-3 projections, and the CLI row output all carry them,
//! so they must stay byte-identical (`open_code`, `power_shell_companion`,
//! `verified`, ...). The unit tests in this module pin them.

use serde::{Deserialize, Serialize};

/// One of the ten supported coding harnesses, in canonical probe/dashboard
/// order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
    OpenCode,
    Copilot,
    Grok,
    Qwen,
    Iflow,
    Trae,
    Codebuddy,
    Kimi,
}

impl AgentKind {
    #[must_use]
    pub const fn all() -> [Self; 10] {
        [
            Self::Claude,
            Self::Codex,
            Self::OpenCode,
            Self::Copilot,
            Self::Grok,
            Self::Qwen,
            Self::Iflow,
            Self::Trae,
            Self::Codebuddy,
            Self::Kimi,
        ]
    }

    #[must_use]
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
            Self::Grok => "grok",
            Self::Qwen => "qwen",
            Self::Iflow => "iflow",
            Self::Trae => "trae",
            Self::Codebuddy => "codebuddy",
            Self::Kimi => "kimi",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Copilot => "Copilot",
            Self::Grok => "Grok",
            Self::Qwen => "Qwen",
            Self::Iflow => "iFlow",
            Self::Trae => "TRAE",
            Self::Codebuddy => "CodeBuddy",
            Self::Kimi => "Kimi",
        }
    }
}

/// Outcome of one probe step. Only `Verified` counts as detected by the
/// harness registry; a bare PATH hit never does.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Verified,
    Failed,
    Unavailable,
    NotRun,
}

/// How a harness launcher is executed on this machine: a direct executable,
/// a PowerShell companion wrapper around a Windows script shim, or nothing
/// resolvable at all.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LauncherKind {
    DirectExecutable,
    PowerShellCompanion,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_wire_names_are_pinned() {
        let encoded = serde_json::to_string(&AgentKind::all()).expect("encode agents");
        assert_eq!(
            encoded,
            r#"["claude","codex","open_code","copilot","grok","qwen","iflow","trae","codebuddy","kimi"]"#
        );
        for agent in AgentKind::all() {
            let name = serde_json::to_string(&agent).expect("encode agent");
            assert_eq!(
                serde_json::from_str::<AgentKind>(&name).expect("decode agent"),
                agent
            );
        }
    }

    #[test]
    fn probe_state_wire_names_are_pinned() {
        let states = [
            ProbeState::Verified,
            ProbeState::Failed,
            ProbeState::Unavailable,
            ProbeState::NotRun,
        ];
        let encoded = serde_json::to_string(&states).expect("encode states");
        assert_eq!(encoded, r#"["verified","failed","unavailable","not_run"]"#);
        for state in states {
            let name = serde_json::to_string(&state).expect("encode state");
            assert_eq!(
                serde_json::from_str::<ProbeState>(&name).expect("decode state"),
                state
            );
        }
    }

    #[test]
    fn launcher_kind_wire_names_are_pinned() {
        let kinds = [
            LauncherKind::DirectExecutable,
            LauncherKind::PowerShellCompanion,
            LauncherKind::Unavailable,
        ];
        let encoded = serde_json::to_string(&kinds).expect("encode kinds");
        assert_eq!(
            encoded,
            r#"["direct_executable","power_shell_companion","unavailable"]"#
        );
        for kind in kinds {
            let name = serde_json::to_string(&kind).expect("encode kind");
            assert_eq!(
                serde_json::from_str::<LauncherKind>(&name).expect("decode kind"),
                kind
            );
        }
    }

    #[test]
    fn command_and_display_names_cover_every_agent() {
        for agent in AgentKind::all() {
            assert!(!agent.command_name().is_empty());
            assert!(!agent.display_name().is_empty());
            assert!(!agent.command_name().contains('\0') && !agent.display_name().contains('\0'));
        }
        // The command names are the registry keys; a few are pinned here
        // because the store, CLI, and Python parity all depend on them.
        assert_eq!(AgentKind::OpenCode.command_name(), "opencode");
        assert_eq!(AgentKind::Iflow.display_name(), "iFlow");
        assert_eq!(AgentKind::Trae.display_name(), "TRAE");
        assert_eq!(AgentKind::Codebuddy.display_name(), "CodeBuddy");
    }
}
