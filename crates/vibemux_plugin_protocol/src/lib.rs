#![forbid(unsafe_code)]
//! Isolated Protobuf wire, manifest, negotiation, and lifecycle contracts for plugins.

pub mod error;
pub mod frame;
pub mod lifecycle;
pub mod limits;
pub mod manifest;
pub mod negotiation;
pub mod wire;

pub use error::PluginProtocolError;

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 0;
