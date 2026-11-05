#![forbid(unsafe_code)]
//! Out-of-process native terminal observer; no canonical store access.
pub mod backend;
pub mod config;
pub mod observation_worker;
