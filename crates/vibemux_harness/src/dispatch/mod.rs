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
//! - [`capability`] states which structured adapters exist for each harness
//!   and whether any of them can execute;
//! - [`launch_spec`] builds the shell-free argv for one route;
//! - [`json_line_framer`], [`capture_budget`], and [`observation`] turn stdout
//!   bytes into bounded, classified, byte-exact records;
//! - [`protocol_session`] drives the per-protocol handshake and correlates
//!   the terminal evidence;
//! - [`outcome`] and [`attempt`] decide the attempt result and its canonical
//!   Task/Run mapping;
//! - [`events`] builds the content-free canonical event drafts.

pub mod attempt;
pub mod capability;
pub mod capture_budget;
pub mod digest;
pub mod error_code;
pub mod events;
pub mod final_text;
#[cfg(feature = "test_helpers")]
pub mod fixture_guard;
pub mod json_line_framer;
pub mod launch_spec;
pub mod observation;
pub mod outcome;
pub mod protocol_session;
pub mod request;
pub mod route_config;
pub mod writable_profile;

pub use attempt::{DispatchPhase, DispatchTrigger, PhaseTransition, TransitionResult};
pub use capability::{AdapterCapability, AdapterSupport, adapter_capabilities, adapter_capability};
pub use digest::Sha256Digest;
pub use error_code::DispatchError;
#[cfg(feature = "test_helpers")]
pub use fixture_guard::{
    FixtureLaunchGuard, build_writable_fixture_launch_spec, fixture_profile_id,
};
pub use observation::{ObservationKind, ObservedRecord};
pub use outcome::{AttemptOutcome, OutcomeDecision, ProcessExit};
pub use protocol_session::{ProtocolSession, SessionRoute, SessionTerminal};
pub use request::{DispatchRequest, PromptDigest};
pub use route_config::{DispatchConfig, DispatchLimits, DispatchRoute, NativeProtocol};

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    /// The `TypeId` of a value's type. Each function item has its own type, so
    /// two paths to one function yield the same id.
    fn type_of<T: 'static>(_: &T) -> TypeId {
        TypeId::of::<T>()
    }

    /// Downstream crates import the dispatch contracts from this root, so
    /// every re-export must stay and must name the item its module defines.
    #[test]
    fn root_reexports_name_their_module_items() {
        let reexports = [
            (
                "DispatchPhase",
                TypeId::of::<super::DispatchPhase>(),
                TypeId::of::<super::attempt::DispatchPhase>(),
            ),
            (
                "DispatchTrigger",
                TypeId::of::<super::DispatchTrigger>(),
                TypeId::of::<super::attempt::DispatchTrigger>(),
            ),
            (
                "PhaseTransition",
                TypeId::of::<super::PhaseTransition>(),
                TypeId::of::<super::attempt::PhaseTransition>(),
            ),
            (
                "TransitionResult",
                TypeId::of::<super::TransitionResult>(),
                TypeId::of::<super::attempt::TransitionResult>(),
            ),
            (
                "AdapterCapability",
                TypeId::of::<super::AdapterCapability>(),
                TypeId::of::<super::capability::AdapterCapability>(),
            ),
            (
                "AdapterSupport",
                TypeId::of::<super::AdapterSupport>(),
                TypeId::of::<super::capability::AdapterSupport>(),
            ),
            (
                "adapter_capabilities",
                type_of(&super::adapter_capabilities),
                type_of(&super::capability::adapter_capabilities),
            ),
            (
                "adapter_capability",
                type_of(&super::adapter_capability),
                type_of(&super::capability::adapter_capability),
            ),
            (
                "Sha256Digest",
                TypeId::of::<super::Sha256Digest>(),
                TypeId::of::<super::digest::Sha256Digest>(),
            ),
            (
                "DispatchError",
                TypeId::of::<super::DispatchError>(),
                TypeId::of::<super::error_code::DispatchError>(),
            ),
            (
                "ObservationKind",
                TypeId::of::<super::ObservationKind>(),
                TypeId::of::<super::observation::ObservationKind>(),
            ),
            (
                "ObservedRecord",
                TypeId::of::<super::ObservedRecord>(),
                TypeId::of::<super::observation::ObservedRecord>(),
            ),
            (
                "AttemptOutcome",
                TypeId::of::<super::AttemptOutcome>(),
                TypeId::of::<super::outcome::AttemptOutcome>(),
            ),
            (
                "OutcomeDecision",
                TypeId::of::<super::OutcomeDecision>(),
                TypeId::of::<super::outcome::OutcomeDecision>(),
            ),
            (
                "ProcessExit",
                TypeId::of::<super::ProcessExit>(),
                TypeId::of::<super::outcome::ProcessExit>(),
            ),
            (
                "ProtocolSession",
                TypeId::of::<super::ProtocolSession>(),
                TypeId::of::<super::protocol_session::ProtocolSession>(),
            ),
            (
                "SessionRoute",
                TypeId::of::<super::SessionRoute>(),
                TypeId::of::<super::protocol_session::SessionRoute>(),
            ),
            (
                "SessionTerminal",
                TypeId::of::<super::SessionTerminal>(),
                TypeId::of::<super::protocol_session::SessionTerminal>(),
            ),
            (
                "DispatchRequest",
                TypeId::of::<super::DispatchRequest>(),
                TypeId::of::<super::request::DispatchRequest>(),
            ),
            (
                "PromptDigest",
                TypeId::of::<super::PromptDigest>(),
                TypeId::of::<super::request::PromptDigest>(),
            ),
            (
                "DispatchConfig",
                TypeId::of::<super::DispatchConfig>(),
                TypeId::of::<super::route_config::DispatchConfig>(),
            ),
            (
                "DispatchLimits",
                TypeId::of::<super::DispatchLimits>(),
                TypeId::of::<super::route_config::DispatchLimits>(),
            ),
            (
                "DispatchRoute",
                TypeId::of::<super::DispatchRoute>(),
                TypeId::of::<super::route_config::DispatchRoute>(),
            ),
            (
                "NativeProtocol",
                TypeId::of::<super::NativeProtocol>(),
                TypeId::of::<super::route_config::NativeProtocol>(),
            ),
        ];
        for (name, root, module) in reexports {
            assert_eq!(root, module, "{name}");
        }
    }

    #[cfg(feature = "test_helpers")]
    #[test]
    fn fixture_guard_reexports_name_their_module_items() {
        let reexports = [
            (
                "FixtureLaunchGuard",
                TypeId::of::<super::FixtureLaunchGuard>(),
                TypeId::of::<super::fixture_guard::FixtureLaunchGuard>(),
            ),
            (
                "build_writable_fixture_launch_spec",
                type_of(&super::build_writable_fixture_launch_spec),
                type_of(&super::fixture_guard::build_writable_fixture_launch_spec),
            ),
            (
                "fixture_profile_id",
                type_of(&super::fixture_profile_id),
                type_of(&super::fixture_guard::fixture_profile_id),
            ),
        ];
        for (name, root, module) in reexports {
            assert_eq!(root, module, "{name}");
        }
    }
}
