//! Desktop icons on the login-shell desktop: one icon view (`@ferroshell/desktop.slint`,
//! overridable) per monitor, kept at the bottom of the z-order as part of the desktop. The layout and selection
//! rules are `fsh_core::desktop_icons`; what's on the desktop is `fsh_win::desktop_items`.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use fsh_common::paths;
use fsh_core::desktop_icons::{self as rules, Cell, Direction, Grid, IconSize, SortBy, SortKey};
use fsh_widgets::theme_binding;
use fsh_win::desktop_items::{self, DesktopEntry, ViewSettings};
use fsh_win::icon::RgbaImage;
use fsh_win::{Hwnd, Monitor};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use slint::{ComponentHandle as _, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use slint_interpreter::{ComponentInstance, Struct, Value};

use crate::app::{App, with};
use crate::panel::hwnd_of;

type GlobalCallback = Box<dyn Fn(&[Value]) -> Value>;

/// Where the user put an icon (kept per monitor, by device name).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    device: String,
    cell: Cell,
}

struct Surface {
    monitor: Monitor,
    instance: ComponentInstance,
    hwnd: StdCell<Option<Hwnd>>,
    attach: slint::Timer,
    grid: Grid,
    scale: f64,
    model: Rc<VecModel<Value>>,
    /// Item indices shown here, in model order.
    members: RefCell<Vec<usize>>,
}

struct Item {
    entry: DesktopEntry,
    surface: usize,
    cell: Cell,
}

/// A rubber band being dragged: its view, where it started, and what was selected before.
#[derive(Clone)]
struct Band {
    surface: usize,
    start: (f64, f64),
    base: HashSet<usize>,
}

struct Drag {
    from: Cell,
    target: Option<(usize, Cell)>,
}

#[derive(Default)]
pub struct DesktopIcons {
    surfaces: RefCell<Vec<Surface>>,
    items: RefCell<Vec<Item>>,
    selected: RefCell<HashSet<usize>>,
    anchor: StdCell<Option<usize>>,
    focused: StdCell<Option<usize>>,
    renaming: StdCell<Option<usize>>,
    band: RefCell<Option<Band>>,
    drag: RefCell<Option<Drag>>,
    saved: RefCell<HashMap<String, Saved>>,
    view: StdCell<Option<ViewSettings>>,
    /// Icons by item key and modification time.
    images: RefCell<HashMap<(String, i64), Image>>,
    refresh_timer: slint::Timer,
    refreshing: StdCell<bool>,
    unavailable: StdCell<bool>,
    /// Shown in an ordinary window alongside Explorer, for trying it out
    /// (`fsh-ctl shell debug.desktop_preview`).
    preview: StdCell<bool>,
}

fn layout_file() -> PathBuf {
    paths::state_dir().join("desktop-layout.json")
}

fn to_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&img.pixels, img.width, img.height))
}

fn strukt(fields: Vec<(&str, Value)>) -> Value {
    let s: Struct = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    Value::Struct(s)
}

fn num(a: &[Value], i: usize) -> f64 {
    match a.get(i) {
        Some(Value::Number(n)) => *n,
        _ => 0.0,
    }
}

