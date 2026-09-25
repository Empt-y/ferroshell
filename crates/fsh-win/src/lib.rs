//! Safe wrappers over the Win32 APIs Ferroshell needs. This is the only crate in the
//! workspace that may use `unsafe`; everything it exports is safe to call.

pub mod appbar;
pub mod audio;
pub mod brightness;
pub mod bluetooth;
pub mod appsfolder;
pub mod clipboard;
pub mod com;
pub mod console;
pub mod crash;
pub mod hotkey;
pub mod icon;
pub mod identity;
pub mod job;
pub mod keyhook;
pub mod media;
pub mod menu;
pub mod monitor;
pub mod network;
pub mod notifications;
pub mod panel;
pub mod power;
pub mod process;
pub mod session;
pub mod shellhook;
pub mod startup;
pub mod system;
pub mod taskbar;
pub mod thumbnail;
pub mod tray;
pub mod window;
pub mod winevent;
pub mod winfo;
pub mod winops;

pub use monitor::{Monitor, Rect};
pub use window::{Hwnd, MessageWindow};

/// Encode a string as a NUL-terminated UTF-16 buffer for `PCWSTR` parameters.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
