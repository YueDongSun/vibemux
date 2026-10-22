#![forbid(unsafe_code)]
//! GUI entrypoint. Reads persisted user config (theme + window size),
//! runs the probe, builds a `ViewModel`, and hands it to the egui app.
//!
//! On Windows this is always a GUI-subsystem binary, in debug and release
//! builds alike: launching it opens only the egui window, never a console,
//! the way `pythonw` runs a script. With no console to print to, a startup
//! failure or a panic is written to `frontend_errors.log` next to the
//! settings file (see `vibemux_frontend::startup_log`).

#![cfg_attr(windows, windows_subsystem = "windows")]

use vibemux_frontend::{
    ViewModel, gui, startup_log,
    theme::serialize::{UserConfig, load_user_config},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> eframe::Result<()> {
    startup_log::install_panic_hook();
    let user_config: UserConfig = load_user_config();
    let report = vibemux_probe::run_probe(&vibemux_probe::ProbeConfig::from_environment()).await;
    let view_model = ViewModel::from_report(&report);
    let result = gui::run_gui(view_model, user_config);
    if let Err(error) = &result {
        startup_log::record_startup_error(&error.to_string());
    }
    result
}
