//! Process-tree containment for native harness processes (ADR 029 §6).
//!
//! One safe API over a platform primitive:
//!
//! 1. [`ProcessTree::prepare_command`] before spawning. On POSIX the child
//!    leads a new process group; on Windows nothing changes.
//! 2. [`ProcessTree::contain`] with the spawned child's id, before the child
//!    is sent any input. On Windows the child joins a kill-on-close Job
//!    Object; on POSIX its group is verified.
//! 3. [`ProcessTree::terminate`], or dropping the tree, kills every
//!    remaining member.
//!
//! The caller must still hold the un-reaped child for `process_id` when it
//! calls [`ProcessTree::contain`]: an un-reaped child keeps its id from being
//! reused on both platforms.
//!
//! Containment is not an OS sandbox. On Windows, assignment happens after
//! process creation, so a descendant the child creates on its own before
//! `contain` returns is not a member; withholding input only covers
//! descendants started in response to input. [`run_launch_trampoline`]
//! closes that window for vendor CLIs: the contained process is the
//! trampoline, which starts the vendor only after the go byte. A member
//! cannot break away through `CreateProcess`, because the job never
//! permits it, but processes started on its behalf by system services (WMI,
//! Task Scheduler, COM servers) are outside the job. On POSIX, a member can
//! leave the group with `setsid` or `setpgid`, and a crash of the owning
//! process leaves the group running; there is no kill-on-close equivalent.

mod launch_trampoline;
#[cfg(unix)]
mod posix_process_group;
#[cfg(windows)]
mod windows_job_object;

use std::{fmt, process::Command};

pub use launch_trampoline::{
    LAUNCH_GO_BYTE, TRAMPOLINE_EXIT_NO_CODE, TRAMPOLINE_EXIT_NOT_RELEASED,
    TRAMPOLINE_EXIT_SPAWN_FAILED, TRAMPOLINE_EXIT_USAGE, run_launch_trampoline,
};

#[cfg(unix)]
use posix_process_group::ProcessGroup as PlatformTree;
#[cfg(windows)]
use windows_job_object::JobObject as PlatformTree;

use crate::PlatformError;

/// The containment step that failed. Fixed names only; no path, command
/// line, or environment is ever attached to a containment error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTreeOperation {
    /// Windows: creating the Job Object.
    Create,
    /// Windows: setting the kill-on-close limit.
    Configure,
    /// Windows: opening the child process by id.
    Open,
    /// Windows: assigning the child to the Job Object.
    Assign,
    /// Checking that the id is containable: never this process; on POSIX,
    /// also never init, and only the leader of a group other than this
    /// process's.
    Verify,
    /// Killing the members.
    Terminate,
    /// Windows: reading the job's process accounting.
    Query,
}

impl ProcessTreeOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Configure => "configure",
            Self::Open => "open",
            Self::Assign => "assign",
            Self::Verify => "verify",
            Self::Terminate => "terminate",
            Self::Query => "query",
        }
    }
}

impl fmt::Display for ProcessTreeOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A contained child process and its descendants. Dropping it terminates
/// every remaining member.
#[derive(Debug)]
pub struct ProcessTree {
    inner: PlatformTree,
}

impl ProcessTree {
    /// Configures `command` so the child it spawns can be contained. Call it
    /// before spawning; tokio users pass `Command::as_std_mut()`.
    pub fn prepare_command(command: &mut Command) {
        PlatformTree::prepare_command(command);
    }

    /// Contains the running child `process_id` and every descendant it
    /// creates from now on. The caller must hold the un-reaped child.
    ///
    /// On error the child keeps running uncontained; the caller must kill
    /// and reap it. A child that already exited fails here too.
    pub fn contain(process_id: u32) -> Result<Self, PlatformError> {
        if process_id == std::process::id() {
            return Err(PlatformError::ProcessTree {
                operation: ProcessTreeOperation::Verify,
                os_code: None,
            });
        }
        Ok(Self {
            inner: PlatformTree::contain(process_id)?,
        })
    }

    /// Kills every remaining member.
    ///
    /// On POSIX, call it before reaping the child: once the leader is reaped
    /// and the group is empty, its id may be reused. After the first success
    /// no further signal is sent, so a later call cannot reach a reused id.
    /// On Windows every call terminates the job again, which also kills any
    /// member created while an earlier call ran. A failed call may be
    /// retried, and drop retries it.
    pub fn terminate(&mut self) -> Result<(), PlatformError> {
        self.inner.terminate()
    }

    /// Windows only: the number of live members, from the job's accounting.
    #[cfg(windows)]
    pub fn active_process_count(&self) -> Result<u32, PlatformError> {
        self.inner.active_process_count()
    }

    /// POSIX only: whether the contained child has exited, observed without
    /// reaping it. A caller that must let the child finish on its own polls
    /// this, then calls [`ProcessTree::terminate`] to remove any member left
    /// behind, and only then reaps the child, which keeps the ordering rule.
    /// The caller must not reap the child by any other path in between.
    #[cfg(unix)]
    pub fn leader_exited(&self) -> Result<bool, PlatformError> {
        self.inner.leader_exited()
    }
}

impl Drop for ProcessTree {
    /// Terminates like [`ProcessTree::terminate`], with the same POSIX
    /// ordering rule: drop the tree before reaping the child.
    fn drop(&mut self) {
        // Best effort: drop cannot report failure. On Windows, closing the
        // job handle afterwards kills any member that survived.
        let _ = self.terminate();
    }
}
