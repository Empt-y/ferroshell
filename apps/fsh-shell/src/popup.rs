//! Applet popups: a widget's `popup.slint` opened next to it (the clock's calendar, the
//! volume mixer, …). One popup is open at a time; it closes on Escape or when focus moves
//! elsewhere, like a menu. The launcher shares the window styling and placement.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use fsh_config::{Edge, Theme};
use fsh_widgets::theme_binding;
use fsh_win::monitor::Monitor;
use fsh_win::{Hwnd, Rect, panel as win_panel, winfo, winops};
use slint::ComponentHandle as _;
use slint::ModelRc;
use slint_interpreter::{ComponentInstance, Value};

use crate::app::{App, with};
use crate::panel::{Panel, PanelKey, hwnd_of};

const FOCUS_GRACE: Duration = Duration::from_millis(400);

#[derive(Default)]
pub struct Popups {
    /// Compiled popups by widget instance id; cleared when panels are rebuilt.
    cache: RefCell<HashMap<String, ComponentInstance>>,
    /// The open popup's widget instance id.
    open: RefCell<Option<String>>,
    /// Most recently closed popup and when: a click on its widget that closed it by
    /// taking focus shouldn't reopen it straight away.
    closed: RefCell<Option<(String, Instant)>>,
    hwnd: Cell<Option<Hwnd>>,
    shown_at: Cell<Option<Instant>>,
    watch_timer: slint::Timer,
    attach_timer: slint::Timer,
}

/// Where a popup of `w`×`h` physical pixels goes: against the panel's inner edge, lined up
/// with the item (`item` in the panel's logical coordinates): centred on it, or starting at it,
/// and kept inside the work area.
pub(crate) fn place(panel: Option<&Panel>, item: Option<[f64; 4]>, monitor: &Monitor, w: i32, h: i32, centred: bool) -> Rect {
    let s = f64::from(monitor.scale());
    let anchor = panel.map(|p| {
        let item = match item {
            Some([ix, iy, iw, ih]) => {
                let (x, y) = p.to_screen(ix, iy);
                [x, y, (iw * s) as i32, (ih * s) as i32]
            }
            None => [p.rect.left, p.rect.top, 0, 0],
        };
        (p.config.edge, p.rect, item)
    });
    place_px(anchor, monitor.work, s, w, h, centred)
}

/// [place] in physical pixels: nchor is the panel's edge, its rectangle and the item's
/// [x, y, w, h] on screen.
fn place_px(anchor: Option<(Edge, Rect, [i32; 4])>, work: Rect, scale: f64, w: i32, h: i32, centred: bool) -> Rect {
    let gap = (6.0 * scale) as i32;
    let (w, h) = (w.min(work.width() - 2 * gap), h.min(work.height() - 2 * gap));
    let (mut x, mut y) = match anchor {
        Some((edge, panel, [ix, iy, iw, ih])) => {
            let (ax, ay) = if centred { (ix + iw / 2 - w / 2, iy + ih / 2 - h / 2) } else { (ix, iy) };
            match edge {
                Edge::Bottom => (ax, panel.top - gap - h),
                Edge::Top => (ax, panel.bottom + gap),
                Edge::Left => (panel.right + gap, ay),
                Edge::Right => (panel.left - gap - w, ay),
            }
        }
        None => (work.left + (work.width() - w) / 2, work.top + (work.height() - h) / 2),
    };
    x = x.clamp(work.left + gap, (work.right - w - gap).max(work.left));
    y = y.clamp(work.top + gap, (work.bottom - h - gap).max(work.top));
    Rect::new(x, y, x + w, y + h)
}

/// Popup window styling shared by applet popups and the launcher.
pub(crate) fn style_window(hwnd: Hwnd, rect: Rect, theme: &Theme) {
    win_panel::make_popup_window(hwnd);
    win_panel::set_rect(hwnd, rect);
    win_panel::set_rounded_corners(hwnd, true);
    apply_backdrop(hwnd, theme);
}

/// The theme's DWM material behind a popup-coloured window.
pub(crate) fn apply_backdrop(hwnd: Hwnd, theme: &Theme) {
    let backdrop = match theme.backdrop {
        fsh_config::Backdrop::None => win_panel::Backdrop::None,
        fsh_config::Backdrop::Acrylic => win_panel::Backdrop::Acrylic,
        fsh_config::Backdrop::Mica => win_panel::Backdrop::Mica,
    };
    let bg = theme.color("popup-background");
    win_panel::set_backdrop(hwnd, backdrop, (u32::from(bg.r) + u32::from(bg.g) + u32::from(bg.b)) < 384);
}

