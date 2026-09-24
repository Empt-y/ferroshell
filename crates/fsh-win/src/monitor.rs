//! Display monitors and their DPI.

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

/// A rectangle in physical (device) pixels, right/bottom exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub(crate) fn from_raw(r: RECT) -> Self {
        Self { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
    pub(crate) fn to_raw(self) -> RECT {
        RECT { left: self.left, top: self.top, right: self.right, bottom: self.bottom }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    /// Opaque monitor handle; changes when the display configuration changes.
    pub handle: isize,
    /// Device name such as `\\.\DISPLAY1`; stable across reconfiguration.
    pub device: String,
    pub rect: Rect,
    /// The monitor minus space reserved by app bars (ours and others').
    pub work: Rect,
    pub primary: bool,
    /// Effective DPI (96 = 100% scaling).
    pub dpi: u32,
}

impl Monitor {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// All monitors, ordered left-to-right then top-to-bottom so indices in the config are
/// predictable ("monitor 0 is the leftmost").
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        unsafe {
            if GetMonitorInfoW(h, &mut info.monitorInfo as *mut _).as_bool() {
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
                out.push(Monitor {
                    handle: h.0 as isize,
                    device: String::from_utf16_lossy(&info.szDevice[..len]),
                    rect: Rect::from_raw(info.monitorInfo.rcMonitor),
                    work: Rect::from_raw(info.monitorInfo.rcWork),
                    primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                    dpi: dx,
                });
            }
        }
        BOOL(1)
    }
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut _ as isize));
    }
    out.sort_by_key(|m| (m.rect.left, m.rect.top));
    out
}
