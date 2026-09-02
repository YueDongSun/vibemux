#![forbid(unsafe_code)]
//! Plain-text dump entrypoint. Prints the probe-derived view model as
//! ASCII text to stdout. Supersedes the old `--once` flag.

use vibemux_frontend::{ViewModel, tui::snapshot::debug_snapshot};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let report = vibemux_probe::run_probe(&vibemux_probe::ProbeConfig::from_environment()).await;
    let view_model = ViewModel::from_report(&report);
    print!("{}", debug_snapshot(&view_model));
}
