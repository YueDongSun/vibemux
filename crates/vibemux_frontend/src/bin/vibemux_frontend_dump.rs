#![forbid(unsafe_code)]
//! Plain-text dump entrypoint. Prints the probe-derived view model as
//! ASCII text to stdout. Supersedes the old `--once` flag.

use std::process::ExitCode;
use vibemux_frontend::{ViewModel, tui::snapshot::debug_snapshot};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let config = match vibemux_frontend::probe_env::probe_config_from_environment() {
        Ok(config) => config,
        Err(code) => {
            eprintln!("{code}");
            return ExitCode::from(4);
        }
    };
    let report = vibemux_probe::run_probe(&config).await;
    let view_model = ViewModel::from_report(&report);
    print!("{}", debug_snapshot(&view_model));
    ExitCode::SUCCESS
}
