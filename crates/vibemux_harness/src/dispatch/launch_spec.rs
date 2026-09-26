//! Shell-free launch specification for one native route.
//!
//! Argv comes only from the protocol profile, which pins the structured
//! transport and the read-only, no-approval posture (ADR 029 §4); the route
//! config contributes the executable and an optional model, never flags. The
//! daemon executes the result directly (no shell) with a cleared environment
//! containing only [`SYSTEM_ENVIRONMENT_NAMES`] and the route's allowlisted
//! names.

use std::{fmt, path::PathBuf};

use crate::AgentKind;

use super::{DispatchError, DispatchRoute, NativeProtocol};

/// Names always forwarded when set; Windows processes need them to start.
pub const SYSTEM_ENVIRONMENT_NAMES: [&str; 4] = ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionMode {
    /// Initialize-only handshake: no session, no prompt, no model request.
    Probe,
    /// One prompt, one turn.
    Execute,
}

#[derive(Clone, Eq, PartialEq)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    /// Names to forward from the daemon environment, system names first.
    pub environment_names: Vec<String>,
}

impl fmt::Debug for LaunchSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchSpec")
            .field("argument_count", &self.arguments.len())
            .field("environment_name_count", &self.environment_names.len())
            .finish_non_exhaustive()
    }
}

pub fn build_launch_spec(
    route: &DispatchRoute,
    mode: SessionMode,
) -> Result<LaunchSpec, DispatchError> {
    if !route.protocol.accepts(route.harness) {
        return Err(DispatchError::ConfigRouteInvalid);
    }
    let arguments = protocol_arguments(route, mode)?;
    let mut environment_names: Vec<String> = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for name in &route.environment_names {
        if !environment_names.contains(name) {
            environment_names.push(name.clone());
        }
    }
    Ok(LaunchSpec {
        executable: route.executable.clone(),
        arguments,
        environment_names,
    })
}

fn protocol_arguments(
    route: &DispatchRoute,
    mode: SessionMode,
) -> Result<Vec<String>, DispatchError> {
    let owned = |values: &[&str]| values.iter().map(|value| (*value).to_string()).collect();
    let model = route.model.as_deref();
    let arguments: Vec<String> = match route.protocol {
        // The model, if any, travels in thread/start params, not argv.
        NativeProtocol::CodexAppServer => owned(&["app-server", "--stdio"]),
        NativeProtocol::Acp => match route.harness {
            AgentKind::OpenCode => owned(&["acp"]),
            AgentKind::Copilot => owned(&["--acp", "--no-auto-update"]),
            AgentKind::Grok => owned(&["agent", "stdio"]),
            _ => return Err(DispatchError::ConfigRouteInvalid),
        },
        // `--tools=` removes every built-in tool, `--strict-mcp-config` skips
        // project and user MCP servers, and `--restricted` ignores project and
        // user settings files, so a repository cannot re-enable tools.
        NativeProtocol::ClaudeStreamJson => {
            let mut arguments: Vec<String> = owned(&[
                "--bare",
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-mode",
                "dontAsk",
                "--tools=",
                "--strict-mcp-config",
                "--restricted",
                "--no-session-persistence",
            ]);
            if let Some(model) = model {
                arguments.extend(["--model".to_string(), model.to_string()]);
            }
            arguments
        }
        NativeProtocol::CodexExec => {
            if mode == SessionMode::Probe {
                return Err(DispatchError::ProbeUnsupported);
            }
            let mut arguments: Vec<String> =
                owned(&["exec", "--json", "--sandbox", "read-only", "--ephemeral"]);
            if let Some(model) = model {
                arguments.extend(["--model".to_string(), model.to_string()]);
            }
            // `-` reads the prompt from stdin, keeping it out of argv.
            arguments.push("-".to_string());
            arguments
        }
    };
    Ok(arguments)
}
