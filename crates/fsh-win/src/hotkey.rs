//! Global hotkeys delivered as `WM_HOTKEY` to a window.

use anyhow::Context;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};

use crate::Hwnd;

pub const WM_HOTKEY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    fn to_raw(self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT;
        if self.ctrl {
            m |= MOD_CONTROL;
        }
        if self.alt {
            m |= MOD_ALT;
        }
        if self.shift {
            m |= MOD_SHIFT;
        }
        if self.win {
            m |= MOD_WIN;
        }
        m
    }
}

/// Register a system-wide hotkey. `vk` is a virtual-key code (letters are their ASCII
/// uppercase value, e.g. `b'E'`).
pub fn register(hwnd: Hwnd, id: i32, mods: Modifiers, vk: u32) -> anyhow::Result<()> {
    unsafe { RegisterHotKey(Some(hwnd.raw()), id, mods.to_raw(), vk) }
        .context("RegisterHotKey (is another program already using this key combination?)")
}

pub fn unregister(hwnd: Hwnd, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd.raw()), id);
    }
}
