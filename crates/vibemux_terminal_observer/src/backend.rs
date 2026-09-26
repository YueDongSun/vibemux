//! Read-only WezTerm inventory and explicit focus. Terminal text stays native.

use crate::config::ObserverConfig;
use serde::Deserialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use vibemux_plugin_protocol::{
    terminal::{MAX_CWD_BYTES, MAX_TERMINAL_PANES, valid_inventory, valid_label},
    wire::{TerminalFocusRequest, TerminalInventory, TerminalPane},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

pub async fn inventory(
    config: &ObserverConfig,
    expected_cwd: &str,
) -> Result<TerminalInventory, &'static str> {
    if !valid_label(expected_cwd, MAX_CWD_BYTES) || !Path::new(expected_cwd).is_absolute() {
        return Err("terminal_invalid_cwd");
    }
    let instance_id = config.verify_instance()?;
    let output = command(config, &["list", "--format", "json"]).await?;
    let panes = parse_inventory(&output, expected_cwd)?;
    if config.verify_instance()? != instance_id {
        return Err("terminal_instance_changed");
    }
    let inventory = TerminalInventory { instance_id, panes };
    if !valid_inventory(&inventory) {
        return Err("terminal_invalid_inventory");
    }
    Ok(inventory)
}

pub async fn focus(
    config: &ObserverConfig,
    request: &TerminalFocusRequest,
) -> Result<(), &'static str> {
    let current = inventory(config, &request.expected_cwd).await?;
    if current.instance_id != request.instance_id
        || !current
            .panes
            .iter()
            .any(|pane| pane.pane_id == request.pane_id)
    {
        return Err("terminal_identity_mismatch");
    }
    command(config, &["activate-pane", "--pane-id", &request.pane_id]).await?;
    if config.verify_instance()? != request.instance_id {
        return Err("terminal_instance_changed");
    }
    Ok(())
}

async fn command(config: &ObserverConfig, arguments: &[&str]) -> Result<Vec<u8>, &'static str> {
    let mut command = Command::new(&config.wezterm_executable);
    command
        .args(["--skip-config", "cli", "--no-auto-start"])
        .args(arguments)
        .env_clear()
        .env("WEZTERM_UNIX_SOCKET", &config.socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Windows loader support only; no provider or shell environment inheritance.
    for key in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| "terminal_command_failed")?;
    let stdout = child.stdout.take().ok_or("terminal_command_failed")?;
    let result = timeout(COMMAND_TIMEOUT, async {
        let mut output = Vec::new();
        stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "terminal_command_failed")?;
        if output.len() > MAX_OUTPUT_BYTES {
            return Err("terminal_output_too_large");
        }
        let status = child.wait().await.map_err(|_| "terminal_command_failed")?;
        if !status.success() {
            return Err("terminal_command_failed");
        }
        Ok(output)
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        outcome => {
            let _ = child.start_kill();
            let _ = timeout(COMMAND_TIMEOUT, child.wait()).await;
            outcome.unwrap_or(Err("terminal_command_timeout"))
        }
    }
}

#[derive(Deserialize)]
struct WeztermPane {
    pane_id: u64,
    window_id: u64,
    tab_id: u64,
    cwd: String,
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    is_dead: bool,
}

/// File-identity comparison (volume serial + file index on Windows, device +
/// inode elsewhere), the same check the daemon applies to every returned pane.
/// Comparing canonical paths lexically could drop a valid pane whose reported
/// spelling differs in case, which `canonicalize` does not promise to fold.
pub fn same_directory(left: &str, right: &str) -> bool {
    same_file::is_same_file(left, right).unwrap_or(false)
}

pub fn parse_inventory(
    bytes: &[u8],
    expected_cwd: &str,
) -> Result<Vec<TerminalPane>, &'static str> {
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err("terminal_output_too_large");
    }
    let source: Vec<WeztermPane> =
        serde_json::from_slice(bytes).map_err(|_| "terminal_invalid_inventory")?;
    let mut panes = Vec::new();
    for pane in source {
        if pane.is_dead {
            continue;
        }
        // WezTerm emits file://HOST/path URLs. Reject remote hosts instead of
        // converting them into local paths or UNC network access.
        let cwd = if pane.cwd.starts_with("file:") {
            let mut url = url::Url::parse(&pane.cwd).map_err(|_| "terminal_invalid_inventory")?;
            if let Some(host) = url.host_str() {
                let local_host = sysinfo::System::host_name().unwrap_or_default();
                if !host.eq_ignore_ascii_case("localhost")
                    && !host.eq_ignore_ascii_case(&local_host)
                {
                    continue;
                }
                url.set_host(None)
                    .map_err(|_| "terminal_invalid_inventory")?;
            }
            url.to_file_path()
                .map_err(|_| "terminal_invalid_inventory")?
                .to_string_lossy()
                .into_owned()
        } else {
            pane.cwd
        };
        if !same_directory(&cwd, expected_cwd) {
            continue;
        }
        if panes.len() >= MAX_TERMINAL_PANES {
            return Err("terminal_inventory_capacity");
        }
        panes.push(TerminalPane {
            pane_id: pane.pane_id.to_string(),
            cwd,
            workspace: pane.workspace,
            window_id: pane.window_id.to_string(),
            tab_id: pane.tab_id.to_string(),
        });
    }
    Ok(panes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_filters_cwd_without_returning_terminal_titles() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":cwd,"title":"PRIVATE PROMPT"},
            {"pane_id":4,"window_id":2,"tab_id":3,"cwd":"missing_directory"}
        ]);
        let panes = parse_inventory(&serde_json::to_vec(&input).unwrap(), &cwd).unwrap();
        assert_eq!(panes.len(), 1);
        assert!(!serde_json::to_string(&panes).unwrap().contains("PRIVATE"));
    }
    #[test]
    fn invalid_and_oversized_inventory_is_rejected() {
        assert!(parse_inventory(b"invalid", "missing").is_err());
        assert!(parse_inventory(&vec![b' '; MAX_OUTPUT_BYTES + 1], "missing").is_err());
        assert!(!same_directory("missing", "missing"));
    }
    #[test]
    fn directory_identity_ignores_path_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        assert!(same_directory(
            &cwd,
            &format!("{cwd}{}.", std::path::MAIN_SEPARATOR)
        ));
        assert!(!same_directory(&cwd, &other.path().to_string_lossy()));
    }
    #[cfg(windows)]
    #[test]
    fn windows_inventory_matches_differently_cased_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        let reported = format!(
            "file:///{}",
            cwd.to_uppercase().replace(std::path::MAIN_SEPARATOR, "/")
        );
        let input = serde_json::json!([{"pane_id":1,"window_id":2,"tab_id":3,"cwd":reported}]);
        let panes =
            parse_inventory(&serde_json::to_vec(&input).unwrap(), &cwd.to_lowercase()).unwrap();
        assert_eq!(panes.len(), 1);
    }
}
