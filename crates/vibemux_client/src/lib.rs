#![forbid(unsafe_code)]
//! GUI-neutral VibeMux client: daemon control, probe, Ask, and Git
//! workspace I/O shared by every frontend (the egui GUI, the read-only
//! console, and a future desktop app).
//!
//! Nothing here renders UI or owns a terminal. Backend code that needs to
//! wake its frontend takes an `Arc<dyn ChangeNotifier>` instead of a
//! frontend-specific repaint handle; the GUI wraps its own repaint context
//! in a newtype that implements `ChangeNotifier`.

pub mod ask;
pub mod change_notifier;
pub(crate) mod cli_runner;
pub mod daemon_control;
pub mod daemon_feed;
pub mod daemon_model;
pub mod daemon_session;
pub mod display_text;
pub mod probe_env;
pub mod project_folder;
pub mod quick_search;
pub mod supervisor_client;
pub(crate) mod supervisor_mapping;
pub mod supervisor_model;
pub mod view_model;
pub mod workspace_inventory;
