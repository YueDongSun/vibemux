//! Process-tree containment (ADR 029 §6): a contained child and the
//! descendant it spawns both die on terminate and on drop, and on Windows
//! when the process owning the tree dies; a member cannot break away on
//! Windows; ids that must not be contained are refused.
//!
//! The test binary re-executes itself in helper roles. The parent waits for
//! a go line (so the test contains it first, as the daemon must), spawns a
//! grandchild that shares its stdout, and reports it. End of file on that
//! stdout therefore means every member holding the pipe has exited, which
//! works on both platforms without process lookups.

#![forbid(unsafe_code)]

use std::{
    env,
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStdout, Command, Stdio},
    sync::{Mutex, MutexGuard, PoisonError, mpsc},
    thread,
    time::{Duration, Instant},
};

use vibemux_platform::{PlatformError, ProcessTree, ProcessTreeOperation};

const ROLE_VARIABLE: &str = "VIBEMUX_PROCESS_TREE_TEST_ROLE";
const HELPER_TEST_NAME: &str = "helper_process_entry_point";
const GO_LINE: &str = "go";
const READY_MARKER: &str = "vibemux_process_tree_ready";
const OWNER_MARKER: &str = "vibemux_process_tree_owner_ready";
#[cfg(windows)]
const BREAKAWAY_MARKER: &str = "vibemux_process_tree_breakaway";
#[cfg(unix)]
const GROUP_MEMBER_MARKER: &str = "vibemux_process_tree_group_member";
/// Bounds any helper leaked by a failing test.
const HELPER_LIFETIME: Duration = Duration::from_secs(60);
const MARKER_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const SURVIVAL_WINDOW: Duration = Duration::from_millis(1500);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
#[cfg(windows)]
const ERROR_ACCESS_DENIED: i32 = 5;

/// Spawning tests run one at a time so no other test's helpers compete for
/// the CPU during a survival window or an exit deadline.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SPAWN_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn helper_command(role: &str) -> Command {
    let mut command = Command::new(env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            HELPER_TEST_NAME,
            "--nocapture",
            "--test-threads=1",
            "--quiet",
        ])
        .env(ROLE_VARIABLE, role);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    command
}

/// Entry point of the helper roles; a no-op in a normal test run.
#[test]
fn helper_process_entry_point() {
    let Ok(role) = env::var(ROLE_VARIABLE) else {
        return;
    };
    match role.as_str() {
        "parent" => run_parent(),
        "grandchild" => thread::sleep(HELPER_LIFETIME),
        "owner" => run_owner(),
        #[cfg(windows)]
        "breakaway" => run_breakaway(),
        #[cfg(unix)]
        "group_leader" => run_group_leader(),
        #[cfg(unix)]
        "group_member" => run_group_member(),
        other => panic!("unknown helper role {other}"),
    }
}

/// Never drops a tree that must not exist: it could name the test process,
/// its group, or (as group 1) every process the user may signal.
fn forget_unexpected(tree: ProcessTree, what: &str) -> ! {
    std::mem::forget(tree);
    panic!("{what} was contained");
}

fn run_parent() {
    let mut line = String::new();
    // Anything but the go line (including end of file) means the test did
    // not contain this process, so spawn nothing.
    if io::stdin().read_line(&mut line).is_err() || line.trim_end() != GO_LINE {
        return;
    }
    let mut grandchild = helper_command("grandchild")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn grandchild");
    println!("{READY_MARKER} {}", grandchild.id());
    io::stdout().flush().expect("flush ready marker");
    // The grandchild sleeps for `HELPER_LIFETIME`, which bounds this wait.
    let _ = grandchild.wait();
}

/// Owns a contained parent until it is killed.
fn run_owner() {
    let mut parent = helper_command("parent");
    parent.stdin(Stdio::piped());
    ProcessTree::prepare_command(&mut parent);
    let mut parent = parent.spawn().expect("spawn parent");
    let _tree = ProcessTree::contain(parent.id()).expect("contain parent");
    let mut stdin = parent.stdin.take().expect("parent stdin");
    writeln!(stdin, "{GO_LINE}").expect("send go");
    println!("{OWNER_MARKER}");
    io::stdout().flush().expect("flush owner marker");
    let _ = parent.wait();
}

/// Once contained, tries to start a child outside the job and reports the
/// outcome.
#[cfg(windows)]
fn run_breakaway() {
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() || line.trim_end() != GO_LINE {
        return;
    }
    let mut command = helper_command("grandchild");
    command.stdin(Stdio::null()).stdout(Stdio::null());
    std::os::windows::process::CommandExt::creation_flags(
        &mut command,
        CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW,
    );
    match command.spawn() {
        Ok(mut escaped) => {
            let _ = escaped.kill();
            let _ = escaped.wait();
            println!("{BREAKAWAY_MARKER} escaped");
        }
        Err(error) => println!("{BREAKAWAY_MARKER} refused {:?}", error.raw_os_error()),
    }
    io::stdout().flush().expect("flush breakaway marker");
}

