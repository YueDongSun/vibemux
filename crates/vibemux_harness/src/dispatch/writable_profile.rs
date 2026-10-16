//! Writable worker launch profiles for supervised workflows (ADR 031 §6).
//!
//! These profiles are distinct from the read-only dispatch profiles of
//! ADR 029, which stay unchanged. A writable profile runs in the attempt's
//! owned worktree and grants only the abstract tools the admitted contract
//! permits: reading, searching, edits, writes, and development-test commands
//! are independent grants. No profile grants an unrestricted shell,
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
use serde::{Deserialize, Serialize};

pub const CLAUDE_WRITABLE_PROFILE: &str = "claude_stream_json_writable_v1";

/// The exact abstract tools granted to one workflow turn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WritableGrant {
    pub read_files: bool,
    pub search_files: bool,
    pub edit_files: bool,
    pub write_files: bool,
    pub run_dev_tests: bool,
}

impl WritableGrant {
    pub const NONE: Self = Self {
        read_files: false,
        search_files: false,
        edit_files: false,
        write_files: false,
        run_dev_tests: false,
    };

    /// Review and answer turns: read and search only, no mutation or shell.
    pub const READ_ONLY: Self = Self {
        read_files: true,
        search_files: true,
        edit_files: false,
        write_files: false,
        run_dev_tests: false,
    };
}

/// The profile identity recorded in the contract for `protocol`.
pub fn writable_profile_id(protocol: NativeProtocol) -> Result<&'static str, DispatchError> {
    match protocol {
        NativeProtocol::ClaudeStreamJson => Ok(CLAUDE_WRITABLE_PROFILE),
        // Codex read-only/workspace-write is not an exact abstract-tool deny
        // posture. App-server approvals and ACP permission requests are also
        // uncertified; DSH SDK sessions are not workflow workers.
        NativeProtocol::CodexExec
        | NativeProtocol::CodexAppServer
        | NativeProtocol::Acp
        | NativeProtocol::DshSdkJsonRpc => Err(DispatchError::ExecutionDisabled),
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
            let mut tools = Vec::new();
            let mut allowed = Vec::new();
            if grant.read_files {
                tools.push("Read");
                allowed.push("Read");
            }
            if grant.search_files {
                tools.extend(["Glob", "Grep"]);
                allowed.extend(["Glob", "Grep"]);
            }
            if grant.edit_files {
                tools.push("Edit");
                allowed.push("Edit");
            }
            if grant.write_files {
                tools.push("Write");
                allowed.push("Write");
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
        NativeProtocol::CodexExec
        | NativeProtocol::CodexAppServer
        | NativeProtocol::Acp
        | NativeProtocol::DshSdkJsonRpc => {
            return Err(DispatchError::ExecutionDisabled);
        }
    };
    if let Some(model) = &route.model {
        arguments.extend(["--model".to_string(), model.clone()]);
    }
    Ok(LaunchSpec {
        executable: route.executable.clone(),
        arguments,
        environment_names: writable_environment_names(route),
    })
}

pub(crate) fn writable_environment_names(route: &DispatchRoute) -> Vec<String> {
    let mut environment_names: Vec<String> = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for name in &route.environment_names {
        if !environment_names.contains(name) {
            environment_names.push(name.clone());
        }
    }
    environment_names
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
                read_files: true,
                search_files: true,
                edit_files: true,
                write_files: true,
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
    fn each_claude_grant_is_independent_and_empty_means_no_tools() {
        let claude = route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
        let cases = [
            (WritableGrant::NONE, "--tools=", "--allowedTools="),
            (
                WritableGrant {
                    read_files: true,
                    ..WritableGrant::NONE
                },
                "--tools=Read",
                "--allowedTools=Read",
            ),
            (
                WritableGrant {
                    search_files: true,
                    ..WritableGrant::NONE
                },
                "--tools=Glob,Grep",
                "--allowedTools=Glob,Grep",
            ),
            (
                WritableGrant {
                    edit_files: true,
                    ..WritableGrant::NONE
                },
                "--tools=Edit",
                "--allowedTools=Edit",
            ),
            (
                WritableGrant {
                    write_files: true,
                    ..WritableGrant::NONE
                },
                "--tools=Write",
                "--allowedTools=Write",
            ),
            (
                WritableGrant {
                    run_dev_tests: true,
                    ..WritableGrant::NONE
                },
                "--tools=Bash",
                "--allowedTools=Bash(node --test:*)",
            ),
        ];
        for (grant, expected_tools, expected_allowed) in cases {
            let spec = build_writable_launch_spec(&claude, grant).expect("exact Claude grant");
            assert!(
                spec.arguments
                    .iter()
                    .any(|argument| argument == expected_tools)
            );
            assert!(
                spec.arguments
                    .iter()
                    .any(|argument| argument == expected_allowed)
            );
            assert_eq!(
                spec.arguments
                    .iter()
                    .filter(|argument| argument.starts_with("--tools="))
                    .count(),
                1
            );
            assert_eq!(
                spec.arguments
                    .iter()
                    .filter(|argument| argument.starts_with("--allowedTools="))
                    .count(),
                1
            );
        }
        let json = serde_json::to_string(&WritableGrant::READ_ONLY).expect("serialize grant");
        assert_eq!(
            serde_json::from_str::<WritableGrant>(&json).expect("parse strict grant"),
            WritableGrant::READ_ONLY
        );
        assert!(serde_json::from_str::<WritableGrant>(r#"{"read_files":true,"search_files":true,"edit_files":false,"write_files":false,"run_dev_tests":false,"extra":true}"#).is_err());
    }

    #[test]
    fn acp_app_server_and_non_executing_routes_are_refused() {
        for (harness, protocol) in [
            (AgentKind::OpenCode, NativeProtocol::Acp),
            (AgentKind::Codex, NativeProtocol::CodexExec),
            (AgentKind::Codex, NativeProtocol::CodexAppServer),
            (AgentKind::Dsh, NativeProtocol::DshSdkJsonRpc),
        ] {
            assert_eq!(
                writable_profile_id(protocol),
                Err(DispatchError::ExecutionDisabled)
            );
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
