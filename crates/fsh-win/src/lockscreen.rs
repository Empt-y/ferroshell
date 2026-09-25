//! Windows' lock screen picture. Ferroshell leaves the lock screen itself to Windows (it
//! runs on the secure desktop) but can keep its picture in step with the wallpaper.

use std::path::{Path, PathBuf};

use anyhow::Context;
use windows::Storage::StorageFile;
use windows::System::UserProfile::{LockScreen, UserProfilePersonalizationSettings};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::HSTRING;

/// `WM_SETTINGCHANGE`'s `wParam` when the wallpaper changed.
pub const SPI_SETDESKWALLPAPER: usize = 0x0014;

/// The current desktop wallpaper file, if there is one (not a solid colour or slideshow
/// without a current picture).
pub fn current_wallpaper() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..end]));
    (!path.as_os_str().is_empty() && path.is_file()).then_some(path)
}

/// Sets the lock screen picture. Needs COM (MTA) on the calling thread; blocks until done.
/// Tries the personalisation API first, then the older `LockScreen` one (which needs
/// package identity). Returns which one worked.
pub fn set_image(path: &Path) -> anyhow::Result<&'static str> {
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
        .and_then(|op| op.join())
        .with_context(|| format!("opening {}", path.display()))?;
    let mut problems = Vec::new();
    if UserProfilePersonalizationSettings::IsSupported().unwrap_or(false) {
        match UserProfilePersonalizationSettings::Current()
            .and_then(|s| s.TrySetLockScreenImageAsync(&file))
            .and_then(|op| op.join())
        {
            Ok(true) => return Ok("personalisation settings"),
            Ok(false) => problems.push("personalisation settings: Windows declined".to_owned()),
            Err(e) => problems.push(format!("personalisation settings: {e}")),
        }
    } else {
        problems.push("personalisation settings: not supported here".to_owned());
    }
    match LockScreen::SetImageFileAsync(&file).and_then(|op| op.join()) {
        Ok(()) => Ok("lock screen"),
        Err(e) => {
            problems.push(format!("lock screen: {e}"));
            anyhow::bail!("{}", problems.join("; "))
        }
    }
}
