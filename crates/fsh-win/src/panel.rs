//! Turning an ordinary top-level window (e.g. one created by Slint/winit) into a shell
//! panel: no taskbar/Alt+Tab entry, never takes focus, stays on top, optional backdrop.

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_MAINWINDOW, DWMSBT_NONE,
    DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use crate::Hwnd;
use crate::monitor::Rect;

/// Apply panel window styles. Call after the window exists (for Slint: after `show()`).
pub fn make_panel_window(hwnd: Hwnd) {
    unsafe {
        let h = hwnd.raw();
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0;
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Style a popup that takes keyboard focus (e.g. the launcher): no taskbar/Alt+Tab entry,
/// always on top, but activatable, unlike panels.
pub fn make_popup_window(hwnd: Hwnd) {
    unsafe {
        let h = hwnd.raw();
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let ex = (ex & !(WS_EX_APPWINDOW.0 | WS_EX_NOACTIVATE.0)) | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0;
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

/// Keep the panel above normal windows, or drop it behind (e.g. while a game is fullscreen).
pub fn set_topmost(hwnd: Hwnd, topmost: bool) {
    unsafe {
        let _ = SetWindowPos(
            hwnd.raw(),
            Some(if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Move/resize in physical pixels without activating or changing z-order.
pub fn set_rect(hwnd: Hwnd, r: Rect) {
    unsafe {
        let _ = SetWindowPos(
            hwnd.raw(),
            None,
            r.left,
            r.top,
            r.width(),
            r.height(),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

pub fn window_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd.raw(), &mut r).ok().map(|_| Rect::from_raw(r)) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    None,
    Acrylic,
    Mica,
}

/// Ask DWM to draw a Windows 11 material behind the window's transparent pixels.
/// Silently does nothing on systems that don't support it.
pub fn set_backdrop(hwnd: Hwnd, backdrop: Backdrop, dark: bool) {
    let kind: DWM_SYSTEMBACKDROP_TYPE = match backdrop {
        Backdrop::None => DWMSBT_NONE,
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        Backdrop::Mica => DWMSBT_MAINWINDOW,
    };
    unsafe {
        let h = hwnd.raw();
        let dark = i32::from(dark);
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            size_of::<i32>() as u32,
        );
        // The material only shows through where the frame is extended into the client area.
        let margins = if backdrop == Backdrop::None {
            MARGINS::default()
        } else {
            MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 }
        };
        let _ = DwmExtendFrameIntoClientArea(h, &margins);
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &kind as *const _ as *const _,
            size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        );
    }
}

/// Let DWM round the window's corners (Windows 11), e.g. for floating panels.
pub fn set_rounded_corners(hwnd: Hwnd, round: bool) {
    let pref: DWM_WINDOW_CORNER_PREFERENCE = if round { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd.raw(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}
