//! Hosting notification-area ("system tray") icons.
//!
//! `Shell_NotifyIcon` finds the tray with `FindWindow("Shell_TrayWnd")` and sends it a
//! `WM_COPYDATA`. We create a hidden window of that class and keep it first in z-order,
//! so those messages reach us, and forward every message on to Explorer's own tray. That
//! keeps Explorer in sync (its notifications keep working, and nothing is lost when
//! Ferroshell stops) and passes app-bar messages, which travel the same way, untouched.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetWindowTextW, HICON, HWND_BROADCAST, HWND_TOPMOST, PostMessageW,
    SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW, SetWindowPos, WM_CONTEXTMENU,
    WM_COPYDATA, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use crate::icon::{RgbaImage, hicon_to_rgba};
use crate::window::MessageWindow;
use crate::{Hwnd, winfo};

/// Window title of our tray window, so other code can tell it apart from Explorer's.
pub const OUR_TRAY_TITLE: &str = "Ferroshell Tray";
const TRAY_CLASS: &str = "Shell_TrayWnd";
const SIGNATURE: u32 = 0x3475_3423;
const COPYDATA_NOTIFYICON: usize = 1;
const FORWARD_TIMEOUT_MS: u32 = 2000;

pub const NIM_ADD: u32 = 0;
pub const NIM_MODIFY: u32 = 1;
pub const NIM_DELETE: u32 = 2;
pub const NIM_SETFOCUS: u32 = 3;
pub const NIM_SETVERSION: u32 = 4;

pub const NIF_MESSAGE: u32 = 0x01;
pub const NIF_ICON: u32 = 0x02;
pub const NIF_TIP: u32 = 0x04;
pub const NIF_STATE: u32 = 0x08;
pub const NIF_GUID: u32 = 0x20;
pub const NIS_HIDDEN: u32 = 0x01;

const NIN_SELECT: u32 = 0x0400;

/// One `Shell_NotifyIcon` call, decoded from the 32-bit wire layout
/// (`TRAYNOTIFYDATAW`: signature, message, then `NOTIFYICONDATAW` with 32-bit handles).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyCommand {
    pub message: u32,
    pub owner: isize,
    pub uid: u32,
    pub flags: u32,
    pub callback: u32,
    pub icon: isize,
    pub tip: String,
    pub state: u32,
    pub state_mask: u32,
    /// `uVersion` for NIM_SETVERSION (shares storage with the balloon timeout).
    pub version: u32,
    pub guid: Option<[u8; 16]>,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn wstr_at(b: &[u8], off: usize, chars: usize) -> String {
    let end = (off + chars * 2).min(b.len());
    let Some(bytes) = b.get(off..end) else { return String::new() };
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Decode the payload of a tray `WM_COPYDATA`. Returns `None` for malformed data.
pub fn parse_notify(data: &[u8]) -> Option<NotifyCommand> {
    if u32_at(data, 0)? != SIGNATURE {
        return None;
    }
    let n = 8; // NOTIFYICONDATA starts after signature + message
    let flags = u32_at(data, n + 12)?;
    let guid = (flags & NIF_GUID != 0).then(|| data.get(n + 936..n + 952)).flatten().map(|g| {
        let mut out = [0u8; 16];
        out.copy_from_slice(g);
        out
    });
    Some(NotifyCommand {
        message: u32_at(data, 4)?,
        owner: u32_at(data, n + 4)? as i32 as isize,
        uid: u32_at(data, n + 8)?,
        flags,
        callback: u32_at(data, n + 16).unwrap_or(0),
        icon: u32_at(data, n + 20).unwrap_or(0) as i32 as isize,
        tip: if flags & NIF_TIP != 0 { wstr_at(data, n + 24, 128) } else { String::new() },
        state: u32_at(data, n + 280).unwrap_or(0),
        state_mask: u32_at(data, n + 284).unwrap_or(0),
        version: u32_at(data, n + 800).unwrap_or(0),
        guid,
    })
}

/// Hidden `Shell_TrayWnd` that receives icon registrations. Lives on its creating thread,
/// which must run a message loop.
pub struct TrayHost {
    window: MessageWindow,
}

impl TrayHost {
    /// `on_notify` gets each decoded command, plus the icon as pixels when the command
    /// carries one (converted immediately: the app may destroy the handle right after).
    pub fn new(on_notify: impl Fn(NotifyCommand, Option<RgbaImage>) + 'static) -> anyhow::Result<Self> {
        let window = MessageWindow::with_class(
            TRAY_CLASS,
            OUR_TRAY_TITLE,
            Box::new(move |hwnd, msg, wparam, lparam| {
                if msg != WM_COPYDATA || lparam == 0 {
                    return None;
                }
                // SAFETY: for WM_COPYDATA the system guarantees lparam points to a valid
                // COPYDATASTRUCT (and its buffer) for the duration of the call.
                let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
                if cds.dwData == COPYDATA_NOTIFYICON && !cds.lpData.is_null() {
                    let data = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
                    if let Some(cmd) = parse_notify(data) {
                        let icon = (cmd.flags & NIF_ICON != 0 && cmd.icon != 0)
                            .then(|| hicon_to_rgba(HICON(cmd.icon as *mut _)))
                            .flatten();
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_notify(cmd, icon)));
                    }
                    forward_to_explorer(hwnd, wparam, lparam);
                    return Some(1);
                }
                // App-bar and other shell messages: Explorer answers them.
                Some(forward_to_explorer(hwnd, wparam, lparam).unwrap_or(0))
            }),
        )?;
        let host = Self { window };
        host.raise();
        Ok(host)
    }

    pub fn hwnd(&self) -> Hwnd {
        self.window.hwnd()
    }

    /// Put our tray window first among `Shell_TrayWnd`s, so `FindWindow` returns it.
    /// Call periodically: Explorer can raise its own again (e.g. after restarting).
    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd().raw(), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Is our window the one `Shell_NotifyIcon` will find?
    pub fn is_first(&self) -> bool {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::FindWindowW(windows::core::w!("Shell_TrayWnd"), None)
                .is_ok_and(|h| Hwnd::from_raw(h) == self.hwnd())
        }
    }
}

fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Explorer's tray window (any `Shell_TrayWnd` that isn't one of ours).
pub fn explorer_tray() -> Option<Hwnd> {
    crate::window::find_all_by_class(TRAY_CLASS).into_iter().find(|h| title(h.raw()) != OUR_TRAY_TITLE)
}

pub fn is_our_tray(hwnd: Hwnd) -> bool {
    title(hwnd.raw()) == OUR_TRAY_TITLE
}

fn forward_to_explorer(ours: Hwnd, wparam: usize, lparam: isize) -> Option<isize> {
    let target = explorer_tray().filter(|h| *h != ours)?;
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            target.raw(),
            WM_COPYDATA,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG,
            FORWARD_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (ok.0 != 0).then_some(result as isize)
}

/// Ask every app to re-register its tray icons (what Explorer does after it restarts).
pub fn broadcast_taskbar_created() {
    let msg = crate::window::register_window_message("TaskbarCreated");
    if msg != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Everything needed to talk back to an icon's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconTarget {
    pub owner: isize,
    pub uid: u32,
    pub callback: u32,
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    Left,
    Right,
    Middle,
    Double,
    Hover,
}

/// Deliver a click to the icon's owner the way Explorer does for its protocol version.
/// `x`, `y` is the icon's anchor point in screen pixels (used by version-4 apps to place
/// menus and flyouts).
pub fn send_mouse(target: IconTarget, mouse: Mouse, x: i32, y: i32) {
    if target.callback == 0 {
        return;
    }
    let owner = Hwnd(target.owner);
    // The owner will want to show a menu or window; let it take the foreground.
    let pid = winfo::pid(owner);
    if pid != 0 {
        unsafe {
            let _ = AllowSetForegroundWindow(pid);
        }
    }
    let messages: &[u32] = match (mouse, target.version >= 3) {
        (Mouse::Left, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
        (Mouse::Left, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        (Mouse::Right, true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        (Mouse::Right, false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
        (Mouse::Middle, _) => &[WM_MBUTTONDOWN, WM_MBUTTONUP],
        (Mouse::Double, _) => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
        (Mouse::Hover, _) => &[WM_MOUSEMOVE],
    };
    for &m in messages {
        let (wparam, lparam) = if target.version >= 4 {
            let wp = (x as u16 as usize) | ((y as u16 as usize) << 16);
            let lp = (m as u16 as isize) | ((target.uid as u16 as isize) << 16);
            (wp, lp)
        } else {
            (target.uid as usize, m as isize)
        };
        owner.post(target.callback, wparam, lparam);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers a real icon through `Shell_NotifyIconW` and checks our window receives it.
    /// Only meaningful when Explorer's taskbar isn't running: while it is, Explorer's tray
    /// window is the one found, and this test can pass or fail depending on timing.
    /// Touches the live system tray (briefly adds and removes an icon), so it's opt-in:
    /// `cargo test -p fsh-win -- --ignored`.
    #[test]
    #[ignore = "interacts with the live system tray"]
    fn intercepts_shell_notify_icon() {
        use std::sync::mpsc;
        use std::time::Duration;
        use windows::Win32::UI::Shell::{NIF_ICON as F_ICON, NIF_MESSAGE as F_MSG, NIF_TIP as F_TIP, NIM_ADD as M_ADD, NIM_DELETE as M_DEL, NOTIFYICONDATAW, Shell_NotifyIconW};
        use windows::Win32::UI::WindowsAndMessaging::{IDI_APPLICATION, LoadIconW};

        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let tray = std::thread::spawn(move || {
            let host = TrayHost::new(move |cmd, icon| {
                let _ = tx.send((cmd, icon.is_some()));
            })
            .unwrap();
            ready_tx.send(host.is_first()).unwrap();
            crate::window::pump_for(Duration::from_secs(4));
        });
        assert!(ready_rx.recv().unwrap(), "our tray window should be the one FindWindow returns");

        let owner = MessageWindow::new("tray test owner", Box::new(|_, _, _, _| None)).unwrap();
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: owner.hwnd().raw(),
            uID: 42,
            uFlags: F_MSG | F_ICON | F_TIP,
            uCallbackMessage: crate::window::WM_APP + 5,
            hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap(),
            ..Default::default()
        };
        for (i, u) in "Ferroshell test".encode_utf16().enumerate() {
            nid.szTip[i] = u;
        }
        assert!(unsafe { Shell_NotifyIconW(M_ADD, &nid) }.as_bool());
        let (cmd, has_icon) = rx.recv_timeout(Duration::from_secs(2)).expect("tray host got nothing");
        assert_eq!((cmd.message, cmd.uid, cmd.owner), (NIM_ADD, 42, owner.hwnd().0));
        assert_eq!(cmd.tip, "Ferroshell test");
        assert!(has_icon, "icon pixels were converted");

        assert!(unsafe { Shell_NotifyIconW(M_DEL, &nid) }.as_bool());
        let (cmd, _) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(cmd.message, NIM_DELETE);
        tray.join().unwrap();
    }

    fn put_u32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn sample(flags: u32, tip: &str) -> Vec<u8> {
        let mut b = vec![0u8; 8 + 956];
        put_u32(&mut b, 0, SIGNATURE);
        put_u32(&mut b, 4, NIM_MODIFY);
        put_u32(&mut b, 8, 956);
        put_u32(&mut b, 12, 0x0012_3456); // hWnd
        put_u32(&mut b, 16, 7); // uID
        put_u32(&mut b, 20, flags);
        put_u32(&mut b, 24, 0x8001); // callback message
        put_u32(&mut b, 28, 0xABCD); // hIcon
        for (i, u) in tip.encode_utf16().enumerate() {
            b[32 + i * 2..34 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
        put_u32(&mut b, 8 + 280, NIS_HIDDEN);
        put_u32(&mut b, 8 + 284, NIS_HIDDEN);
        put_u32(&mut b, 8 + 800, 4);
        b[8 + 936..8 + 952].copy_from_slice(&[9; 16]);
        b
    }

    #[test]
    fn parses_notify_data() {
        let c = parse_notify(&sample(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_STATE | NIF_GUID, "Volume: 50%")).unwrap();
        assert_eq!((c.message, c.owner, c.uid, c.callback, c.icon), (NIM_MODIFY, 0x0012_3456, 7, 0x8001, 0xABCD));
        assert_eq!(c.tip, "Volume: 50%");
        assert_eq!((c.state, c.state_mask, c.version), (NIS_HIDDEN, NIS_HIDDEN, 4));
        assert_eq!(c.guid, Some([9; 16]));
    }

    #[test]
    fn ignores_tip_and_guid_without_flags_and_rejects_garbage() {
        let c = parse_notify(&sample(NIF_ICON, "ignored")).unwrap();
        assert_eq!((c.tip.as_str(), c.guid), ("", None));
        assert!(parse_notify(&[0; 64]).is_none(), "bad signature");
        assert!(parse_notify(&sample(0, "")[..16]).is_none(), "truncated");
        // Old, shorter structures still parse (missing tail fields default to 0).
        let short = &sample(NIF_ICON, "")[..8 + 24];
        assert_eq!(parse_notify(short).unwrap().state, 0);
    }
}
