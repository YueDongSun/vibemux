//! Harness request dispatch: pure contracts and state machines for sending one
//! text prompt to a first-party structured harness adapter (ADR 029).
//!
//! Everything here is deterministic logic over values supplied by the caller.
//! No process, pipe, filesystem, clock, or network access happens in this
//! module tree; `vibemuxd` reads the operator config, spawns the vendor CLI,
//! moves bytes, and persists state through its single writer, applying these
//! rules at each step:
//!
//! - [`request`] and [`route_config`] validate the typed request and the
//!   trusted operator route config;
//! - [`launch_spec`] builds the shell-free argv for one route;
//! - [`json_line_framer`], [`capture_budget`], and [`observation`] turn stdout
//!   bytes into bounded, classified, byte-exact records;
//! - [`protocol_session`] drives the per-protocol handshake and correlates
//!   the terminal evidence;
//! - [`outcome`] and [`attempt`] decide the attempt result and its canonical
//!   Task/Run mapping;
//! - [`events`] builds the content-free canonical event drafts.

pub mod attempt;
pub mod capture_budget;
pub mod digest;
pub mod error_code;
pub mod events;
pub mod json_line_framer;
pub mod launch_spec;
pub mod observation;
pub mod outcome;
pub mod protocol_session;
pub mod request;
pub mod route_config;

pub use attempt::{DispatchPhase, DispatchTrigger, PhaseTransition, TransitionResult};
pub use digest::Sha256Digest;
pub use error_code::DispatchError;
pub use observation::{ObservationKind, ObservedRecord};
pub use outcome::{AttemptOutcome, OutcomeDecision, ProcessExit};
pub use protocol_session::{ProtocolSession, SessionTerminal};
pub use request::{DispatchRequest, PromptDigest};
pub use route_config::{DispatchConfig, DispatchLimits, DispatchRoute, NativeProtocol};
