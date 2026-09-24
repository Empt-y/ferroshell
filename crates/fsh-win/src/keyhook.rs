//! Taking over a lone Windows-key press, so it opens Ferroshell's launcher instead of the
//! Windows Start menu, while every Win+<key> shortcut keeps working; and, when Ferroshell
//! replaces Explorer, the volume and media keys (nothing else would handle them).
//!
//! A low-level keyboard hook runs on its own thread with its own message loop. Its
//! callback does nothing but update a tiny state machine and, on a lone Win tap, replace
//! the key release with "dummy key, Win up" (so Windows sees a chord and doesn't open
//! Start) and notify the caller. It never blocks: Windows silently removes hooks that are
//! slow, and a stuck hook would stall everyone's input.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::window::{self, MessageWindow, WM_APP};
use crate::Hwnd;

const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// An unassigned virtual key: pressing it does nothing but makes Win+it a "chord".
const VK_DUMMY: u16 = 0xE8;

/// A volume or media key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    VolumeUp,
    VolumeDown,
    Mute,
    PlayPause,
    Next,
    Previous,
    Stop,
}

impl MediaKey {
    pub fn from_vk(vk: u32) -> Option<Self> {
        Some(match vk {
            0xAD => Self::Mute,
            0xAE => Self::VolumeDown,
            0xAF => Self::VolumeUp,
            0xB0 => Self::Next,
            0xB1 => Self::Previous,
            0xB2 => Self::Stop,
            0xB3 => Self::PlayPause,
            _ => return None,
        })
    }

    /// Held volume keys repeat; the others act once per press.
    pub fn repeats(self) -> bool {
        matches!(self, Self::VolumeUp | Self::VolumeDown)
    }
}

/// What the hook reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// A lone Windows-key tap.
    WinTap,
    Media(MediaKey),
}

static WIN_TAP: AtomicBool = AtomicBool::new(false);
static MEDIA_KEYS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAction {
    /// Let the key through unchanged.
    Pass,
    /// Swallow this Win release, replay it after a dummy key, and report a lone tap.
    ReplaceWinUp,
}

/// Decides when a Win press was a lone tap. Pure, so it's unit-tested.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinKeyState {
    win_down: bool,
    chord: bool,
}

impl WinKeyState {
    pub fn on_key(&mut self, vk: u32, down: bool, injected: bool) -> HookAction {
        if injected {
            // Our own replay, and other software's synthetic input, pass untouched. That
            // also keeps "open the Windows Start menu" (which taps Win by injection) working.
            return HookAction::Pass;
        }
        let is_win = vk == VK_LWIN || vk == VK_RWIN;
        match (is_win, down) {
            (true, true) => {
                if !self.win_down {
                    self.win_down = true;
                    self.chord = false;
                }
                HookAction::Pass
            }
            (true, false) => {
                let lone = self.win_down && !self.chord;
                self.win_down = false;
                self.chord = false;
                if lone { HookAction::ReplaceWinUp } else { HookAction::Pass }
            }
            (false, true) => {
                if self.win_down {
                    self.chord = true;
                }
                HookAction::Pass
            }
            (false, false) => HookAction::Pass,
        }
    }
}

type OnEvent = Box<dyn Fn(KeyEvent) + Send>;

