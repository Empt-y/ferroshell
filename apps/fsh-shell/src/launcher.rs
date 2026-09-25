//! The application launcher (start menu): window lifecycle, what it shows, and what its
//! buttons do. The look is `@ferroshell/launcher.slint` (overridable); search and ranking
//! are `fsh_core::launcher`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use fsh_config::ConfigEditor;
use fsh_core::launcher::{self as core, AppEntry, History, ResultKind, SearchOptions, SearchResult};
use fsh_widgets::theme_binding;
use fsh_win::menu::{self, Anchor as MenuAnchor, MenuItem};
use fsh_win::session::{self, PowerAction};
use fsh_win::{Hwnd, Rect, winfo, winops};
use slint::{ComponentHandle as _, Image, ModelRc, VecModel};
use slint_interpreter::{ComponentInstance, Struct, Value};

use crate::app::{App, config_path, with};
use crate::panel::{PanelKey, hwnd_of};

const RECENT_COUNT: usize = 20;
type GlobalCallback = Box<dyn Fn(&[Value]) -> Value>;
const FOCUS_GRACE: Duration = Duration::from_millis(400);

/// Where the launcher was opened from.
#[derive(Debug, Clone)]
pub enum Anchor {
    /// A launcher button: its panel and its rectangle within the panel (logical px).
    Panel { key: PanelKey, rect: Option<[f64; 4]> },
    /// The Windows key or a command: the panel on the monitor under the cursor.
    Cursor,
}

enum Row {
    Header(String),
    App(String),
    Result(SearchResult),
}

#[derive(Default)]
pub struct LauncherState {
    pub apps: Vec<AppEntry>,
    pub icons: HashMap<String, Image>,
    pub history: History,
    rows: Vec<Row>,
    categories: Vec<String>,
    category: usize,
    query: String,
    selected: usize,
    confirm: Option<PowerAction>,
}

#[derive(Default)]
pub struct Launcher {
    instance: RefCell<Option<ComponentInstance>>,
    hwnd: Cell<Option<Hwnd>>,
    visible: Cell<bool>,
    shown_at: Cell<Option<Instant>>,
    watch_timer: slint::Timer,
    attach_timer: slint::Timer,
    unavailable: Cell<bool>,
    pub state: RefCell<LauncherState>,
}

fn history_path() -> std::path::PathBuf {
    fsh_common::paths::state_dir().join("launcher-history.json")
}

