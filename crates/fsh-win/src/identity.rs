//! Package identity of the current process (see `packaging/identity/`). Some Windows APIs,
//! notably reading other apps' notifications, only work for processes that have one.

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::core::PWSTR;

/// The package full name (`Ferroshell.Shell_0.1.0.0_x64__<publisher id>`), or `None` when
/// the process runs without identity (the identity package isn't registered, or the exe
/// isn't the one in the package's external location).
pub fn package_full_name() -> Option<String> {
    let mut len = 0u32;
    let rc = unsafe { GetCurrentPackageFullName(&mut len, None) };
    if rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        // APPMODEL_ERROR_NO_PACKAGE: no identity.
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let rc = unsafe { GetCurrentPackageFullName(&mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    (rc == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processes_have_no_identity() {
        // The test harness isn't the packaged fsh-shell.exe.
        assert_eq!(package_full_name(), None);
    }
}
