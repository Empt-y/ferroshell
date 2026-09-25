//! The desktop window when Ferroshell replaces Explorer: a `Progman` ("Program Manager")
//! window at the bottom of the z-order showing the wallpaper, registered as the shell window
//! so programs that look for Explorer's desktop find it, and so `explorer.exe` opens as a
//! plain file manager instead of trying to become the shell.

use anyhow::Context;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, PaintDesktop};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DestroyWindow, FindWindowW, GetCursorPos, GetShellWindow, GetSystemMetrics, HWND_BOTTOM,
    IDC_ARROW, LoadCursorW, RegisterClassExW, SC_TASKLIST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SetWindowPos, WINDOWPOS, WM_CLOSE, WM_CONTEXTMENU, WM_DISPLAYCHANGE,
    WM_ERASEBKGND, WM_PAINT, WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING, WNDCLASSEXW, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::Hwnd;
use crate::window::{WndProc, trampoline};

// Not bound by windows-rs; documented in winuser.h's history and what every alternative
// shell calls.
windows_core::link!("user32.dll" "system" fn SetShellWindow(hwnd: HWND) -> BOOL);
windows_core::link!("user32.dll" "system" fn SetTaskmanWindow(hwnd: HWND) -> BOOL);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEvent {
    /// Right-click (or the menu key) on the desktop, at screen coordinates.
    ContextMenu { x: i32, y: i32 },
    /// Ctrl+Esc, which Windows sends to the "task manager" window: open the launcher.
    TaskList,
}

/// Whether a desktop already exists (Explorer's, or another shell's).
pub fn desktop_exists() -> bool {
    unsafe { !GetShellWindow().is_invalid() || FindWindowW(w!("Progman"), PCWSTR::null()).is_ok_and(|h| !h.is_invalid()) }
}

pub struct DesktopWindow {
    hwnd: Hwnd,
    shell_window: bool,
    taskman_window: bool,
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn cover_virtual_screen(hwnd: Hwnd) {
    let (x, y, cx, cy) = virtual_screen();
    unsafe {
        let _ = SetWindowPos(hwnd.raw(), Some(HWND_BOTTOM), x, y, cx, cy, SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(hwnd.raw()), None, true);
    }
}

impl DesktopWindow {
    /// Creates the desktop on the calling (UI) thread, which must pump messages. Fails if
    /// another desktop exists; check [`desktop_exists`] first.
    pub fn create(on_event: Box<dyn Fn(DesktopEvent)>) -> anyhow::Result<Self> {
        anyhow::ensure!(!desktop_exists(), "a desktop window already exists (is Explorer running?)");
        let class_name = w!("Progman");
        let proc_: WndProc = Box::new(move |hwnd, msg, wparam, lparam| match msg {
            WM_ERASEBKGND => Some(1),
            WM_PAINT => {
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    let hdc = BeginPaint(hwnd.raw(), &mut ps);
                    let _ = PaintDesktop(hdc);
                    let _ = EndPaint(hwnd.raw(), &ps);
                }
                Some(0)
            }
            // Stay underneath everything, whoever tries to raise us.
            WM_WINDOWPOSCHANGING => {
                unsafe {
                    let pos = &mut *(lparam as *mut WINDOWPOS);
                    pos.hwndInsertAfter = HWND_BOTTOM;
                }
                None
            }
            WM_DISPLAYCHANGE => {
                cover_virtual_screen(hwnd);
                None
            }
            // Wallpaper, colour or work-area changes.
            WM_SETTINGCHANGE => {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd.raw()), None, true);
                }
                None
            }
            WM_CONTEXTMENU => {
                let (mut x, mut y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
                if lparam == -1 {
                    // From the keyboard: open at the pointer.
                    let mut p = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                    }
                    (x, y) = (p.x, p.y);
                }
                on_event(DesktopEvent::ContextMenu { x, y });
                Some(0)
            }
            WM_SYSCOMMAND if (wparam & 0xfff0) as u32 == SC_TASKLIST => {
                on_event(DesktopEvent::TaskList);
                Some(0)
            }
            // Alt+F4 on the desktop must never close it.
            WM_CLOSE => Some(0),
            _ => None,
        });

        let (x, y, cx, cy) = virtual_screen();
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(trampoline),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let err = windows::core::Error::from_thread();
                if err.code() != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                    return Err(err).context("RegisterClassExW(Progman)");
                }
            }
            let param = Box::into_raw(Box::new(proc_));
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name,
                w!("Program Manager"),
                WS_POPUP | WS_CLIPCHILDREN | WS_VISIBLE,
                x,
                y,
                cx,
                cy,
                None,
                None,
                Some(instance.into()),
                Some(param as *const _),
            )
            .context("CreateWindowExW(Progman)")?
        };
        let hwnd = Hwnd::from_raw(hwnd);
        cover_virtual_screen(hwnd);
        let shell_window = unsafe { SetShellWindow(hwnd.raw()) }.as_bool();
        let taskman_window = unsafe { SetTaskmanWindow(hwnd.raw()) }.as_bool();
        Ok(Self { hwnd, shell_window, taskman_window })
    }

    pub fn hwnd(&self) -> Hwnd {
        self.hwnd
    }

    /// Whether Windows accepted it as the shell window / task manager window.
    pub fn registered(&self) -> (bool, bool) {
        (self.shell_window, self.taskman_window)
    }

    /// Repaint, e.g. after the wallpaper changed.
    pub fn refresh(&self) {
        cover_virtual_screen(self.hwnd);
    }
}

impl Drop for DesktopWindow {
    fn drop(&mut self) {
        // Destroying the shell window unregisters it.
        unsafe {
            let _ = DestroyWindow(self.hwnd.raw());
        }
    }
}
