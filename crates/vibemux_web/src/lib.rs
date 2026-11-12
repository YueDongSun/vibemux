#![forbid(unsafe_code)]
//! Operator-started read-only mobile surface; canonical state remains daemon-owned.

pub mod api;
pub mod config;
pub mod console_model;
pub mod console_poller;
pub mod content_label;
pub mod control_bridge;
pub mod enrollment;
pub mod local_page;
pub mod private_state;
#[cfg(feature = "push")]
pub mod push;
#[cfg(feature = "push")]
mod push_message;
pub mod server;
#[cfg(feature = "push")]
mod vapid;
