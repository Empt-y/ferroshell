//! Process-level helpers: single-instance guards and waiting on other processes.

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{
    CreateMutexW, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::PCWSTR;

use crate::wide;

/// Holds a named mutex for the life of the process. Dropping it releases the name.
pub struct SingleInstance {
    handle: HANDLE,
}

// SAFETY-adjacent: a mutex handle is a process-wide kernel object reference; moving it
// between threads is fine.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// Returns `Ok(None)` if another process already holds `name`.
    pub fn acquire(name: &str) -> anyhow::Result<Option<Self>> {
        let w = wide(&format!("Local\\{name}"));
        unsafe {
            let handle = CreateMutexW(None, false, PCWSTR(w.as_ptr())).context("CreateMutexW")?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(Self { handle }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Block until the process with `pid` exits. Returns immediately (with `Ok`) if it
/// doesn't exist any more.
pub fn wait_for_exit(pid: u32) -> anyhow::Result<()> {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            return Ok(());
        };
        WaitForSingleObject(handle, INFINITE);
        let _ = CloseHandle(handle);
    }
    Ok(())
}
