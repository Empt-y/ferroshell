//! Hover previews: a popup above the hovered task showing live DWM thumbnails of its
//! windows (or just the title, for pinned apps that aren't running).
//!
//! The popup's look comes from `@ferroshell/popups.slint`; the shell computes the layout,
//! because each live thumbnail has to be placed by DWM at exactly the matching spot.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use fsh_config::Edge;
use fsh_widgets::theme_binding;
use fsh_win::thumbnail::Thumbnail;
use fsh_win::{Hwnd, Rect, panel as win_panel, winops};
use slint::{ComponentHandle as _, ModelRc, VecModel};
use slint_interpreter::{ComponentInstance, Struct, Value};

use crate::app::{App, with};
use crate::panel::{PanelKey, hwnd_of};

const SHOW_DELAY: Duration = Duration::from_millis(450);
const HIDE_DELAY: Duration = Duration::from_millis(250);
const PAD: f64 = 8.0;
const GAP: f64 = 8.0;
const TITLE_H: f64 = 26.0;
const MAX_W: f64 = 220.0;
const MAX_H: f64 = 130.0;
const MAX_WINDOWS: usize = 8;

#[derive(Clone, PartialEq)]
pub struct Hover {
    pub key: PanelKey,
    pub task_id: String,
    /// A tray icon (shows its tooltip) rather than a task (shows window previews).
    pub tray: bool,
    /// Hovered item's rectangle in panel coordinates (logical px): x, y, w, h.
    pub rect: [f64; 4],
}

struct Popup {
    instance: ComponentInstance,
    hwnd: Option<Hwnd>,
    thumbs: Vec<Thumbnail>,
    windows: Vec<isize>,
    visible: bool,
}

#[derive(Default)]
pub struct Thumbs {
    popup: RefCell<Option<Popup>>,
    hovered: RefCell<Option<Hover>>,
    popup_hovered: Cell<bool>,
    show_timer: slint::Timer,
    hide_timer: slint::Timer,
    unavailable: Cell<bool>,
}

impl App {
    pub fn task_hover(&self, key: &PanelKey, id: &str, hovering: bool, rect: [f64; 4]) {
        self.hover(key, id, false, hovering, rect);
    }

    pub fn tray_hover(&self, key: &PanelKey, id: &str, hovering: bool, rect: [f64; 4]) {
        self.hover(key, id, true, hovering, rect);
    }

    fn hover(&self, key: &PanelKey, id: &str, tray: bool, hovering: bool, rect: [f64; 4]) {
        let t = &self.thumbnails;
        if hovering {
            *t.hovered.borrow_mut() = Some(Hover { key: key.clone(), task_id: id.to_owned(), tray, rect });
            t.hide_timer.stop();
            let visible = t.popup.borrow().as_ref().is_some_and(|p| p.visible);
            if visible {
                self.show_preview();
            } else {
                t.show_timer.start(slint::TimerMode::SingleShot, SHOW_DELAY, || {
                    with(|a| a.show_preview());
                });
            }
        } else {
            let mut h = t.hovered.borrow_mut();
            if h.as_ref().is_some_and(|h| h.task_id == id) {
                *h = None;
            }
            drop(h);
            t.show_timer.stop();
            self.schedule_hide_preview();
        }
    }

    fn schedule_hide_preview(&self) {
        self.thumbnails.hide_timer.start(slint::TimerMode::SingleShot, HIDE_DELAY, || {
            with(|a| {
                let t = &a.thumbnails;
                if !t.popup_hovered.get() && t.hovered.borrow().is_none() {
                    a.hide_preview();
                }
            });
        });
    }

    pub(crate) fn hide_preview(&self) {
        let t = &self.thumbnails;
        t.show_timer.stop();
        t.popup_hovered.set(false);
        if let Some(p) = t.popup.borrow_mut().as_mut() {
            p.thumbs.clear();
            p.windows.clear();
            if p.visible {
                p.visible = false;
                let _ = p.instance.hide();
            }
        }
    }

