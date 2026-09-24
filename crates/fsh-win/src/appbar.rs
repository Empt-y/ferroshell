//! Screen-space reservation through the AppBar API, so maximized windows stop at the panel.
//!
//! The reservation is registered on a hidden window of our own rather than on the (Slint-
//! owned) panel window, so we never have to hook another library's window procedure. The
//! hidden window is kept at the panel's position so Windows attributes it to the right
//! monitor (full-screen notifications are per monitor).

use std::cell::Cell;
use std::rc::Rc;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS,
    ABN_FULLSCREENAPP, ABN_POSCHANGED, ABN_STATECHANGE, APPBARDATA, SHAppBarMessage,
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
            anyhow::bail!("registering app bar: ABM_NEW failed");
        }
        Ok(Self { window, registered: Rc::new(Cell::new(true)) })
    }

    /// Reserve `thickness` physical pixels along `edge` of `monitor`. Returns the rectangle
    /// Windows granted, which can be smaller if other app bars already use that edge.
    pub fn set_pos(&self, edge: Edge, monitor: Rect, thickness: i32) -> Rect {
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
