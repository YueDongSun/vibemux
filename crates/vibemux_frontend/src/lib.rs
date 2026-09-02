#![forbid(unsafe_code)]
//! VibeMux unified frontend crate.
//!
//! - Read-only TUI debug view (legacy, ASCII-only).
//! - egui-based GUI central console (preferred entry point for
//!   humans; theme switcher, harness panels, diagnostics, settings).
//!
//! Both surfaces consume the same `view_model::ViewModel` so the
//! backend shape never diverges across UIs.

pub mod gui;
pub mod probe_env;
pub mod theme;
pub mod tui;
pub mod view_model;

pub use view_model::ViewModel;