pub fn load_history() -> History {
    std::fs::read(history_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_history(h: &History) {
    if let Ok(json) = serde_json::to_vec(h) {
        // Off the UI thread: never wait on the disk there.
        std::thread::spawn(move || {
            let _ = std::fs::write(history_path(), json);
        });
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn power_label(a: &str) -> &'static str {
    match a {
        "lock" => "Lock",
        "sleep" => "Sleep",
        "restart" => "Restart",
        "shutdown" => "Shut down",
        "logout" => "Sign out",
        _ => "",
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}

fn strukt(fields: Vec<(&str, Value)>) -> Value {
    let s: Struct = fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    Value::Struct(s)
}

fn string(s: impl AsRef<str>) -> Value {
    Value::String(s.as_ref().into())
}

fn model(items: Vec<Value>) -> Value {
    Value::Model(ModelRc::new(VecModel::from(items)))
}

/// Does `word` resolve to a program on PATH (or an App Paths registration Windows knows)?
fn on_path(word: &str) -> bool {
    let has_ext = std::path::Path::new(word).extension().is_some();
    let exts: Vec<String> = if has_ext {
        vec![String::new()]
    } else {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into()).split(';').map(str::to_lowercase).collect()
    };
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.push(std::path::PathBuf::from(&windir));
    dirs.iter().any(|d| exts.iter().any(|e| d.join(format!("{word}{e}")).is_file()))
}

impl App {
    fn launcher_config(&self) -> fsh_config::LauncherConfig {
        self.state.borrow().config.config().launcher.clone()
    }

    pub(crate) fn on_apps_update(&self, update: crate::apps::Update) {
        {
            let mut st = self.launcher.state.borrow_mut();
            match update {
                crate::apps::Update::Apps(apps) => {
                    st.categories = core::categories(&apps);
                    st.apps = apps;
                }
                crate::apps::Update::Icons(icons) => {
                    for (id, img) in icons {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img.pixels, img.width, img.height);
                        st.icons.insert(id, Image::from_rgba8(buf));
                    }
                }
            }
        }
        if self.launcher.visible.get() {
            self.launcher_refresh(false);
        }
    }

    /// For `fsh-ctl shell dump_state`: what the launcher is showing.
    pub fn launcher_state(&self) -> serde_json::Value {
        let st = self.launcher.state.borrow();
        serde_json::json!({
            "visible": self.launcher.visible.get(),
            "query": st.query,
            "category": st.category,
            "apps": st.apps.len(),
            "icons": st.icons.len(),
            "rows": st.rows.len(),
            "selected": self.launcher_selected(),
            "first_rows": st.rows.iter().take(5).map(|r| match r {
                Row::Header(h) => format!("# {h}"),
                Row::App(id) => format!("app {id}"),
                Row::Result(r) => format!("{:?} {}", r.kind, r.title),
            }).collect::<Vec<_>>(),
        })
    }

    pub fn toggle_launcher(&self, anchor: Anchor) {
        if self.launcher.visible.get() {
            self.hide_launcher();
        } else {
            self.show_launcher(anchor);
        }
    }

    pub fn hide_launcher(&self) {
        let l = &self.launcher;
        l.watch_timer.stop();
        l.visible.set(false);
        l.state.borrow_mut().confirm = None;
        if let Some(i) = l.instance.borrow().as_ref() {
            let _ = i.hide();
        }
    }

    fn ensure_launcher(&self) -> bool {
        let l = &self.launcher;
        if l.unavailable.get() {
            return false;
        }
        if l.instance.borrow().is_some() {
            return true;
        }
        let overrides = self.library_overrides();
        let (def, errors) = self.composer.compile_library_with_overrides("launcher.slint", "LauncherWindow", &overrides);
        for e in errors {
            tracing::warn!("launcher override ignored: {e}");
        }
        let instance = match def.and_then(|d| d.create().map_err(|e| e.to_string())) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("launcher unavailable: {e}");
                l.unavailable.set(true);
                return false;
            }
        };
        let cb = |name: &str, f: GlobalCallback| {
            if let Err(e) = instance.set_global_callback("Launcher", name, f) {
                tracing::warn!("Launcher.{name}: {e}");
            }
        };
        let text = |a: &[Value], i: usize| match a.get(i) {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        };
        let num = |a: &[Value], i: usize| match a.get(i) {
            Some(Value::Number(n)) => *n,
            _ => 0.0,
        };
        cb("search", Box::new(move |a| {
            let q = text(a, 0);
            with(|app| app.launcher_search(q));
            Value::Void
        }));
        cb("select-category", Box::new(move |a| {
            let i = num(a, 0) as usize;
            with(|app| app.launcher_select_category(i));
            Value::Void
        }));
        cb("activate", Box::new(move |a| {
            let (i, admin) = (num(a, 0) as usize, matches!(a.get(1), Some(Value::Bool(true))));
            with(|app| app.launcher_activate(i, admin));
            Value::Void
        }));
        cb("context", Box::new(move |a| {
            let (i, x, y) = (num(a, 0) as usize, num(a, 1), num(a, 2));
            // After the click handler returns: the menu runs a modal loop.
            slint::Timer::single_shot(Duration::ZERO, move || {
                with(|app| app.launcher_context(i, x, y));
            });
            Value::Void
        }));
        cb("navigate", Box::new(move |a| {
            let key = text(a, 0);
            with(|app| app.launcher_navigate(&key));
            Value::Void
        }));
        cb("escape", Box::new(|_| {
            with(|app| app.launcher_escape());
            Value::Void
        }));
        cb("power", Box::new(move |a| {
            let id = text(a, 0);
            with(|app| app.launcher_power(&id));
            Value::Void
        }));
        cb("confirm-power", Box::new(|a| {
            let yes = matches!(a.first(), Some(Value::Bool(true)));
            with(|app| app.launcher_confirm(yes));
            Value::Void
        }));

        let name = session::user_display_name();
        let set = |p: &str, v: Value| {
            let _ = instance.set_global_property("Launcher", p, v);
        };
        set("user-name", string(&name));
        set("initials", string(initials(&name)));
        if let Some(img) = session::account_picture().and_then(|p| Image::load_from_path(&p).ok()) {
            set("avatar", Value::Image(img));
            set("has-avatar", Value::Bool(true));
        }
        *l.instance.borrow_mut() = Some(instance);
        true
    }

    fn show_launcher(&self, anchor: Anchor) {
        self.hide_preview();
        self.close_popup();
        if !self.ensure_launcher() {
            return;
        }
        let cfg = self.launcher_config();
        {
            let mut st = self.launcher.state.borrow_mut();
            st.query.clear();
            st.confirm = None;
            // Start on Favourites if there are any, else All applications.
            st.category = if cfg.favourites.is_empty() { 1 } else { 0 };
        }
        let rect = self.launcher_rect(&anchor, &cfg);
        {
            let inst = self.launcher.instance.borrow();
            let Some(i) = inst.as_ref() else { return };
            let st = self.state.borrow();
            theme_binding::apply(i, &st.theme, st.accent);
            let _ = i.invoke("clear-search", &[]);
            i.window().set_position(slint::PhysicalPosition::new(rect.left, rect.top));
            i.window().set_size(slint::PhysicalSize::new(rect.width() as u32, rect.height() as u32));
            if let Err(e) = i.show() {
                tracing::error!("launcher: {e}");
                return;
            }
        }
        self.launcher.visible.set(true);
        self.launcher_refresh(true);

        let l = &self.launcher;
        l.attach_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
            with(|app| app.launcher_attach(rect));
        });
    }

    /// Once the native window exists: style it, give it focus, and start watching for
    /// focus loss.
    fn launcher_attach(&self, rect: Rect) {
        let l = &self.launcher;
        let inst = l.instance.borrow();
        let Some(i) = inst.as_ref() else { return };
        let Some(hwnd) = l.hwnd.get().or_else(|| hwnd_of(i)) else { return };
        l.attach_timer.stop();
        if l.hwnd.get().is_none() {
            l.hwnd.set(Some(hwnd));
        }
        crate::popup::style_window(hwnd, rect, &self.state.borrow().theme);
        winops::activate(hwnd);
        let _ = i.invoke("focus-search", &[]);
        l.shown_at.set(Some(Instant::now()));
        l.watch_timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), || {
            with(|app| app.launcher_watch_focus());
        });
    }

    /// Close when the user clicks elsewhere (like a menu).
    fn launcher_watch_focus(&self) {
        let l = &self.launcher;
        if !l.visible.get() || l.shown_at.get().is_none_or(|t| t.elapsed() < FOCUS_GRACE) {
            return;
        }
        if crate::popup::lost_focus(winfo::foreground(), l.hwnd.get()) {
            self.hide_launcher();
        }
    }

    fn launcher_rect(&self, anchor: &Anchor, cfg: &fsh_config::LauncherConfig) -> Rect {
        let st = self.state.borrow();
        let monitors = fsh_win::monitor::monitors();
        let (cx, cy) = fsh_win::system::cursor_pos();
        let panel = match anchor {
            Anchor::Panel { key, .. } => st.panels.iter().find(|p| &p.key == key),
            Anchor::Cursor => {
                let dev = monitors.iter().find(|m| m.rect.contains_point(cx, cy)).map(|m| m.device.clone());
                st.panels.iter().find(|p| Some(&p.key.device) == dev.as_ref()).or(st.panels.first())
            }
        };
        let monitor = panel
            .map(|p| p.monitor.clone())
            .or_else(|| monitors.iter().find(|m| m.rect.contains_point(cx, cy)).cloned())
            .or_else(|| monitors.first().cloned());
        let Some(m) = monitor else { return Rect::new(100, 100, 740, 720) };
        let s = f64::from(m.scale());
        // Aligned with the button's start (Kickoff-style), not centred on it.
        let item = match anchor {
            Anchor::Panel { rect: Some([ix, iy, _, _]), .. } => Some([*ix, *iy, 0.0, 0.0]),
            _ => None,
        };
        let (w, h) = ((f64::from(cfg.width) * s) as i32, (f64::from(cfg.height) * s) as i32);
        crate::popup::place(panel, item, &m, w, h, false)
    }

    fn launcher_search(&self, query: String) {
        {
            let mut st = self.launcher.state.borrow_mut();
            st.query = query;
            st.confirm = None;
        }
        self.launcher_refresh(true);
    }

    fn launcher_select_category(&self, i: usize) {
        {
            let mut st = self.launcher.state.borrow_mut();
            st.category = i;
            st.query.clear();
        }
        if let Some(inst) = self.launcher.instance.borrow().as_ref() {
            let _ = inst.invoke("clear-search", &[]);
        }
        self.launcher_refresh(true);
        if let Some(inst) = self.launcher.instance.borrow().as_ref() {
            let _ = inst.invoke("focus-search", &[]);
        }
    }

    fn category_names(&self, st: &LauncherState, cfg: &fsh_config::LauncherConfig) -> Vec<String> {
        let mut names = vec!["Favourites".to_owned(), "All applications".to_owned()];
        if cfg.show_recent {
            names.push("Recent".to_owned());
        }
        names.extend(st.categories.iter().cloned());
        names
    }

    /// Rebuild what the launcher shows from state. `reset` also moves the selection to the
    /// top (new search or category).
    fn launcher_refresh(&self, reset: bool) {
        let cfg = self.launcher_config();
        let mut st = self.launcher.state.borrow_mut();
        let st = &mut *st;
        let names = self.category_names(st, &cfg);
        st.category = st.category.min(names.len().saturating_sub(1));
        let by_id: HashMap<&str, &AppEntry> = st.apps.iter().map(|a| (a.id.as_str(), a)).collect();

        let mut rows = Vec::new();
        let (heading, empty) = if !st.query.trim().is_empty() {
            let opts = SearchOptions { web_search: &cfg.web_search, on_path: Some(&on_path) };
            rows.extend(core::search(&st.query, &st.apps, &st.history, now(), &opts).into_iter().map(Row::Result));
            ("Results".to_owned(), "Nothing found. Try a >command, a path, or a calculation.".to_owned())
        } else {
            let name = names.get(st.category).cloned().unwrap_or_default();
            match name.as_str() {
                "Favourites" => rows.extend(cfg.favourites.iter().filter(|id| by_id.contains_key(id.as_str())).map(|id| Row::App(id.clone()))),
                "All applications" => {
                    let mut letter = String::new();
                    for a in core::sorted_apps(&st.apps) {
                        let l = core::section_letter(&a.name);
                        if l != letter {
                            rows.push(Row::Header(l.clone()));
                            letter = l;
                        }
                        rows.push(Row::App(a.id.clone()));
                    }
                }
                "Recent" if cfg.show_recent => {
                    rows.extend(st.history.recent(RECENT_COUNT).into_iter().filter(|id| by_id.contains_key(id.as_str())).map(Row::App));
                }
                cat => {
                    let mut apps: Vec<&AppEntry> = st.apps.iter().filter(|a| core::category_of(a) == cat).collect();
                    apps.sort_by_key(|a| a.name.to_lowercase());
                    rows.extend(apps.into_iter().map(|a| Row::App(a.id.clone())));
                }
            }
            let empty = match name.as_str() {
                "Favourites" => "No favourites yet. Right-click an app and choose Add to favourites.",
                "Recent" => "Apps you open will appear here.",
                _ if st.apps.is_empty() => "Loading apps…",
                _ => "",
            };
            (name, empty.to_owned())
        };

        let icon_for = |r: &SearchResult| -> Image {
            let find = |needle: &str| st.apps.iter().find(|a| a.id.to_lowercase().contains(needle)).and_then(|a| st.icons.get(&a.id)).cloned();
            match r.kind {
                ResultKind::App => st.icons.get(&r.id).cloned(),
                ResultKind::Setting => find("immersivecontrolpanel"),
                ResultKind::Calculator => find("windowscalculator"),
                ResultKind::Run => find("cmd.exe").or_else(|| find("windowsterminal")),
                ResultKind::Web => None,
            }
            .unwrap_or_default()
        };
        let items: Vec<Value> = rows
            .iter()
            .map(|row| match row {
                Row::Header(l) => strukt(vec![("id", string("")), ("title", string(l)), ("kind", string("header"))]),
                Row::App(id) => {
                    let a = by_id.get(id.as_str());
                    strukt(vec![
                        ("id", string(id)),
                        ("title", string(a.map_or(id.as_str(), |a| a.name.as_str()))),
                        ("subtitle", string("")),
                        ("icon", Value::Image(st.icons.get(id).cloned().unwrap_or_default())),
                        ("kind", string("app")),
                        ("favourite", Value::Bool(cfg.favourites.contains(id))),
                    ])
                }
                Row::Result(r) => strukt(vec![
                    ("id", string(&r.id)),
                    ("title", string(&r.title)),
                    ("subtitle", string(&r.subtitle)),
                    ("icon", Value::Image(icon_for(r))),
                    ("kind", string(match r.kind {
                        ResultKind::App => "app",
                        ResultKind::Calculator => "calculator",
                        ResultKind::Run => "run",
                        ResultKind::Setting => "setting",
                        ResultKind::Web => "web",
                    })),
                    ("favourite", Value::Bool(r.kind == ResultKind::App && cfg.favourites.contains(&r.id))),
                ]),
            })
            .collect();

        let first = rows.iter().position(|r| !matches!(r, Row::Header(_))).unwrap_or(0);
        if reset || st.selected >= rows.len() || matches!(rows.get(st.selected), Some(Row::Header(_))) {
            st.selected = first;
        }
        st.rows = rows;

        let inst = self.launcher.instance.borrow();
        let Some(i) = inst.as_ref() else { return };
        let set = |p: &str, v: Value| {
            if let Err(e) = i.set_global_property("Launcher", p, v) {
                tracing::warn!("Launcher.{p}: {e}");
            }
        };
        set("categories", model(names.iter().map(|n| strukt(vec![("name", string(n))])).collect()));
        set("category", Value::Number(st.category as f64));
        set("searching", Value::Bool(!st.query.trim().is_empty()));
        set("heading", string(&heading));
        set("empty-text", string(&empty));
        set("items", model(items));
        set("selected", Value::Number(st.selected as f64));
        set(
            "power-actions",
            model(cfg.power_actions.iter().map(|a| strukt(vec![("id", string(a)), ("label", string(power_label(a)))])).collect()),
        );
        set("confirm", string(st.confirm.map(|c| format!("{c:?}")).unwrap_or_default()));
        set(
            "confirm-text",
            string(match st.confirm {
                Some(PowerAction::Restart) => "Restart now?",
                Some(PowerAction::Shutdown) => "Shut down now?",
                Some(PowerAction::Logout) => "Sign out now?",
                _ => "",
            }),
        );
        if reset {
            let _ = i.invoke("scroll-top", &[]);
        }
    }

    fn launcher_navigate(&self, key: &str) {
        let selected = self.launcher_selected();
        let mut st = self.launcher.state.borrow_mut();
        match key {
            "next-category" | "prev-category" => {
                let n = self.category_names(&st, &self.launcher_config()).len().max(1);
                st.category = if key == "next-category" { (st.category + 1) % n } else { (st.category + n - 1) % n };
                st.query.clear();
                drop(st);
                if let Some(i) = self.launcher.instance.borrow().as_ref() {
                    let _ = i.invoke("clear-search", &[]);
                }
                self.launcher_refresh(true);
                return;
            }
            _ => {}
        }
        let selectable: Vec<usize> = st.rows.iter().enumerate().filter(|(_, r)| !matches!(r, Row::Header(_))).map(|(i, _)| i).collect();
        if selectable.is_empty() {
            return;
        }
        let pos = selectable.iter().position(|&i| i == selected).unwrap_or(0);
        let last = selectable.len() - 1;
        let new = match key {
            "up" => pos.saturating_sub(1),
            "down" => (pos + 1).min(last),
            "page-up" => pos.saturating_sub(8),
            "page-down" => (pos + 8).min(last),
            "home" => 0,
            "end" => last,
            _ => pos,
        };
        st.selected = selectable[new];
        let sel = st.selected;
        drop(st);
        if let Some(i) = self.launcher.instance.borrow().as_ref() {
            let _ = i.set_global_property("Launcher", "selected", Value::Number(sel as f64));
        }
    }

    /// The selection as the UI sees it (hovering changes it on the Slint side).
    fn launcher_selected(&self) -> usize {
        let inst = self.launcher.instance.borrow();
        match inst.as_ref().and_then(|i| i.get_global_property("Launcher", "selected").ok()) {
            Some(Value::Number(n)) if n >= 0.0 => n as usize,
            _ => self.launcher.state.borrow().selected,
        }
    }

    fn launcher_escape(&self) {
        let has_query = !self.launcher.state.borrow().query.is_empty();
        if has_query {
            if let Some(i) = self.launcher.instance.borrow().as_ref() {
                let _ = i.invoke("clear-search", &[]);
            }
            self.launcher_search(String::new());
        } else {
            self.hide_launcher();
        }
    }

    fn record_launch(&self, id: &str) {
        let mut st = self.launcher.state.borrow_mut();
        st.history.record(id, now());
        save_history(&st.history);
    }

    fn launcher_activate(&self, index: usize, admin: bool) {
        let action = {
            let st = self.launcher.state.borrow();
            match st.rows.get(index) {
                Some(Row::App(id)) => Some((ResultKind::App, id.clone())),
                Some(Row::Result(r)) => Some((r.kind, r.id.clone())),
                _ => None,
            }
        };
        let Some((kind, id)) = action else { return };
        match kind {
            ResultKind::App => {
                let target = format!(r"shell:AppsFolder\{id}");
                crate::actions::launch_verb(target, None, if admin { "runas" } else { "open" });
                self.record_launch(&id);
            }
            ResultKind::Calculator => {
                if let Err(e) = fsh_win::clipboard::set_text(&id) {
                    tracing::warn!("could not copy: {e:#}");
                }
            }
            ResultKind::Run => {
                if let Some(cmd) = id.strip_prefix('>') {
                    // In a console that stays open, so output can be read.
                    crate::actions::launch_verb("cmd.exe".into(), Some(format!("/k {cmd}")), if admin { "runas" } else { "open" });
                } else {
                    let target = expand_user_path(&id);
                    crate::actions::launch_verb(target, None, if admin { "runas" } else { "open" });
                }
            }
            ResultKind::Setting => {
                crate::actions::launch(id.clone());
                self.record_launch(&id);
            }
            ResultKind::Web => crate::actions::launch(id),
        }
        self.hide_launcher();
    }

    fn launcher_context(&self, index: usize, x: f64, y: f64) {
        let (id, app) = {
            let st = self.launcher.state.borrow();
            let id = match st.rows.get(index) {
                Some(Row::App(id)) => id.clone(),
                Some(Row::Result(r)) if r.kind == ResultKind::App => r.id.clone(),
                _ => return,
            };
            let app = st.apps.iter().find(|a| a.id == id).cloned();
            (id, app)
        };
        let Some(hwnd) = self.launcher.hwnd.get() else { return };
        let cfg = self.launcher_config();
        let favourite = cfg.favourites.contains(&id);
        let in_recent = self.launcher.state.borrow().history.entries.contains_key(&id);
        let location = app.as_ref().and_then(|a| a.path.clone());
        let packaged = id.contains('!') && !id.contains('\\');
        // Desktop apps, packaged ones included (Windows Terminal), can run elevated; UWP
        // apps can't. Without the index's answer, guess from the id.
        let elevatable = app.as_ref().and_then(|a| a.elevatable).unwrap_or(!packaged);

        const FAV: u32 = 1;
        const PIN: u32 = 2;
        const LOCATION: u32 = 3;
        const ADMIN: u32 = 4;
        const FORGET: u32 = 5;
        const UNINSTALL: u32 = 6;
        let mut items = vec![MenuItem::new(FAV, if favourite { "Remove from favourites" } else { "Add to favourites" })];
        items.push(MenuItem::new(PIN, "Pin to panel"));
        if location.is_some() {
            items.push(MenuItem::new(LOCATION, "Open file location"));
        }
        if elevatable {
            items.push(MenuItem::new(ADMIN, "Run as administrator"));
        }
        if in_recent {
            items.push(MenuItem::new(FORGET, "Remove from recent"));
        }
        items.push(MenuItem::Separator);
        items.push(MenuItem::new(UNINSTALL, "Uninstall…"));

        let (sx, sy) = {
            let r = fsh_win::panel::window_rect(hwnd).unwrap_or_default();
            let scale = f64::from(fsh_win::monitor::monitors().iter().find(|m| m.rect.contains_point(r.left, r.top)).map_or(1.0, |m| m.scale()));
            (r.left + (x * scale) as i32, r.top + (y * scale) as i32)
        };
        match menu::popup(hwnd, sx, sy, MenuAnchor::Below, &items) {
            Some(FAV) => {
                let mut list = cfg.favourites.clone();
                if favourite {
                    list.retain(|f| f != &id);
                } else {
                    list.push(id.clone());
                }
                let result = ConfigEditor::load(&config_path()).and_then(|mut ed| {
                    ed.set_launcher_list("favourites", &list);
                    ed.save(&config_path())
                });
                match result {
                    Ok(()) => self.config_written(),
                    Err(e) => tracing::error!("could not update favourites: {e}"),
                }
                self.launcher_refresh(false);
            }
            Some(PIN) => {
                // Shortcuts and executables pin best by path; Store apps by app id.
                let spec = if packaged { id.clone() } else { location.clone().unwrap_or(id.clone()) };
                let target = {
                    let st = self.state.borrow();
                    st.panels.iter().find(|p| p.tasks.widget_index.is_some()).map(|p| p.key.clone())
                };
                match target {
                    Some(key) => self.set_pinned(&key, &spec, true),
                    None => tracing::warn!("no panel has a task manager to pin to"),
                }
            }
            Some(LOCATION) => {
                if let Some(p) = location {
                    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{p}")).spawn();
                    self.hide_launcher();
                }
            }
            Some(ADMIN) => {
                crate::actions::launch_verb(format!(r"shell:AppsFolder\{id}"), None, "runas");
                self.record_launch(&id);
                self.hide_launcher();
            }
            Some(FORGET) => {
                let mut st = self.launcher.state.borrow_mut();
                st.history.forget(&id);
                save_history(&st.history);
                drop(st);
                self.launcher_refresh(false);
            }
            Some(UNINSTALL) => {
                crate::actions::launch("ms-settings:appsfeatures".into());
                self.hide_launcher();
            }
            _ => {}
        }
    }

    fn launcher_power(&self, id: &str) {
        let Some(action) = PowerAction::parse(id) else { return };
        if action.needs_confirmation() {
            self.launcher.state.borrow_mut().confirm = Some(action);
            self.launcher_refresh(false);
        } else {
            self.hide_launcher();
            do_power(action);
        }
    }

    fn launcher_confirm(&self, yes: bool) {
        let action = self.launcher.state.borrow_mut().confirm.take();
        if let (true, Some(a)) = (yes, action) {
            self.hide_launcher();
            do_power(a);
        } else {
            self.launcher_refresh(false);
        }
    }
}

fn do_power(action: PowerAction) {
    tracing::info!("power action: {action:?}");
    // Sleep blocks until the machine wakes; keep it off the UI thread.
    std::thread::spawn(move || {
        if let Err(e) = session::perform(action) {
            tracing::error!("{action:?} failed: {e:#}");
        }
    });
}

/// `~` and `%VAR%` in typed paths.
fn expand_user_path(s: &str) -> String {
    let s = match s.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", std::env::var("USERPROFILE").unwrap_or_default()),
        None => s.to_owned(),
    };
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(initials("Ash Smith"), "AS");
        assert_eq!(initials("theas"), "T");
        assert!(expand_user_path("~\\Documents").ends_with("\\Documents"));
        assert!(!expand_user_path("~\\Documents").starts_with('~'));
        assert!(on_path("cmd"), "cmd.exe is always on PATH");
        assert!(!on_path("definitely-not-a-real-program-xyz"));
        assert_eq!(power_label("shutdown"), "Shut down");
    }
}
