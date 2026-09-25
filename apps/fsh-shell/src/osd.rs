//! Volume and media keys while Ferroshell replaces Explorer, and the volume/brightness on-screen
//! display (`@ferroshell/osd.slint`, overridable).
//!
//! Alongside Explorer both stay off: Explorer handles the keys and shows its own OSD.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use fsh_widgets::theme_binding;
use fsh_win::keyhook::{KeyEvent, MediaKey};
use fsh_win::media::Control;
use fsh_win::{Hwnd, Rect, panel as win_panel};
use slint::ComponentHandle as _;
use slint_interpreter::{ComponentInstance, Value};

use crate::app::{App, with};
use crate::panel::hwnd_of;
use crate::services::audio::Cmd;

/// Windows' own step for the volume keys.
const KEY_STEP: f64 = 2.0;
const SHOW_FOR: Duration = Duration::from_millis(1500);
/// A volume change this soon after a key press shows the OSD.
const KEY_WINDOW: Duration = Duration::from_secs(1);
const WIDTH: f64 = 260.0;
const HEIGHT: f64 = 52.0;

#[derive(Default)]
pub struct Osd {
    instance: RefCell<Option<ComponentInstance>>,
    unavailable: Cell<bool>,
    hwnd: Cell<Option<Hwnd>>,
    last_key: Cell<Option<Instant>>,
    hide_timer: slint::Timer,
    attach_timer: slint::Timer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
}

/// Take over the volume and media keys? Only when Ferroshell replaces Explorer.
pub(crate) fn media_keys_wanted(safe_mode: bool) -> bool {
    !safe_mode && !fsh_win::taskbar::explorer_taskbar_present()
}

impl App {
    pub(crate) fn on_key_event(&self, e: KeyEvent) {
        match e {
            KeyEvent::WinTap => self.toggle_launcher(crate::launcher::Anchor::Cursor),
            KeyEvent::Media(k) => {
                match k {
                    MediaKey::VolumeUp => self.audio_cmd(Cmd::Step { notches: 1, step: KEY_STEP }),
                    MediaKey::VolumeDown => self.audio_cmd(Cmd::Step { notches: -1, step: KEY_STEP }),
                    MediaKey::Mute => self.audio_cmd(Cmd::ToggleMute(fsh_win::audio::Flow::Output)),
                    MediaKey::PlayPause => self.media_cmd(Control::PlayPause),
                    MediaKey::Next => self.media_cmd(Control::Next),
                    MediaKey::Previous => self.media_cmd(Control::Previous),
                    MediaKey::Stop => {}
                }
                if matches!(k, MediaKey::VolumeUp | MediaKey::VolumeDown | MediaKey::Mute) {
                    self.osd.last_key.set(Some(Instant::now()));
                    // Even when the level can't change (already at 100), show where it is.
                    self.show_osd();
                }
            }
        }
    }

    /// The audio service reported new levels: refresh the OSD if a key caused it.
    pub(crate) fn osd_audio_changed(&self) {
        if self.osd.last_key.get().is_some_and(|t| t.elapsed() < KEY_WINDOW) {
            self.show_osd();
        }
    }

    fn ensure_osd(&self) -> bool {
        let o = &self.osd;
        if o.unavailable.get() {
            return false;
        }
        if o.instance.borrow().is_some() {
            return true;
        }
        let overrides = self.library_overrides();
        let (def, errors) = self.composer.compile_library_with_overrides("osd.slint", "OsdWindow", &overrides);
        for e in errors {
            tracing::warn!("osd override ignored: {e}");
        }
        match def.and_then(|d| d.create().map_err(|e| e.to_string())) {
            Ok(i) => {
                *o.instance.borrow_mut() = Some(i);
                true
            }
            Err(e) => {
                tracing::error!("OSD unavailable: {e}");
                o.unavailable.set(true);
                false
            }
        }
    }

    fn show_osd(&self) {
        let (volume, muted) = self.audio_level();
        self.show_osd_level(OsdKind::Volume, volume, muted);
    }

    /// Show the OSD with a level (0–100) for 1.5 s.
    pub(crate) fn show_osd_level(&self, kind: OsdKind, value: f64, muted: bool) {
        if !self.ensure_osd() {
            return;
        }
        let rect = self.osd_rect();
        {
            let inst = self.osd.instance.borrow();
            let Some(i) = inst.as_ref() else { return };
            let _ = i.set_property("value", Value::Number(value));
            let _ = i.set_property("muted", Value::Bool(muted));
            // Older overrides may not have `kind`; they just keep showing a volume icon.
            let _ = i.set_property("kind", Value::String(match kind {
                OsdKind::Volume => "volume",
                OsdKind::Brightness => "brightness",
            }.into()));
            let st = self.state.borrow();
            theme_binding::apply(i, &st.theme, st.accent);
            i.window().set_position(slint::PhysicalPosition::new(rect.left, rect.top));
            i.window().set_size(slint::PhysicalSize::new(rect.width() as u32, rect.height() as u32));
            if let Err(e) = i.show() {
                tracing::error!("osd: {e}");
                return;
            }
        }
        if self.osd.hwnd.get().is_none() {
            self.osd.attach_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
                with(|app| app.osd_attach(rect));
            });
        } else if let Some(h) = self.osd.hwnd.get() {
            win_panel::set_rect(h, rect);
        }
        self.osd.hide_timer.start(slint::TimerMode::SingleShot, SHOW_FOR, || {
            with(|app| app.hide_osd());
        });
    }

    fn osd_attach(&self, rect: Rect) {
        let inst = self.osd.instance.borrow();
        let Some(hwnd) = inst.as_ref().and_then(hwnd_of) else { return };
        self.osd.attach_timer.stop();
        self.osd.hwnd.set(Some(hwnd));
        // A panel-style window: never takes focus from what you're doing.
        win_panel::make_panel_window(hwnd);
        win_panel::set_rect(hwnd, rect);
        win_panel::set_rounded_corners(hwnd, true);
        let st = self.state.borrow();
        crate::popup::apply_backdrop(hwnd, &st.theme);
    }

    pub(crate) fn hide_osd(&self) {
        self.osd.hide_timer.stop();
        if let Some(i) = self.osd.instance.borrow().as_ref() {
            let _ = i.hide();
        }
    }

    /// Bottom centre of the monitor with the foreground window, above the panel.
    fn osd_rect(&self) -> Rect {
        let monitors = fsh_win::monitor::monitors();
        let (cx, cy) = fsh_win::system::cursor_pos();
        let device = fsh_win::winfo::foreground().and_then(fsh_win::winfo::monitor_device);
        let m = monitors
            .iter()
            .find(|m| Some(&m.device) == device.as_ref())
            .or_else(|| monitors.iter().find(|m| m.rect.contains_point(cx, cy)))
            .or(monitors.first());
        let Some(m) = m else { return Rect::new(0, 0, 260, 52) };
        let s = f64::from(m.scale());
        let (w, h) = ((WIDTH * s) as i32, (HEIGHT * s) as i32);
        let x = m.work.left + (m.work.width() - w) / 2;
        let y = m.work.bottom - h - (48.0 * s) as i32;
        Rect::new(x, y, x + w, y + h)
    }
}