/// Leads its own group and starts a member that shares it.
#[cfg(unix)]
fn run_group_leader() {
    let mut member = helper_command("group_member")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn group member");
    let _ = member.wait();
}

/// Tries to contain its own group through its leader, which must be refused:
/// terminating that tree would kill this process.
#[cfg(unix)]
fn run_group_member() {
    match ProcessTree::contain(std::os::unix::process::parent_id()) {
        Ok(tree) => {
            println!("{GROUP_MEMBER_MARKER} accepted");
            io::stdout().flush().expect("flush group marker");
            forget_unexpected(tree, "the caller's own group");
        }
        Err(error) => println!("{GROUP_MEMBER_MARKER} refused {error:?}"),
    }
    io::stdout().flush().expect("flush group marker");
}

struct Helper {
    child: Child,
    /// Stdout lines; `None` marks end of file.
    lines: mpsc::Receiver<Option<String>>,
}

impl Helper {
    fn spawn(role: &str, prepare: bool) -> Self {
        let mut command = helper_command(role);
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        if prepare {
            ProcessTree::prepare_command(&mut command);
        }
        let mut child = command.spawn().expect("spawn helper");
        let stdout = child.stdout.take().expect("helper stdout");
        Self {
            child,
            lines: read_lines(stdout),
        }
    }

    fn send_go(&mut self) {
        let mut stdin = self.child.stdin.take().expect("helper stdin");
        writeln!(stdin, "{GO_LINE}").expect("send go");
    }

    /// Consumes lines until every marker has been seen, in any order.
    fn wait_for(&self, markers: &[&str]) {
        let deadline = Instant::now() + MARKER_TIMEOUT;
        let mut pending: Vec<&str> = markers.to_vec();
        while !pending.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(Some(line)) => pending.retain(|marker| !line.contains(marker)),
                Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("helper output ended before {pending:?}")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("timed out waiting for {pending:?}"),
            }
        }
    }

    /// Consumes lines until one contains `marker`, and returns it.
    fn line_with(&self, marker: &str) -> String {
        let deadline = Instant::now() + MARKER_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(Some(line)) if line.contains(marker) => return line,
                Ok(Some(_)) => {}
                Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("helper output ended before {marker}")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("timed out waiting for {marker}"),
            }
        }
    }

    /// Whether every holder of the stdout pipe exits within `timeout`.
    fn closed_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(Some(_)) => {}
                Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
            }
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // Failure paths only; a passing test has already reaped the child.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_lines(stdout: ChildStdout) -> mpsc::Receiver<Option<String>> {
    let (sender, receiver) = mpsc::sync_channel(64);
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(Some(line)).is_err() {
                return;
            }
        }
        let _ = sender.send(None);
    });
    receiver
}

/// A contained parent that has spawned its grandchild.
fn contained_parent() -> (Helper, ProcessTree) {
    let mut helper = Helper::spawn("parent", true);
    let tree = ProcessTree::contain(helper.child.id()).expect("contain parent");
    helper.send_go();
    helper.wait_for(&[READY_MARKER]);
    (helper, tree)
}

#[test]
fn terminate_kills_the_child_and_its_descendant() {
    let _serial = serial();
    let (mut helper, mut tree) = contained_parent();
    #[cfg(windows)]
    assert!(tree.active_process_count().expect("job accounting") >= 2);
    tree.terminate().expect("terminate");
    assert!(
        helper.closed_within(EXIT_TIMEOUT),
        "a member survived terminate"
    );
    assert!(!helper.child.wait().expect("reap parent").success());
    #[cfg(windows)]
    assert_eq!(tree.active_process_count().expect("job accounting"), 0);
    tree.terminate().expect("terminate is idempotent");
}

#[test]
fn dropping_the_tree_kills_the_child_and_its_descendant() {
    let _serial = serial();
    let (mut helper, tree) = contained_parent();
    drop(tree);
    assert!(helper.closed_within(EXIT_TIMEOUT), "a member survived drop");
    // Drop must terminate explicitly: on Windows, members killed only by
    // closing the job handle report exit code 0.
    assert!(!helper.child.wait().expect("reap parent").success());
}

