#![forbid(unsafe_code)]
//! GUI entrypoint. Reads persisted user config (theme + window size),
//! runs the probe, builds a `ViewModel`, and hands it to the egui app.
//!
//! On Windows this is a GUI-subsystem binary (release): launching it
//! from Explorer or a launcher opens only the egui window, never a
//! console. Debug builds keep the console so `eprintln!` diagnostics
//! are visible when run from a terminal.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use vibemux_frontend::{
    ViewModel, gui,
    theme::serialize::{UserConfig, load_user_config},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> eframe::Result<()> {
    let user_config: UserConfig = load_user_config();
    let report = vibemux_probe::run_probe(&vibemux_probe::ProbeConfig::from_environment()).await;
    let view_model = ViewModel::from_report(&report);
    run_gui(view_model, user_config)
}

fn run_gui(view_model: ViewModel, user_config: UserConfig) -> eframe::Result<()> {
    gui::run_gui(view_model, user_config)
}
