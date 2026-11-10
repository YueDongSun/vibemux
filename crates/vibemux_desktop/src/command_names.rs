//! The complete list of commands the webview may call.
//!
//! This list is the single source of truth: `build.rs` declares it in the
//! Tauri app manifest, the capability files grant the reviewed window subsets,
//! and `tests/lockdown.rs` checks the complete grant and its restrictions.
//! A command that is missing here is denied to the webview.

/// A `static`, not a `const`: `tauri_build::AppManifest::commands` takes a
/// `&'static [&'static str]`.
pub static COMMAND_NAMES: [&str; 25] = [
    "projects_list",
    "project_pick",
    "project_select",
    "project_forget",
    "daemon_start",
    "daemon_stop",
    "tasks_snapshot",
    "supervisor_action",
    "task_window_open",
    "ask_targets",
    "ask_setup",
    "ask_confirm",
    "ask_submit",
    "ask_cancel",
    "ask_status",
    "ask_reply",
    "console_section",
    "probe_rerun",
    "quick_search",
    "open_link",
    "settings_get",
    "settings_set",
    "provider_profiles_get",
    "provider_profiles_set",
    "about_info",
];