fn text(a: &[Value], i: usize) -> String {
    match a.get(i) {
        Some(Value::String(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn flag(a: &[Value], i: usize) -> bool {
    matches!(a.get(i), Some(Value::Bool(true)))
}

fn icon_size(v: ViewSettings) -> IconSize {
    IconSize::from_pixels(v.icon_size)
}

/// What an item's image is keyed by: pictures that change (the Recycle Bin filling up)
/// aren't cached across refreshes.
fn image_key(e: &DesktopEntry) -> (String, i64) {
    (e.key.clone(), if e.path.is_none() { -1 } else { e.modified })
}

impl App {
    /// As the login shell: icon views on every monitor, as the desktop.
    pub(crate) fn start_desktop_icons(&self) {
        if let Some(saved) = std::fs::read_to_string(layout_file()).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            *self.desktop_icons.saved.borrow_mut() = saved;
        }
        self.desktop_icons.view.set(Some(desktop_items::view_settings()));
        self.build_desktop_surfaces(true);
        self.refresh_desktop_items();
    }

    fn desktop_view(&self) -> ViewSettings {
        self.desktop_icons.view.get().unwrap_or_else(desktop_items::view_settings)
    }

    /// (Re)creates one icon view per monitor (at start, and when displays change).
    pub(crate) fn build_desktop_surfaces(&self, force: bool) {
        let d = &self.desktop_icons;
        if d.unavailable.get() || (self.desktop_hwnd().is_none() && !d.preview.get()) {
            return;
        }
        // WM_DISPLAYCHANGE also arrives when nothing we care about changed (e.g. another
        // app capturing the screen): rebuild only for new monitor geometry or scaling.
        if !force && !d.preview.get() && !d.surfaces.borrow().is_empty() {
            let now: Vec<(String, fsh_win::Rect, fsh_win::Rect, u32)> =
                fsh_win::monitor::monitors().into_iter().map(|m| (m.device, m.rect, m.work, m.dpi)).collect();
            let mut have: Vec<(String, fsh_win::Rect, fsh_win::Rect, u32)> =
                d.surfaces.borrow().iter().map(|s| (s.monitor.device.clone(), s.monitor.rect, s.monitor.work, s.monitor.dpi)).collect();
            let mut now_sorted = now;
            now_sorted.sort_by(|a, b| a.0.cmp(&b.0));
            have.sort_by(|a, b| a.0.cmp(&b.0));
            if now_sorted == have {
                return;
            }
        }
        let old: Vec<Surface> = d.surfaces.borrow_mut().drain(..).collect();
        for s in &old {
            if let (Some(h), Some(desk)) = (s.hwnd.get(), self.desktop.window.borrow().as_ref()) {
                desk.release(h);
            }
            let _ = s.instance.hide();
        }
        drop(old);
        let overrides = self.library_overrides();
        let view = self.desktop_view();
        let mut monitors = fsh_win::monitor::monitors();
        monitors.sort_by_key(|m| !m.primary);
        if d.preview.get() {
            // One window on the primary monitor, inset from its corner.
            monitors.truncate(1);
            if let Some(m) = monitors.first_mut() {
                let (x, y) = (m.work.left + 80, m.work.top + 60);
                m.rect = fsh_win::Rect::new(x, y, x + (m.work.width() * 2 / 3), y + (m.work.height() * 2 / 3));
                m.work = m.rect;
            }
        }
        for (index, monitor) in monitors.into_iter().enumerate() {
            let (def, errors) = self.composer.compile_library_with_overrides("desktop.slint", "DesktopWindow", &overrides);
            for e in errors {
                tracing::warn!("desktop override ignored: {e}");
            }
            let instance = match def.and_then(|d| d.create().map_err(|e| e.to_string())) {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!("desktop icons unavailable: {e}");
                    d.unavailable.set(true);
                    return;
                }
            };
            let scale = f64::from(monitor.scale());
            let (r, w) = (monitor.rect, monitor.work);
            let grid = Grid::new(
                f64::from(w.left - r.left) / scale,
                f64::from(w.top - r.top) / scale,
                f64::from(w.width()) / scale,
                f64::from(w.height()) / scale,
                icon_size(view),
            );
            let model = Rc::new(VecModel::<Value>::default());
            self.wire_desktop_surface(&instance, index);
            let set = |p: &str, v: Value| {
                if let Err(e) = instance.set_global_property("Desktop", p, v) {
                    tracing::warn!("Desktop.{p}: {e}");
                }
            };
            set("icons", Value::Model(ModelRc::from(model.clone())));
            set("cell-width", Value::Number(grid.cell_w));
            set("cell-height", Value::Number(grid.cell_h));
            set("icon-size", Value::Number(f64::from(icon_size(view).pixels())));
            set("show-icons", Value::Bool(view.show_icons));
            {
                let st = self.state.borrow();
                theme_binding::apply(&instance, &st.theme, st.accent);
            }
            instance.window().set_position(slint::PhysicalPosition::new(r.left, r.top));
            instance.window().set_size(slint::PhysicalSize::new(r.width() as u32, r.height() as u32));
            if let Err(e) = instance.show() {
                tracing::error!("desktop view: {e}");
                continue;
            }
            let surface = Surface {
                monitor,
                instance,
                hwnd: StdCell::new(None),
                attach: slint::Timer::default(),
                grid,
                scale,
                model,
                members: RefCell::default(),
            };
            surface.attach.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
                with(|a| a.attach_desktop_surface(index));
            });
            d.surfaces.borrow_mut().push(surface);
        }
        self.apply_desktop_wallpaper();
        self.place_desktop_items();
    }

    fn desktop_hwnd(&self) -> Option<Hwnd> {
        self.desktop.window.borrow().as_ref().map(|w| w.hwnd())
    }

    /// Once the view's native window exists, make it part of the desktop.
    fn attach_desktop_surface(&self, index: usize) {
        let surfaces = self.desktop_icons.surfaces.borrow();
        let Some(s) = surfaces.get(index) else { return };
        let Some(hwnd) = hwnd_of(&s.instance) else { return };
        s.attach.stop();
        s.hwnd.set(Some(hwnd));
        if self.desktop_icons.preview.get() {
            fsh_win::winops::activate(hwnd);
        } else if let Some(desk) = self.desktop.window.borrow().as_ref() {
            // No focus grab: a restarted shell mustn't pull the keyboard from the user's app.
            desk.adopt(hwnd, s.monitor.rect);
        }
        let _ = s.instance.invoke("take-focus", &[]);
    }

    fn wire_desktop_surface(&self, instance: &ComponentInstance, s: usize) {
        let cb = |name: &str, f: GlobalCallback| {
            if let Err(e) = instance.set_global_callback("Desktop", name, f) {
                tracing::warn!("Desktop.{name}: {e}");
            }
        };
        cb("icon-pointer", Box::new(move |a| {
            let (i, kind, ctrl, shift) = (num(a, 0) as usize, text(a, 1), flag(a, 2), flag(a, 3));
            with(|app| app.desktop_icon_pointer(s, i, &kind, ctrl, shift));
            Value::Void
        }));
        cb("icon-drag", Box::new(move |a| {
            let (kind, x, y) = (text(a, 0), num(a, 1), num(a, 2));
            with(|app| app.desktop_icon_drag(s, &kind, x, y));
            Value::Void
        }));
        cb("background-pointer", Box::new(move |a| {
            let (kind, x, y, ctrl) = (text(a, 0), num(a, 1), num(a, 2), flag(a, 3));
            with(|app| app.desktop_background_pointer(s, &kind, x, y, ctrl));
            Value::Void
        }));
        cb("context", Box::new(move |a| {
            let (i, x, y) = (num(a, 0) as i64, num(a, 1), num(a, 2));
            // After the pointer handler returns: menus run a modal loop.
            slint::Timer::single_shot(Duration::ZERO, move || {
                with(|app| app.desktop_icon_context(s, i, x, y));
            });
            Value::Void
        }));
        cb("key", Box::new(move |a| {
            let (t, ctrl, shift, alt) = (text(a, 0), flag(a, 1), flag(a, 2), flag(a, 3));
            Value::Bool(with(|app| app.desktop_key(s, &t, ctrl, shift, alt)).unwrap_or(false))
        }));
        cb("rename-done", Box::new(move |a| {
            let accept = flag(a, 0);
            with(|app| app.desktop_rename_done(accept));
            Value::Void
        }));
    }

    // ------------------------------------------------------------ wallpaper

    /// The wallpaper as Windows has it set (called at start and when it changes).
    pub(crate) fn apply_desktop_wallpaper(&self) {
        let surfaces = self.desktop_icons.surfaces.borrow();
        if surfaces.is_empty() {
            return;
        }
        let solid = fsh_win::wallpaper::background_type() == Some(1);
        let picture = (!solid).then(|| fsh_win::wallpaper::current_picture(&paths::state_dir())).flatten();
        let image = picture.as_ref().and_then(|p| match Image::load_from_path(p) {
            Ok(i) => Some(i),
            Err(e) => {
                tracing::warn!("desktop wallpaper {}: {e:?}", p.display());
                None
            }
        });
        let fit = match fsh_win::wallpaper::wallpaper_fit() {
            "span" => "fill",
            f => f,
        };
        let (r, g, b) = fsh_win::wallpaper::background_colour();
        for s in surfaces.iter() {
            let set = |p: &str, v: Value| {
                let _ = s.instance.set_global_property("Desktop", p, v);
            };
            set("background", Value::Brush(slint::Brush::SolidColor(slint::Color::from_rgb_u8(r, g, b))));
            set("wallpaper-fit", Value::String(fit.into()));
            set("has-wallpaper", Value::Bool(image.is_some()));
            if let Some(img) = &image {
                set("wallpaper", Value::Image(img.clone()));
            }
        }
    }

    // ------------------------------------------------------------ items

    /// Something on the desktop changed: re-read it shortly (changes come in bursts).
    pub(crate) fn desktop_items_changed(&self) {
        self.desktop_icons.refresh_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(250), || {
            with(|a| a.refresh_desktop_items());
        });
    }

    /// Re-reads the desktop and loads missing icons on a worker thread.
    pub(crate) fn refresh_desktop_items(&self) {
        let d = &self.desktop_icons;
        if d.refreshing.replace(true) {
            // One is running: go again when it finishes.
            self.desktop_items_changed();
            return;
        }
        let have: HashSet<(String, i64)> = d.images.borrow().keys().cloned().collect();
        let px = {
            let surfaces = d.surfaces.borrow();
            let scale = surfaces.first().map_or(1.0, |s| s.scale);
            (f64::from(icon_size(self.desktop_view()).pixels()) * scale).round() as i32
        };
        let spawned = std::thread::Builder::new().name("desktop-items".into()).spawn(move || {
            let _com = fsh_win::com::ComGuard::new();
            let entries = desktop_items::entries();
            let images: Vec<((String, i64), RgbaImage)> = entries
                .iter()
                .filter(|e| !have.contains(&image_key(e)))
                .filter_map(|e| {
                    let thumb = e.path.is_some() && !e.is_folder;
                    desktop_items::item_image(&e.key, px, thumb).map(|img| (image_key(e), img))
                })
                .collect();
            let _ = slint::invoke_from_event_loop(move || {
                with(|a| a.desktop_items_loaded(entries, images));
            });
        });
        if let Err(e) = spawned {
            d.refreshing.set(false);
            tracing::warn!("could not read the desktop: {e}");
        }
    }

    fn desktop_items_loaded(&self, entries: Vec<DesktopEntry>, images: Vec<((String, i64), RgbaImage)>) {
        let d = &self.desktop_icons;
        d.refreshing.set(false);
        {
            let mut cache = d.images.borrow_mut();
            for (k, img) in &images {
                cache.insert(k.clone(), to_image(img));
            }
            // Forget pictures of things no longer there.
            let live: HashSet<(String, i64)> = entries.iter().map(image_key).collect();
            cache.retain(|k, _| live.contains(k));
        }
        // Keep the selection (by key) across the refresh.
        let selected_keys: HashSet<String> = {
            let items = d.items.borrow();
            d.selected.borrow().iter().filter_map(|&i| items.get(i).map(|it| it.entry.key.clone())).collect()
        };
        let keys: Vec<SortKey> = entries
            .iter()
            .map(|e| SortKey { name: e.name.clone(), rank: e.rank, size: e.size, type_name: e.type_name.clone(), modified: e.modified })
            .collect();
        let order = rules::sort_order(&keys, SortBy::Name);
        let mut slots: Vec<Option<DesktopEntry>> = entries.into_iter().map(Some).collect();
        let sorted: Vec<DesktopEntry> = order.into_iter().filter_map(|i| slots[i].take()).collect();
        *d.items.borrow_mut() = sorted.into_iter().map(|entry| Item { entry, surface: 0, cell: Cell { col: 0, row: 0 } }).collect();
        *d.selected.borrow_mut() = d
            .items
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, it)| selected_keys.contains(&it.entry.key))
            .map(|(i, _)| i)
            .collect();
        d.renaming.set(None);
        self.place_desktop_items();
    }

    /// Assigns every item a monitor and a cell, then redraws.
    fn place_desktop_items(&self) {
        let d = &self.desktop_icons;
        let surfaces = d.surfaces.borrow();
        if surfaces.is_empty() {
            return;
        }
        let view = self.desktop_view();
        let saved = d.saved.borrow();
        let mut items = d.items.borrow_mut();
        let device_index: HashMap<&str, usize> = surfaces.iter().enumerate().map(|(i, s)| (s.monitor.device.as_str(), i)).collect();
        for it in items.iter_mut() {
            it.surface = if view.auto_arrange {
                0
            } else {
                saved.get(&it.entry.key).and_then(|s| device_index.get(s.device.as_str()).copied()).unwrap_or(0)
            };
        }
        for (si, s) in surfaces.iter().enumerate() {
            let members: Vec<usize> = (0..items.len()).filter(|&i| items[i].surface == si).collect();
            let keys: Vec<String> = members.iter().map(|&i| items[i].entry.key.clone()).collect();
            let here: HashMap<String, Cell> = saved
                .iter()
                .filter(|(_, v)| v.device == s.monitor.device)
                .map(|(k, v)| (k.clone(), v.cell))
                .collect();
            let cells = rules::place(&keys, &here, &s.grid, view.auto_arrange);
            for (&i, c) in members.iter().zip(cells) {
                items[i].cell = c;
            }
            *s.members.borrow_mut() = members;
        }
        drop(items);
        drop(saved);
        drop(surfaces);
        self.redraw_desktop_icons();
    }

    fn redraw_desktop_icons(&self) {
        let d = &self.desktop_icons;
        let surfaces = d.surfaces.borrow();
        let items = d.items.borrow();
        let selected = d.selected.borrow();
        let images = d.images.borrow();
        let renaming = d.renaming.get();
        for s in surfaces.iter() {
            let rows: Vec<Value> = s
                .members
                .borrow()
                .iter()
                .map(|&i| {
                    let it = &items[i];
                    let (x, y) = s.grid.position(it.cell);
                    strukt(vec![
                        ("name", Value::String(it.entry.name.as_str().into())),
                        ("icon", Value::Image(images.get(&image_key(&it.entry)).cloned().unwrap_or_default())),
                        ("x", Value::Number(x)),
                        ("y", Value::Number(y)),
                        ("selected", Value::Bool(selected.contains(&i))),
                        ("renaming", Value::Bool(renaming == Some(i))),
                    ])
                })
                .collect();
            s.model.set_vec(rows);
        }
    }

    fn global_index(&self, s: usize, local: usize) -> Option<usize> {
        self.desktop_icons.surfaces.borrow().get(s)?.members.borrow().get(local).copied()
    }

    // ------------------------------------------------------------ input

    fn desktop_icon_pointer(&self, s: usize, local: usize, kind: &str, ctrl: bool, shift: bool) {
        let Some(i) = self.global_index(s, local) else { return };
        let d = &self.desktop_icons;
        if d.renaming.get().is_some_and(|r| r != i) {
            self.desktop_rename_done(true);
        }
        match kind {
            "double" => self.open_desktop_items(&[i]),
            _ => {
                let current = d.selected.borrow().clone();
                // Pressing an already-selected icon keeps the selection, so it can be dragged.
                let next = if !ctrl && !shift && current.contains(&i) {
                    current
                } else {
                    rules::click(&current, d.anchor.get(), i, ctrl, shift)
                };
                *d.selected.borrow_mut() = next;
                if !shift {
                    d.anchor.set(Some(i));
                }
                d.focused.set(Some(i));
                self.redraw_desktop_icons();
            }
        }
    }

    fn surface_at_screen(&self, x: i32, y: i32) -> Option<usize> {
        self.desktop_icons.surfaces.borrow().iter().position(|s| s.monitor.rect.contains_point(x, y))
    }

    fn desktop_icon_drag(&self, s: usize, kind: &str, x: f64, y: f64) {
        let d = &self.desktop_icons;
        let Some(focused) = d.focused.get() else { return };
        if self.desktop_view().auto_arrange {
            // Auto-arranged icons can't be placed by hand.
            return;
        }
        let (sx, sy) = {
            let surfaces = d.surfaces.borrow();
            let Some(src) = surfaces.get(s) else { return };
            (src.monitor.rect.left + (x * src.scale) as i32, src.monitor.rect.top + (y * src.scale) as i32)
        };
        match kind {
            "move" => {
                let from = d.items.borrow().get(focused).map(|it| it.cell);
                let target = self.surface_at_screen(sx, sy).map(|t| {
                    let surfaces = d.surfaces.borrow();
                    let ts = &surfaces[t];
                    let lx = f64::from(sx - ts.monitor.rect.left) / ts.scale;
                    let ly = f64::from(sy - ts.monitor.rect.top) / ts.scale;
                    (t, ts.grid.cell_at(lx, ly))
                });
                {
                    let mut drag = d.drag.borrow_mut();
                    let dr = drag.get_or_insert(Drag { from: from.unwrap_or(Cell { col: 0, row: 0 }), target: None });
                    dr.target = target;
                }
                for (i, surf) in d.surfaces.borrow().iter().enumerate() {
                    let on = target.filter(|(t, _)| *t == i);
                    let _ = surf.instance.set_global_property("Desktop", "drop-visible", Value::Bool(on.is_some()));
                    if let Some((_, c)) = on {
                        let (px, py) = surf.grid.position(c);
                        let _ = surf.instance.set_global_property("Desktop", "drop-x", Value::Number(px));
                        let _ = surf.instance.set_global_property("Desktop", "drop-y", Value::Number(py));
                    }
                }
            }
            _ => {
                for surf in d.surfaces.borrow().iter() {
                    let _ = surf.instance.set_global_property("Desktop", "drop-visible", Value::Bool(false));
                }
                let Some(drag) = d.drag.borrow_mut().take() else { return };
                if kind == "drop"
                    && let Some((t, to)) = drag.target
                {
                    self.move_desktop_items(drag.from, t, to);
                }
            }
        }
    }

    /// Moves the selection by the offset the dragged icon moved (possibly to another monitor).
    fn move_desktop_items(&self, from: Cell, to_surface: usize, to: Cell) {
        let d = &self.desktop_icons;
        let selected: Vec<usize> = d.selected.borrow().iter().copied().collect();
        {
            let surfaces = d.surfaces.borrow();
            let Some(target) = surfaces.get(to_surface) else { return };
            let mut items = d.items.borrow_mut();
            // Everything on the target monitor, then the moving items (at their current
            // cells, which for another monitor means relative to the dragged one).
            let mut indices: Vec<usize> = (0..items.len()).filter(|&i| items[i].surface == to_surface && !selected.contains(&i)).collect();
            let first_moving = indices.len();
            indices.extend(selected.iter().copied());
            let cells: Vec<Cell> = indices.iter().map(|&i| items[i].cell).collect();
            let moving: Vec<usize> = (first_moving..indices.len()).collect();
            let moved = rules::move_items(&cells, &moving, from, to, &target.grid);
            for (k, &i) in indices.iter().enumerate().skip(first_moving) {
                items[i].cell = moved[k];
                items[i].surface = to_surface;
            }
            let mut saved = d.saved.borrow_mut();
            for it in items.iter() {
                saved.insert(it.entry.key.clone(), Saved { device: surfaces[it.surface].monitor.device.clone(), cell: it.cell });
            }
        }
        self.save_desktop_layout();
        self.place_desktop_items();
    }

    fn save_desktop_layout(&self) {
        let d = &self.desktop_icons;
        // Only positions of things that still exist.
        let live: HashSet<String> = d.items.borrow().iter().map(|it| it.entry.key.clone()).collect();
        d.saved.borrow_mut().retain(|k, _| live.contains(k));
        if let Ok(text) = serde_json::to_string_pretty(&*d.saved.borrow()) {
            let _ = std::fs::write(layout_file(), text);
        }
    }

    fn desktop_background_pointer(&self, s: usize, kind: &str, x: f64, y: f64, ctrl: bool) {
        let d = &self.desktop_icons;
        match kind {
            "down" => {
                if d.renaming.get().is_some() {
                    self.desktop_rename_done(true);
                }
                if !ctrl {
                    d.selected.borrow_mut().clear();
                }
                *d.band.borrow_mut() = Some(Band { surface: s, start: (x, y), base: d.selected.borrow().clone() });
                self.redraw_desktop_icons();
            }
            "move" => {
                let Some(Band { surface: bs, start, base }) = d.band.borrow().clone() else { return };
                if bs != s {
                    return;
                }
                let hits: Vec<usize> = {
                    let surfaces = d.surfaces.borrow();
                    let items = d.items.borrow();
                    let surf = &surfaces[s];
                    let members = surf.members.borrow();
                    let cells: Vec<Cell> = members.iter().map(|&i| items[i].cell).collect();
                    rules::in_rectangle(&cells, &surf.grid, start, (x, y)).into_iter().map(|k| members[k]).collect()
                };
                let mut next = base;
                next.extend(hits);
                if *d.selected.borrow() != next {
                    *d.selected.borrow_mut() = next;
                    self.redraw_desktop_icons();
                }
            }
            _ => {
                *d.band.borrow_mut() = None;
            }
        }
    }

    fn desktop_key(&self, s: usize, key: &str, ctrl: bool, shift: bool, _alt: bool) -> bool {
        let d = &self.desktop_icons;
        if d.renaming.get().is_some() {
            return false;
        }
        let selection: Vec<usize> = {
            let mut v: Vec<usize> = d.selected.borrow().iter().copied().collect();
            v.sort_unstable();
            v
        };
        let dir = match key {
            "\u{f700}" => Some(Direction::Up),
            "\u{f701}" => Some(Direction::Down),
            "\u{f702}" => Some(Direction::Left),
            "\u{f703}" => Some(Direction::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            // Navigate among the icons on this monitor.
            let (cells, members) = {
                let surfaces = d.surfaces.borrow();
                let items = d.items.borrow();
                let members = surfaces[s].members.borrow().clone();
                (members.iter().map(|&i| items[i].cell).collect::<Vec<_>>(), members)
            };
            let current = d.focused.get().and_then(|f| members.iter().position(|&m| m == f));
            if let Some(next) = rules::navigate(&cells, current, dir).map(|k| members[k]) {
                if shift {
                    let anchor = d.anchor.get().unwrap_or(next);
                    *d.selected.borrow_mut() = rules::click(&HashSet::new(), Some(anchor), next, false, true);
                } else {
                    *d.selected.borrow_mut() = HashSet::from([next]);
                    d.anchor.set(Some(next));
                }
                d.focused.set(Some(next));
                self.redraw_desktop_icons();
            }
            return true;
        }
        match (key, ctrl) {
            ("\n" | "\r", _) if !selection.is_empty() => self.open_desktop_items(&selection),
            ("a" | "A", true) => {
                *d.selected.borrow_mut() = (0..d.items.borrow().len()).collect();
                self.redraw_desktop_icons();
            }
            // F2
            ("\u{f705}", _) => {
                if let Some(&i) = selection.first() {
                    self.begin_desktop_rename(i);
                }
            }
            // Delete (Shift+Delete: permanently, after Windows asks)
            ("\u{7f}", _) if !selection.is_empty() => self.delete_desktop_items(&selection, shift),
            // F5
            ("\u{f708}", _) => self.refresh_desktop_items(),
            ("\u{1b}", _) => {
                d.selected.borrow_mut().clear();
                self.redraw_desktop_icons();
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ actions

    /// Opens items: files with their app, folders and classic icons in File Explorer.
    pub(crate) fn open_desktop_items(&self, indices: &[usize]) {
        let items = self.desktop_icons.items.borrow();
        for &i in indices {
            let Some(it) = items.get(i) else { continue };
            if it.entry.is_folder || it.entry.path.is_none() {
                // explorer.exe opens a folder window (never a second shell: our desktop is
                // the shell window).
                crate::actions::launch_verb("explorer.exe".into(), Some(format!("\"{}\"", it.entry.key)), "open");
            } else {
                crate::actions::launch(it.entry.key.clone());
            }
        }
    }

    pub(crate) fn begin_desktop_rename(&self, i: usize) {
        let d = &self.desktop_icons;
        let Some(name) = d.items.borrow().get(i).filter(|it| it.entry.path.is_some()).map(|it| it.entry.name.clone()) else { return };
        d.renaming.set(Some(i));
        for s in d.surfaces.borrow().iter() {
            let _ = s.instance.set_global_property("Desktop", "rename-text", Value::String(name.as_str().into()));
        }
        self.redraw_desktop_icons();
    }

    fn desktop_rename_done(&self, accept: bool) {
        let d = &self.desktop_icons;
        let Some(i) = d.renaming.take() else { return };
        let typed = d
            .surfaces
            .borrow()
            .iter()
            .find_map(|s| match s.instance.get_global_property("Desktop", "rename-text") {
                Ok(Value::String(t)) => Some(t.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        self.redraw_desktop_icons();
        if !accept {
            return;
        }
        let Some((path, display)) = d.items.borrow().get(i).and_then(|it| Some((it.entry.path.clone()?, it.entry.name.clone()))) else { return };
        if typed.trim() == display {
            return;
        }
        let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(new_name) = rules::rename_target(&file_name, &display, &typed) else {
            tracing::info!("rename to {typed:?} refused: not a valid file name");
            return;
        };
        // Keep the icon where it was under its new name.
        if let Some(saved) = d.saved.borrow().get(&path.to_string_lossy().into_owned()).cloned() {
            let new_key = path.with_file_name(&new_name).to_string_lossy().into_owned();
            d.saved.borrow_mut().insert(new_key, saved);
        }
        let owner = self.desktop_hwnd();
        let owner_raw = owner.map(|h| h.0);
        let spawned = std::thread::Builder::new().name("desktop-rename".into()).spawn(move || {
            let _com = fsh_win::com::ComGuard::new();
            if let Err(e) = fsh_win::files::rename(&path, &new_name, owner_raw.map(Hwnd)) {
                tracing::warn!("{e:#}");
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("rename: {e}");
        }
    }

    pub(crate) fn delete_desktop_items(&self, indices: &[usize], permanently: bool) {
        let paths: Vec<PathBuf> = {
            let items = self.desktop_icons.items.borrow();
            indices.iter().filter_map(|&i| items.get(i)?.entry.path.clone()).collect()
        };
        if paths.is_empty() {
            return;
        }
        let owner_raw = self.desktop_hwnd().map(|h| h.0);
        let spawned = std::thread::Builder::new().name("desktop-delete".into()).spawn(move || {
            let _com = fsh_win::com::ComGuard::new();
            let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Err(e) = fsh_win::files::delete(&refs, permanently, owner_raw.map(Hwnd)) {
                tracing::warn!("{e:#}");
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("delete: {e}");
        }
    }

    /// Right-click: on an icon (selecting it first, unless it's part of the selection) or
    /// on empty space.
    fn desktop_icon_context(&self, s: usize, local: i64, x: f64, y: f64) {
        let d = &self.desktop_icons;
        let (sx, sy) = {
            let surfaces = d.surfaces.borrow();
            let Some(surf) = surfaces.get(s) else { return };
            (surf.monitor.rect.left + (x * surf.scale) as i32, surf.monitor.rect.top + (y * surf.scale) as i32)
        };
        if local < 0 {
            d.selected.borrow_mut().clear();
            self.redraw_desktop_icons();
            self.desktop_menu(sx, sy);
            return;
        }
        let Some(i) = self.global_index(s, local as usize) else { return };
        if !d.selected.borrow().contains(&i) {
            *d.selected.borrow_mut() = HashSet::from([i]);
            d.anchor.set(Some(i));
            self.redraw_desktop_icons();
        }
        d.focused.set(Some(i));
        let selection: Vec<usize> = d.selected.borrow().iter().copied().collect();
        self.desktop_item_menu(&selection, sx, sy);
    }

    /// The View / Sort by / Refresh part of the background menu, applied.
    pub(crate) fn desktop_view_command(&self, cmd: ViewCommand) {
        let d = &self.desktop_icons;
        let mut v = self.desktop_view();
        match cmd {
            ViewCommand::Size(px) => v.icon_size = px,
            ViewCommand::AutoArrange => v.auto_arrange = !v.auto_arrange,
            ViewCommand::ShowIcons => v.show_icons = !v.show_icons,
            ViewCommand::Sort(by) => {
                // Sorting lines everything up again in that order.
                let mut items = d.items.borrow_mut();
                let keys: Vec<SortKey> = items
                    .iter()
                    .map(|it| SortKey { name: it.entry.name.clone(), rank: it.entry.rank, size: it.entry.size, type_name: it.entry.type_name.clone(), modified: it.entry.modified })
                    .collect();
                let order = rules::sort_order(&keys, by);
                let mut slots: Vec<Option<Item>> = items.drain(..).map(Some).collect();
                *items = order.into_iter().filter_map(|i| slots[i].take()).collect();
                d.selected.borrow_mut().clear();
                d.saved.borrow_mut().clear();
                drop(items);
                self.save_desktop_layout();
            }
            ViewCommand::Refresh => {
                self.refresh_desktop_items();
                return;
            }
        }
        d.view.set(Some(v));
        desktop_items::save_view_settings(v);
        if matches!(cmd, ViewCommand::Size(_)) {
            d.images.borrow_mut().clear();
            self.build_desktop_surfaces(true);
            self.refresh_desktop_items();
        } else {
            for s in d.surfaces.borrow().iter() {
                let _ = s.instance.set_global_property("Desktop", "show-icons", Value::Bool(v.show_icons));
            }
            self.place_desktop_items();
        }
    }

    /// `debug.desktop_preview`: show (or close) the icon view in a normal window, alongside
    /// Explorer's desktop.
    pub(crate) fn toggle_desktop_preview(&self) -> bool {
        let d = &self.desktop_icons;
        if d.preview.get() {
            for s in d.surfaces.borrow_mut().drain(..) {
                let _ = s.instance.hide();
            }
            d.preview.set(false);
            return false;
        }
        if !d.surfaces.borrow().is_empty() {
            // The real desktop is up (login shell): nothing to preview.
            return false;
        }
        d.preview.set(true);
        self.start_desktop_icons();
        true
    }

    pub(crate) fn desktop_icons_state(&self) -> Json {
        let d = &self.desktop_icons;
        let v = self.desktop_view();
        json!({
            "surfaces": d.surfaces.borrow().iter().map(|s| json!({
                "monitor": s.monitor.device,
                "attached": s.hwnd.get().is_some(),
                "grid": [s.grid.cols, s.grid.rows],
                "icons": s.members.borrow().len(),
            })).collect::<Vec<_>>(),
            "items": d.items.borrow().len(),
            "selected": d.selected.borrow().len(),
            "view": { "icon_size": v.icon_size, "auto_arrange": v.auto_arrange, "show_icons": v.show_icons },
            "unavailable": d.unavailable.get(),
            "preview": d.preview.get(),
        })
    }

    /// Paths of the selected items (for menus).
    pub(crate) fn desktop_selection_keys(&self, indices: &[usize]) -> Vec<(String, Option<PathBuf>)> {
        let items = self.desktop_icons.items.borrow();
        indices.iter().filter_map(|&i| items.get(i).map(|it| (it.entry.key.clone(), it.entry.path.clone()))).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewCommand {
    Size(u32),
    AutoArrange,
    ShowIcons,
    Sort(SortBy),
    Refresh,
}
