//! A Git filter for `tests/unified_commands.rs`: it records in the working
//! directory that Git ran it, then passes the content through unchanged.
//!
//! Git runs a filter command without a shell when the command has no shell
//! metacharacters, so configuring this executable's path proves filter
//! execution without a POSIX shell. A shell-based filter made the test depend
//! on Git for Windows' MSYS `sh`, which can fail to start in some process
//! contexts; the inventory's "no filter ran" assertions would then pass
//! without proving anything.
use std::io;

const MARKER_FILE_NAME: &str = "filter_marker";

fn main() -> io::Result<()> {
    std::fs::write(MARKER_FILE_NAME, b"invoked\n")?;
    io::copy(&mut io::stdin().lock(), &mut io::stdout().lock())?;
    Ok(())
}
