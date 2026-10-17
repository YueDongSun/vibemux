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

use super::{DispatchError, DispatchRoute, NativeProtocol, route_config};

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

/// Restore the classic spelling of a Windows verbatim path that is handed to an
/// interpreter as an argument.
///
/// `std::fs::canonicalize` returns `\\?\C:\...` on Windows. `CreateProcess`
/// accepts that spelling for the image, but Node's CommonJS loader does not: it
/// mis-splits the verbatim form, tries `lstat("C:")`, and exits before loading
/// the script, so a launcher-pair session dies before its first frame and the
/// driver reports a closed transport. The canonical form stays the trust
/// anchor — `canonical_outside_root` decides what is admissible — and only the
/// argument the interpreter receives is restored to the spelling its loader can
/// open.
///
/// `\\?\UNC\server\share\x` maps back to `\\server\share\x`. A verbatim path
/// with no classic equivalent (a volume GUID path) is returned unchanged,
/// because inventing a spelling would be worse than failing loudly.
#[must_use]
pub fn interpreter_script_path(script: &str) -> String {
    const VERBATIM: &str = r"\\?\";
    let Some(rest) = script.strip_prefix(VERBATIM) else {
        return script.to_string();
    };
    let bytes = rest.as_bytes();
    // `\\?\UNC\server\share` is the verbatim spelling of `\\server\share`.
    if bytes.len() >= 4 && bytes[..4].eq_ignore_ascii_case(b"UNC\\") {
        return format!(r"\\{}", &rest[4..]);
    }
    // Only a drive path has a classic equivalent: `\\?\C:\dir\script.js`.
    // A bare volume root (`\\?\C:`) is not the same path as `C:`, so it stays.
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return rest.to_string();
    }
    script.to_string()
}

pub fn build_launch_spec(
    route: &DispatchRoute,
    mode: SessionMode,
) -> Result<LaunchSpec, DispatchError> {
    if !route.protocol.accepts(route.harness) {
        return Err(DispatchError::ConfigRouteInvalid);
    }
    // Repeats the admission gate so no caller can build a prompt-bearing
    // launch for a route that may not execute.
    if mode == SessionMode::Execute
        && (!route.allow_execution || !route.protocol.supports_execution())
    {
        return Err(DispatchError::ExecutionDisabled);
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
        // The launcher pair: `node <entry script> --profile <profile>`. The
        // entry script is the interpreter's first argument, the profile comes
        // from the validated route config, and nothing else reaches argv. The
        // provider, model, and reasoning effort travel in `initialize`
        // params, and the prompt travels in `session/prompt` params, so no
        // prompt text can reach a command line (ADR 032 Decision 3).
        //
        // The script is canonicalized when the route is admitted, which on
        // Windows yields a verbatim `\\?\` path. Node cannot load a script from
        // that spelling, so the argument is de-verbatimized while the admitted
        // canonical form remains the trust anchor.
        NativeProtocol::DshSdkJsonRpc => {
            let script = route
                .entry_script
                .as_ref()
                .ok_or(DispatchError::ConfigRouteInvalid)?;
            let script = script.to_str().ok_or(DispatchError::ConfigRouteInvalid)?;
            let profile = route.effective_profile().to_string();
            if !route_config::profile_is_valid(&profile) {
                return Err(DispatchError::ConfigRouteInvalid);
            }
            vec![
                interpreter_script_path(script),
                "--profile".to_string(),
                profile,
            ]
        }
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

#[cfg(test)]
mod tests {
    use super::interpreter_script_path;

    #[test]
    fn verbatim_drive_paths_lose_the_prefix() {
        assert_eq!(
            interpreter_script_path(
                r"\\?\C:\Users\user\AppData\Roaming\npm\node_modules\@deepseek-ai\dsh\lib\bin.js"
            ),
            r"C:\Users\user\AppData\Roaming\npm\node_modules\@deepseek-ai\dsh\lib\bin.js"
        );
        assert_eq!(
            interpreter_script_path(r"\\?\c:\dsh\bin.js"),
            r"c:\dsh\bin.js"
        );
        assert_eq!(interpreter_script_path(r"\\?\C:\"), r"C:\");
    }

    #[test]
    fn verbatim_unc_paths_return_to_the_share_spelling() {
        assert_eq!(
            interpreter_script_path(r"\\?\UNC\server\share\bin.js"),
            r"\\server\share\bin.js"
        );
        assert_eq!(
            interpreter_script_path(r"\\?\unc\server\share\bin.js"),
            r"\\server\share\bin.js"
        );
    }

    #[test]
    fn classic_paths_are_unchanged() {
        assert_eq!(interpreter_script_path(r"C:\dsh\bin.js"), r"C:\dsh\bin.js");
        assert_eq!(
            interpreter_script_path("/usr/local/lib/dsh/bin.js"),
            "/usr/local/lib/dsh/bin.js"
        );
        assert_eq!(interpreter_script_path("bin.js"), "bin.js");
        assert_eq!(interpreter_script_path(""), "");
    }

    #[test]
    fn verbatim_paths_without_a_classic_equivalent_are_unchanged() {
        let volume = r"\\?\Volume{1e0f0e2a-0000-0000-0000-100000000000}\dsh\bin.js";
        assert_eq!(interpreter_script_path(volume), volume);
        // The verbatim volume root is not the drive-relative path `C:`.
        assert_eq!(interpreter_script_path(r"\\?\C:"), r"\\?\C:");
        assert_eq!(interpreter_script_path(r"\\?\UNC"), r"\\?\UNC");
    }
}
