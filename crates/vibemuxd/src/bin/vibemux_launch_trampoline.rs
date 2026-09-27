#![forbid(unsafe_code)]
//! Launch trampoline for harness dispatch (ADR 029 §6). The daemon contains
//! this process before it writes the go byte, so the vendor CLI it then
//! starts is contained from its first instruction. Usage:
//! `vibemux_launch_trampoline <absolute vendor executable> [vendor args...]`.

fn main() {
    let code = vibemux_platform::run_launch_trampoline(std::env::args_os().skip(1));
    std::process::exit(code);
}
