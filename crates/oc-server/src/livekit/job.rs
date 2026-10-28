//! Windows job object with "kill on close": every process in it is killed when the last
//! handle to the job closes. The camp holds the only handle, and Windows closes the
//! handles of a process that dies however it dies — so LiveKit cannot outlive the camp.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Dropping it kills the processes assigned to it.
pub struct Job(OwnedHandle);

impl Job {
    pub fn new() -> io::Result<Self> {
        // SAFETY: both arguments may be null: default security, no name.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a fresh, valid handle nobody else owns; `OwnedHandle`
        // closes it exactly once.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });

        // SAFETY: a plain C struct of integers; all-zero is its documented "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .expect("the struct is a few hundred bytes");
        // SAFETY: the pointer and size describe `limits`, which outlives the call; the
        // information class matches the struct's type.
        let ok = unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Puts `process` (a handle of a running child) into the job.
    pub fn assign(&self, process: RawHandle) -> io::Result<()> {
        // SAFETY: both handles are valid for the call: the job is ours, the process
        // handle belongs to a `Child` the caller still holds.
        let ok = unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
