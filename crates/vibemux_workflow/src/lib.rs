#![forbid(unsafe_code)]
//! Pure contracts and reducers for supervised multi-harness workflows
//! (ADR 031).
//!
//! This crate performs no I/O. It defines the versioned TaskSpec, its
//! source-coverage validation and policy intersection, the deterministic
//! English renderer and contract identity, AAG route bindings and call
//! receipts, slot eligibility and lease fencing, the context broker's
//! message and bundle rules, candidate snapshot scope checks, the workflow
//! acceptance gates, the comparison selector, the supervisor's typed
//! proposal surface, and the bounded optimizer. `vibemuxd` performs every
//! effect and `vibemux_store` persists every state change through the
//! single writer.

pub mod canonical_json;
pub mod checkpoint;
pub mod compiler_validation;
pub mod context_bundle;
pub mod contract;
pub mod gates;
pub mod gateway;
pub mod identifiers;
pub mod leases;
pub mod messages;
pub mod optimizer;
pub mod policy;
pub mod receipts;
pub mod renderer;
pub mod selection;
pub mod slots;
pub mod snapshot;
pub mod source_request;
pub mod supervisor;
pub mod task_spec;
pub mod templates;
pub mod workflow_record;

pub use identifiers::{IdentifierError, PathPattern, SpecIdentifier};
pub use vibemux_harness::dispatch::Sha256Digest;