/// Should focus moving to `fg` close a popup whose window is `own`?
pub(crate) fn lost_focus(fg: Option<Hwnd>, own: Option<Hwnd>) -> bool {
    // Our own context menus ("#32768") keep the popup as their owner.
    fg.is_some() && fg != own && !fg.is_some_and(|h| winfo::class_name(h) == "#32768")
}

impl App {
    /// `Shell.invoke("popup", "<instance-id>|x,y,w,h")` from a widget.
    pub(crate) fn toggle_popup(&self, key: &PanelKey, arg: &str) {
        let (instance, rect) = arg.split_once('|').unwrap_or((arg, ""));
        let v: Vec<f64> = rect.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let rect = (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]]);
        let was_open = self.popups.open.borrow().as_deref() == Some(instance);
        let just_closed = self.popups.closed.borrow().as_ref().is_some_and(|(i, t)| i == instance && t.elapsed() < FOCUS_GRACE);
        self.close_popup();
        if !was_open && !just_closed {
            self.show_popup(key, instance, rect);
        }
    }

    pub(crate) fn close_popup(&self) {
        let p = &self.popups;
        p.watch_timer.stop();
        p.attach_timer.stop();
        let Some(id) = p.open.borrow_mut().take() else { return };
        if let Some(i) = p.cache.borrow().get(&id) {
            let _ = i.hide();
        }
        p.hwnd.set(None);
        *p.closed.borrow_mut() = Some((id, Instant::now()));
    }

    /// Panels were rebuilt: compiled popups may be stale.
    pub(crate) fn clear_popups(&self) {
        self.close_popup();
        self.popups.cache.borrow_mut().clear();
    }

    pub(crate) fn popup_state(&self) -> serde_json::Value {
        serde_json::json!({
            "open": *self.popups.open.borrow(),
            "compiled": self.popups.cache.borrow().keys().cloned().collect::<Vec<_>>(),
        })
    }

    fn ensure_applet_popup(&self, key: &PanelKey, instance: &str) -> Option<(i32, i32)> {
        let st = self.state.borrow();
        let panel = st.panels.iter().find(|p| &p.key == key)?;
        let statuses = &st.statuses.iter().find(|(n, _)| n == &panel.name)?.1;
        let index = statuses.iter().position(|s| s.instance == instance)?;
        let entry = panel.config.widgets.get(index)?;
        let pkg = st.registry.get(&entry.id).ok()?;
        let spec = pkg.manifest.popup.clone()?;
        let scale = f64::from(panel.monitor.scale());
        let size = ((f64::from(spec.width) * scale) as i32, (f64::from(spec.height) * scale) as i32);
        if self.popups.cache.borrow().contains_key(instance) {
            return Some(size);
        }
        let started = Instant::now();
        let built = self
            .composer
            .build_popup(&st.registry, entry, instance)
            .and_then(|(def, _)| def.create().map_err(|e| e.to_string()));
        let inst = match built {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("popup for {} unavailable: {e}", entry.id);
                return None;
            }
        };
        let err = st.config.error().map(crate::app::first_line).unwrap_or_default();
        crate::panel::bind_shell(&inst, key, &panel.config, self.safe_mode, &err, ModelRc::from(panel.tasks.model.clone()));
        let _ = inst.set_global_property("Shell", "tray", Value::Model(ModelRc::from(panel.tray.model.clone())));
        self.bind_services(&inst);
        tracing::info!("compiled popup for {} in {:?}", entry.id, started.elapsed());
        self.popups.cache.borrow_mut().insert(instance.to_owned(), inst);
        Some(size)
    }

    fn show_popup(&self, key: &PanelKey, instance: &str, item: Option<[f64; 4]>) {
        self.hide_preview();
        self.hide_launcher();
        let Some((w, h)) = self.ensure_applet_popup(key, instance) else { return };
        let rect = {
            let st = self.state.borrow();
            let Some(panel) = st.panels.iter().find(|p| &p.key == key) else { return };
            place(Some(panel), item, &panel.monitor, w, h, true)
        };
        {
            let cache = self.popups.cache.borrow();
            let Some(i) = cache.get(instance) else { return };
            let st = self.state.borrow();
            theme_binding::apply(i, &st.theme, st.accent);
            let _ = i.set_global_property("Shell", "now", Value::Number(chrono::Utc::now().timestamp() as f64));
            i.window().set_position(slint::PhysicalPosition::new(rect.left, rect.top));
            i.window().set_size(slint::PhysicalSize::new(rect.width() as u32, rect.height() as u32));
            if let Err(e) = i.show() {
                tracing::error!("popup: {e}");
                return;
            }
        }
        *self.popups.open.borrow_mut() = Some(instance.to_owned());
        self.popups.attach_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
            with(|app| app.popup_attach(rect));
        });
    }

    fn popup_attach(&self, rect: Rect) {
        let p = &self.popups;
        let open = p.open.borrow().clone();
        let cache = p.cache.borrow();
        let Some(i) = open.as_ref().and_then(|id| cache.get(id)) else {
            p.attach_timer.stop();
            return;
        };
        let Some(hwnd) = hwnd_of(i) else { return };
        p.attach_timer.stop();
        p.hwnd.set(Some(hwnd));
        style_window(hwnd, rect, &self.state.borrow().theme);
        winops::activate(hwnd);
        let _ = i.invoke("take-focus", &[]);
        p.shown_at.set(Some(Instant::now()));
        p.watch_timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), || {
            with(|app| app.popup_watch_focus());
        });
    }

    fn popup_watch_focus(&self) {
        let p = &self.popups;
        if p.shown_at.get().is_none_or(|t| t.elapsed() < FOCUS_GRACE) {
            return;
        }
        if lost_focus(winfo::foreground(), p.hwnd.get()) {
            self.close_popup();
        }
    }

    pub(crate) fn for_each_popup(&self, f: &dyn Fn(&ComponentInstance)) {
        for i in self.popups.cache.borrow().values() {
            f(i);
        }
    }

    /// Keep an open popup's clock current.
    pub(crate) fn popup_tick(&self, now: i64) {
        let open = self.popups.open.borrow();
        let cache = self.popups.cache.borrow();
        if let Some(i) = open.as_ref().and_then(|id| cache.get(id)) {
            let _ = i.set_global_property("Shell", "now", Value::Number(now as f64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1920×1080 with a 48px panel at the bottom, at 100%.
    const WORK: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };
    const BOTTOM: Rect = Rect { left: 0, top: 1032, right: 1920, bottom: 1080 };

    #[test]
    fn centred_above_a_bottom_panel_item() {
        let r = place_px(Some((Edge::Bottom, BOTTOM, [1000, 1036, 80, 40])), WORK, 1.0, 300, 400, true);
        assert_eq!((r.left, r.top, r.width(), r.height()), (890, 626, 300, 400));
    }

    #[test]
    fn kept_inside_the_work_area() {
        // The clock in the far corner: pushed left, with a gap.
        let r = place_px(Some((Edge::Bottom, BOTTOM, [1860, 1036, 60, 40])), WORK, 1.0, 300, 400, true);
        assert_eq!(r.right, 1914);
        // Taller than the screen: shrunk to fit.
        let r = place_px(Some((Edge::Bottom, BOTTOM, [10, 1036, 40, 40])), WORK, 1.0, 300, 5000, true);
        assert_eq!((r.top, r.bottom), (6, 1026));
    }

    #[test]
    fn other_edges_and_alignment() {
        let top = Rect { left: 0, top: 0, right: 1920, bottom: 48 };
        let work = Rect { left: 0, top: 48, right: 1920, bottom: 1080 };
        let r = place_px(Some((Edge::Top, top, [100, 4, 40, 40])), work, 1.0, 300, 400, false);
        assert_eq!((r.left, r.top), (100, 54), "starts at the item, below the panel");
        let left = Rect { left: 0, top: 0, right: 48, bottom: 1080 };
        let work = Rect { left: 48, top: 0, right: 1920, bottom: 1080 };
        let r = place_px(Some((Edge::Left, left, [4, 500, 40, 40])), work, 1.25, 300, 400, true);
        assert_eq!((r.left, r.top), (55, 320));
        let r = place_px(None, WORK, 1.0, 300, 400, true);
        assert_eq!((r.left, r.top), (810, 316), "no panel: centred on the monitor");
    }
}