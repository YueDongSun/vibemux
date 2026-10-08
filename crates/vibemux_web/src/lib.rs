#![forbid(unsafe_code)]
//! Operator-started read-only mobile surface; canonical state remains daemon-owned.

pub mod api;
pub mod config;
pub mod control_bridge;
pub mod enrollment;
pub mod private_state;
pub mod push;
pub mod server;
