#![forbid(unsafe_code)]
//! TUI debug entrypoint. Slim, read-only terminal view of the probe
//! report and the active theme.

use vibemux_frontend::{ViewModel, tui};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let report = vibemux_probe::run_probe(&vibemux_probe::ProbeConfig::from_environment()).await;
    let view_model = ViewModel::from_report(&report);
    if let Err(error) = tui::run_tui(&view_model) {
        eprintln!("frontend_tui failed: {error}");
        std::process::exit(4);
    }
}