/// Hook state, plus the media key currently held (to report auto-repeat only for volume).
static STATE: Mutex<(WinKeyState, Option<OnEvent>, Option<MediaKey>)> =
    Mutex::new((WinKeyState { win_down: false, chord: false }, None, None));

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if down || up {
            let injected = info.flags.0 & LLKHF_INJECTED.0 != 0;
            // try_lock: never wait inside a keyboard hook.
            if let Ok(mut guard) = STATE.try_lock() {
                let (state, on_event, held) = &mut *guard;
                let report = |e: KeyEvent| {
                    if let Some(f) = on_event.as_ref() {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(e)));
                    }
                };
                if let Some(mk) = MediaKey::from_vk(info.vkCode).filter(|_| !injected && MEDIA_KEYS.load(Ordering::Relaxed)) {
                    if down {
                        if *held != Some(mk) || mk.repeats() {
                            report(KeyEvent::Media(mk));
                        }
                        *held = Some(mk);
                    } else {
                        *held = None;
                    }
                    return LRESULT(1);
                }
                if WIN_TAP.load(Ordering::Relaxed) && state.on_key(info.vkCode, down, injected) == HookAction::ReplaceWinUp {
                    let inputs = [key(VK_DUMMY, false), key(VK_DUMMY, true), key(info.vkCode as u16, true)];
                    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
                    report(KeyEvent::WinTap);
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

const WM_STOP: u32 = WM_APP + 1;

/// The hook is installed while this lives. Only one per process.
pub struct KeyHook {
    control: Hwnd,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeyHook {
    /// Start the hook thread. `on_event` runs on that thread for every lone Win tap and
    /// media key (as enabled by [`KeyHook::set_features`]) and must return immediately
    /// (e.g. post to another thread).
    pub fn start(on_event: impl Fn(KeyEvent) + Send + 'static) -> anyhow::Result<Self> {
        if let Ok(mut g) = STATE.lock() {
            *g = (WinKeyState::default(), Some(Box::new(on_event)), None);
        }
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("keyhook".into()).spawn(move || {
            let result = (|| -> anyhow::Result<(HHOOK, MessageWindow)> {
                let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) }
                    .context("SetWindowsHookExW(WH_KEYBOARD_LL)")?;
                let control = MessageWindow::new(
                    "Ferroshell Key Hook",
                    Box::new(|_, msg, _, _| {
                        (msg == WM_STOP).then(|| {
                            window::post_quit(0);
                            0
                        })
                    }),
                )?;
                Ok((hook, control))
            })();
            match result {
                Ok((hook, control)) => {
                    let _ = tx.send(Ok(control.hwnd()));
                    window::run_message_loop();
                    unsafe {
                        let _ = UnhookWindowsHookEx(hook);
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })?;
        let control = rx.recv_timeout(Duration::from_secs(5)).context("key hook thread did not start")??;
        Ok(Self { control, thread: Some(thread) })
    }

    /// Which keys to take over: lone Win taps and/or volume and media keys. Keys not taken
    /// over pass through to Windows untouched.
    pub fn set_features(&self, win_tap: bool, media_keys: bool) {
        WIN_TAP.store(win_tap, Ordering::Relaxed);
        MEDIA_KEYS.store(media_keys, Ordering::Relaxed);
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        self.control.post(WM_STOP, 0, 0);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        WIN_TAP.store(false, Ordering::Relaxed);
        MEDIA_KEYS.store(false, Ordering::Relaxed);
        if let Ok(mut g) = STATE.lock() {
            g.1 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 0x41;
    const E: u32 = 0x45;

    #[test]
    fn lone_tap_is_replaced() {
        let mut s = WinKeyState::default();
        assert_eq!(s.on_key(VK_LWIN, true, false), HookAction::Pass);
        assert_eq!(s.on_key(VK_LWIN, true, false), HookAction::Pass, "auto-repeat");
        assert_eq!(s.on_key(VK_LWIN, false, false), HookAction::ReplaceWinUp);
        // Right Win works too.
        s.on_key(VK_RWIN, true, false);
        assert_eq!(s.on_key(VK_RWIN, false, false), HookAction::ReplaceWinUp);
    }

    #[test]
    fn chords_pass_through() {
        let mut s = WinKeyState::default();
        s.on_key(VK_LWIN, true, false);
        assert_eq!(s.on_key(E, true, false), HookAction::Pass);
        assert_eq!(s.on_key(E, false, false), HookAction::Pass);
        assert_eq!(s.on_key(VK_LWIN, false, false), HookAction::Pass, "Win+E is not a tap");
        // The next lone tap works again.
        s.on_key(VK_LWIN, true, false);
        assert_eq!(s.on_key(VK_LWIN, false, false), HookAction::ReplaceWinUp);
    }

    #[test]
    fn keys_released_in_other_orders_and_injected_input() {
        let mut s = WinKeyState::default();
        // Holding A, then tapping Win while A is still down: A was pressed first, so the
        // Win press itself is alone → still a tap (Windows would open Start here too).
        s.on_key(A, true, false);
        s.on_key(VK_LWIN, true, false);
        s.on_key(A, false, false);
        assert_eq!(s.on_key(VK_LWIN, false, false), HookAction::ReplaceWinUp);
        // Injected Win taps (e.g. our own "open Windows Start") are never touched.
        assert_eq!(s.on_key(VK_LWIN, true, true), HookAction::Pass);
        assert_eq!(s.on_key(VK_LWIN, false, true), HookAction::Pass);
        // A stray Win-up without a down isn't a tap.
        assert_eq!(WinKeyState::default().on_key(VK_LWIN, false, false), HookAction::Pass);
    }

    #[test]
    fn media_keys() {
        assert_eq!(MediaKey::from_vk(0xAF), Some(MediaKey::VolumeUp));
        assert_eq!(MediaKey::from_vk(0xB3), Some(MediaKey::PlayPause));
        assert_eq!(MediaKey::from_vk(A), None);
        assert!(MediaKey::VolumeDown.repeats() && !MediaKey::PlayPause.repeats());
    }
}
