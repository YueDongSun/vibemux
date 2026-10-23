#![forbid(unsafe_code)]
//! GUI-neutral VibeMux client: daemon control, probe, and Git workspace I/O
//! shared by every frontend (the egui GUI, the read-only console, and a
//! future desktop app).
//!
//! Everything here was extracted unchanged from `vibemux_frontend`'s data
//! and I/O modules: a pure refactor, not a behavior change. `ask/` and
//! `supervisor_client.rs` stay in `vibemux_frontend` until a later task
//! replaces their `egui::Context` dependency with a `ChangeNotifier` trait.

pub mod cli_runner;
pub mod daemon_control;
pub mod daemon_feed;
pub mod daemon_model;
pub mod daemon_session;
pub mod display_text;
pub mod probe_env;
pub mod project_folder;
pub mod quick_search;
pub mod supervisor_mapping;
pub mod supervisor_model;
pub mod view_model;
pub mod workspace_inventory;
