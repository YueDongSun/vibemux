//! `vibemuxctl plugins status`: the daemon plugin registry as a read-only
//! Control v2 observation. It cannot register, start, restart, or cancel a
//! plugin; the registry handle behind it is status-only.

use std::ffi::OsString;

use serde_json::{Value, json};
use vibemuxd::process::DaemonPaths;

use crate::{
    CliCommand, DaemonCliError,
    argument_scan::{scan_tail, split_action},
    dispatch_commands::map_dispatch_error,
    harness_client,
};

/// Parses the arguments after the `plugins` verb.
pub(crate) fn parse_plugin_arguments(arguments: &[OsString]) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = split_action(arguments)?;
    if action != "status" {
        return Err(DaemonCliError::InvalidArguments);
    }
    let tail = scan_tail(rest, &[])?;
    tail.without_positionals()?;
    Ok(CliCommand::Plugins {
        project_root: tail.project_root,
    })
}

/// Reads every registered plugin's state from the running daemon.
pub async fn run_plugin_status(paths: &DaemonPaths) -> Result<Value, DaemonCliError> {
    let client = harness_client(paths).await?;
    let plugins = client.plugin_status().await.map_err(map_dispatch_error)?;
    Ok(json!({"ok": true, "plugins": plugins}))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_plugin_arguments(&arguments)
    }

    #[test]
    fn status_takes_only_a_project_root() {
        assert_eq!(
            parse(&["status"]),
            Ok(CliCommand::Plugins { project_root: None })
        );
        assert_eq!(
            parse(&["status", "--project-root", "C:\\project"]),
            Ok(CliCommand::Plugins {
                project_root: Some(PathBuf::from("C:\\project")),
            })
        );
    }

    #[test]
    fn anything_else_is_refused() {
        for arguments in [
            &[][..],
            &["list"][..],
            &["status", "extra"][..],
            &["status", "--limit", "1"][..],
            &["status", "--project-root"][..],
        ] {
            assert_eq!(
                parse(arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }
}
