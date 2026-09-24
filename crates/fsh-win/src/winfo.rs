//! Reading facts about top-level windows. Nothing here sends messages to the window, so
//! none of it can block on a hung application.

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, GetMonitorInfoW, MonitorFromWindow};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use crate::Hwnd;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: Hwnd,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub visible: bool,
    /// Hidden by DWM: on another virtual desktop, or a suspended/hidden UWP window.
    pub cloaked: bool,
    pub minimized: bool,
    pub tool_window: bool,
    pub app_window: bool,
    pub no_activate: bool,
    pub has_owner: bool,
}

/// All top-level windows in z-order.
pub fn top_level_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe { (*(lparam.0 as *mut Vec<Hwnd>)).push(Hwnd::from_raw(hwnd)) };
        BOOL(1)
    }
    let mut out: Vec<Hwnd> = Vec::with_capacity(256);
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

pub fn class_name(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd.raw(), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn title(hwnd: Hwnd) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd.raw());
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd.raw(), &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn pid(hwnd: Hwnd) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd.raw(), Some(&mut pid)) };
    pid
}

pub fn is_cloaked(hwnd: Hwnd) -> bool {
    let mut cloaked = 0u32;
    let r = unsafe {
        DwmGetWindowAttribute(hwnd.raw(), DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, size_of::<u32>() as u32)
    };
    r.is_ok() && cloaked != 0
}

pub fn info(hwnd: Hwnd) -> Option<WindowInfo> {
    if !hwnd.exists() {
        return None;
    }
    unsafe {
        let h = hwnd.raw();
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        Some(WindowInfo {
            hwnd,
            pid: pid(hwnd),
            title: title(hwnd),
            class: class_name(hwnd),
            visible: IsWindowVisible(h).as_bool(),
            cloaked: is_cloaked(hwnd),
            minimized: IsIconic(h).as_bool(),
            tool_window: ex & WS_EX_TOOLWINDOW.0 != 0,
            app_window: ex & WS_EX_APPWINDOW.0 != 0,
            no_activate: ex & WS_EX_NOACTIVATE.0 != 0,
            has_owner: GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()),
        })
    }
}

pub fn foreground() -> Option<Hwnd> {
    let h = unsafe { GetForegroundWindow() };
    (!h.is_invalid()).then(|| Hwnd::from_raw(h))
}

/// Full path of a process's executable.
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// App id of a packaged (Store/UWP/MSIX) process.
pub fn process_app_id(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 512];
        let mut len = buf.len() as u32;
        let r = GetApplicationUserModelId(handle, &mut len, Some(PWSTR(buf.as_mut_ptr())));
        let _ = CloseHandle(handle);
        if r.is_err() || len == 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// Does the window's thread answer messages within `timeout_ms`? Check this before any
/// API that may message the window internally, so a hung app can't hang the caller.
pub fn is_responsive(hwnd: Hwnd, timeout_ms: u32) -> bool {
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{IsHungAppWindow, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_NULL};
    unsafe {
        if IsHungAppWindow(hwnd.raw()).as_bool() {
            return false;
        }
        SendMessageTimeoutW(hwnd.raw(), WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, timeout_ms, None).0 != 0
    }
}

/// The explicit AppUserModelID a window was given (apps that group their windows set this).
/// Requires COM on the calling thread. May message the window: check [`is_responsive`] first.
pub fn window_app_id(hwnd: Hwnd) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd.raw()).ok()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let s = PropVariantToStringAlloc(&value).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|s| !s.is_empty())
    }
}

/// For a UWP frame window (`ApplicationFrameWindow`), the hosted app's core window.
pub fn uwp_core_window(frame: Hwnd) -> Option<Hwnd> {
    unsafe extern "system" fn find(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Option<Hwnd>) };
        let h = Hwnd::from_raw(hwnd);
        if class_name(h) == "Windows.UI.Core.CoreWindow" {
            *out = Some(h);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut found: Option<Hwnd> = None;
    unsafe {
        let _ = EnumChildWindows(Some(frame.raw()), Some(find), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

/// Is the foreground window a full-screen app (game, video, presentation) covering
/// `monitor`? The desktop and shell windows don't count.
pub fn fullscreen_app_on(monitor: crate::Rect) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let Some(fg) = foreground() else { return false };
    if matches!(class_name(fg).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return false;
    }
    if pid(fg) == std::process::id() || is_cloaked(fg) {
        return false;
    }
    let mut r = RECT::default();
    if unsafe { GetWindowRect(fg.raw(), &mut r) }.is_err() {
        return false;
    }
    r.left <= monitor.left && r.top <= monitor.top && r.right >= monitor.right && r.bottom >= monitor.bottom
}

/// Device name (e.g. `\\.\DISPLAY1`) of the monitor a window is mostly on.
pub fn monitor_device(hwnd: Hwnd) -> Option<String> {
    unsafe {
        let m = MonitorFromWindow(hwnd.raw(), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(m, &mut info.monitorInfo as *mut _).as_bool() {
            return None;
        }
        let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        Some(String::from_utf16_lossy(&info.szDevice[..len]))
    }
}
