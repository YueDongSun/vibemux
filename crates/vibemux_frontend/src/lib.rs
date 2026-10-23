#![forbid(unsafe_code)]
//! VibeMux unified frontend crate.
//!
//! - Read-only TUI debug view (legacy, ASCII-only).
//! - Navigable read-only dashboard (`dashboard`, the `vibemux_dashboard`
//!   binary and `vibemuxctl ui`): probe report plus local workspace inventory.
//! - DSH terminal surface (`dsh_tui`, the `vibemux_dsh_tui` binary): a live
//!   DeepSeek Harness SDK session with an input loop of its own.
//! - egui-based GUI central console (preferred entry point for
//!   humans; theme switcher, harness panels, diagnostics, settings).
//!
//! Both diagnostic surfaces consume the same `view_model::ViewModel` so the
//! backend shape never diverges across UIs. The DSH surface reads its own
//! session driver instead, because it owns a runtime process rather than a
//! read-only probe report.

pub mod ask;
pub mod dashboard;
pub mod dsh_tui;
pub mod gui;
mod live_app;
mod live_ask;
mod live_console;
#[cfg(feature = "gui_screenshot")]
pub mod preview;
pub mod startup_log;
pub mod supervisor_client;
pub mod theme;
pub mod tui;

pub use vibemux_client::{
    daemon_control, probe_env, project_folder, quick_search, supervisor_model, view_model,
};
use vibemux_client::{daemon_session, supervisor_mapping, workspace_inventory};

pub use supervisor_model::*;
pub use view_model::ViewModel;
