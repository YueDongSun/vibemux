//! Windows Job Object containment for native harness processes (ADR 029 §6).
//!
//! The job is unnamed, its handle is not inheritable, and its only limit is
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; breakaway is never permitted, so
//! `CREATE_BREAKAWAY_FROM_JOB` fails with access denied. The child joins it
//! by assignment, and every process a member creates later joins it too. Terminating the job, or closing its last handle (which
//! also happens when the owning process dies), kills every member. Members
//! killed by closing the handle report exit code 0, so `terminate` is always
//! called first and uses a nonzero code; an exit status is never evidence
//! that a contained process succeeded.
#![allow(unsafe_code)] // ADR 029: narrow Job Object create/assign/terminate/query surface

use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Command,
    ptr,
};

use windows_sys::Win32::{
    Foundation::FALSE,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_LIMIT_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject,
        },
        Threading::{IO_COUNTERS, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
    },
};

use super::ProcessTreeOperation;
use crate::PlatformError;

/// Exit code given to every member killed by `terminate`.
const TERMINATED_EXIT_CODE: u32 = 1;

// Both structures are a few dozen bytes, so the casts cannot truncate.
const EXTENDED_LIMIT_SIZE: u32 = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
const ACCOUNTING_SIZE: u32 = size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32;

#[derive(Debug)]
pub(super) struct JobObject {
    handle: OwnedHandle,
}

impl JobObject {
    pub(super) fn prepare_command(_command: &mut Command) {
        // Membership comes from assignment after spawn; nothing to set here.
    }

    pub(super) fn contain(process_id: u32) -> Result<Self, PlatformError> {
        let job = Self::create()?;
        let process = open_process(process_id)?;
        // SAFETY: both handles are open and owned for the whole call. The job
        // handle comes from CreateJobObjectW with full access, which includes
        // JOB_OBJECT_ASSIGN_PROCESS; `process` was opened with
        // PROCESS_SET_QUOTA | PROCESS_TERMINATE, the rights assignment needs.
        let assigned = unsafe {
            AssignProcessToJobObject(job.handle.as_raw_handle(), process.as_raw_handle())
        };
        if assigned == FALSE {
            return Err(last_error(ProcessTreeOperation::Assign));
        }
        Ok(job)
    }

    fn create() -> Result<Self, PlatformError> {
        // SAFETY: null attributes request the default security descriptor and
        // a non-inheritable handle; a null name creates an unnamed job. No
        // pointer is retained after the call.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(last_error(ProcessTreeOperation::Create));
        }
        // SAFETY: `raw` is a new, non-null handle that nothing else owns, so
        // `OwnedHandle` may take ownership and close it exactly once.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let limits = kill_on_close_limits();
        // SAFETY: `limits` is a fully initialized
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION that outlives the call; the
        // length is its exact size, as JobObjectExtendedLimitInformation
        // requires; the job handle is open and owned.
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                EXTENDED_LIMIT_SIZE,
            )
        };
        if configured == FALSE {
            return Err(last_error(ProcessTreeOperation::Configure));
        }
        Ok(Self { handle })
    }

    pub(super) fn terminate(&self) -> Result<(), PlatformError> {
        // SAFETY: the job handle is open and owned by `self`, with full access
        // (including JOB_OBJECT_TERMINATE). The call takes no pointers.
        let terminated =
            unsafe { TerminateJobObject(self.handle.as_raw_handle(), TERMINATED_EXIT_CODE) };
        if terminated == FALSE {
            return Err(last_error(ProcessTreeOperation::Terminate));
        }
        Ok(())
    }

    pub(super) fn active_process_count(&self) -> Result<u32, PlatformError> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION {
            TotalUserTime: 0,
            TotalKernelTime: 0,
            ThisPeriodTotalUserTime: 0,
            ThisPeriodTotalKernelTime: 0,
            TotalPageFaultCount: 0,
            TotalProcesses: 0,
            ActiveProcesses: 0,
            TotalTerminatedProcesses: 0,
        };
        // SAFETY: `accounting` is a writable, aligned
        // JOBOBJECT_BASIC_ACCOUNTING_INFORMATION that outlives the call; the
        // length is its exact size, as JobObjectBasicAccountingInformation
        // requires; the return-length pointer is optional and passed as null;
        // the job handle is open and owned (full access includes
        // JOB_OBJECT_QUERY).
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                ptr::from_mut(&mut accounting).cast(),
                ACCOUNTING_SIZE,
                ptr::null_mut(),
            )
        };
        if queried == FALSE {
            return Err(last_error(ProcessTreeOperation::Query));
        }
        Ok(accounting.ActiveProcesses)
    }
}

fn open_process(process_id: u32) -> Result<OwnedHandle, PlatformError> {
    // SAFETY: OpenProcess takes no pointers; the handle it returns is either
    // null or new and owned by this function. It is not inheritable.
    let raw = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, process_id) };
    if raw.is_null() {
        return Err(last_error(ProcessTreeOperation::Open));
    }
    // SAFETY: `raw` is a new, non-null process handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

const fn kill_on_close_limits() -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            PerProcessUserTimeLimit: 0,
            PerJobUserTimeLimit: 0,
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            MinimumWorkingSetSize: 0,
            MaximumWorkingSetSize: 0,
            ActiveProcessLimit: 0,
            Affinity: 0,
            PriorityClass: 0,
            SchedulingClass: 0,
        },
        IoInfo: IO_COUNTERS {
            ReadOperationCount: 0,
            WriteOperationCount: 0,
            OtherOperationCount: 0,
            ReadTransferCount: 0,
            WriteTransferCount: 0,
            OtherTransferCount: 0,
        },
        ProcessMemoryLimit: 0,
        JobMemoryLimit: 0,
        PeakProcessMemoryUsed: 0,
        PeakJobMemoryUsed: 0,
    }
}

fn last_error(operation: ProcessTreeOperation) -> PlatformError {
    PlatformError::ProcessTree {
        operation,
        os_code: io::Error::last_os_error().raw_os_error(),
    }
}
