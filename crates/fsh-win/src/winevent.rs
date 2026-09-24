//! System-wide window events (created, destroyed, shown, renamed, focused, minimized,
//! cloaked) via out-of-context WinEvent hooks. Callbacks arrive on the installing thread,
//! which must run a message loop.

use std::cell::RefCell;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS};

use crate::Hwnd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinEvent {
    Created,
    Destroyed,
    Shown,
    Hidden,
    TitleChanged,
    Foreground,
    MinimizeStart,
    MinimizeEnd,
    Cloaked,
    Uncloaked,
}

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;
const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;

type Sink = Box<dyn Fn(WinEvent, Hwnd)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Hooks stay installed until this is dropped (on the same thread).
pub struct WinEventHooks {
    hooks: Vec<HWINEVENTHOOK>,
}

impl WinEventHooks {
    /// Install hooks delivering events to `sink` on this thread. Only one set per thread.
    pub fn install(sink: impl Fn(WinEvent, Hwnd) + 'static) -> Self {
        SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        let hooks = ranges
            .iter()
            .filter_map(|&(min, max)| {
                let h = unsafe {
                    SetWinEventHook(min, max, None, Some(callback), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
                };
                if h.is_invalid() {
                    tracing::warn!("SetWinEventHook({min:#x}..{max:#x}) failed");
                    None
                } else {
                    Some(h)
                }
            })
            .collect();
        Self { hooks }
    }
}

impl Drop for WinEventHooks {
    fn drop(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        SINK.with(|s| s.borrow_mut().take());
    }
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.is_invalid() || id_object != OBJID_WINDOW || id_child != CHILDID_SELF {
        return;
    }
    let ev = match event {
        EVENT_OBJECT_CREATE => WinEvent::Created,
        EVENT_OBJECT_DESTROY => WinEvent::Destroyed,
        EVENT_OBJECT_SHOW => WinEvent::Shown,
        EVENT_OBJECT_HIDE => WinEvent::Hidden,
        EVENT_OBJECT_NAMECHANGE => WinEvent::TitleChanged,
        EVENT_SYSTEM_FOREGROUND => WinEvent::Foreground,
        EVENT_SYSTEM_MINIMIZESTART => WinEvent::MinimizeStart,
        EVENT_SYSTEM_MINIMIZEEND => WinEvent::MinimizeEnd,
        EVENT_OBJECT_CLOAKED => WinEvent::Cloaked,
        EVENT_OBJECT_UNCLOAKED => WinEvent::Uncloaked,
        _ => return,
    };
    let _ = std::panic::catch_unwind(|| {
        SINK.with(|s| {
            if let Ok(s) = s.try_borrow()
                && let Some(f) = s.as_ref()
            {
                f(ev, Hwnd::from_raw(hwnd));
            }
        });
    });
}
