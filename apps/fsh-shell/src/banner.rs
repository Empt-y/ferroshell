//! Notification banners (`@ferroshell/banner.slint`, overridable): bottom-right, above the
//! panel, never taking focus. Clicking one opens the sending app; it hides itself after a
//! few seconds unless the pointer is over it.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use fsh_widgets::theme_binding;
use fsh_win::notifications::Toast;
use fsh_win::{Hwnd, Rect, panel as win_panel};
use slint::{ComponentHandle as _, Image};
use slint_interpreter::{ComponentInstance, Value};

use crate::app::{App, with};
use crate::panel::hwnd_of;

const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 104.0;
const MARGIN: f64 = 12.0;

#[derive(Default)]
pub struct Banner {
    instance: RefCell<Option<ComponentInstance>>,
    unavailable: Cell<bool>,
    hwnd: Cell<Option<Hwnd>>,
    /// The notification shown, for "open".
    showing: Cell<Option<u32>>,
    hide_timer: slint::Timer,
    attach_timer: slint::Timer,
}

impl App {
    fn ensure_banner(&self) -> bool {
        let b = &self.banner;
        if b.unavailable.get() {
            return false;
        }
        if b.instance.borrow().is_some() {
            return true;
        }
        let (def, errors) = self.composer.compile_library_with_overrides("banner.slint", "BannerWindow", &self.library_overrides());
        for e in errors {
            tracing::warn!("banner override ignored: {e}");
        }
        match def.and_then(|d| d.create().map_err(|e| e.to_string())) {
            Ok(i) => {
                let _ = i.set_callback("clicked", |_| {
                    with(|app| {
                        if let Some(id) = app.banner.showing.get() {
                            app.open_notification(id);
                        }
                    });
                    Value::Void
                });
                let _ = i.set_callback("closed", |_| {
                    with(|app| app.hide_banner());
                    Value::Void
                });
                *b.instance.borrow_mut() = Some(i);
                true
            }
            Err(e) => {
                tracing::error!("notification banners unavailable: {e}");
                b.unavailable.set(true);
                false
            }
        }
    }

    pub(crate) fn show_banner(&self, toast: &Toast, more: usize, icon: Option<Image>, seconds: u64) {
        if !self.ensure_banner() {
            return;
        }
        let rect = self.banner_rect();
        {
            let inst = self.banner.instance.borrow();
            let Some(i) = inst.as_ref() else { return };
            let set = |p: &str, v: Value| {
                let _ = i.set_property(p, v);
            };
            set("app", Value::String(toast.app_name.as_str().into()));
            set("has-icon", Value::Bool(icon.is_some()));
            set("app-icon", Value::Image(icon.unwrap_or_default()));
            set("summary", Value::String(toast.title.as_str().into()));
            set("body", Value::String(toast.body.as_str().into()));
            set("more", Value::Number(more as f64));
            let st = self.state.borrow();
            theme_binding::apply(i, &st.theme, st.accent);
            i.window().set_position(slint::PhysicalPosition::new(rect.left, rect.top));
            i.window().set_size(slint::PhysicalSize::new(rect.width() as u32, rect.height() as u32));
            if let Err(e) = i.show() {
                tracing::error!("banner: {e}");
                return;
            }
        }
        self.banner.showing.set(Some(toast.id));
        if self.banner.hwnd.get().is_none() {
            self.banner.attach_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
                with(|app| app.banner_attach(rect));
            });
        } else if let Some(h) = self.banner.hwnd.get() {
            win_panel::set_rect(h, rect);
        }
        self.banner.hide_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(seconds), || {
            with(|app| app.banner_timeout());
        });
    }

    fn banner_attach(&self, rect: Rect) {
        let inst = self.banner.instance.borrow();
        let Some(hwnd) = inst.as_ref().and_then(hwnd_of) else { return };
        self.banner.attach_timer.stop();
        self.banner.hwnd.set(Some(hwnd));
        // Panel-style: clickable, but never takes focus from what you're doing.
        win_panel::make_panel_window(hwnd);
        win_panel::set_rect(hwnd, rect);
        win_panel::set_rounded_corners(hwnd, true);
        crate::popup::apply_backdrop(hwnd, &self.state.borrow().theme);
    }

    /// Time's up, unless the pointer is over the banner: then check again shortly.
    fn banner_timeout(&self) {
        let hovered = self
            .banner
            .instance
            .borrow()
            .as_ref()
            .and_then(|i| i.get_property("hovered").ok())
            .is_some_and(|v| v == Value::Bool(true));
        if hovered {
            self.banner.hide_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(1), || {
                with(|app| app.banner_timeout());
            });
        } else {
            self.hide_banner();
        }
    }

    pub(crate) fn hide_banner(&self) {
        self.banner.hide_timer.stop();
        self.banner.showing.set(None);
        if let Some(i) = self.banner.instance.borrow().as_ref() {
            let _ = i.hide();
        }
    }

    /// Bottom-right of the first panel's monitor, above the panel.
    fn banner_rect(&self) -> Rect {
        let st = self.state.borrow();
        let m = st
            .panels
            .first()
            .map(|p| p.monitor.clone())
            .or_else(|| fsh_win::monitor::monitors().into_iter().find(|m| m.primary));
        let Some(m) = m else { return Rect::new(0, 0, 360, 104) };
        let s = f64::from(m.scale());
        let (w, h, margin) = ((WIDTH * s) as i32, (HEIGHT * s) as i32, (MARGIN * s) as i32);
        let x = m.work.right - w - margin;
        let y = m.work.bottom - h - margin;
        Rect::new(x, y, x + w, y + h)
    }
}
