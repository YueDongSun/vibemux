//! The typed commands the webview may call.
//!
//! Every command is a thin wrapper: it converts the arguments, and a
//! service does the work. The list is complete from this step on, so the
//! capability file and the app manifest stay stable; the commands that
//! belong to a later step answer `not_implemented` until then.
//!
//! The webview never passes a filesystem path. It passes the `project_key`
//! of a folder the Rust side already knows.

use std::sync::Arc;

use tauri::State;

use crate::app_state::AppState;
use crate::command_error::CommandError;
use crate::desktop_settings::{DesktopSettings, SettingsPatch};
use crate::ui_contract::AboutInfo;

fn not_implemented(command: &str) -> CommandError {
    CommandError::new(
        "not_implemented",
        format!("{command} arrives with a later step of the desktop app."),
    )
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_list(_state: State<'_, Arc<AppState>>) -> Result<(), CommandError> {
    Err(not_implemented("projects_list"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn project_pick(_state: State<'_, Arc<AppState>>) -> Result<(), CommandError> {
    Err(not_implemented("project_pick"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn project_select(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("project_select"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn project_forget(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("project_forget"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn daemon_start(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("daemon_start"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn daemon_stop(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("daemon_stop"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tasks_snapshot(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("tasks_snapshot"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn supervisor_action(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
    _action: serde_json::Value,
) -> Result<(), CommandError> {
    Err(not_implemented("supervisor_action"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn task_window_open(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
    _task_id: String,
) -> Result<(), CommandError> {
    Err(not_implemented("task_window_open"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_targets(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_targets"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_setup(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_setup"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_confirm(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_confirm"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_submit(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
    _harness: String,
    _prompt: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_submit"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_cancel(
    _state: State<'_, Arc<AppState>>,
    _ask_id: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_cancel"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_status(
    _state: State<'_, Arc<AppState>>,
    _ask_id: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_status"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ask_reply(
    _state: State<'_, Arc<AppState>>,
    _ask_id: String,
) -> Result<(), CommandError> {
    Err(not_implemented("ask_reply"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn console_section(
    _state: State<'_, Arc<AppState>>,
    _project_key: String,
    _section: String,
) -> Result<(), CommandError> {
    Err(not_implemented("console_section"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn probe_rerun(_state: State<'_, Arc<AppState>>) -> Result<(), CommandError> {
    Err(not_implemented("probe_rerun"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quick_search(
    _state: State<'_, Arc<AppState>>,
    _query: String,
) -> Result<(), CommandError> {
    Err(not_implemented("quick_search"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_link(_state: State<'_, Arc<AppState>>, _url: String) -> Result<(), CommandError> {
    Err(not_implemented("open_link"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_get(
    state: State<'_, Arc<AppState>>,
) -> Result<DesktopSettings, CommandError> {
    Ok(state.settings())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_set(
    state: State<'_, Arc<AppState>>,
    patch: SettingsPatch,
) -> Result<DesktopSettings, CommandError> {
    let worker_state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || worker_state.apply_settings(&patch))
        .await
        .map_err(|error| CommandError::new("settings_write_failed", error.to_string()))?
        .map_err(|error| CommandError::new("settings_write_failed", error.to_string()))?;
    Ok(state.settings())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn about_info() -> Result<AboutInfo, CommandError> {
    Ok(AboutInfo {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        webview_version: tauri::webview_version().unwrap_or_else(|_| "unknown".to_owned()),
    })
}
