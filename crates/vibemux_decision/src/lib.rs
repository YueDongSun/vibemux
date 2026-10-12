#![forbid(unsafe_code)]
//! Pure contracts for advisory orchestration decisions (ADR 034).
//!
//! This crate performs no I/O and reads no clock. It defines the typed
//! question and its closed option set, the enumerated facts a backend may
//! see, backend answer validation and the confidence gate, operator local
//! rules, the deterministic fallback resolver, the operator config schema,
//! the persisted content-free record, its canonical event drafts, and the
//! line protocol spoken with the out-of-process decision peer. `vibemuxd`
//! performs every effect, `vibemux_store` persists every state change
//! through the single writer, and `vibemux_decision_peer` owns the network.

pub mod answer;
pub mod config;
pub mod error;
pub mod events;
pub mod facts;
pub mod identifiers;
pub mod kind;
pub mod probability;
pub mod question;
pub mod record;
pub mod resolve;
pub mod rules;
pub mod wire;

pub use answer::{BackendAnswer, OptionProbability, ValidatedAnswer};
pub use config::{DecisionConfig, DecisionConfigError, JevConfig};
pub use error::{DecisionError, ExternalError};
pub use facts::{PromptSizeBucket, RouteFacts};
pub use identifiers::{ESCALATE_OPTION, Label, OptionId};
pub use kind::DecisionKind;
pub use probability::{Millionths, Threshold};
pub use question::{DecisionQuestion, QuestionOption};
pub use record::{DECISION_TTL, DecisionConsumption, DecisionRecord, NewDecision};
pub use resolve::{
    DecisionSource, DecisionStatus, ExternalEvidence, ExternalResult, ExternalStatus,
    RouteResolution, SkipReason, plan_external, resolve_route,
};
pub use rules::LocalRule;
pub use vibemux_harness::AgentKind;
pub use vibemux_harness::dispatch::{PromptDigest, Sha256Digest};
pub use wire::{DecisionBackend, PeerRequest, PeerResult, PeerResultStatus};
