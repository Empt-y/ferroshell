//! What's on the desktop, as Explorer shows it: the files in your Desktop folder and the
//! Public one, plus the classic icons (This PC, Recycle Bin…) that Settings > Desktop icon
//! settings turns on, and Explorer's view settings for the desktop.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_PublicDesktop, IShellItemImageFactory, KF_FLAG_DEFAULT, SHCreateItemFromParsingName,
    SHFILEINFOW, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath, SIIGBF_BIGGERSIZEOK,
    SIIGBF_ICONONLY,
};
use windows::core::{GUID, PCWSTR};

use crate::icon::RgbaImage;
use crate::wide;

/// One icon on the desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopEntry {
    /// Its shell parsing name: a file path, or `::{CLSID}` for a classic icon.
    pub key: String,
    pub name: String,
    /// 0 classic icons, 1 folders, 2 files (Windows lists them in that order).
    pub rank: u8,
    pub size: u64,
    pub type_name: String,
    /// Last-modified time (Unix seconds).
    pub modified: i64,
    pub is_folder: bool,
    /// A file (not a classic icon): can be renamed and deleted.
    pub path: Option<PathBuf>,
}

/// The icons "Desktop icon settings" controls, with whether each shows by default.
const CLASSIC: [(&str, bool); 5] = [
    ("{20D04FE0-3AEA-1069-A2D8-08002B30309D}", false), // This PC
    ("{59031a47-3f72-44a7-89c5-5595fe6b30ee}", false), // your user folder
    ("{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", false), // Network
    ("{645FF040-5081-101B-9F08-00AA002F954E}", true),  // Recycle Bin
    ("{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}", false), // Control Panel
];

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

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path
    }
}

/// The user's Desktop folder (where new files go).
pub fn desktop_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

/// The folders whose contents are on the desktop: yours, then Public.
pub fn desktop_folders() -> Vec<PathBuf> {
    [known_folder(&FOLDERID_Desktop), known_folder(&FOLDERID_PublicDesktop)].into_iter().flatten().collect()
}

/// A shell item's display name (as Explorer shows it: no `.lnk`, and no extension when
/// known extensions are hidden). Needs COM.
pub fn display_name(parsing_name: &str) -> Option<String> {
    crate::winops::display_name(parsing_name)
}

fn type_name(path: &Path, is_folder: bool) -> String {
    let w = wide(&path.to_string_lossy());
    let mut info = SHFILEINFOW::default();
    let attrs = if is_folder { 0x10 } else { 0x80 }; // FILE_ATTRIBUTE_DIRECTORY / NORMAL
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(w.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(attrs),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if ok == 0 {
        return String::new();
    }
    let end = info.szTypeName.iter().position(|&u| u == 0).unwrap_or(info.szTypeName.len());
    String::from_utf16_lossy(&info.szTypeName[..end])
}

/// Everything on the desktop, unsorted. Hidden and system files stay hidden, as they do
/// by default in Explorer. Needs COM (for names).
pub fn entries() -> Vec<DesktopEntry> {
    let mut out = Vec::new();
    const PANEL: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel";
    for (clsid, default_shown) in CLASSIC {
        let shown = read_dword(PANEL, clsid).map_or(default_shown, |v| v == 0);
        if !shown {
            continue;
        }
        let key = format!("::{clsid}");
        let Some(name) = display_name(&key) else { continue };
        out.push(DesktopEntry { key, name, rank: 0, size: 0, type_name: String::new(), modified: 0, is_folder: true, path: None });
    }
    for dir in desktop_folders() {
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        for e in list.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0 {
                continue;
            }
            let path = e.path();
            let key = path.to_string_lossy().into_owned();
            let is_folder = meta.is_dir();
            let name = display_name(&key).unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            out.push(DesktopEntry {
                key,
                name,
                rank: if is_folder { 1 } else { 2 },
                size: if is_folder { 0 } else { meta.len() },
                type_name: type_name(&path, is_folder),
                modified,
                is_folder,
                path: Some(path),
            });
        }
    }
    out
}

/// Explorer's view settings for the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewSettings {
    /// Icon size in pixels (32 small, 48 medium, 96 large).
    pub icon_size: u32,
    pub auto_arrange: bool,
    /// View > Show desktop icons.
    pub show_icons: bool,
}

const BAG: &str = r"Software\Microsoft\Windows\Shell\Bags\1\Desktop";
const FWF_AUTOARRANGE: u32 = 0x1;
const FWF_NOICONS: u32 = 0x1000;

pub fn view_settings() -> ViewSettings {
    let flags = read_dword(BAG, "FFlags").unwrap_or(0x40200224);
    ViewSettings {
        icon_size: read_dword(BAG, "IconSize").unwrap_or(48),
        auto_arrange: flags & FWF_AUTOARRANGE != 0,
        show_icons: flags & FWF_NOICONS == 0,
    }
}

/// Saves view changes where Explorer keeps them, so it agrees if it runs again.
pub fn save_view_settings(v: ViewSettings) {
    use windows::Win32::System::Registry::{REG_DWORD, RegSetKeyValueW};
    let mut flags = read_dword(BAG, "FFlags").unwrap_or(0x40200224);
    flags = if v.auto_arrange { flags | FWF_AUTOARRANGE } else { flags & !FWF_AUTOARRANGE };
    flags = if v.show_icons { flags & !FWF_NOICONS } else { flags | FWF_NOICONS };
    let key = wide(BAG);
    for (name, value) in [("FFlags", flags), ("IconSize", v.icon_size)] {
        let n = wide(name);
        unsafe {
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(n.as_ptr()),
                REG_DWORD.0,
                Some(&value as *const u32 as *const _),
                4,
            );
        }
    }
}

/// A desktop item's picture at about `size` pixels: a thumbnail for pictures and videos,
/// else its icon. Needs COM.
pub fn item_image(parsing_name: &str, size: i32, thumbnail: bool) -> Option<RgbaImage> {
    if !thumbnail {
        return crate::icon::shell_item_icon(parsing_name, size);
    }
    let w = wide(parsing_name);
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let hbm = factory
            .GetImage(windows::Win32::Foundation::SIZE { cx: size, cy: size }, SIIGBF_BIGGERSIZEOK)
            .or_else(|_| factory.GetImage(windows::Win32::Foundation::SIZE { cx: size, cy: size }, SIIGBF_ICONONLY))
            .ok()?;
        let img = crate::icon::hbitmap_to_rgba(hbm);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        img
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_this_desktop() {
        let _com = crate::com::ComGuard::new();
        let items = entries();
        for e in &items {
            println!("{} | {} | rank {} | {} | {}", e.name, e.key, e.rank, e.type_name, e.size);
        }
        assert!(!desktop_folders().is_empty());
        assert!(items.iter().all(|e| !e.name.is_empty() && !e.name.eq_ignore_ascii_case("desktop.ini")));
        println!("{:?}", view_settings());
        let first = items.iter().find(|e| e.path.is_some()).map(|e| e.key.clone());
        if let Some(k) = first {
            let img = item_image(&k, 48, true).expect("an icon");
            assert!(img.width >= 32 && img.pixels.chunks(4).any(|p| p[3] > 0));
        }
    }
}
