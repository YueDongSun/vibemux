#![forbid(unsafe_code)]
//! The VibeMux desktop app: a Tauri shell around the GUI-neutral
//! `vibemux_client`.
//!
//! The webview holds no daemon credential and no filesystem path. It calls
//! the fixed command list in [`command_names`] and receives content-free
//! events; the only command that returns vendor text is `ask_reply`.

pub mod app_state;
pub mod command_error;
pub mod command_names;
pub mod commands;
pub mod desktop_settings;
pub mod event_sink;
pub mod navigation;
pub mod ui_contract;
pub mod windows;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use crate::app_state::AppState;
use crate::desktop_settings::SETTINGS_FILE_NAME;
use crate::event_sink::{EventSink, TauriEventSink};

/// Starts the desktop app.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let sink: Arc<dyn EventSink> = Arc::new(TauriEventSink::new(handle.clone()));
            app.manage(Arc::new(AppState::new(settings_path(&handle), sink)));
            windows::open_main_window(&handle)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::projects_list,
            commands::project_pick,
            commands::project_select,
            commands::project_forget,
            commands::daemon_start,
            commands::daemon_stop,
            commands::tasks_snapshot,
            commands::supervisor_action,
            commands::task_window_open,
            commands::ask_targets,
            commands::ask_setup,
            commands::ask_confirm,
            commands::ask_submit,
            commands::ask_cancel,
            commands::ask_status,
            commands::ask_reply,
            commands::console_section,
            commands::probe_rerun,
            commands::quick_search,
            commands::open_link,
            commands::settings_get,
            commands::settings_set,
            commands::about_info,
        ])
        .run(tauri::generate_context!())
        .expect("the VibeMux desktop app stopped unexpectedly");
}

/// The settings file in the Tauri app config directory.
fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(SETTINGS_FILE_NAME))
        .unwrap_or_else(|_| PathBuf::from(SETTINGS_FILE_NAME))
}
