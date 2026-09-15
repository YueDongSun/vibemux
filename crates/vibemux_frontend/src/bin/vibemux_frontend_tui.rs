#![forbid(unsafe_code)]
//! TUI debug entrypoint. Slim, read-only terminal view of the probe
//! report and the active theme.
//!
//! Flags (ported from the single-shell frontend):
//! - `--json`              print the versioned probe report as JSON and exit
//! - `--theme <name>`      start with classic|high-contrast|mono|light
//!   (`VIBEMUX_FRONTEND_THEME` applies when no flag is given); unknown
//!   themes or unknown arguments exit with code 4.

use std::process::ExitCode;

use vibemux_frontend::{ViewModel, tui};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut theme = tui::Theme::from_environment();
    let mut json = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--theme" => {
                let Some(value) = arguments.next() else {
                    eprintln!(
                        "unknown or missing --theme value (expected one of: classic, high-contrast, mono, light)"
                    );
                    return ExitCode::from(4);
                };
                match tui::Theme::from_name(&value) {
                    Some(resolved) => theme = resolved,
                    None => {
                        eprintln!(
                            "unknown or missing --theme value (expected one of: classic, high-contrast, mono, light)"
                        );
                        return ExitCode::from(4);
                    }
                }
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(4);
            }
        }
    }

    let report = vibemux_probe::run_probe(&vibemux_probe::ProbeConfig::from_environment()).await;
    if json {
        // Machine-readable evidence for scripts and CI; theme selection
        // never affects this output.
        match serde_json::to_string_pretty(&report) {
            Ok(text) => {
                println!("{text}");
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("frontend_tui failed to serialize report: {error}");
                return ExitCode::from(4);
            }
        }
    }

    let view_model = ViewModel::from_report(&report);
    if let Err(error) = tui::run_tui(&view_model, theme) {
        eprintln!("frontend_tui failed: {error}");
        return ExitCode::from(4);
    }
    ExitCode::SUCCESS
}
