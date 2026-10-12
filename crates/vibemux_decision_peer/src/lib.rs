#![forbid(unsafe_code)]
//! The one-shot out-of-process decision peer (ADR 034 §6).
//!
//! `vibemuxd` performs no decision-API network I/O; it spawns this peer,
//! writes one [`vibemux_decision::wire::PeerRequest`] line to its stdin, and
//! reads one [`vibemux_decision::wire::PeerResult`] line from its stdout.
//! This crate owns the only network call: [`jev_client`] is the bounded
//! HTTP adapter and [`peer_main`] is the run loop the binary calls.

pub mod jev_client;
pub mod peer_main;

pub use jev_client::{ClientLimits, Credential, JevClient};
