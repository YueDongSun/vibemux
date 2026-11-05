#![forbid(unsafe_code)]
//! Read-only "Ask a harness" from the GUI, through ADR 029 daemon dispatch.
//!
//! The GUI never runs a harness itself: it asks the active folder's daemon
//! to run one structured, read-only turn of Claude Code or Codex, follows the
//! attempt, and shows the reply. Replies live in daemon memory, so a daemon
//! restart loses them. Setup writes default routes only on an explicit click
//! and never replaces an operator's config.

mod driver;
pub mod observation;
pub mod reply_text;
pub mod routes;
pub mod session;
pub mod worker;

pub use session::{AskEnd, AskPhase, AskUpdate};
pub use worker::{AskHandle, SetupOutcome, UNCONFIGURED};

pub use observation::{
    AskAdmission, AskCanonicalStatus, AskObservation, AskObserverState, AskOutputState,
    AskSubmissionState,
};
pub use worker::{AskControl, ObservedAskHandle};
