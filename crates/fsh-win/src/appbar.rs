//! Screen-space reservation through the AppBar API, so maximized windows stop at the panel.
//!
//! The reservation is registered on a hidden window of our own rather than on the (Slint-
//! owned) panel window, so we never have to hook another library's window procedure. The
//! hidden window is kept at the panel's position so Windows attributes it to the right
//! monitor (full-screen notifications are per monitor).
//!
//! App bars are served by Explorer's taskbar. When there is none (Ferroshell is the login
//! shell), registration fails and the panel reserves its edge itself by setting the
//! monitor's work area.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS,
    ABN_FULLSCREENAPP, ABN_POSCHANGED, ABN_STATECHANGE, APPBARDATA, SHAppBarMessage,
};

use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, PostMessageW, SPI_SETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    WM_SETTINGCHANGE,
};

use crate::Hwnd;
use crate::monitor::Rect;
use crate::window::{MessageWindow, WM_APP};

const WM_APPBAR_CALLBACK: u32 = WM_APP + 0x40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    fn raw(self) -> u32 {
        match self {
            Edge::Top => ABE_TOP,
            Edge::Bottom => ABE_BOTTOM,
            Edge::Left => ABE_LEFT,
            Edge::Right => ABE_RIGHT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppBarEvent {
    /// A full-screen app opened (`true`) or closed (`false`) on this app bar's monitor.
    FullScreen(bool),
    /// Another app bar moved or the work area changed; call [`AppBar::set_pos`] again.
    PositionChanged,
}

pub struct AppBar {
    window: MessageWindow,
    registered: Rc<Cell<bool>>,
    /// No app-bar server answered: we set the work area ourselves.
    self_managed: bool,
}

thread_local! {
    /// Self-managed reservations (UI thread): (app bar window, monitor, edge, thickness).
    static RESERVED: RefCell<Vec<(isize, Rect, Edge, i32)>> = const { RefCell::new(Vec::new()) };
}

/// The monitor minus every self-managed reservation on it.
fn work_area(monitor: Rect, reserved: &[(isize, Rect, Edge, i32)]) -> Rect {
    let mut r = monitor;
    for (_, _, edge, t) in reserved.iter().filter(|(_, m, _, _)| *m == monitor) {
        match edge {
            Edge::Top => r.top = r.top.max(monitor.top + t),
            Edge::Bottom => r.bottom = r.bottom.min(monitor.bottom - t),
            Edge::Left => r.left = r.left.max(monitor.left + t),
            Edge::Right => r.right = r.right.min(monitor.right - t),
        }
    }
    r
}

fn apply_work_area(monitor: Rect) {
    let area = RESERVED.with(|r| work_area(monitor, &r.borrow()));
    let mut raw = area.to_raw();
    unsafe {
        // Don't send the change synchronously: a hung window would hang us. Windows moves
        // maximised windows itself; others hear about it from the posted broadcast.
        let _ = SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut raw as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let _ = PostMessageW(Some(HWND_BROADCAST), WM_SETTINGCHANGE, WPARAM(SPI_SETWORKAREA.0 as usize), LPARAM(0));
    }
}

impl AppBar {
    pub fn new(on_event: impl Fn(AppBarEvent) + 'static) -> anyhow::Result<Self> {
        let window = MessageWindow::new(
            "Ferroshell AppBar",
            Box::new(move |_, msg, wparam, lparam| {
                if msg != WM_APPBAR_CALLBACK {
                    return None;
                }
                match wparam as u32 {
                    ABN_FULLSCREENAPP => on_event(AppBarEvent::FullScreen(lparam != 0)),
                    ABN_POSCHANGED | ABN_STATECHANGE => on_event(AppBarEvent::PositionChanged),
                    _ => {}
                }
                Some(0)
            }),
        )?;
        let mut abd = data(window.hwnd());
        abd.uCallbackMessage = WM_APPBAR_CALLBACK;
        let ok = unsafe { SHAppBarMessage(ABM_NEW, &mut abd) } != 0;
        if !ok {
            tracing::info!("no app-bar server (Explorer's taskbar isn't running); reserving panel space ourselves");
        }
        Ok(Self { window, registered: Rc::new(Cell::new(ok)), self_managed: !ok })
    }

    /// Reserve `thickness` physical pixels along `edge` of `monitor`. Returns the rectangle
    /// Windows granted, which can be smaller if other app bars already use that edge.
    pub fn set_pos(&self, edge: Edge, monitor: Rect, thickness: i32) -> Rect {
        if self.self_managed {
            let mut r = monitor;
            match edge {
                Edge::Top => r.bottom = r.top + thickness,
                Edge::Bottom => r.top = r.bottom - thickness,
                Edge::Left => r.right = r.left + thickness,
                Edge::Right => r.left = r.right - thickness,
            }
            let me = self.window.hwnd().0;
            let old = RESERVED.with(|res| {
                let mut res = res.borrow_mut();
                let old = res.iter().position(|e| e.0 == me).map(|i| res.remove(i).1);
                res.push((me, monitor, edge, thickness));
                old
            });
            if let Some(m) = old.filter(|m| *m != monitor) {
                apply_work_area(m);
            }
            apply_work_area(monitor);
            crate::panel::set_rect(self.window.hwnd(), r);
            return r;
        }
        let mut abd = data(self.window.hwnd());
        abd.uEdge = edge.raw();
        abd.rc = monitor.to_raw();
        unsafe {
            SHAppBarMessage(ABM_QUERYPOS, &mut abd);
        }
        // QUERYPOS shrinks the rect to avoid other app bars; re-apply our thickness from
        // the adjusted edge.
        let mut r = abd.rc;
        match edge {
            Edge::Top => r.bottom = r.top + thickness,
            Edge::Bottom => r.top = r.bottom - thickness,
            Edge::Left => r.right = r.left + thickness,
            Edge::Right => r.left = r.right - thickness,
        }
        abd.rc = r;
        unsafe {
            SHAppBarMessage(ABM_SETPOS, &mut abd);
        }
        let granted = Rect::from_raw(abd.rc);
        crate::panel::set_rect(self.window.hwnd(), granted);
        granted
    }

    pub fn hwnd(&self) -> Hwnd {
        self.window.hwnd()
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        if self.self_managed {
            let me = self.window.hwnd().0;
            let freed = RESERVED.with(|res| {
                let mut res = res.borrow_mut();
                res.iter().position(|e| e.0 == me).map(|i| res.remove(i).1)
            });
            if let Some(m) = freed {
                apply_work_area(m);
            }
        }
        if self.registered.replace(false) {
            let mut abd = data(self.window.hwnd());
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut abd);
            }
        }
    }
}

fn data(hwnd: Hwnd) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd.raw(),
        lParam: LPARAM(0),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_area_subtracts_reservations_on_that_monitor() {
        let m = Rect::new(0, 0, 1920, 1080);
        let other = Rect::new(1920, 0, 3840, 1080);
        let res = vec![(1, m, Edge::Bottom, 48), (2, m, Edge::Top, 30), (3, other, Edge::Left, 60)];
        assert_eq!(work_area(m, &res), Rect::new(0, 30, 1920, 1032));
        assert_eq!(work_area(other, &res), Rect::new(1980, 0, 3840, 1080));
        assert_eq!(work_area(m, &[]), m);
    }
}
