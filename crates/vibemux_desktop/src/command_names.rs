//! The complete list of commands the webview may call.
//!
//! This list is the single source of truth: `build.rs` declares it in the
//! Tauri app manifest, `capabilities/main_window.json` grants exactly these
//! commands, and `tests/lockdown.rs` checks all three against each other.
//! A command that is missing here is denied to the webview.

/// A `static`, not a `const`: `tauri_build::AppManifest::commands` takes a
/// `&'static [&'static str]`.
pub static COMMAND_NAMES: [&str; 23] = [
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
    "about_info",
];
