#![cfg(feature = "test_helpers")]
//! Launch trampoline contract (ADR 029 §6): the vendor starts only after the
//! go byte, inherits every later stdin byte, and runs inside the tree the
//! daemon contained before releasing it.

use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use vibemux_platform::{
    LAUNCH_GO_BYTE, ProcessTree, TRAMPOLINE_EXIT_NOT_RELEASED, TRAMPOLINE_EXIT_SPAWN_FAILED,
    TRAMPOLINE_EXIT_USAGE,
};

const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_native_fixture");
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn trampoline(arguments: &[&OsStr]) -> Command {
    let mut command = Command::new(TRAMPOLINE);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    command
}

fn fixture_arguments<'a>(mode: &'a str, ready_path: Option<&'a Path>) -> Vec<&'a OsStr> {
    let mut arguments = vec![
        OsStr::new(FIXTURE),
        OsStr::new("--fixture_mode"),
        OsStr::new(mode),
    ];
    if let Some(path) = ready_path {
        arguments.extend([OsStr::new("--fixture_ready_path"), path.as_os_str()]);
    }
    arguments
}

/// Writes `input`, closes stdin, and returns the exit code. The write may
/// fail when the trampoline exits without reading, which some cases expect.
fn run_with_input(mut child: Child, input: &[u8]) -> Option<i32> {
    let mut stdin = child.stdin.take().expect("stdin");
    let _ = stdin.write_all(input);
    drop(stdin);
    wait_for_exit(&mut child).code()
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll exit") {
            return status;
        }
        assert!(Instant::now() < deadline, "the trampoline did not exit");
        thread::sleep(POLL_INTERVAL);
    }
}

/// Reads stdout to EOF on a thread; `None` if it stays open.
fn read_to_end_within(child: &mut Child, timeout: Duration) -> Option<Vec<u8>> {
    let mut stdout = child.stdout.take().expect("stdout");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver.recv_timeout(timeout).ok()
}

fn wait_for_pid(ready_path: &Path) -> u32 {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        // The fixture creates the file before writing the pid.
        if let Some(pid) = std::fs::read_to_string(ready_path)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the descendant never started");
        thread::sleep(POLL_INTERVAL);
    }
}

fn process_gone_within(process_id: u32, timeout: Duration) -> bool {
    let pid = sysinfo::Pid::from_u32(process_id);
    let deadline = Instant::now() + timeout;
    let mut system = sysinfo::System::new();
    loop {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        // An orphan the init process has not reaped yet is already dead.
        let gone = system
            .process(pid)
            .is_none_or(|process| process.status() == sysinfo::ProcessStatus::Zombie);
        if gone {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[test]
fn bytes_after_the_go_byte_reach_the_vendor_intact() {
    let mut child = trampoline(&fixture_arguments("echo_stdin", None))
        .spawn()
        .expect("spawn trampoline");
    let payload = b"first line\n\x06 second line\r\nno trailing newline";
    let mut input = vec![LAUNCH_GO_BYTE];
    input.extend_from_slice(payload);
    // One write: the trampoline must take exactly the go byte and leave the
    // rest in the pipe for the vendor.
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(&input).expect("write stdin");
    drop(stdin);
    let echoed = read_to_end_within(&mut child, EXIT_TIMEOUT).expect("stdout closed");
    assert_eq!(echoed, payload);
    assert_eq!(wait_for_exit(&mut child).code(), Some(0));
}

#[test]
fn the_vendor_exit_code_is_the_trampoline_exit_code() {
    let child = trampoline(&fixture_arguments("exit_code", None))
        .spawn()
        .expect("spawn trampoline");
    assert_eq!(run_with_input(child, &[LAUNCH_GO_BYTE]), Some(7));
}

#[test]
fn an_unreleased_trampoline_starts_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    for (index, input) in [&b""[..], &b"x"[..]].into_iter().enumerate() {
        let ready_path = temp.path().join(format!("ready_{index}"));
        let child = trampoline(&fixture_arguments("early_descendant", Some(&ready_path)))
            .spawn()
            .expect("spawn trampoline");
        assert_eq!(
            run_with_input(child, input),
            Some(TRAMPOLINE_EXIT_NOT_RELEASED)
        );
        assert!(
            !ready_path.exists(),
            "the vendor started without the go byte"
        );
    }
}

#[test]
fn a_missing_vendor_fails_with_the_spawn_code() {
    let temp = tempfile::tempdir().expect("temp");
    let missing = temp.path().join("missing_vendor.exe");
    let child = trampoline(&[missing.as_os_str()])
        .spawn()
        .expect("spawn trampoline");
    assert_eq!(
        run_with_input(child, &[LAUNCH_GO_BYTE]),
        Some(TRAMPOLINE_EXIT_SPAWN_FAILED)
    );
}

#[test]
fn a_missing_or_relative_vendor_path_is_a_usage_error() {
    let child = trampoline(&[]).spawn().expect("spawn trampoline");
    assert_eq!(
        run_with_input(child, &[LAUNCH_GO_BYTE]),
        Some(TRAMPOLINE_EXIT_USAGE)
    );
    let child = trampoline(&[OsStr::new("codex")])
        .spawn()
        .expect("spawn trampoline");
    assert_eq!(
        run_with_input(child, &[LAUNCH_GO_BYTE]),
        Some(TRAMPOLINE_EXIT_USAGE)
    );
}

#[test]
fn a_descendant_started_before_any_input_is_inside_the_tree() {
    let temp = tempfile::tempdir().expect("temp");
    let ready_path = temp.path().join("ready");
    let mut command = trampoline(&fixture_arguments("early_descendant", Some(&ready_path)));
    ProcessTree::prepare_command(&mut command);
    let mut child = command.spawn().expect("spawn trampoline");
    let mut tree = ProcessTree::contain(child.id()).expect("contain");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(&[LAUNCH_GO_BYTE]).expect("release");
    stdin.flush().expect("flush");
    // The vendor started the descendant before reading any input.
    let descendant = wait_for_pid(&ready_path);
    #[cfg(windows)]
    assert!(tree.active_process_count().expect("job accounting") >= 3);
    tree.terminate().expect("terminate");
    // The descendant shares stdout, so EOF means every holder is gone.
    assert!(
        read_to_end_within(&mut child, EXIT_TIMEOUT).is_some(),
        "a tree member kept stdout open"
    );
    drop(stdin);
    assert!(!wait_for_exit(&mut child).success());
    assert!(process_gone_within(descendant, EXIT_TIMEOUT));
}
