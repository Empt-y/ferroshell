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

/// The "File description" from an executable's version resource ("Mozilla Firefox"), if it
/// has one.
pub fn file_description(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let query = |sub: &str| -> Option<(*const u8, u32)> {
            let sub = wide(sub);
            let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(data.as_ptr().cast(), PCWSTR(sub.as_ptr()), &mut ptr, &mut len).as_bool().then_some((ptr as *const u8, len))
        };
        // The first language/code page, then the neutral English fallbacks.
        let mut langs = Vec::new();
        if let Some((p, len)) = query(r"\VarFileInfo\Translation")
            && len >= 4
        {
            let words = std::slice::from_raw_parts(p.cast::<u16>(), 2);
            langs.push(format!("{:04x}{:04x}", words[0], words[1]));
        }
        langs.extend(["040904b0".to_owned(), "040904e4".to_owned(), "000004b0".to_owned()]);
        for lang in langs {
            if let Some((p, len)) = query(&format!(r"\StringFileInfo\{lang}\FileDescription")) {
                let chars = std::slice::from_raw_parts(p.cast::<u16>(), len as usize);
                let s = String::from_utf16_lossy(chars).trim_end_matches('\0').trim().to_owned();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}