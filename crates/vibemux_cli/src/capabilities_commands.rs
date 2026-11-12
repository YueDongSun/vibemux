//! `vibemuxctl capabilities`: the daemon's content-free capability catalog.
//!
//! The operation is additive at Control v10. A daemon that predates it answers
//! with an invalid frame; the CLI reports `control_capabilities_unavailable`
//! instead of leaking the generic frame error. The command never starts a
//! daemon, a probe, or a model call.

use std::ffi::OsString;

use serde_json::{Value, json};
use vibemuxd::control::ControlError;
use vibemuxd::process::DaemonPaths;

use crate::{
    CliCommand, DaemonCliError, argument_scan::scan_tail, dispatch_commands::map_dispatch_error,
    harness_client,
};

/// Parses the arguments after the `capabilities` verb: only `--project-root`.
pub(crate) fn parse_capability_arguments(
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let tail = scan_tail(arguments, &[])?;
    tail.without_positionals()?;
    Ok(CliCommand::Capabilities {
        project_root: tail.project_root,
    })
}

/// Reads the catalog from the running daemon.
pub async fn run_capabilities(paths: &DaemonPaths) -> Result<Value, DaemonCliError> {
    let client = harness_client(paths).await?;
    let catalog = client.capabilities().await.map_err(|error| match error {
        // Older v10 daemons do not know the additive operation and drop the
        // request as an invalid frame. Name that condition instead of
        // surfacing the transport detail.
        ControlError::InvalidFrame => DaemonCliError::Control {
            code: "control_capabilities_unavailable".to_string(),
        },
        other => map_dispatch_error(other),
    })?;
    Ok(json!({"ok": true, "capabilities": catalog}))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_capability_arguments(&arguments)
    }

    #[test]
    fn capabilities_takes_only_a_project_root() {
        assert_eq!(
            parse(&[]),
            Ok(CliCommand::Capabilities { project_root: None })
        );
        assert_eq!(
            parse(&["--project-root", "C:\\project"]),
            Ok(CliCommand::Capabilities {
                project_root: Some(PathBuf::from("C:\\project")),
            })
        );
    }

    #[test]
    fn positionals_and_unknown_flags_are_refused() {
        for arguments in [
            &["extra"][..],
            &["--limit", "1"][..],
            &["--project-root"][..],
            &["--project-root", "--unknown"][..],
        ] {
            assert_eq!(
                parse(arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }
}
