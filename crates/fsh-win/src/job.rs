//! Job objects: plugin processes are placed in one so they die with the shell (even if
//! the shell is killed) and can't use unbounded memory.

use std::os::windows::io::AsRawHandle;

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};

pub struct Job(HANDLE);

// A job handle is a kernel object reference; it can be used from any thread.
unsafe impl Send for Job {}

impl Job {
    /// A job that kills its processes when the last handle to it closes, with an optional
    /// per-process memory limit.
    pub fn new(memory_limit_bytes: Option<usize>) -> anyhow::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(None, None).context("CreateJobObjectW")?;
            let job = Job(handle);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            if let Some(limit) = memory_limit_bytes {
                info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = limit;
            }
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .context("SetInformationJobObject")?;
            Ok(job)
        }
    }

    pub fn assign(&self, child: &std::process::Child) -> anyhow::Result<()> {
        let process = HANDLE(child.as_raw_handle());
        unsafe { AssignProcessToJobObject(self.0, process) }.context("AssignProcessToJobObject")
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
