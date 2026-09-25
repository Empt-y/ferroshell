//! Windows' desktop background settings, setting the wallpaper, and downloading pictures
//! (for Spotlight). The rules live in `fsh_core::wallpaper`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use windows::Foundation::Uri;
use windows::Storage::Streams::DataReader;
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::SHGetPathFromIDListW;
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::wide;

const WALLPAPERS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Wallpapers";
const SLIDESHOW: &str = r"Control Panel\Personalization\Desktop Slideshow";

fn read_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
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

fn read_string(subkey: &str, value: &str) -> Option<String> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut size = 0u32;
    unsafe {
        let r = RegGetValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut size));
        if r != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if r != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// `BackgroundType` as Settings wrote it (0 picture, 1 colour, 2 slideshow, 3 Spotlight).
pub fn background_type() -> Option<u32> {
    read_dword(WALLPAPERS, "BackgroundType")
}

/// Slideshow interval (ms) and shuffle, as Settings wrote them.
pub fn slideshow_options() -> (Option<u32>, bool) {
    (read_dword(SLIDESHOW, "Interval"), read_dword(SLIDESHOW, "Shuffle").unwrap_or(0) != 0)
}

/// The slideshow folders' encoded ID lists (`SlideshowDirectoryPath1`, `2`, ...), falling
/// back to `slideshow.ini`'s `ImagesRootPIDL`.
pub fn slideshow_folder_pidls() -> Vec<String> {
    let mut out: Vec<String> = (1..=16).map_while(|n| read_string(WALLPAPERS, &format!("SlideshowDirectoryPath{n}"))).collect();
    if out.is_empty() {
        let ini = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Themes\slideshow.ini"));
        if let Some(text) = ini.and_then(|p| std::fs::read_to_string(p).ok()) {
            out.extend(text.lines().filter_map(|l| l.trim().strip_prefix("ImagesRootPIDL=")).map(str::to_owned));
        }
    }
    out
}

/// The file-system path of an ITEMIDLIST (as `fsh_core::wallpaper::decode_slideshow_pidl`
/// returns), if it is a folder on disk.
pub fn pidl_path(pidl: &[u8]) -> Option<PathBuf> {
    // Keep it in u16s so the ID list is suitably aligned.
    let mut aligned = vec![0u16; pidl.len().div_ceil(2) + 1];
    unsafe { std::ptr::copy_nonoverlapping(pidl.as_ptr(), aligned.as_mut_ptr() as *mut u8, pidl.len()) };
    let mut out = [0u16; 260];
    let ok = unsafe { SHGetPathFromIDListW(aligned.as_ptr() as *const ITEMIDLIST, &mut out) }.as_bool();
    let end = out.iter().position(|&u| u == 0).unwrap_or(out.len());
    (ok && end > 0).then(|| PathBuf::from(String::from_utf16_lossy(&out[..end])))
}

/// Sets the desktop wallpaper (and tells everyone it changed).
pub fn set_wallpaper(path: &Path) -> anyhow::Result<()> {
    let p = wide(&path.to_string_lossy());
    unsafe {
        SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(p.as_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)
    }
    .with_context(|| format!("setting the wallpaper to {}", path.display()))
}

/// The user's locale name, e.g. `en-GB`.
pub fn user_locale() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n <= 1 {
        return "en-US".into();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

/// Downloads a URL's body as text. Blocking; needs COM (MTA) on the thread.
pub fn http_get_text(url: &str) -> anyhow::Result<String> {
    let client = HttpClient::new()?;
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    let text = client.GetStringAsync(&uri).and_then(|op| op.join()).with_context(|| format!("GET {url}"))?;
    Ok(text.to_string_lossy())
}

/// Downloads a URL's body to a file (written to a temporary name, then renamed).
pub fn http_download(url: &str, to: &Path) -> anyhow::Result<()> {
    let client = HttpClient::new()?;
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    let buffer = client.GetBufferAsync(&uri).and_then(|op| op.join()).with_context(|| format!("GET {url}"))?;
    let len = buffer.Length()? as usize;
    let reader = DataReader::FromBuffer(&buffer)?;
    let mut bytes = vec![0u8; len];
    reader.ReadBytes(&mut bytes)?;
    anyhow::ensure!(len > 0, "{url} was empty");
    let tmp = to.with_extension("part");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, to).with_context(|| format!("renaming to {}", to.display()))?;
    Ok(())
}
