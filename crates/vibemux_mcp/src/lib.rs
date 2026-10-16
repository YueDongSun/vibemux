#![forbid(unsafe_code)]
//! First-party, read-only MCP frontend. The daemon remains the state owner.

pub mod config;
pub mod control_reader;
#[cfg(feature = "test_helpers")]
pub mod fixture_support;
pub mod framing;
pub mod projection;
pub mod protocol;
mod reader_owner;
pub mod session;
pub mod supervisor;
pub mod tools;
