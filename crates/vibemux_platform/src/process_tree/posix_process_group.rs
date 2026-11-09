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
        let result = kill_process_group(self.leader, Signal::KILL);
        #[cfg(target_os = "macos")]
        let result = macos_termination_result(
            self.leader,
            result,
            || self.leader_exited(),
            || {
                libproc::processes::pids_by_type(libproc::processes::ProcFilter::ByProgramGroup {
                    pgrpid: self.leader.as_raw_nonzero().get().unsigned_abs(),
                })
            },
        );
        match result {
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

/// Darwin's `killpg` skips zombies and returns EPERM when it finds no
/// signalable member. That same error can mean a live member denied the
/// signal, so an exited leader alone is not evidence that cleanup succeeded.
#[cfg(target_os = "macos")]
fn macos_termination_result(
    leader: Pid,
    result: Result<(), Errno>,
    leader_exited: impl FnOnce() -> Result<bool, PlatformError>,
    group_members: impl FnOnce() -> std::io::Result<Vec<u32>>,
) -> Result<(), Errno> {
    if result != Err(Errno::PERM) || !matches!(leader_exited(), Ok(true)) {
        return result;
    }
    let Ok(members) = group_members() else {
        return result;
    };
    // XNU's PROC_PGRP_ONLY query checks permission for the entire request,
    // then scans all live and zombie processes under proc_list_lock, without
    // a UID filter. libproc 0.14.11 sizes its buffer using nprocs + 20 and
    // preserves its capacity. The kernel also caps its temporary buffer at
    // nprocs + 20, so neither cap can truncate a single-row result. Refuse a
    // full one-slot buffer as well: a partial list must never prove absence.
    //
    // Accept only our unreaped zombie, whose PID cannot be reused. Empty
    // lists, extra members (even zombies), and query failures stay errors.
    // Inspecting other members' states separately would introduce fork and
    // PID-reuse races, so it cannot establish the same proof.
    if members.capacity() > 1
        && members.as_slice() == [leader.as_raw_nonzero().get().unsigned_abs()]
    {
        Ok(())
    } else {
        result
    }
}

fn verify_failed(os_code: Option<i32>) -> PlatformError {
    PlatformError::ProcessTree {
        operation: ProcessTreeOperation::Verify,
        os_code,
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    fn leader() -> Pid {
        Pid::from_raw(42).expect("test pid")
    }

    fn members(pids: &[u32]) -> std::io::Result<Vec<u32>> {
        let mut result = Vec::with_capacity(pids.len() + 1);
        result.extend_from_slice(pids);
        Ok(result)
    }

    #[test]
    fn permission_denied_accepts_only_the_owned_unreaped_zombie() {
        assert_eq!(
            macos_termination_result(leader(), Err(Errno::PERM), || Ok(true), || members(&[42])),
            Ok(())
        );
    }

    #[test]
    fn other_signal_results_do_not_query_or_change() {
        for result in [Ok(()), Err(Errno::SRCH), Err(Errno::IO), Err(Errno::ACCESS)] {
            assert_eq!(
                macos_termination_result(
                    leader(),
                    result,
                    || panic!("must not inspect leader"),
                    || panic!("must not enumerate processes"),
                ),
                result
            );
        }
    }

    #[test]
    fn living_or_unobservable_leader_preserves_permission_denied() {
        for observation in [
            Ok(false),
            Err(verify_failed(Some(Errno::CHILD.raw_os_error()))),
        ] {
            assert_eq!(
                macos_termination_result(
                    leader(),
                    Err(Errno::PERM),
                    || observation,
                    || panic!("must not enumerate without an owned zombie"),
                ),
                Err(Errno::PERM)
            );
        }
    }

    #[test]
    fn another_member_or_incomplete_membership_preserves_permission_denied() {
        // An additional PID may be a live process we cannot signal. Never
        // infer successful cleanup from the leader's exit in these cases.
        for pids in [&[][..], &[43], &[42, 43], &[42, 42]] {
            assert_eq!(
                macos_termination_result(leader(), Err(Errno::PERM), || Ok(true), || members(pids)),
                Err(Errno::PERM)
            );
        }
        let full = vec![42];
        assert_eq!(full.capacity(), 1);
        assert_eq!(
            macos_termination_result(leader(), Err(Errno::PERM), || Ok(true), || Ok(full)),
            Err(Errno::PERM)
        );
    }

    #[test]
    fn enumeration_failure_preserves_the_original_permission_error() {
        for errno in [Errno::PERM, Errno::ACCESS, Errno::SRCH, Errno::IO] {
            assert_eq!(
                macos_termination_result(
                    leader(),
                    Err(Errno::PERM),
                    || Ok(true),
                    || Err(std::io::Error::from_raw_os_error(errno.raw_os_error())),
                ),
                Err(Errno::PERM)
            );
        }
    }
}
