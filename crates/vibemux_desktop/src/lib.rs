#![forbid(unsafe_code)]
//! The VibeMux desktop app: a Tauri shell around the GUI-neutral
//! `vibemux_client`.
//!
//! The webview holds no daemon credential and no filesystem path. It calls
//! the fixed command list in [`command_names`] and receives content-free
//! events. Vendor text is returned through `ask_reply` and the explicit private
//! read variants of `chat_request`.

pub mod app_state;
pub mod ask_registry;
mod chat_bridge;
mod chat_export;
pub mod command_arguments;
pub mod command_error;
pub mod command_names;
pub mod commands;
pub mod desktop_settings;
pub mod event_sink;
mod link_bridge;
pub mod link_contract;
pub mod link_effects;
#[cfg(all(windows, feature = "native_qa"))]
pub mod native_qa;
pub mod navigation;
pub mod probe_contract;
pub mod probe_registry;
pub mod project_registry;
pub mod project_status;
pub mod read_contract;
pub mod read_services;
pub mod task_windows;
pub mod terminal_bridge;
pub mod ui_contract;
pub mod windows;
pub mod workspace_cleanup_bridge;
mod workspace_path_pin;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use crate::app_state::AppState;
use crate::desktop_settings::SETTINGS_FILE_NAME;
use crate::event_sink::{EventSink, TauriEventSink};

/// Rust-only launch selection. Default builds expose only the standard variant.
pub(crate) enum LaunchMode {
    Standard,
    #[cfg(all(windows, feature = "native_qa"))]
    NativeQa {
        settings_path: PathBuf,
        webview_data_dir: PathBuf,
        state_capture: Arc<std::sync::OnceLock<Arc<AppState>>>,
    },
}

/// Builds the same desktop shell for its normal entry point and isolated QA.
pub(crate) fn build_app(mode: LaunchMode) -> tauri::Result<tauri::App<tauri::Wry>> {
    tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink: Arc<dyn EventSink> = Arc::new(TauriEventSink::new(handle.clone()));
            let settings_file = match &mode {
                LaunchMode::Standard => settings_path(&handle),
                #[cfg(all(windows, feature = "native_qa"))]
                LaunchMode::NativeQa { settings_path, .. } => settings_path.clone(),
            };
            let state = match &mode {
                LaunchMode::Standard => AppState::new(settings_file, sink),
                #[cfg(all(windows, feature = "native_qa"))]
                LaunchMode::NativeQa { .. } => native_qa::app_state(settings_file, sink),
            };
            let state = Arc::new(state);
            app.manage(Arc::clone(&state));
            #[cfg(all(windows, feature = "native_qa"))]
            if let LaunchMode::NativeQa { state_capture, .. } = &mode
                && let Some(managed) = app.try_state::<Arc<AppState>>()
            {
                let _ = state_capture.set(Arc::clone(&managed));
            }
            let profile = match &mode {
                LaunchMode::Standard => windows::WindowProfile::default(),
                #[cfg(all(windows, feature = "native_qa"))]
                LaunchMode::NativeQa {
                    webview_data_dir, ..
                } => windows::WindowProfile::native_qa(webview_data_dir.clone()),
            };
            app.manage(profile);
            match &mode {
                LaunchMode::Standard => {
                    windows::open_main_window(&handle)?;
                }
                #[cfg(all(windows, feature = "native_qa"))]
                LaunchMode::NativeQa {
                    webview_data_dir, ..
                } => {
                    windows::open_native_qa_window(&handle, webview_data_dir)?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::Destroyed)
                && let Some(state) = window.try_state::<Arc<AppState>>()
            {
                state.close_main_view();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::projects_list,
            commands::chat_confirm,
            commands::chat_pick_files,
            commands::chat_request,
            commands::chat_owner_request,
            commands::chat_execution_request,
            commands::chat_approval_request,
            commands::chat_export_request,
            commands::chat_search_request,
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
            commands::workspace_cleanup_request,
            commands::settings_get,
            commands::settings_set,
            commands::about_info,
        ])
        .build(tauri::generate_context!())
}

/// Starts the desktop app.
pub fn run() {
    build_app(LaunchMode::Standard)
        .expect("the VibeMux desktop app could not start")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                reconcile_app_exit(app);
            }
        });
}

/// Reconciles only app-owned Asks for either desktop event-loop owner.
pub(crate) fn reconcile_app_exit(app: &tauri::AppHandle) -> usize {
    // Cancel only app-owned work; no daemon is stopped here.
    let state = app.state::<Arc<AppState>>();
    state.probes.begin_shutdown();
    let unresolved = state.asks.shutdown();
    let unresolved_probes = state.probes.shutdown();
    if unresolved > 0 {
        eprintln!("desktop_shutdown_unreconciled: {unresolved} Ask outcomes remain unknown");
    }
    if unresolved_probes > 0 {
        eprintln!(
            "desktop_diagnostics_unreconciled: {unresolved_probes} local check outcomes remain unknown"
        );
    }
    unresolved
}

/// The settings file in the Tauri app config directory.
fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(SETTINGS_FILE_NAME))
        .unwrap_or_else(|_| PathBuf::from(SETTINGS_FILE_NAME))
}
