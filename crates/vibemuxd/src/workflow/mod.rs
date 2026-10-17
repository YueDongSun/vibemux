//! The dual-track workflow service (ADR 031).

pub mod aag_gateway;
pub(crate) mod bounded_file;
pub(crate) mod broker;
pub(crate) mod candidate;
pub(crate) mod collector;
pub mod coordinator;
pub mod error;
pub mod evaluate;
pub mod evidence;
pub mod integration;
pub mod model_compiler;
pub mod model_runtime;
pub(crate) mod model_supervisor;
pub mod native_runtime;
pub mod prepare;
pub mod request;
pub(crate) mod runtime;
pub mod service;
pub mod settings;
pub(crate) mod share;
pub mod startup;
pub(crate) mod state_files;
pub(crate) mod state_protection;
pub(crate) mod supervisor_actions;
pub(crate) mod supervisor_context;
pub mod turns;
pub(crate) mod verifier_runner;

pub use service::{WorkflowService, WorkflowServiceSettings};
