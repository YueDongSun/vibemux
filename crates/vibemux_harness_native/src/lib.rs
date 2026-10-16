//! Bounded vendor stream adapters and local request dispatch.
//!
//! Observations are not canonical Task/Run events or execution permissions.

pub mod cli;
pub mod config;
pub mod contract;
pub mod dispatch;
pub mod error;
pub mod json_lines;
pub mod native;
pub mod normalize;
pub mod plugin;
mod process;
mod session;

pub use error::Error;
