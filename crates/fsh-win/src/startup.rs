//! Reading and starting what Windows starts at sign-in (Run/RunOnce keys, Startup folders,
//! packaged startup tasks). Explorer does this when it's the shell; Ferroshell does it when
//! it replaces Explorer. The rules for what to start are in `fsh_core::startup`.

use std::path::PathBuf;

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, REG_BINARY, REG_CREATED_NEW_KEY, REG_EXPAND_SZ, REG_OPTION_VOLATILE, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RRF_RT_REG_DWORD, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::System::Threading::{
    CREATE_DEFAULT_ERROR_MODE, CREATE_NEW_PROCESS_GROUP, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{FOLDERID_CommonStartup, FOLDERID_Startup, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CLEANBOOT};
use windows::core::{GUID, HSTRING, PCWSTR, PWSTR};

use crate::wide;

/// Where a Run/RunOnce key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
    /// `HKLM\SOFTWARE\WOW6432Node\...`, what 32-bit installers write.
    LocalMachine32,
}

impl Hive {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentUser => "HKCU",
            Self::LocalMachine => "HKLM",
            Self::LocalMachine32 => "HKLM (32-bit)",
        }
    }

    fn root(self) -> HKEY {
        match self {
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine | Self::LocalMachine32 => HKEY_LOCAL_MACHINE,
        }
    }

    fn view(self) -> REG_SAM_FLAGS {
        match self {
            Self::LocalMachine32 => KEY_WOW64_32KEY,
            _ => KEY_WOW64_64KEY,
        }
    }

    /// The `StartupApproved` subkey Task Manager keeps this hive's Run switches in.
    fn approved_subkey(self) -> &'static str {
        match self {
            Self::LocalMachine32 => "Run32",
            _ => "Run",
        }
    }
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
const TASK_STATE: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const DONE_KEY: &str = r"Software\Ferroshell\Volatile\StartupDone";

struct Key(HKEY);

impl Key {
    fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<Self> {
        let path = wide(path);
        let mut key = HKEY::default();
        let r = unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) };
        (r == ERROR_SUCCESS).then_some(Self(key))
    }

    /// Every value as `(name, type, data)`.
    fn values(&self) -> Vec<(String, REG_VALUE_TYPE, Vec<u8>)> {
        let mut out = Vec::new();
        let mut data = vec![0u8; 1024];
        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 16384];
            let mut name_len = name.len() as u32;
            let mut ty = 0u32;
            let mut data_len = data.len() as u32;
            let r = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut ty),
                    Some(data.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if r == ERROR_MORE_DATA {
                data.resize(data_len as usize + 2, 0);
                continue;
            }
            if r == ERROR_NO_MORE_ITEMS || r != ERROR_SUCCESS {
                break;
            }
            out.push((
                String::from_utf16_lossy(&name[..name_len as usize]),
                REG_VALUE_TYPE(ty),
                data[..data_len as usize].to_vec(),
            ));
            index += 1;
        }
        out
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn utf16_string(data: &[u8]) -> String {
    let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Expands `%VARIABLES%` against this process's environment.
pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let src = wide(s);
    let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None) };
    if needed == 0 {
        return s.to_owned();
    }
    let mut buf = vec![0u16; needed as usize];
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) };
    if n == 0 || n as usize > buf.len() {
        return s.to_owned();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

/// A Run or RunOnce key's entries as `(value name, command line)`, environment expanded.
pub fn run_entries(hive: Hive, once: bool) -> Vec<(String, String)> {
    let path = if once { RUN_ONCE } else { RUN };
    let Some(key) = Key::open(hive.root(), path, KEY_READ | hive.view()) else { return vec![] };
    key.values()
        .into_iter()
        .filter(|(_, ty, _)| *ty == REG_SZ || *ty == REG_EXPAND_SZ)
        .map(|(name, ty, data)| {
            let cmd = utf16_string(&data);
            (name, if ty == REG_EXPAND_SZ { expand_env(&cmd) } else { cmd })
        })
        .collect()
}

/// The Task Manager switch data for a Run entry, from the matching hive (and, for machine
/// entries, the user's own override if there is one).
pub fn run_approval(hive: Hive, name: &str) -> Option<Vec<u8>> {
    let sub = format!(r"{APPROVED}\{}", hive.approved_subkey());
    let own = || approval_value(HKEY_CURRENT_USER, &sub, name);
    match hive {
        Hive::CurrentUser => own(),
        _ => approval_value(HKEY_LOCAL_MACHINE, &sub, name).or_else(own),
    }
}

/// The switch data for a Startup-folder item (by file name, e.g. `Ollama.lnk`).
pub fn folder_approval(common: bool, file_name: &str) -> Option<Vec<u8>> {
    let sub = format!(r"{APPROVED}\StartupFolder");
    let own = approval_value(HKEY_CURRENT_USER, &sub, file_name);
    if common { approval_value(HKEY_LOCAL_MACHINE, &sub, file_name).or(own) } else { own }
}

fn approval_value(root: HKEY, subkey: &str, name: &str) -> Option<Vec<u8>> {
    let key = Key::open(root, subkey, KEY_READ | KEY_WOW64_64KEY)?;
    key.values()
        .into_iter()
        .find(|(n, ty, _)| n.eq_ignore_ascii_case(name) && *ty == REG_BINARY)
        .map(|(_, _, data)| data)
}

/// Deletes a RunOnce value (Explorer removes each before or after running it).
pub fn delete_run_once(hive: Hive, name: &str) -> anyhow::Result<()> {
    let key = Key::open(hive.root(), RUN_ONCE, KEY_SET_VALUE | hive.view())
        .with_context(|| format!("opening {} RunOnce for writing", hive.label()))?;
    let name = wide(name);
    let r = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
    anyhow::ensure!(r == ERROR_SUCCESS, "deleting the RunOnce value failed ({r:?})");
    Ok(())
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path
    }
}

