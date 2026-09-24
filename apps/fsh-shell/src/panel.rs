//! One panel window on one monitor: a compiled widget composition, its app-bar screen
//! reservation, and the Win32 styling that makes it behave like a panel.

use std::cell::Cell;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use fsh_config::{Edge, PanelConfig, Rgba, Theme};
use fsh_core::geometry;
use fsh_widgets::{Composer, PanelSpec, Registry, WidgetStatus, theme_binding};
use fsh_win::appbar::{AppBar, AppBarEvent};
use fsh_win::monitor::Monitor;
use fsh_win::{Hwnd, Rect, panel as win_panel};
use slint::ComponentHandle as _;
use slint::{ModelRc, SharedString};
use slint_interpreter::{ComponentInstance, Value};

use crate::app;
use crate::taskbar::PanelTasks;

/// Identifies "the same panel" across rebuilds, so its app bar can be reused (re-registering
/// makes every maximized window on the monitor jump).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PanelKey {
    pub config_index: usize,
    pub device: String,
}

/// An app bar kept across a rebuild, with the cells its notification handler shares.
pub type ReusableAppBar = (AppBar, Rc<Cell<Option<Hwnd>>>, Rc<Cell<bool>>);
type ShellCallback = Box<dyn Fn(&[Value]) -> Value>;

pub struct Panel {
    pub key: PanelKey,
    pub name: String,
    pub instance: ComponentInstance,
    pub appbar: AppBar,
    pub hwnd: Rc<Cell<Option<Hwnd>>>,
    pub monitor: Monitor,
    pub config: PanelConfig,
    pub rect: Rect,
    pub fullscreen_app: Rc<Cell<bool>>,
    pub tasks: PanelTasks,
}

pub struct Build<'a> {
    pub composer: &'a Composer,
    pub registry: &'a Registry,
    pub config: &'a PanelConfig,
    pub monitor: &'a Monitor,
    pub key: PanelKey,
    pub theme: &'a Theme,
    pub accent: Option<Rgba>,
    pub safe_mode: bool,
    pub config_error: &'a str,
    pub tasks: PanelTasks,
    pub reuse_appbar: Option<ReusableAppBar>,
}

fn edge_name(e: Edge) -> &'static str {
    match e {
        Edge::Top => "top",
        Edge::Bottom => "bottom",
        Edge::Left => "left",
        Edge::Right => "right",
    }
}

pub(crate) fn hwnd_of(instance: &ComponentInstance) -> Option<Hwnd> {
    let handle = instance.window().window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(Hwnd(h.hwnd.get())),
        _ => None,
    }
}

