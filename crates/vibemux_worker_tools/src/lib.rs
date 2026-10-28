#![forbid(unsafe_code)]
//! Closed worker effects over a fresh owned text snapshot (ADR 044).
//!
//! This component is not a daemon admission authority or an OS sandbox.
//! The caller owns the trusted snapshot, attempt binding and process lifecycle.

pub mod contract;
pub mod dev_tests;
pub mod owner_admission;
pub mod owner_output;
pub mod protocol;
pub mod server;
pub mod test_server;
pub mod test_session;
pub mod workspace;
