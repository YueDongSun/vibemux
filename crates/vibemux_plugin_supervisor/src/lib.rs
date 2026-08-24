#![forbid(unsafe_code)]
//! Bounded out-of-process plugin supervision over the validated v1 protocol.

pub mod error;
pub mod launch;
pub mod report;
pub mod supervisor;

pub use error::PluginSupervisorError;
pub use launch::ResolvedPluginLaunch;
pub use report::{PluginExitReport, PluginStderrReport};
pub use supervisor::{PluginSession, PluginSupervisorConfig, spawn_plugin};
