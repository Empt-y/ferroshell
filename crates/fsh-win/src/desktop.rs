//! The desktop window when Ferroshell replaces Explorer: a `Progman` ("Program Manager")
//! window at the bottom of the z-order showing the wallpaper, registered as the shell window
//! so programs that look for Explorer's desktop find it, and so `explorer.exe` opens as a
//! plain file manager instead of trying to become the shell.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DefSubclassProc, FOLDERID_Desktop, FOLDERID_PublicDesktop, SHCNE_ALLEVENTS, SetWindowSubclass, SHCNRF_InterruptLevel, SHCNRF_ShellLevel,
    SHChangeNotifyDeregister, SHChangeNotifyEntry, SHChangeNotifyRegister, SHGetKnownFolderIDList, SHParseDisplayName,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, PaintDesktop};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CWP_SKIPINVISIBLE, ChildWindowFromPointEx, CreateWindowExW, DestroyWindow, GWL_EXSTYLE,
    GWLP_HWNDPARENT, GetWindowLongPtrW, SWP_NOZORDER, SetForegroundWindow,
    SetWindowLongPtrW, WM_PARENTNOTIFY, WM_SETFOCUS, WS_EX_APPWINDOW, FindWindowW, GetCursorPos, GetShellWindow, GetSystemMetrics, HWND_BOTTOM,
    IDC_ARROW, LoadCursorW, RegisterClassExW, SC_TASKLIST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SetWindowPos, WINDOWPOS, WM_CLOSE, WM_CONTEXTMENU, WM_DISPLAYCHANGE,
    WM_ERASEBKGND, WM_PAINT, WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING, WNDCLASSEXW, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::window::{WM_APP, WndProc, trampoline};
use crate::{Hwnd, Rect, wide};

/// Posted by the shell when something on the desktop changes (see `SHChangeNotifyRegister`).
const WM_SHELL_CHANGE: u32 = WM_APP + 0x50;

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
    /// A file appeared, went, was renamed or changed its icon on the desktop (or the
    /// Recycle Bin filled or emptied). Several arrive at once; debounce.
    ItemsChanged,
}

