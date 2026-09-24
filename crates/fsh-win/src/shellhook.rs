//! Shell hook messages (window flashing for attention, etc.) delivered to a window of ours.

use windows::Win32::UI::WindowsAndMessaging::{DeregisterShellHookWindow, RegisterShellHookWindow};

use crate::Hwnd;

pub const HSHELL_WINDOWCREATED: usize = 1;
pub const HSHELL_WINDOWDESTROYED: usize = 2;
pub const HSHELL_WINDOWACTIVATED: usize = 4;
pub const HSHELL_REDRAW: usize = 6;
pub const HSHELL_RUDEAPPACTIVATED: usize = 0x8004;
pub const HSHELL_FLASH: usize = 0x8006;

/// Registration lasts until dropped. Messages arrive as [`ShellHook::message`] with the
/// code in `wparam` and the window in `lparam`.
pub struct ShellHook {
    hwnd: Hwnd,
    pub message: u32,
}

impl ShellHook {
    pub fn register(hwnd: Hwnd) -> Option<Self> {
        let message = crate::window::register_window_message("SHELLHOOK");
        let ok = unsafe { RegisterShellHookWindow(hwnd.raw()).as_bool() };
        (ok && message != 0).then_some(Self { hwnd, message })
    }
}

impl Drop for ShellHook {
    fn drop(&mut self) {
        unsafe {
            let _ = DeregisterShellHookWindow(self.hwnd.raw());
        }
    }
}
