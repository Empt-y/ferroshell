//! Minidumps for native crashes (access violations, stack overflows, aborts from C code).
//! Rust panics are logged separately by `fsh-common`'s panic hook.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithIndirectlyReferencedMemory,
    MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows::Win32::System::ErrorReporting::{
    WER_FAULT_REPORTING, WER_FAULT_REPORTING_FLAG_QUEUE, WER_FAULT_REPORTING_NO_UI, WerSetFlags,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};
use windows::core::PCWSTR;

use crate::wide;

/// Prepared ahead of time so the filter does as little as possible inside a crashed process.
static DUMP_PATH: OnceLock<Vec<u16>> = OnceLock::new();

/// Write a minidump to `<dir>/<name>-<pid>.dmp` if this process dies from an unhandled
/// native exception. Returns the path the dump will be written to.
pub fn install_minidump_handler(dir: &Path, name: &str) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("{name}-{}.dmp", std::process::id()));
    let _ = DUMP_PATH.set(wide(&path.to_string_lossy()));
    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
        // By default Windows Error Reporting keeps a crashed process alive for several
        // seconds while it collects a report, delaying the supervisor's restart. Queue the
        // report instead and never show a crash dialog.
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        let _ = WerSetFlags(WER_FAULT_REPORTING(WER_FAULT_REPORTING_FLAG_QUEUE.0 | WER_FAULT_REPORTING_NO_UI));
    }
    path
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    let Some(path) = DUMP_PATH.get() else { return EXCEPTION_CONTINUE_SEARCH };
    unsafe {
        let Ok(file) = CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let exception = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: GetCurrentThreadId(),
            ExceptionPointers: info as *mut _,
            ClientPointers: false.into(),
        };
        let _ = MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file,
            MiniDumpWithIndirectlyReferencedMemory | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
            Some(&exception),
            None,
            None,
        );
        let _ = CloseHandle(file);
    }
    // Let Windows carry on terminating the process; the supervisor notices the exit.
    EXCEPTION_CONTINUE_SEARCH
}
