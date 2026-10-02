//! Writable worker launch profiles for supervised workflows (ADR 031 §6).
//!
//! These profiles are distinct from the read-only dispatch profiles of
//! ADR 029, which stay unchanged. A writable profile runs in the attempt's
//! owned worktree and grants only the abstract tools the admitted contract
//! permits: reading and searching always, file edits and development-test
//! commands only when granted. No profile grants an unrestricted shell,
//! network tools, MCP servers, user or project settings, recursive agent
//! delegation, or session persistence.
//!
//! The flags restrict what the vendor CLI offers its model; they are not an
//! OS sandbox. A vendor that can run commands as the same user can still
//! write outside the worktree, so the daemon enforces scope again on the
//! collected candidate. Each profile is uncertified for a given installed
//! vendor version until a live certification turn records evidence.

use super::{
    DispatchError, DispatchRoute, NativeProtocol,
    launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES},
};

pub const CLAUDE_WRITABLE_PROFILE: &str = "claude_stream_json_writable_v1";
pub const CODEX_WRITABLE_PROFILE: &str = "codex_exec_writable_v1";

/// Abstract tool grants of one turn. Reading and searching are always on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WritableGrant {
    pub edit_files: bool,
    pub run_dev_tests: bool,
}

impl WritableGrant {
    /// Review and answer turns: read and search only.
    pub const READ_ONLY: Self = Self {
        edit_files: false,
        run_dev_tests: false,
    };
}

/// The profile identity recorded in the contract for `protocol`.
pub fn writable_profile_id(protocol: NativeProtocol) -> Result<&'static str, DispatchError> {
    match protocol {
        NativeProtocol::ClaudeStreamJson => Ok(CLAUDE_WRITABLE_PROFILE),
        NativeProtocol::CodexExec => Ok(CODEX_WRITABLE_PROFILE),
        // App-server approvals and ACP permission requests have no certified
        // writable posture yet.
        NativeProtocol::CodexAppServer | NativeProtocol::Acp => {
            Err(DispatchError::ExecutionDisabled)
        }
    }
}

pub fn build_writable_launch_spec(
    route: &DispatchRoute,
    grant: WritableGrant,
) -> Result<LaunchSpec, DispatchError> {
    if !route.protocol.accepts(route.harness) {
        return Err(DispatchError::ConfigRouteInvalid);
    }
    if !route.allow_execution || !route.protocol.supports_execution() {
        return Err(DispatchError::ExecutionDisabled);
    }
    writable_profile_id(route.protocol)?;
    let owned = |values: &[&str]| -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    };
    let mut arguments = match route.protocol {
        NativeProtocol::ClaudeStreamJson => {
            let mut tools = vec!["Read", "Glob", "Grep"];
            let mut allowed = tools.clone();
            if grant.edit_files {
                tools.extend(["Edit", "Write"]);
                allowed.extend(["Edit", "Write"]);
            }
            if grant.run_dev_tests {
                tools.push("Bash");
                allowed.push("Bash(node --test:*)");
            }
            let mut arguments = owned(&[
                "--bare",
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                // Anything not pre-allowed is denied, never prompted.
                "--permission-mode",
                "dontAsk",
            ]);
            arguments.push(format!("--tools={}", tools.join(",")));
            arguments.push(format!("--allowedTools={}", allowed.join(",")));
            arguments.extend(owned(&[
                "--strict-mcp-config",
                "--restricted",
                "--no-session-persistence",
            ]));
            arguments
        }
        NativeProtocol::CodexExec => {
            let sandbox = if grant.edit_files || grant.run_dev_tests {
                "workspace-write"
            } else {
                "read-only"
            };
            owned(&["exec", "--json", "--sandbox", sandbox, "--ephemeral"])
        }
        NativeProtocol::CodexAppServer | NativeProtocol::Acp => {
            return Err(DispatchError::ExecutionDisabled);
        }
    };
    if let Some(model) = &route.model {
        arguments.extend(["--model".to_string(), model.clone()]);
    }
    if route.protocol == NativeProtocol::CodexExec {
        // `-` reads the prompt from stdin, keeping it out of argv.
        arguments.push("-".to_string());
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::AgentKind;

    fn route(harness: AgentKind, protocol: NativeProtocol) -> DispatchRoute {
        serde_json::from_value(serde_json::json!({
            "harness": harness,
            "protocol": protocol,
            "executable": PathBuf::from("C:/vendor/tool.exe"),
            "environment_names": ["VENDOR_API_KEY"],
            "enabled": true,
            "allow_execution": true,
            "model": "model-x",
        }))
        .expect("route")
    }

    #[test]
    fn claude_writable_profile_grants_only_the_listed_tools() {
        let spec = build_writable_launch_spec(
            &route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
            WritableGrant {
                edit_files: true,
                run_dev_tests: true,
            },
        )
        .expect("spec");
        assert!(
            spec.arguments
                .contains(&"--tools=Read,Glob,Grep,Edit,Write,Bash".to_string())
        );
        assert!(
            spec.arguments.contains(
                &"--allowedTools=Read,Glob,Grep,Edit,Write,Bash(node --test:*)".to_string()
            )
        );
        for required in [
            "dontAsk",
            "--strict-mcp-config",
            "--restricted",
            "--no-session-persistence",
        ] {
            assert!(
                spec.arguments.iter().any(|argument| argument == required),
                "{required}"
            );
        }
        assert!(!spec.arguments.iter().any(|argument| argument == "--tools="));
        assert!(
            !spec
                .arguments
                .iter()
                .any(|argument| argument.contains("Task") || argument.contains("WebFetch"))
        );
        let read_only = build_writable_launch_spec(
            &route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
            WritableGrant::READ_ONLY,
        )
        .expect("read only");
        assert!(
            read_only
                .arguments
                .contains(&"--tools=Read,Glob,Grep".to_string())
        );
    }

    #[test]
    fn codex_writable_profile_uses_workspace_write_and_stdin_prompt() {
        let spec = build_writable_launch_spec(
            &route(AgentKind::Codex, NativeProtocol::CodexExec),
            WritableGrant {
                edit_files: true,
                run_dev_tests: false,
            },
        )
        .expect("spec");
        assert_eq!(
            &spec.arguments[..5],
            [
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--ephemeral"
            ]
        );
        assert_eq!(spec.arguments.last().map(String::as_str), Some("-"));
        let review = build_writable_launch_spec(
            &route(AgentKind::Codex, NativeProtocol::CodexExec),
            WritableGrant::READ_ONLY,
        )
        .expect("review");
        assert!(
            review
                .arguments
                .windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
    }

    #[test]
    fn acp_app_server_and_non_executing_routes_are_refused() {
        for (harness, protocol) in [
            (AgentKind::OpenCode, NativeProtocol::Acp),
            (AgentKind::Codex, NativeProtocol::CodexAppServer),
        ] {
            assert_eq!(
                build_writable_launch_spec(&route(harness, protocol), WritableGrant::READ_ONLY),
                Err(DispatchError::ExecutionDisabled)
            );
        }
        let mut disabled = route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
        disabled.allow_execution = false;
        assert_eq!(
            build_writable_launch_spec(&disabled, WritableGrant::READ_ONLY),
            Err(DispatchError::ExecutionDisabled)
        );
    }
}