pub fn create(b: Build<'_>) -> anyhow::Result<(Panel, Vec<WidgetStatus>)> {
    let name = format!("panel-{}-{}", b.key.config_index, b.monitor.device.trim_start_matches(r"\\.\"));
    let spec = PanelSpec { edge: b.config.edge, floating: b.config.floating, widgets: b.config.widgets.clone() };
    let (def, statuses) = b.composer.build_panel(b.registry, &spec, &name)?;
    let instance = def.create()?;

    bind_shell(&instance, &b.key, b.config, b.safe_mode, b.config_error, ModelRc::from(b.tasks.model.clone()));
    theme_binding::apply(&instance, b.theme, b.accent);

    let (appbar, hwnd_cell, fullscreen) = match b.reuse_appbar {
        Some(reused) => reused,
        None => {
            let hwnd_cell: Rc<Cell<Option<Hwnd>>> = Rc::default();
            let fullscreen: Rc<Cell<bool>> = Rc::default();
            let (h, fs) = (hwnd_cell.clone(), fullscreen.clone());
            let appbar = AppBar::new(move |ev| match ev {
                AppBarEvent::FullScreen(on) => {
                    fs.set(on);
                    if let Some(hwnd) = h.get() {
                        tracing::info!("full-screen app {}; panel {}", if on { "opened" } else { "closed" }, if on { "lowered" } else { "raised" });
                        win_panel::set_topmost(hwnd, !on);
                    }
                }
                AppBarEvent::PositionChanged => app::schedule_relayout(),
            })?;
            (appbar, hwnd_cell, fullscreen)
        }
    };

    let mut panel = Panel {
        key: b.key,
        name,
        instance,
        appbar,
        hwnd: hwnd_cell,
        monitor: b.monitor.clone(),
        config: b.config.clone(),
        rect: Rect::default(),
        fullscreen_app: fullscreen,
        tasks: b.tasks,
    };
    panel.place(b.theme);

    // Position before showing so the window is created on the right monitor with the right
    // scale factor. The native window only exists once the event loop has processed the
    // show request, so Win32 styling happens later in `attach_native`.
    panel.instance.show()?;
    panel.attach_native(b.theme);
    Ok((panel, statuses))
}

impl Panel {
    /// Apply panel styling once the native window exists. Returns false if it doesn't yet;
    /// the app retries on a short timer.
    pub fn attach_native(&self, theme: &Theme) -> bool {
        if self.hwnd.get().is_some() {
            return true;
        }
        let Some(hwnd) = hwnd_of(&self.instance) else { return false };
        self.hwnd.set(Some(hwnd));
        win_panel::make_panel_window(hwnd);
        win_panel::set_rect(hwnd, self.rect);
        // A game may already be full-screen (e.g. the shell was just restarted); the app
        // bar notification only fires on changes, so check directly.
        self.fullscreen_app.set(self.fullscreen_app.get() || fsh_win::winfo::fullscreen_app_on(self.monitor.rect));
        if self.fullscreen_app.get() {
            win_panel::set_topmost(hwnd, false);
        }
        self.apply_window_theme(theme);
        tracing::debug!("panel {} attached to native window {hwnd:?}", self.name);
        true
    }

    /// (Re)compute the app-bar reservation and window rectangle and move the window there.
    pub fn place(&mut self, theme: &Theme) {
        let scale = self.monitor.scale();
        let margin = theme.metric("floating-margin");
        let reserve = geometry::reserve_px(self.config.thickness, self.config.floating, margin, scale);
        let granted = self.appbar.set_pos(geometry::appbar_edge(self.config.edge), self.monitor.rect, reserve);
        let rect = geometry::window_rect(granted, self.config.floating, margin, scale);
        self.rect = rect;
        let window = self.instance.window();
        window.set_position(slint::PhysicalPosition::new(rect.left, rect.top));
        window.set_size(slint::PhysicalSize::new(rect.width().max(1) as u32, rect.height().max(1) as u32));
        if let Some(hwnd) = self.hwnd.get() {
            win_panel::set_rect(hwnd, rect);
        }
    }

    pub fn apply_theme(&self, theme: &Theme, accent: Option<Rgba>) {
        theme_binding::apply(&self.instance, theme, accent);
        self.apply_window_theme(theme);
    }

    fn apply_window_theme(&self, theme: &Theme) {
        let Some(hwnd) = self.hwnd.get() else { return };
        let backdrop = match theme.backdrop {
            fsh_config::Backdrop::None => win_panel::Backdrop::None,
            fsh_config::Backdrop::Acrylic => win_panel::Backdrop::Acrylic,
            fsh_config::Backdrop::Mica => win_panel::Backdrop::Mica,
        };
        let bg = theme.color("panel-background");
        let dark = (u32::from(bg.r) + u32::from(bg.g) + u32::from(bg.b)) < 3 * 128;
        win_panel::set_backdrop(hwnd, backdrop, dark);
        win_panel::set_rounded_corners(hwnd, self.config.floating);
    }

    pub fn set_now(&self, now: i64) {
        let _ = self.instance.set_global_property("Shell", "now", Value::Number(now as f64));
    }

    /// Re-check whether a full-screen app covers this panel's monitor and adjust z-order.
    /// Backs up the app bar notification, which can be missed.
    pub fn update_fullscreen(&self) {
        let Some(hwnd) = self.hwnd.get() else { return };
        let now = fsh_win::winfo::fullscreen_app_on(self.monitor.rect);
        if now != self.fullscreen_app.get() {
            self.fullscreen_app.set(now);
            tracing::info!("full-screen app {} on {}", if now { "detected" } else { "gone" }, self.monitor.device);
            win_panel::set_topmost(hwnd, !now);
        }
    }

    pub fn set_sources_revision(&self, rev: i32) {
        let _ = self.instance.set_global_property("Shell", "sources-revision", Value::Number(f64::from(rev)));
    }

    pub fn set_config_error(&self, error: &str) {
        let _ = self.instance.set_global_property("Shell", "config-error", Value::String(error.into()));
    }

    /// Convert a position in the panel's logical coordinates to physical screen pixels.
    pub fn to_screen(&self, x: f64, y: f64) -> (i32, i32) {
        let s = f64::from(self.monitor.scale());
        (self.rect.left + (x * s).round() as i32, self.rect.top + (y * s).round() as i32)
    }

    pub fn hide(&self) {
        let _ = self.instance.hide();
    }

    /// Give up the window but keep the app bar for a replacement panel.
    pub fn into_reusable(self) -> (PanelKey, ReusableAppBar) {
        self.hide();
        self.hwnd.set(None);
        (self.key, (self.appbar, self.hwnd, self.fullscreen_app))
    }
}

fn bind_shell(instance: &ComponentInstance, key: &PanelKey, config: &PanelConfig, safe_mode: bool, config_error: &str, tasks: ModelRc<Value>) {
    let set = |name: &str, value: Value| {
        if let Err(e) = instance.set_global_property("Shell", name, value) {
            tracing::warn!("Shell.{name}: {e}");
        }
    };
    set("vertical", Value::Bool(config.edge.is_vertical()));
    set("edge", Value::String(edge_name(config.edge).into()));
    set("thickness", Value::Number(f64::from(config.thickness)));
    set("safe-mode", Value::Bool(safe_mode));
    set("config-error", Value::String(config_error.into()));
    set("now", Value::Number(chrono::Utc::now().timestamp() as f64));
    set("tasks", Value::Model(tasks));

    let callback = |name: &str, f: ShellCallback| {
        if let Err(e) = instance.set_global_callback("Shell", name, f) {
            tracing::warn!("Shell.{name}: {e}");
        }
    };
    callback(
        "format-time",
        Box::new(|args| {
            let now = num(args, 0) as i64;
            Value::String(SharedString::from(crate::timefmt::format_time(now, &text(args, 1))))
        }),
    );
    callback("source", Box::new(|args| Value::String(app::source_value(&text(args, 0)).into())));
    let k = key.clone();
    callback("activate-task", Box::new(move |args| {
        app::with(|a| a.activate_task(&k, &text(args, 0)));
        Value::Void
    }));
    let k = key.clone();
    callback("task-action", Box::new(move |args| {
        app::with(|a| a.task_action(&k, &text(args, 0), &text(args, 1)));
        Value::Void
    }));
    let k = key.clone();
    callback("task-menu", Box::new(move |args| {
        app::with(|a| a.task_menu(&k, &text(args, 0), num(args, 1), num(args, 2)));
        Value::Void
    }));
    let k = key.clone();
    callback("task-hover", Box::new(move |args| {
        let hovering = matches!(args.get(1), Some(Value::Bool(true)));
        let rect = [num(args, 2), num(args, 3), num(args, 4), num(args, 5)];
        app::with(|a| a.task_hover(&k, &text(args, 0), hovering, rect));
        Value::Void
    }));
    callback("invoke", Box::new(|args| {
        crate::actions::invoke(&text(args, 0), &text(args, 1));
        Value::Void
    }));
}

fn text(args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(Value::String(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn num(args: &[Value], i: usize) -> f64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n,
        _ => 0.0,
    }
}
