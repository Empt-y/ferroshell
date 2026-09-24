//! Native context menus.

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    PostMessageW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TPM_TOPALIGN, TRACK_POPUP_MENU_FLAGS, TrackPopupMenuEx, WM_NULL,
};
use windows::core::{PCSTR, PCWSTR, w};

use crate::{Hwnd, wide};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Item { id: u32, label: String, enabled: bool, checked: bool },
    Separator,
}

impl MenuItem {
    pub fn new(id: u32, label: impl Into<String>) -> Self {
        MenuItem::Item { id, label: label.into(), enabled: true, checked: false }
    }
    pub fn disabled(label: impl Into<String>) -> Self {
        MenuItem::Item { id: 0, label: label.into(), enabled: false, checked: false }
    }
}

/// Which way the menu opens from the anchor point (away from the panel's edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Above,
    Below,
    LeftOf,
    RightOf,
}

/// Show a menu at screen position (x, y) and wait for a choice. Returns the chosen item's
/// id, or `None` if dismissed. Runs a modal loop on the calling (UI) thread.
pub fn popup(owner: Hwnd, x: i32, y: i32, anchor: Anchor, items: &[MenuItem]) -> Option<u32> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            match item {
                MenuItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuItem::Item { id, label, enabled, checked } => {
                    let mut flags: MENU_ITEM_FLAGS = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    let text = wide(&label.replace('&', "&&"));
                    let _ = AppendMenuW(menu, flags, *id as usize, PCWSTR(text.as_ptr()));
                }
            }
        }
        let align: TRACK_POPUP_MENU_FLAGS = match anchor {
            Anchor::Above => TPM_BOTTOMALIGN | TPM_LEFTALIGN,
            Anchor::Below => TPM_TOPALIGN | TPM_LEFTALIGN,
            Anchor::LeftOf => TPM_RIGHTALIGN | TPM_TOPALIGN,
            Anchor::RightOf => TPM_LEFTALIGN | TPM_TOPALIGN,
        };
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(owner.raw());
        let chosen = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | align).0, x, y, owner.raw(), None);
        let _ = PostMessageW(Some(owner.raw()), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        (chosen.0 > 0).then_some(chosen.0 as u32)
    }
}

/// Let native menus follow dark mode. Uses the (undocumented but stable since 1903)
/// `SetPreferredAppMode` export of uxtheme; does nothing if it's unavailable.
pub fn set_dark_menus(dark: bool) {
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type FlushMenuThemes = unsafe extern "system" fn();
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(uxtheme, PCSTR(135 as *const u8)) {
            let set: SetPreferredAppMode = std::mem::transmute(f);
            set(if dark { 2 } else { 3 }); // ForceDark / ForceLight
        }
        if let Some(f) = GetProcAddress(uxtheme, PCSTR(136 as *const u8)) {
            let flush: FlushMenuThemes = std::mem::transmute(f);
            flush();
        }
    }
}
