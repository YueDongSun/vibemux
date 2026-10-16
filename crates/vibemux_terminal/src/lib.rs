#![forbid(unsafe_code)]
//! First-party terminal service. Terminal output never changes canonical state.

pub mod client;
pub mod host;
pub mod model;
pub mod protocol;
pub mod workspace;

pub mod wire {
    include!(concat!(env!("OUT_DIR"), "/vibemux.native_terminal.v1.rs"));
}
