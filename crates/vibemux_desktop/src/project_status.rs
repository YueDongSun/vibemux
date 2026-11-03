//! Read-only availability and explicit project mutations over shared clients.

use std::{sync::Arc, time::Duration};

use vibemux_client::{
    ask::{SetupOutcome, worker::spawn_setup},
    change_notifier::NoopNotifier,
    daemon_control::{self, DaemonCommand, DaemonControlResult},
    daemon_session::DaemonSession,
};
use vibemux_harness::{HarnessRow, dispatch::route_config::DispatchCatalogEntry};
use vibemuxd::process::DaemonPaths;

use crate::{
    command_error::CommandError,
    project_registry::failure,
    ui_contract::{AskHarness, AskTarget, AskTargets, DaemonState},
};

const HEALTH_TIMEOUT: Duration = Duration::from_millis(1500);

/// The effect boundary used by services and their deterministic tests.
pub trait ProjectServices: Send + Sync {
    fn status(&self, paths: &DaemonPaths) -> (DaemonState, Option<String>);
    fn targets(&self, paths: &DaemonPaths) -> AskTargets;
    fn lifecycle(&self, paths: &DaemonPaths, command: DaemonCommand) -> Result<(), CommandError>;
    fn setup(&self, paths: &DaemonPaths) -> Result<(), CommandError>;
}

pub struct SharedProjectServices;

fn runtime() -> Result<tokio::runtime::Runtime, CommandError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            failure(
                "desktop_runtime_unavailable",
                "The local operation runtime is unavailable.",
            )
        })
}

impl ProjectServices for SharedProjectServices {
    fn status(&self, paths: &DaemonPaths) -> (DaemonState, Option<String>) {
        let result: Result<(), String> = (|| {
            let runtime = runtime().map_err(|error| error.code)?;
            let (client, _) = DaemonSession::default().connect(paths)?;
            let health = runtime
                .block_on(client.health_with_deadline(HEALTH_TIMEOUT))
                .map_err(|error| error.code().to_owned())?;
            if health.healthy {
                Ok(())
            } else {
                Err("daemon_unhealthy".to_owned())
            }
        })();
        match result {
            Ok(()) => (DaemonState::Running, None),
            Err(code) if code == "daemon_not_running" => (DaemonState::Stopped, Some(code)),
            Err(code) => (DaemonState::Unreachable, Some(safe_code(&code))),
        }
    }

    fn targets(&self, paths: &DaemonPaths) -> AskTargets {
        let result: Result<(Vec<DispatchCatalogEntry>, Vec<HarnessRow>), String> = (|| {
            let runtime = runtime().map_err(|error| error.code)?;
            let (client, _) = DaemonSession::default().connect(paths)?;
            runtime.block_on(async {
                let catalog = client
                    .harness_dispatch_catalog()
                    .await
                    .map_err(|error| error.code().to_owned())?;
                let harnesses = client
                    .harness_snapshot()
                    .await
                    .map_err(|error| error.code().to_owned())?;
                Ok((catalog, harnesses))
            })
        })();
        match result {
            Ok((catalog, harnesses)) => targets_from_catalog(&catalog, &harnesses),
            Err(code) => unavailable_targets(&code),
        }
    }

    fn lifecycle(&self, paths: &DaemonPaths, command: DaemonCommand) -> Result<(), CommandError> {
        match daemon_control::run(command, paths.project_root()) {
            DaemonControlResult::Done { .. } => Ok(()),
            DaemonControlResult::Failed { code, .. } => Err(failure(
                &safe_code(&code),
                "The daemon operation did not complete. Check the installed daemon and CLI, then refresh its status.",
            )),
        }
    }

    fn setup(&self, paths: &DaemonPaths) -> Result<(), CommandError> {
        // Refuse before the shared worker can write detection or configuration.
        if vibemux_client::ask::routes::config_path(paths.project_root()).exists() {
            return Err(failure(
                "ask_config_exists",
                "Existing harness routes were preserved. Edit them outside the app, then explicitly restart the daemon.",
            ));
        }
        if self.status(paths).0 != DaemonState::Stopped {
            return Err(failure(
                "daemon_stop_required",
                "Stop the daemon explicitly before configuring routes. Setup may start and restart it.",
            ));
        }
        let receiver = spawn_setup(paths.project_root().to_owned(), Arc::new(NoopNotifier));
        match receiver.recv() {
            Ok(SetupOutcome::Done { .. }) => Ok(()),
            // Shared setup messages may contain paths; expose fixed copy only.
            Ok(SetupOutcome::Failed(_)) | Err(_) => Err(failure(
                "ask_setup_failed",
                "Setup did not complete. Build the companion binaries and install a detected native Claude Code or Codex executable; existing routes are preserved.",
            )),
        }
    }
}

pub fn targets_from_catalog(
    catalog: &[DispatchCatalogEntry],
    harnesses: &[HarnessRow],
) -> AskTargets {
    let targets = [AskHarness::Claude, AskHarness::Codex].into_iter().map(|harness| {
        let reason = match catalog.iter().find(|route| route.harness == harness.agent()) {
            None => Some("No route is configured for this harness."),
            Some(route) if !route.protocol.accepts(harness.agent()) || !route.protocol.supports_read_only_dispatch() => Some("This route protocol is not supported for a one-turn Ask."),
            Some(route) if !route.enabled => Some("This route is disabled in the project configuration."),
            Some(route) if !route.allow_execution => Some("This route does not allow execution."),
            Some(_) if !harnesses.iter().any(|row| row.name == harness.agent().command_name() && row.available) => Some("The daemon has no available detection for this harness. Run explicit setup or refresh detection with the CLI."),
            Some(_) => None,
        };
        AskTarget { harness, label: harness.label().to_owned(), available: reason.is_none(), reason: reason.map(str::to_owned) }
    }).collect();
    AskTargets {
        targets,
        setup_offered: false,
        code: None,
    }
}

pub fn unavailable_targets(code: &str) -> AskTargets {
    let reason = match code {
        "daemon_not_running" => "Start the project daemon to inspect its routes.",
        "harness_dispatch_unconfigured" => {
            "No harness routes are configured. Setup is an explicit action and may restart the daemon."
        }
        "control_unsupported_version" => {
            "This daemon is too old for Ask (needs Control v5 or newer). Upgrade the CLI and daemon together."
        }
        _ => {
            "The authenticated daemon connection or route catalog is unavailable. Refresh after resolving the reported code."
        }
    };
    AskTargets {
        targets: [AskHarness::Claude, AskHarness::Codex]
            .into_iter()
            .map(|harness| AskTarget {
                harness,
                label: harness.label().to_owned(),
                available: false,
                reason: Some(reason.to_owned()),
            })
            .collect(),
        setup_offered: matches!(code, "harness_dispatch_unconfigured" | "daemon_not_running"),
        code: Some(safe_code(code)),
    }
}

pub(crate) fn safe_code(code: &str) -> String {
    if code.len() <= 96
        && !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        code.to_owned()
    } else {
        "desktop_operation_failed".to_owned()
    }
}
