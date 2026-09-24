//! Session and power actions, and who's logged in.

use std::path::PathBuf;

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::{
    EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, EWX_SHUTDOWN, EXIT_WINDOWS_FLAGS, ExitWindowsEx, LockWorkStation,
    SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR, w};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Lock,
    Sleep,
    Restart,
    Shutdown,
    Logout,
}

impl PowerAction {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lock" => Self::Lock,
            "sleep" => Self::Sleep,
            "restart" => Self::Restart,
            "shutdown" => Self::Shutdown,
            "logout" => Self::Logout,
            _ => return None,
        })
    }

    /// Actions that end the session and should be confirmed first.
    pub fn needs_confirmation(self) -> bool {
        matches!(self, Self::Restart | Self::Shutdown | Self::Logout)
    }
}

/// Our own process may shut down/restart the machine once this privilege is enabled on
/// its token (standard requirement of `ExitWindowsEx`).
fn enable_shutdown_privilege() -> anyhow::Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).context("OpenProcessToken")?;
        let mut luid = LUID::default();
        let r = LookupPrivilegeValueW(PCWSTR::null(), w!("SeShutdownPrivilege"), &mut luid);
        let result = r.context("LookupPrivilegeValueW").and_then(|_| {
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None).context("AdjustTokenPrivileges")
        });
        let _ = CloseHandle(token);
        result
    }
}

pub fn perform(action: PowerAction) -> anyhow::Result<()> {
    let exit = |flags: EXIT_WINDOWS_FLAGS| unsafe {
        ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED).context("ExitWindowsEx")
    };
    match action {
        PowerAction::Lock => unsafe { LockWorkStation() }.context("LockWorkStation"),
        PowerAction::Sleep => {
            if unsafe { SetSuspendState(false, false, false) } {
                Ok(())
            } else {
                anyhow::bail!("SetSuspendState failed (sleep may be disabled on this PC)")
            }
        }
        PowerAction::Logout => exit(EWX_LOGOFF),
        PowerAction::Restart => {
            enable_shutdown_privilege()?;
            exit(EWX_REBOOT)
        }
        PowerAction::Shutdown => {
            enable_shutdown_privilege()?;
            exit(EWX_SHUTDOWN | EWX_POWEROFF)
        }
    }
}

/// The user's display name ("Ash Smith"), falling back to the account name.
pub fn user_display_name() -> String {
    let mut buf = vec![0u16; 256];
    let mut len = buf.len() as u32;
    let ok = unsafe { GetUserNameExW(NameDisplay, Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    if ok && len > 0 {
        let s = String::from_utf16_lossy(&buf[..len as usize]);
        if !s.trim().is_empty() {
            return s;
        }
    }
    std::env::var("USERNAME").unwrap_or_default()
}

fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len);
        let _ = CloseHandle(token);
        ok.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(s.0 as *mut _)));
        out
    }
}

/// The user's account picture, if Windows has cached one (largest available size).
pub fn account_picture() -> Option<PathBuf> {
    let public = std::env::var_os("PUBLIC").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    let dir = public.join("AccountPictures").join(user_sid()?);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("png")))
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actions_and_knows_which_to_confirm() {
        assert_eq!(PowerAction::parse("shutdown"), Some(PowerAction::Shutdown));
        assert_eq!(PowerAction::parse("nope"), None);
        assert!(PowerAction::Restart.needs_confirmation());
        assert!(!PowerAction::Lock.needs_confirmation());
    }

    #[test]
    fn knows_the_user() {
        assert!(!user_display_name().is_empty());
        assert!(user_sid().is_some_and(|s| s.starts_with("S-1-")));
    }
}