/// Stops Windows parking minimised windows as little title bars at the bottom-left of the
/// screen (what it does when no shell hides them; Explorer does this at start-up too).
/// Lasts until sign-out.
pub fn hide_minimized_windows() {
    use windows::Win32::UI::WindowsAndMessaging::{
        ARW_HIDE, MINIMIZEDMETRICS, MINIMIZEDMETRICS_ARRANGE, SPI_GETMINIMIZEDMETRICS, SPI_SETMINIMIZEDMETRICS,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut mm = MINIMIZEDMETRICS { cbSize: size_of::<MINIMIZEDMETRICS>() as u32, ..Default::default() };
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETMINIMIZEDMETRICS,
            mm.cbSize,
            Some(&mut mm as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        mm.iArrange = MINIMIZEDMETRICS_ARRANGE(mm.iArrange.0 | ARW_HIDE);
        let _ = SystemParametersInfoW(
            SPI_SETMINIMIZEDMETRICS,
            mm.cbSize,
            Some(&mut mm as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
}

/// Whether a desktop already exists (Explorer's, or another shell's).
pub fn desktop_exists() -> bool {
    unsafe { !GetShellWindow().is_invalid() || FindWindowW(w!("Progman"), PCWSTR::null()).is_ok_and(|h| !h.is_invalid()) }
}

pub struct DesktopWindow {
    hwnd: Hwnd,
    shell_window: bool,
    taskman_window: bool,
    /// Windows shown inside the desktop (the icon views), which get keyboard focus.
    children: Rc<RefCell<Vec<Hwnd>>>,
    /// Change-notification registration and the ID lists it watches.
    notify_id: u32,
    watched: Vec<*mut ITEMIDLIST>,
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
        let children: Rc<RefCell<Vec<Hwnd>>> = Rc::default();
        let kids = children.clone();
        let proc_: WndProc = Box::new(move |hwnd, msg, wparam, lparam| match msg {
            // A click in a child view: give it the keyboard (Windows activates us, the
            // top-level window, but leaves focus here).
            WM_PARENTNOTIFY if matches!(wparam & 0xffff, 0x0201 | 0x0204 | 0x0207) => {
                let p = POINT { x: (lparam & 0xffff) as i16 as i32, y: ((lparam >> 16) & 0xffff) as i16 as i32 };
                unsafe {
                    let child = ChildWindowFromPointEx(hwnd.raw(), p, CWP_SKIPINVISIBLE);
                    if !child.is_invalid() && child != hwnd.raw() {
                        let _ = SetFocus(Some(child));
                    }
                }
                None
            }
            WM_SETFOCUS => {
                if let Some(first) = kids.borrow().first() {
                    unsafe {
                        let _ = SetFocus(Some(first.raw()));
                    }
                }
                Some(0)
            }
            WM_SHELL_CHANGE => {
                on_event(DesktopEvent::ItemsChanged);
                Some(0)
            }
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
            // Forwarded up from a child view, which handles right-clicks itself.
            WM_CONTEXTMENU if wparam != hwnd.0 as usize => Some(0),
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
        let (notify_id, watched) = watch_desktop(hwnd);
        Ok(Self { hwnd, shell_window, taskman_window, children, notify_id, watched })
    }

    /// Makes another window (an icon view) part of the desktop, covering `screen`: a tool
    /// window (never in the taskbar or Alt+Tab) owned by the desktop, so it stays just above
    /// it at the bottom of the z-order however it's activated. It stays a top-level window,
    /// so it keeps its own monitor's scaling.
    ///
    /// Give it the work area, not the whole monitor: a window exactly covering a monitor is
    /// treated as a full-screen game (presented directly, over the panels).
    pub fn adopt(&self, view: Hwnd, screen: Rect) {
        // The window style stays as its toolkit made it (a frameless window that hides its
        // own caption); only the extended style changes.
        unsafe {
            let ex = (GetWindowLongPtrW(view.raw(), GWL_EXSTYLE) as u32 & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0;
            SetWindowLongPtrW(view.raw(), GWL_EXSTYLE, ex as isize);
            SetWindowLongPtrW(view.raw(), GWLP_HWNDPARENT, self.hwnd.0);
            let _ = SetWindowSubclass(view.raw(), Some(stay_at_bottom), 0x4653, 0);
            let _ = SetWindowPos(
                view.raw(),
                Some(HWND_BOTTOM),
                screen.left,
                screen.top,
                screen.width(),
                screen.height(),
                SWP_NOACTIVATE,
            );
        }
        let mut kids = self.children.borrow_mut();
        if !kids.contains(&view) {
            kids.push(view);
        }
    }

    /// A view went away (e.g. its monitor was unplugged).
    pub fn release(&self, view: Hwnd) {
        self.children.borrow_mut().retain(|h| *h != view);
    }

    /// Gives the keyboard to a view.
    pub fn focus(&self, view: Hwnd) {
        unsafe {
            let _ = SetForegroundWindow(view.raw());
            let _ = SetFocus(Some(view.raw()));
        }
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

/// Keeps a desktop view underneath every other window, even when it's clicked.
unsafe extern "system" fn stay_at_bottom(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if msg == WM_WINDOWPOSCHANGING && lparam.0 != 0 {
            let pos = &mut *(lparam.0 as *mut WINDOWPOS);
            if pos.flags.0 & SWP_NOZORDER.0 == 0 {
                pos.hwndInsertAfter = HWND_BOTTOM;
            }
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

/// Asks the shell to tell us about changes in both Desktop folders and the Recycle Bin.
fn watch_desktop(hwnd: Hwnd) -> (u32, Vec<*mut ITEMIDLIST>) {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    unsafe {
        for id in [&FOLDERID_Desktop, &FOLDERID_PublicDesktop] {
            if let Ok(p) = SHGetKnownFolderIDList(id, 0, None) {
                pidls.push(p);
            }
        }
        let bin = wide("::{645FF040-5081-101B-9F08-00AA002F954E}");
        let mut p = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(bin.as_ptr()), None, &mut p, 0, None).is_ok() && !p.is_null() {
            pidls.push(p);
        }
        let entries: Vec<SHChangeNotifyEntry> =
            pidls.iter().map(|p| SHChangeNotifyEntry { pidl: *p, fRecursive: false.into() }).collect();
        let id = SHChangeNotifyRegister(
            hwnd.raw(),
            SHCNRF_ShellLevel | SHCNRF_InterruptLevel,
            SHCNE_ALLEVENTS.0 as i32,
            WM_SHELL_CHANGE,
            entries.len() as i32,
            entries.as_ptr(),
        );
        (id, pidls)
    }
}

impl Drop for DesktopWindow {
    fn drop(&mut self) {
        unsafe {
            if self.notify_id != 0 {
                let _ = SHChangeNotifyDeregister(self.notify_id);
            }
            for p in self.watched.drain(..) {
                CoTaskMemFree(Some(p as *const _));
            }
        }
        // Destroying the shell window unregisters it.
        unsafe {
            let _ = DestroyWindow(self.hwnd.raw());
        }
    }
}