/// Negative control: without the tree the grandchild keeps the pipe open,
/// so end of file in the other tests is evidence that it was killed.
#[test]
fn a_descendant_outlives_its_killed_parent_until_the_tree_is_terminated() {
    let _serial = serial();
    let (mut helper, mut tree) = contained_parent();
    helper.child.kill().expect("kill the parent only");
    helper.child.wait().expect("reap parent");
    assert!(
        !helper.closed_within(SURVIVAL_WINDOW),
        "the grandchild should still hold the pipe"
    );
    #[cfg(windows)]
    assert!(tree.active_process_count().expect("job accounting") >= 1);
    tree.terminate().expect("terminate");
    assert!(
        helper.closed_within(EXIT_TIMEOUT),
        "the grandchild survived terminate"
    );
}

/// Kill-on-close: killing the owner closes its job handle.
#[cfg(windows)]
#[test]
fn the_tree_dies_with_the_process_that_owns_it() {
    let _serial = serial();
    let mut owner = Helper::spawn("owner", false);
    owner.wait_for(&[OWNER_MARKER, READY_MARKER]);
    owner.child.kill().expect("kill owner");
    owner.child.wait().expect("reap owner");
    assert!(
        owner.closed_within(EXIT_TIMEOUT),
        "a member survived its owner"
    );
}

/// Breakaway is never permitted: a member cannot use `CreateProcess` to
/// start a child outside the job.
#[cfg(windows)]
#[test]
fn a_member_cannot_break_away_from_the_tree() {
    let _serial = serial();
    let mut helper = Helper::spawn("breakaway", true);
    let _tree = ProcessTree::contain(helper.child.id()).expect("contain helper");
    helper.send_go();
    let line = helper.line_with(BREAKAWAY_MARKER);
    assert!(
        line.ends_with(&format!("refused Some({ERROR_ACCESS_DENIED})")),
        "{line}"
    );
}

/// Without `prepare_command` the child shares the caller's group, so
/// signalling its id as a group would miss its descendants.
#[cfg(unix)]
#[test]
fn a_child_outside_its_own_group_is_refused() {
    let _serial = serial();
    let helper = Helper::spawn("grandchild", false);
    match ProcessTree::contain(helper.child.id()) {
        Ok(tree) => forget_unexpected(tree, "a child in the caller's group"),
        Err(error) => assert_eq!(
            error,
            PlatformError::ProcessTree {
                operation: ProcessTreeOperation::Verify,
                os_code: None,
            }
        ),
    }
}

/// A member of a contained group cannot contain that group through its
/// leader: terminating the tree would kill the caller.
#[cfg(unix)]
#[test]
fn the_callers_own_group_is_refused() {
    let _serial = serial();
    let helper = Helper::spawn("group_leader", true);
    let line = helper.line_with(GROUP_MEMBER_MARKER);
    assert!(
        line.ends_with("refused ProcessTree { operation: Verify, os_code: None }"),
        "{line}"
    );
}

/// The leader's exit is visible without reaping it, so the tree can still be
/// terminated before the reap that could free its group id.
#[cfg(unix)]
#[test]
fn a_leader_exit_is_observed_without_reaping_it() {
    let _serial = serial();
    let mut helper = Helper::spawn("parent", true);
    let mut tree = ProcessTree::contain(helper.child.id()).expect("contain parent");
    assert_eq!(tree.leader_exited(), Ok(false));
    // End of file instead of the go line: the parent exits without spawning.
    drop(helper.child.stdin.take());
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while !tree.leader_exited().expect("observe the leader") {
        assert!(Instant::now() < deadline, "the leader did not exit");
        thread::sleep(Duration::from_millis(10));
    }
    tree.terminate().expect("terminate after the exit");
    // Still reapable, with its own status.
    let status = helper.child.wait().expect("reap the leader");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn invalid_process_ids_fail_closed_with_a_content_free_error() {
    // On POSIX, 1 is init, and group 1 would be every process the user may
    // signal. On Windows no process has id 0, 1, or u32::MAX.
    let unopenable = if cfg!(windows) {
        ProcessTreeOperation::Open
    } else {
        ProcessTreeOperation::Verify
    };
    let cases = [
        (0, unopenable),
        (1, unopenable),
        (u32::MAX, unopenable),
        (std::process::id(), ProcessTreeOperation::Verify),
    ];
    for (process_id, expected) in cases {
        let error = match ProcessTree::contain(process_id) {
            Ok(tree) => forget_unexpected(tree, &format!("process id {process_id}")),
            Err(error) => error,
        };
        let PlatformError::ProcessTree { operation, .. } = error else {
            panic!("unexpected error {error:?}");
        };
        assert_eq!(operation, expected, "{process_id}");
        assert_eq!(error.code(), "platform_process_tree_failed");
    }
    let error = PlatformError::ProcessTree {
        operation: ProcessTreeOperation::Assign,
        os_code: Some(5),
    };
    assert_eq!(
        error.to_string(),
        "process-tree containment failed during assign"
    );
}
