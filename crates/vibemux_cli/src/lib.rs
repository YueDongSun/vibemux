#![forbid(unsafe_code)]
//! Unified local CLI with backward-compatible daemon client exports.

pub mod application;
pub mod command;
mod daemon_client;
mod error;
pub mod failure;
pub mod output;
mod parser;
pub use vibemux_probe::snapshot as probe;
pub mod recovery;
pub mod workspace;

pub use daemon_client::{
    DEFAULT_HEALTH_PROBE_TIMEOUT, DEFAULT_POLL_INTERVAL, DEFAULT_SHUTDOWN_TIMEOUT,
    DEFAULT_STARTUP_TIMEOUT, DaemonBootstrapConfig, DaemonStartOutcome, daemon_executable,
    daemon_health, daemon_paths, start_daemon, stop_daemon,
};
pub use error::DaemonCliError;
pub use parser::{CliCommand, parse_cli_arguments};
