#![forbid(unsafe_code)]
//! Entry point for the one-shot decision peer binary (ADR 034 §6).

use std::io::{stderr, stdin, stdout};

use vibemux_decision_peer::peer_main;

fn main() {
    let env_lookup = |name: &str| std::env::var(name).ok();
    let code = peer_main::run(stdin(), stdout(), stderr(), &env_lookup);
    std::process::exit(code);
}
