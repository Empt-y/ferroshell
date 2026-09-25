//! The installed-apps list, as Windows Start shows it: the `shell:AppsFolder` virtual
//! folder (desktop apps from Start-menu shortcuts plus Store/packaged apps).

use anyhow::Context;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName, SIGDN, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{GUID, Interface, w};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellApp {
    pub name: String,
    /// What follows `shell:AppsFolder\` to launch it: an AUMID, or a path (possibly
    /// starting with a known-folder GUID).
    pub parsing_name: String,
    /// `System.AppUserModel.HostEnvironment`: 0 classic desktop app, 1 UWP app, 2 desktop
    /// app in a package (e.g. Windows Terminal). `None` if Windows didn't say.
    pub host_environment: Option<u32>,
}

impl ShellApp {
    /// Can it be started elevated ("Run as administrator")? Everything but UWP apps.
    pub fn elevatable(&self) -> Option<bool> {
        self.host_environment.map(|h| h != 1)
    }
}

/// `System.AppUserModel.HostEnvironment` (not in windows-rs).
const PKEY_APPUSERMODEL_HOSTENVIRONMENT: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 14 };

fn host_environment(item: &IShellItem) -> Option<u32> {
    let item2: IShellItem2 = item.cast().ok()?;
    unsafe { item2.GetUInt32(&PKEY_APPUSERMODEL_HOSTENVIRONMENT) }.ok()
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Enumerate every app. Requires COM on the calling thread; takes tens to hundreds of
/// milliseconds, so call it from a worker thread.
pub fn enumerate() -> anyhow::Result<Vec<ShellApp>> {
    let mut out = Vec::new();
    unsafe {
        let folder: IShellItem = SHCreateItemFromParsingName(w!("shell:AppsFolder"), None).context("opening shell:AppsFolder")?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems).context("enumerating apps")?;
        loop {
            let mut batch: [Option<IShellItem>; 16] = Default::default();
            let mut fetched = 0u32;
            let hr = items.Next(&mut batch, Some(&mut fetched));
            for item in batch.iter().take(fetched as usize).flatten() {
                if let (Some(name), Some(parsing_name)) =
                    (display_name(item, SIGDN_NORMALDISPLAY), display_name(item, SIGDN_PARENTRELATIVEPARSING))
                    && !name.trim().is_empty()
                {
                    out.push(ShellApp { name, parsing_name, host_environment: host_environment(item) });
                }
            }
            if hr.is_err() || fetched == 0 {
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the real app list; opt-in because results depend on the machine.
    #[test]
    #[ignore = "reads the live app list"]
    fn finds_well_known_apps() {
        let _com = crate::com::ComGuard::new();
        let apps = enumerate().unwrap();
        assert!(apps.len() > 20, "only {} apps", apps.len());
        let has = |needle: &str| apps.iter().any(|a| a.parsing_name.to_lowercase().contains(needle));
        assert!(has("windowscalculator"), "Calculator");
        assert!(has("windows.immersivecontrolpanel") || has("settings"), "Settings");
        eprintln!("{} apps, e.g. {:?}", apps.len(), &apps[..5.min(apps.len())]);
        // Matches what Explorer reports: Terminal is a packaged desktop app, Calculator UWP.
        let host = |needle: &str| apps.iter().find(|a| a.parsing_name.to_lowercase().contains(needle)).and_then(|a| a.host_environment);
        assert_eq!(host("windowscalculator"), Some(1));
        if apps.iter().any(|a| a.parsing_name.contains("WindowsTerminal")) {
            assert_eq!(host("windowsterminal"), Some(2));
        }
    }
}
