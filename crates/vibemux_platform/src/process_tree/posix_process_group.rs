//! POSIX process-group containment (ADR 029 §6). Safe code only: the group
//! is created by `std` and looked up and signalled through `rustix`.

use std::{os::unix::process::CommandExt, process::Command};

use rustix::{
    io::Errno,
    process::{Pid, Signal, WaitId, WaitIdOptions, getpgid, getpgrp, kill_process_group, waitid},
};

use super::ProcessTreeOperation;
use crate::PlatformError;

#[derive(Debug)]
pub(super) struct ProcessGroup {
    leader: Pid,
    /// Set after the first successful kill; the group id may be reused once
    /// the group is empty, so it is never signalled again.
    signalled: bool,
}

impl ProcessGroup {
    pub(super) fn prepare_command(command: &mut Command) {
        // The child calls `setpgid(0, 0)` before exec, so it leads a new group
        // that every descendant it forks inherits.
        command.process_group(0);
    }

    pub(super) fn contain(process_id: u32) -> Result<Self, PlatformError> {
        let leader = i32::try_from(process_id)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or(verify_failed(None))?;
        // Signalling group 1 is `kill(-1)`, which reaches every process this
        // user may signal.
        if leader == Pid::INIT {
            return Err(verify_failed(None));
        }
        let group =
            getpgid(Some(leader)).map_err(|errno| verify_failed(Some(errno.raw_os_error())))?;
        // A child spawned without `prepare_command` shares its parent's group.
        // Signalling its id as a group would miss its descendants, and a
        // group equal to ours would kill this process, so refuse both.
        if group != leader || group == getpgrp() {
            return Err(verify_failed(None));
        }
        Ok(Self {
            leader,
            signalled: false,
        })
    }

    pub(super) fn terminate(&mut self) -> Result<(), PlatformError> {
        if self.signalled {
            return Ok(());
        }
        match kill_process_group(self.leader, Signal::KILL) {
            // ESRCH: every member has already exited.
            Ok(()) | Err(Errno::SRCH) => {
                self.signalled = true;
                Ok(())
            }
            Err(errno) => Err(PlatformError::ProcessTree {
                operation: ProcessTreeOperation::Terminate,
                os_code: Some(errno.raw_os_error()),
            }),
        }
    }

    pub(super) fn leader_exited(&self) -> Result<bool, PlatformError> {
        // `NOWAIT` leaves the leader a zombie, so its id, and with it the
        // group id, stays reserved until the caller reaps it.
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(self.leader), options) {
            Ok(status) => Ok(status.is_some()),
            Err(errno) => Err(PlatformError::ProcessTree {
                operation: ProcessTreeOperation::Query,
                os_code: Some(errno.raw_os_error()),
            }),
        }
    }
}

fn verify_failed(os_code: Option<i32>) -> PlatformError {
    PlatformError::ProcessTree {
        operation: ProcessTreeOperation::Verify,
        os_code,
    }
}
