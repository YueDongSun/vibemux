#![forbid(unsafe_code)]
//! GUI-neutral VibeMux client: daemon control, probe, Ask, and Git
//! workspace I/O shared by every frontend (the egui GUI, the read-only
//! console, and a future desktop app).
//!
//! Everything here was extracted unchanged from `vibemux_frontend`'s data
//! and I/O modules, except that `FrontendClient::start` and the Ask workers
//! now take an `Arc<dyn ChangeNotifier>` instead of an `egui::Context`: a
//! pure refactor, not a behavior change. The GUI wraps its repaint context
//! in a newtype that implements `ChangeNotifier`.

pub mod ask;
pub mod change_notifier;
pub mod cli_runner;
pub mod daemon_control;
pub mod daemon_feed;
pub mod daemon_model;
pub mod daemon_session;
pub mod display_text;
pub mod probe_env;
pub mod project_folder;
pub mod quick_search;
pub mod supervisor_client;
pub mod supervisor_mapping;
pub mod supervisor_model;
pub mod view_model;
pub mod workspace_inventory;
