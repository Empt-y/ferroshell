//! System appearance settings and small system actions.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_LWIN,
};
use windows::core::PCWSTR;

use crate::wide;

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
    r.is_ok().then_some(data)
}

/// Whether Windows is set to use dark mode for apps.
pub fn prefers_dark() -> bool {
    read_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
        .map(|v| v == 0)
        .unwrap_or(true)
}

/// The user's Windows accent colour as `(r, g, b)`.
pub fn accent_color() -> Option<(u8, u8, u8)> {
    // Stored as 0xAABBGGRR.
    let v = read_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")?;
    Some((v as u8, (v >> 8) as u8, (v >> 16) as u8))
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

/// Tap the Windows key (opens the Windows Start menu while we have no launcher of our own).
pub fn tap_windows_key() {
    let inputs = [key(VK_LWIN, false), key(VK_LWIN, true)];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

/// The mouse cursor's position in screen pixels.
pub fn cursor_pos() -> (i32, i32) {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
