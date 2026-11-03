//! Bounded, generation-fenced persistent harness process owners.
//!
//! Canonical admission and private transcript persistence stay with the daemon
//! writer. This runtime claims each admitted turn once before vendor input.

mod actor;
pub mod api;
pub mod approval_api;
#[cfg(feature = "test_helpers")]
pub mod approval_diagnostic;
mod approval_runtime;
mod contracts;
pub mod execution_api;
mod execution_sink;
#[cfg(feature = "test_helpers")]
pub mod fixture_runtime;
mod input_writer;
pub mod owner_api;
mod service;
mod sink_state;
mod transport;
mod writer_sink;
pub(crate) use execution_sink::ChatExecutionSink;
pub(crate) use writer_sink::ChatWriterSink;

pub use service::{ChatService, ChatServiceError, ChatSessionAction};

pub(crate) use actor::spawn_runtime;
pub use contracts::*;

#[cfg(test)]
mod tests;

#[cfg(feature = "test_helpers")]
pub(crate) mod approval_probe;

#[cfg(all(test, feature = "test_helpers", windows))]
mod native_tests;
