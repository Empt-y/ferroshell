//! Hidden top-level windows with closure-based window procedures, plus the message loop.

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, IsWindow, KillTimer, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW, TranslateMessage,
    WINDOW_EX_STYLE, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

use crate::wide;

/// A window handle that can cross threads. Win32 handles are process-global integers;
/// posting to them from any thread is fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub(crate) fn raw(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    pub(crate) fn from_raw(h: HWND) -> Self {
        Self(h.0 as isize)
    }

    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    /// True if the handle still names a window.
    pub fn exists(self) -> bool {
        unsafe { IsWindow(Some(self.raw())).as_bool() }
    }

    /// Post a message to this window's queue (non-blocking, callable from any thread).
    pub fn post(self, msg: u32, wparam: usize, lparam: isize) -> bool {
        unsafe { PostMessageW(Some(self.raw()), msg, WPARAM(wparam), LPARAM(lparam)).is_ok() }
    }

    /// Start (or reset) a `WM_TIMER` on this window. Call from the window's thread.
    pub fn set_timer(self, id: usize, interval_ms: u32) {
        unsafe {
            SetTimer(Some(self.raw()), id, interval_ms, None);
        }
    }

    pub fn kill_timer(self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.raw()), id);
        }
    }
}

pub const WM_TIMER: u32 = 0x0113;

/// First message id available to applications (`WM_APP`).
pub const WM_APP: u32 = 0x8000;

/// Window procedure closure: return `Some(result)` to handle a message, `None` to fall
/// through to `DefWindowProcW`. It is `Fn` (not `FnMut`) because window procedures are
/// re-entrant; use `Cell`/`RefCell` for state.
pub type WndProc = Box<dyn Fn(Hwnd, u32, usize, isize) -> Option<isize>>;

/// An invisible top-level window. Unlike message-only windows it receives broadcast messages
/// such as `TaskbarCreated`, which is why the supervisor needs one.
pub struct MessageWindow {
    hwnd: Hwnd,
}

impl MessageWindow {
    pub fn new(title: &str, proc_: WndProc) -> anyhow::Result<Self> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let class = format!("Ferroshell.MessageWindow.{}.{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        Self::with_class(&class, title, proc_)
    }

    /// Like [`MessageWindow::new`] with a specific window class name (e.g. `Shell_TrayWnd`,
    /// which other programs look windows up by). The class is registered once per process.
    pub fn with_class(class: &str, title: &str, proc_: WndProc) -> anyhow::Result<Self> {
        let class_name = wide(class);
        let title = wide(title);
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(trampoline),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let err = windows::core::Error::from_thread();
                // Already registered by an earlier window of this class: fine.
                if err.code() != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                    return Err(err).context("RegisterClassExW");
                }
            }
            let boxed: Box<WndProc> = Box::new(proc_);
            let param = Box::into_raw(boxed);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                Some(param as *const _),
            );
            match hwnd {
                Ok(h) => Ok(Self { hwnd: Hwnd::from_raw(h) }),
                // Ownership of `param` passed to the window at WM_NCCREATE and WM_NCDESTROY frees
                // it. If creation failed before WM_NCCREATE we can't tell, so the (tiny) box is
                // leaked rather than risking a double free; this only happens at startup.
                Err(e) => Err(e).context("CreateWindowExW"),
            }
        }
    }

    pub fn hwnd(&self) -> Hwnd {
        self.hwnd
    }

    /// Start (or reset) a `WM_TIMER` with the given id.
    pub fn set_timer(&self, id: usize, interval_ms: u32) {
        unsafe {
            SetTimer(Some(self.hwnd.raw()), id, interval_ms, None);
        }
    }

    pub fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd.raw()), id);
        }
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd.raw());
        }
    }
}

pub(crate) unsafe extern "system" fn trampoline(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndProc;
        if msg == WM_NCDESTROY && !ptr.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if !ptr.is_null() {
            let f = &*ptr;
            // Never let a panic unwind across the FFI boundary.
            let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                f(Hwnd::from_raw(hwnd), msg, wparam.0, lparam.0)
            }));
            match handled {
                Ok(Some(r)) => return LRESULT(r),
                Ok(None) => {}
                Err(_) => tracing::error!("window procedure panicked on message {msg:#x}"),
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Run the thread's message loop until `WM_QUIT`. Returns the quit code.
pub fn run_message_loop() -> i32 {
    let mut msg = MSG::default();
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    msg.wParam.0 as i32
}

/// Dispatch this thread's messages for duration (for threads without a message loop,
/// e.g. tests). Returns early if WM_QUIT arrives.
pub fn pump_for(duration: std::time::Duration) {
    use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, WM_QUIT};
    let deadline = std::time::Instant::now() + duration;
    let mut msg = MSG::default();
    loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        unsafe {
            MsgWaitForMultipleObjects(None, false, left.as_millis().min(100) as u32, QS_ALLINPUT);
        }
    }
}

/// Ask the current thread's message loop to exit.
pub fn post_quit(code: i32) {
    unsafe { PostQuitMessage(code) }
}

pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_SETTINGCHANGE: u32 = 0x001A;
pub const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;

/// For `WM_SETTINGCHANGE` only: the `lparam` names the changed area (e.g.
/// `"ImmersiveColorSet"` when light/dark mode or the accent colour changes), or is null.
pub fn setting_change_area(lparam: isize) -> Option<String> {
    if lparam == 0 {
        return None;
    }
    unsafe { PCWSTR(lparam as *const u16).to_string().ok() }
}

/// Every top-level window of a class, found the way `FindWindow` finds them. Unlike
/// `EnumWindows`, this also sees shell windows kept in special z-order bands (on recent
/// Windows 11 builds Explorer's taskbar can be one of them).
pub fn find_all_by_class(class: &str) -> Vec<Hwnd> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let class_w = wide(class);
    let mut out = Vec::new();
    let mut after: Option<HWND> = None;
    for _ in 0..64 {
        let found = unsafe { FindWindowExW(None, after, PCWSTR(class_w.as_ptr()), PCWSTR::null()) };
        match found {
            Ok(h) if !h.is_invalid() => {
                out.push(Hwnd::from_raw(h));
                after = Some(h);
            }
            _ => break,
        }
    }
    out
}

/// Register (or look up) a system-wide message id by name, e.g. `"TaskbarCreated"`.
pub fn register_window_message(name: &str) -> u32 {
    let w = wide(name);
    unsafe { RegisterWindowMessageW(PCWSTR(w.as_ptr())) }
}
