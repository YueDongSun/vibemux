//! Launch trampoline logic (ADR 029 §6). Safe code; the `vibemuxd` package
//! ships the binary, which is a thin entry point around
//! [`run_launch_trampoline`].
//!
//! The daemon spawns the trampoline with the vendor executable and its
//! arguments as the trampoline's own arguments, contains it with
//! [`crate::ProcessTree`], and only then writes [`LAUNCH_GO_BYTE`] to its
//! stdin. The trampoline reads exactly that one byte from an unbuffered
//! duplicate of its stdin, so no protocol byte written after it is consumed.
//! Then it spawns the vendor, which inherits stdin, stdout, stderr, the
//! environment, and the working directory, and so is a member of the tree
//! from its first instruction. The trampoline waits for the vendor and
//! exits with its exit code.

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
};

/// The byte that releases the trampoline. Anything else, including end of
/// file, makes it exit without spawning.
pub const LAUNCH_GO_BYTE: u8 = 0x06;

/// No vendor executable, or one that is not an absolute path.
pub const TRAMPOLINE_EXIT_USAGE: i32 = 120;
/// Stdin delivered end of file or a byte other than [`LAUNCH_GO_BYTE`].
pub const TRAMPOLINE_EXIT_NOT_RELEASED: i32 = 121;
/// The vendor could not be spawned.
pub const TRAMPOLINE_EXIT_SPAWN_FAILED: i32 = 122;
/// The vendor ended without an exit code (a signal on POSIX), or waiting
/// for it failed.
pub const TRAMPOLINE_EXIT_NO_CODE: i32 = 123;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs the trampoline and returns its exit code. `arguments` excludes the
/// trampoline's own program name: the first is the absolute vendor
/// executable, the rest are the vendor's arguments.
///
/// The trampoline's own failure codes can collide with a vendor's code. All
/// of them are nonzero, so a collision never makes an attempt look clean.
pub fn run_launch_trampoline(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    let mut arguments = arguments.into_iter();
    let Some(executable) = arguments.next() else {
        return TRAMPOLINE_EXIT_USAGE;
    };
    // An absolute path means no PATH search can pick a different program.
    if !Path::new(&executable).is_absolute() {
        return TRAMPOLINE_EXIT_USAGE;
    }
    let vendor_arguments: Vec<OsString> = arguments.collect();
    if !matches!(read_go_byte_from_stdin(), Ok(true)) {
        return TRAMPOLINE_EXIT_NOT_RELEASED;
    }
    launch(&executable, &vendor_arguments)
}

fn read_go_byte_from_stdin() -> io::Result<bool> {
    // The duplicate is closed before the vendor is spawned.
    let mut stdin = unbuffered_stdin()?;
    await_go_byte(&mut stdin)
}

/// Reads exactly one byte and reports whether it is the go byte.
fn await_go_byte(reader: &mut impl Read) -> io::Result<bool> {
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(byte[0] == LAUNCH_GO_BYTE),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// `std::io::Stdin` buffers up to 8 KiB per read, which would swallow the
/// protocol bytes that follow the go byte. A duplicated handle read through
/// `File` is unbuffered.
#[cfg(unix)]
fn unbuffered_stdin() -> io::Result<File> {
    use std::os::fd::AsFd;

    Ok(File::from(io::stdin().as_fd().try_clone_to_owned()?))
}

#[cfg(windows)]
fn unbuffered_stdin() -> io::Result<File> {
    use std::os::windows::io::AsHandle;

    Ok(File::from(io::stdin().as_handle().try_clone_to_owned()?))
}

fn launch(executable: &OsStr, arguments: &[OsString]) -> i32 {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // The trampoline has no console, so a console vendor started without
    // this flag would open a new console window.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    let Ok(mut child) = command.spawn() else {
        return TRAMPOLINE_EXIT_SPAWN_FAILED;
    };
    match child.wait() {
        Ok(status) => status.code().unwrap_or(TRAMPOLINE_EXIT_NO_CODE),
        Err(_) => TRAMPOLINE_EXIT_NO_CODE,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn the_go_byte_is_consumed_alone() {
        let mut input = Cursor::new(vec![LAUNCH_GO_BYTE, b'{', b'}']);
        assert!(await_go_byte(&mut input).unwrap());
        assert_eq!(input.position(), 1);
    }

    #[test]
    fn end_of_file_or_another_byte_does_not_release() {
        assert!(!await_go_byte(&mut Cursor::new(Vec::new())).unwrap());
        let mut input = Cursor::new(vec![b'{', LAUNCH_GO_BYTE]);
        assert!(!await_go_byte(&mut input).unwrap());
        assert_eq!(input.position(), 1);
    }

    #[test]
    fn a_missing_or_relative_executable_is_refused_before_stdin_is_read() {
        assert_eq!(run_launch_trampoline([]), TRAMPOLINE_EXIT_USAGE);
        assert_eq!(
            run_launch_trampoline([OsString::from("codex"), OsString::from("exec")]),
            TRAMPOLINE_EXIT_USAGE
        );
    }
}
