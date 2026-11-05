//! Pure reusable Claude stream-json and Codex app-server sessions.
//!
//! The daemon owns launch profiles, exact resume authority, process trees,
//! deadlines, canonical state and private transcript storage. This module only
//! validates input and vendor correlation and returns bounded typed effects.
//! Existing one-shot dispatch machines and their profiles remain independent.
//!
//! Source contracts: https://learn.chatgpt.com/docs/app-server and
//! https://code.claude.com/docs/en/agent-sdk/typescript#user_message_uuid.
//! Installed qualification inputs: Codex 0.166.0 schemas and Claude 2.1.304 help
//! inspected 2026-10-31. Live persistent execution is a separate qualification.

mod approval;
mod claude;
mod codex;
mod command_progress;
mod decoder;
mod session;
mod text;
mod tool_records;
mod types;
pub use decoder::{MAX_INPUT_CHUNK_BYTES, SessionDecoder};
pub use session::SessionMachine;
pub use types::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "tests/command_progress.rs"]
mod command_progress_tests;