    fn ensure_popup(&self) -> bool {
        let t = &self.thumbnails;
        if t.unavailable.get() {
            return false;
        }
        if t.popup.borrow().is_some() {
            return true;
        }
        let def = match self.composer.compile_library("popups.slint", "PreviewPopup") {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("previews disabled: popups.slint failed to compile:\n{e}");
                t.unavailable.set(true);
                return false;
            }
        };
        let instance = match def.create() {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("previews disabled: {e}");
                t.unavailable.set(true);
                return false;
            }
        };
        let _ = instance.set_callback("activate", |args| {
            let i = num(args) as usize;
            with(|a| a.preview_activate(i));
            Value::Void
        });
        let _ = instance.set_callback("close-window", |args| {
            let i = num(args) as usize;
            with(|a| a.preview_close(i));
            Value::Void
        });
        let _ = instance.set_callback("hover-changed", |args| {
            let hovering = matches!(args.first(), Some(Value::Bool(true)));
            with(|a| {
                a.thumbnails.popup_hovered.set(hovering);
                if hovering {
                    a.thumbnails.hide_timer.stop();
                } else {
                    a.schedule_hide_preview();
                }
            });
            Value::Void
        });
        *t.popup.borrow_mut() = Some(Popup { instance, hwnd: None, thumbs: vec![], windows: vec![], visible: false });
        true
    }

    /// Show (or update) the preview for the currently hovered task.
    pub(crate) fn show_preview(&self) {
        let Some(hover) = self.thumbnails.hovered.borrow().clone() else { return };
        if !self.ensure_popup() {
            return;
        }
        let st = self.state.borrow();
        let Some(panel) = st.panels.iter().find(|p| p.key == hover.key) else { return };
        // What to show: a task's windows, or just a title (tray tooltip, pinned launcher).
        let (windows, fallback_title, icon_img) = if hover.tray {
            let mut tray = self.tray.borrow_mut();
            let Some(icon) = tray.model.find(&hover.task_id).cloned() else { return };
            let label = tray.label(&icon);
            let img = tray.icons.get(&icon.icon_key()).cloned().unwrap_or_default();
            (Vec::new(), label, img)
        } else {
            let Some(task) = panel.tasks.find(&hover.task_id) else { return };
            let img = self.tasks.borrow().icons.get(&task.icon).cloned().unwrap_or_default();
            (task.windows.iter().copied().take(MAX_WINDOWS).collect::<Vec<isize>>(), task.title.clone(), img)
        };
        let scale = f64::from(panel.monitor.scale());
        let tasks = self.tasks.borrow();

        let mut popup_ref = self.thumbnails.popup.borrow_mut();
        let Some(popup) = popup_ref.as_mut() else { return };
        theme_binding::apply(&popup.instance, &st.theme, st.accent);

        // The native window must exist before DWM thumbnails can target it. On first use,
        // show it off-screen once and come back when it exists.
        if popup.hwnd.is_none() {
            popup.hwnd = hwnd_of(&popup.instance);
            if let Some(h) = popup.hwnd {
                win_panel::make_panel_window(h);
                win_panel::set_rounded_corners(h, true);
            } else {
                popup.instance.window().set_position(slint::PhysicalPosition::new(-32000, -32000));
                popup.instance.window().set_size(slint::PhysicalSize::new(1, 1));
                let _ = popup.instance.show();
                popup.visible = true;
                slint::Timer::single_shot(Duration::from_millis(30), || {
                    with(|a| a.show_preview());
                });
                return;
            }
        }
        let Some(dest) = popup.hwnd else { return };

        // Register thumbnails first: their source sizes decide the layout.
        popup.thumbs.clear();
        popup.windows.clear();
        let vertical = panel.config.edge.is_vertical();
        let mut entries = Vec::new();
        let (mut cx, mut cy) = (PAD, PAD);
        let (mut max_w, mut max_h) = (0.0f64, 0.0f64);
        let mut rects = Vec::new();
        let items: Vec<Option<isize>> = if windows.is_empty() { vec![None] } else { windows.iter().map(|w| Some(*w)).collect() };
        for w in items {
            let thumb = w.and_then(|h| Thumbnail::register(dest, Hwnd(h)));
            let (tw, th) = match thumb.as_ref().and_then(Thumbnail::source_size) {
                Some((sw, sh)) if sw > 0 && sh > 0 => {
                    let k = (MAX_W / sw as f64).min(MAX_H / sh as f64).min(1.0 / scale);
                    ((sw as f64 * k).max(80.0), sh as f64 * k)
                }
                // Title only: size to the text.
                _ => ((fallback_title.chars().count() as f64 * 7.0 + 48.0).clamp(80.0, 360.0), 0.0),
            };
            let title = w
                .and_then(|h| tasks.windows.iter().find(|x| x.hwnd == h).map(|x| x.title.clone()))
                .unwrap_or_else(|| fallback_title.clone());
            entries.push(entry(&title, icon_img.clone(), cx, cy, tw, th, w.is_some_and(|h| Some(h) == tasks.foreground)));
            if let Some(t) = thumb {
                // Inset slightly so the thumbnail sits inside the entry's rounded background.
                let r = Rect::new(
                    ((cx + 4.0) * scale).round() as i32,
                    ((cy + TITLE_H) * scale).round() as i32,
                    ((cx + tw - 4.0) * scale).round() as i32,
                    ((cy + TITLE_H + th - 4.0) * scale).round() as i32,
                );
                rects.push(r);
                popup.thumbs.push(t);
                if let Some(h) = w {
                    popup.windows.push(h);
                }
            } else if let Some(h) = w {
                popup.windows.push(h);
            }
            max_w = max_w.max(tw);
            max_h = max_h.max(th);
            if vertical {
                cy += TITLE_H + th + GAP;
            } else {
                cx += tw + GAP;
            }
        }
        let (w, h) = if vertical {
            (PAD * 2.0 + max_w, cy - GAP + PAD)
        } else {
            (cx - GAP + PAD, PAD * 2.0 + TITLE_H + max_h)
        };

        // Place next to the hovered item, away from the panel edge, inside the monitor.
        let (pw, ph) = ((w * scale).round() as i32, (h * scale).round() as i32);
        let [ix, iy, iw, ih] = hover.rect;
        let (icx, icy) = panel.to_screen(ix + iw / 2.0, iy + ih / 2.0);
        let gap = (6.0 * scale) as i32;
        let (mut x, mut y) = match panel.config.edge {
            Edge::Bottom => (icx - pw / 2, panel.rect.top - ph - gap),
            Edge::Top => (icx - pw / 2, panel.rect.bottom + gap),
            Edge::Left => (panel.rect.right + gap, icy - ph / 2),
            Edge::Right => (panel.rect.left - pw - gap, icy - ph / 2),
        };
        let m = panel.monitor.rect;
        x = x.clamp(m.left, (m.right - pw).max(m.left));
        y = y.clamp(m.top, (m.bottom - ph).max(m.top));

        let model = ModelRc::new(VecModel::from(entries));
        let _ = popup.instance.set_property("entries", Value::Model(model));
        popup.instance.window().set_size(slint::PhysicalSize::new(pw as u32, ph as u32));
        popup.instance.window().set_position(slint::PhysicalPosition::new(x, y));
        win_panel::set_rect(dest, Rect::new(x, y, x + pw, y + ph));
        let dark = {
            let bg = st.theme.color("popup-background");
            (u32::from(bg.r) + u32::from(bg.g) + u32::from(bg.b)) < 3 * 128
        };
        win_panel::set_backdrop(dest, win_panel::Backdrop::None, dark);
        if !popup.visible {
            let _ = popup.instance.show();
            popup.visible = true;
        }
        win_panel::set_topmost(dest, true);
        for (t, r) in popup.thumbs.iter().zip(rects) {
            t.show(r, 255);
        }
    }

    fn preview_activate(&self, i: usize) {
        let hwnd = self.thumbnails.popup.borrow().as_ref().and_then(|p| p.windows.get(i).copied());
        self.hide_preview();
        match hwnd {
            Some(h) => winops::activate(Hwnd(h)),
            None => {
                // A title-only entry: behave like clicking the item itself.
                if let Some(h) = self.thumbnails.hovered.borrow().clone() {
                    if h.tray {
                        self.tray_click(&h.key, &h.task_id, "left", h.rect);
                    } else {
                        self.activate_task(&h.key, &h.task_id);
                    }
                }
            }
        }
    }

    fn preview_close(&self, i: usize) {
        let hwnd = self.thumbnails.popup.borrow().as_ref().and_then(|p| p.windows.get(i).copied());
        if let Some(h) = hwnd {
            winops::close(Hwnd(h));
        }
        self.hide_preview();
    }
}

fn entry(title: &str, icon: slint::Image, x: f64, y: f64, w: f64, h: f64, active: bool) -> Value {
    let s: Struct = [
        ("title", Value::String(title.into())),
        ("icon", Value::Image(icon)),
        ("x", Value::Number(x)),
        ("y", Value::Number(y)),
        ("w", Value::Number(w)),
        ("h", Value::Number(h)),
        ("active", Value::Bool(active)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    Value::Struct(s)
}

fn num(args: &[Value]) -> f64 {
    match args.first() {
        Some(Value::Number(n)) => *n,
        _ => 0.0,
    }
}