/// The files in the user's (or all users') Startup folder, sorted by name.
pub fn startup_folder(common: bool) -> Vec<PathBuf> {
    let Some(dir) = known_folder(if common { &FOLDERID_CommonStartup } else { &FOLDERID_Startup }) else { return vec![] };
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("desktop.ini")))
        .collect();
    files.sort();
    files
}

/// An installed app package that isn't a framework or resource package.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub family_name: String,
    pub install_path: PathBuf,
}

/// The current user's app packages. Needs COM (MTA) on the calling thread.
pub fn user_packages() -> anyhow::Result<Vec<PackageInfo>> {
    let pm = windows::Management::Deployment::PackageManager::new().context("PackageManager")?;
    let mut out = Vec::new();
    for p in pm.FindPackagesByUserSecurityId(&HSTRING::new()).context("FindPackagesByUserSecurityId")? {
        if p.IsFramework().unwrap_or(true) || p.IsResourcePackage().unwrap_or(false) {
            continue;
        }
        let (Ok(id), Ok(path)) = (p.Id(), p.InstalledPath()) else { continue };
        let Ok(family) = id.FamilyName() else { continue };
        out.push(PackageInfo { family_name: family.to_string(), install_path: PathBuf::from(path.to_string()) });
    }
    Ok(out)
}

/// A packaged startup task's user state (`StartupTaskState`), if it has one yet.
pub fn startup_task_state(family_name: &str, task_id: &str) -> Option<u32> {
    let (subkey, value) = (wide(&format!(r"{TASK_STATE}\{family_name}\{task_id}")), wide("State"));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

/// Whether startup apps have already been started this sign-in, marking them started if
/// not. Uses a volatile registry key, which Windows discards at sign-out, so restarting
/// Ferroshell never starts them twice. Returns `true` the first time.
pub fn claim_startup_run() -> anyhow::Result<bool> {
    let path = wide(DONE_KEY);
    let mut key = HKEY::default();
    let mut disposition = Default::default();
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_VOLATILE,
            KEY_QUERY_VALUE,
            None,
            &mut key,
            Some(&mut disposition),
        )
    };
    anyhow::ensure!(r == ERROR_SUCCESS, "creating the startup marker failed ({r:?})");
    drop(Key(key));
    Ok(disposition == REG_CREATED_NEW_KEY)
}

/// Whether Windows itself booted in safe mode (only `*` RunOnce entries run then).
pub fn windows_safe_mode() -> bool {
    unsafe { GetSystemMetrics(SM_CLEANBOOT) != 0 }
}

/// Starts a Run-key command line as Explorer does (`CreateProcess`, not the shell), outside
/// our console and Ctrl+C group. Returns the process id.
pub fn run_command_line(command: &str) -> anyhow::Result<u32> {
    let mut cmd = wide(command);
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_DEFAULT_ERROR_MODE | CREATE_NEW_PROCESS_GROUP,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    }
    .with_context(|| format!("starting {command}"))?;
    unsafe {
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    Ok(pi.dwProcessId)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_environment() {
        let windir = std::env::var("WINDIR").unwrap();
        assert!(expand_env(r"%WINDIR%\system32").eq_ignore_ascii_case(&format!(r"{windir}\system32")));
        assert_eq!(expand_env("plain"), "plain");
    }

    #[test]
    fn decodes_registry_strings() {
        let data: Vec<u8> = "abc\0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(utf16_string(&data), "abc");
    }
}
