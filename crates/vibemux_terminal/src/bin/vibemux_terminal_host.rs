#![forbid(unsafe_code)]
fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--pty_child")) {
        let Some(executable) = arguments.next() else {
            std::process::exit(120);
        };
        std::process::exit(vibemux_terminal::host::run_pty_child(std::path::Path::new(
            &executable,
        )));
    }
    let result = vibemux_terminal::host::run_host();
    if let Err(error) = result {
        let notice =
            vibemux_terminal::host::status("failed", error.0, std::path::Path::new(""), "", 0);
        let _ = vibemux_terminal::protocol::write_message(&mut std::io::stdout(), &notice);
    }
    // Ends any still-blocked private stdin reader after PTY cleanup.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
