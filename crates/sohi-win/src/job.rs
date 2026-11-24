//! Job objects: stopping a process with everything it started.

use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

use crate::{OwnedHandle, WinError};

/// A job whose processes are stopped when it is dropped.
#[derive(Debug)]
pub struct Job(OwnedHandle);

impl Job {
    /// An empty job.
    ///
    /// # Errors
    /// Returns a [`WinError`] if Windows refuses to make one.
    pub fn new() -> Result<Self, WinError> {
        // SAFETY: no name, default security.
        let job = unsafe { CreateJobObjectW(None, None) }
            .map_err(|e| WinError::api("CreateJobObjectW", &e))?;
        // SAFETY: just created and owned only here.
        let job = Self(unsafe { OwnedHandle::from_raw(job) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of_val(&limits)).unwrap_or(0);
        // SAFETY: a live job, and a struct of the class and size named.
        unsafe {
            SetInformationJobObject(
                job.0.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size,
            )
        }
        .map_err(|e| WinError::api("SetInformationJobObject", &e))?;
        Ok(job)
    }

    /// Adds `child`, and with it whatever it starts from now on.
    ///
    /// # Errors
    /// Returns a [`WinError`] if the process is gone already.
    pub fn adopt(&self, child: &Child) -> Result<(), WinError> {
        let process = HANDLE(child.as_raw_handle());
        // SAFETY: two live handles; the child's stays open as long as it.
        unsafe { AssignProcessToJobObject(self.0.raw(), process) }
            .map_err(|e| WinError::api("AssignProcessToJobObject", &e))
    }
}
