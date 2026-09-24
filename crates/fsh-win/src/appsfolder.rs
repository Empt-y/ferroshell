//! The installed-apps list, as Windows Start shows it: the `shell:AppsFolder` virtual
//! folder (desktop apps from Start-menu shortcuts plus Store/packaged apps).

use anyhow::Context;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::w;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellApp {
    pub name: String,
    /// What follows `shell:AppsFolder\` to launch it: an AUMID, or a path (possibly
    /// starting with a known-folder GUID).
    pub parsing_name: String,
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
                    out.push(ShellApp { name, parsing_name });
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
    }
}
