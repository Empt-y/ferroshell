//! The shell application: owns config, theme, widget registry and panels, and reacts to
//! system events. Lives on the UI thread; other threads reach it with
//! `slint::invoke_from_event_loop` + [`with`]. Task-manager behaviour is in `app_tasks.rs`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use fsh_common::{exit_codes, paths};
use fsh_config::{Config, LoadOutcome, Rgba, Theme};
use fsh_core::geometry;
use fsh_widgets::{Composer, Layout, Origin, Registry, WidgetStatus};
use fsh_win::monitor;
use fsh_win::window::{self, MessageWindow};
use serde_json::{Value as Json, json};
use slint::Model as _;

use crate::panel::{self, Panel, PanelKey};
use crate::taskbar::{PanelTasks, Tasks};
use crate::tracker::{Cmd, Tracker};

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Run `f` with the app, if it exists. Never holds a borrow of the global while `f` runs,
/// so callbacks may re-enter safely.
pub fn with<R>(f: impl FnOnce(&App) -> R) -> Option<R> {
    let app = APP.with(|a| a.borrow().clone());
    app.map(|a| f(&a))
}

/// Quit the event loop; `run` then exits the process with `code`.
pub fn request_exit(code: i32) {
    with(|a| a.exit_code.set(code));
    let _ = slint::quit_event_loop();
}

/// Drop the app (removing app bars) after the event loop has finished.
pub fn shutdown() {
    let app = APP.with(|a| a.borrow_mut().take());
    if let Some(app) = app {
        // Give the Windows key back and stop hosting tray icons first.
        drop(app.keyhook.borrow_mut().take());
        drop(app.tray_thread.borrow_mut().take());
        let panels = std::mem::take(&mut app.state.borrow_mut().panels);
        for p in &panels {
            p.hide();
        }
        drop(panels);
    }
}

pub fn schedule_relayout() {
    with(|a| a.schedule_relayout());
}

pub fn source_value(name: &str) -> String {
    with(|a| a.sources.borrow().get(name).cloned()).flatten().unwrap_or_default()
}

pub(crate) struct State {
    pub config: LoadOutcome,
    pub theme_name: String,
    pub theme: Theme,
    pub theme_warnings: Vec<String>,
    pub theme_override: Option<String>,
    pub accent: Option<Rgba>,
    pub registry: Registry,
    pub panels: Vec<Panel>,
    pub statuses: Vec<(String, Vec<WidgetStatus>)>,
    pub panel_errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pending {
    None,
    Config,
    Everything,
}

pub struct App {
    pub(crate) safe_mode: bool,
    /// Running as the login shell (`--replace`): we provide the desktop too.
    pub(crate) replace: bool,
    layout: Layout,
    pub(crate) composer: Composer,
    pub(crate) state: RefCell<State>,
    exit_code: Cell<i32>,
    pub(crate) tasks: RefCell<Tasks>,
    pub(crate) tracker: Tracker,
    pub(crate) desktop_shown: RefCell<Option<Vec<isize>>>,
    pub(crate) thumbnails: crate::thumbs::Thumbs,
    pub sources: RefCell<HashMap<String, String>>,
    sources_revision: Cell<i32>,
    pub(crate) scripts: crate::scripts::ScriptHost,
    pub(crate) plugins: crate::plugins::PluginHost,
    pub(crate) tray: RefCell<crate::traybar::TrayState>,
    pub(crate) tray_thread: RefCell<Option<crate::tray::TrayThread>>,
    pub(crate) tray_timer: slint::Timer,
    pub(crate) launcher: crate::launcher::Launcher,
    pub(crate) popups: crate::popup::Popups,
    pub(crate) services: crate::services::Services,
    pub(crate) osd: crate::osd::Osd,
    pub(crate) banner: crate::banner::Banner,
    pub(crate) desktop: crate::desktop::Desktop,
    app_indexer: crate::apps::AppIndexer,
    keyhook: RefCell<Option<fsh_win::keyhook::KeyHook>>,
    keyhook_features: Cell<(bool, bool)>,
    relayout_pending: Cell<bool>,
    last_relayout: Cell<Option<Instant>>,
    reload_pending: Cell<Pending>,
    fingerprints: Cell<(u64, u64)>,
    watch_timer: slint::Timer,
    attach_timer: slint::Timer,
    attach_started: Cell<Option<Instant>>,
    _events: MessageWindow,
}

impl App {
    pub fn start(safe_mode: bool, replace: bool) -> anyhow::Result<Rc<App>> {
        let layout = Layout::new(paths::state_dir().join("builtin"), paths::config_dir());
        let written = layout.extract_builtin()?;
        tracing::info!("built-in assets at {} ({written} files updated)", layout.builtin.display());

        let events = MessageWindow::new("Ferroshell Events", Box::new(on_system_message))?;
        let tracker = Tracker::spawn(|update| {
            let _ = slint::invoke_from_event_loop(move || {
                with(|a| a.on_tracker_update(update));
            });
        })?;
        let app = Rc::new(App {
            safe_mode,
            replace,
            composer: Composer::new(layout.clone()),
            layout,
            state: RefCell::new(State {
                config: LoadOutcome::Loaded(Config::defaults()),
                theme_name: String::new(),
                theme: Theme::default(),
                theme_warnings: vec![],
                theme_override: None,
                accent: None,
                registry: Registry::default(),
                panels: vec![],
                statuses: vec![],
                panel_errors: vec![],
            }),
            exit_code: Cell::new(exit_codes::QUIT),
            tasks: RefCell::default(),
            tracker,
            desktop_shown: RefCell::default(),
            thumbnails: crate::thumbs::Thumbs::default(),
            sources: RefCell::default(),
            sources_revision: Cell::new(0),
            scripts: crate::scripts::ScriptHost::spawn()?,
            plugins: crate::plugins::PluginHost::spawn()?,
            tray: RefCell::default(),
            tray_thread: RefCell::new(None),
            tray_timer: slint::Timer::default(),
            launcher: crate::launcher::Launcher::default(),
            popups: crate::popup::Popups::default(),
            services: crate::services::Services::default(),
            osd: crate::osd::Osd::default(),
            banner: crate::banner::Banner::default(),
            desktop: crate::desktop::Desktop::default(),
            app_indexer: crate::apps::AppIndexer::spawn(|u| {
                let _ = slint::invoke_from_event_loop(move || {
                    with(|a| a.on_apps_update(u));
                });
            })?,
            keyhook: RefCell::new(None),
            keyhook_features: Cell::new((false, false)),
            relayout_pending: Cell::new(false),
            last_relayout: Cell::new(None),
            reload_pending: Cell::new(Pending::None),
            fingerprints: Cell::new((0, 0)),
            watch_timer: slint::Timer::default(),
            attach_timer: slint::Timer::default(),
            attach_started: Cell::new(None),
            _events: events,
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));
        app.launcher.state.borrow_mut().history = crate::launcher::load_history();

        if replace {
            app.start_desktop();
        }
        app.load();
        app.rebuild_panels();
        app.sync_pinned();
        schedule_tick();
        if !safe_mode {
            app.fingerprints.set(app.fingerprints_now());
            app.watch_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                with(|a| a.poll_for_changes());
            });
        }
        Ok(app)
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code.get()
    }

    // ---------------------------------------------------------------- loading

    fn load(&self) {
        let mut st = self.state.borrow_mut();
        st.config = if self.safe_mode {
            LoadOutcome::Loaded(Config::defaults())
        } else {
            fsh_config::config::load(&config_path(), &paths::state_dir().join("config.lastgood.toml"))
        };
        let name = st.theme_override.clone().unwrap_or_else(|| st.config.config().theme.clone());
        let (theme_name, theme, warnings) = self.resolve_theme(&name);
        for w in &warnings {
            tracing::warn!("theme {theme_name}: {w}");
        }
        st.accent = theme.system_accent.then(system_accent).flatten();
        st.theme_name = theme_name;
        st.theme = theme;
        st.theme_warnings = warnings;

        let mut roots = vec![];
        if !self.safe_mode {
            roots.push((self.layout.user_widgets(), Origin::User));
            if let Some(dir) = self.layout.theme_dir(&st.theme_name) {
                roots.push((dir.join("widgets"), Origin::Theme));
            }
        }
        roots.push((self.layout.builtin_widgets(), Origin::Builtin));
        st.registry = Registry::scan(&roots);
        for w in &st.registry.warnings {
            tracing::warn!("{w}");
        }
        let dark = {
            let bg = st.theme.color("panel-background");
            (u32::from(bg.r) + u32::from(bg.g) + u32::from(bg.b)) < 3 * 128
        };
        fsh_win::menu::set_dark_menus(dark);
    }

    /// Returns (resolved name, theme, warnings). Never fails: problems fall back to defaults.
    fn resolve_theme(&self, requested: &str) -> (String, Theme, Vec<String>) {
        let name = match requested {
            "auto" if fsh_win::system::prefers_dark() => "breeze-dark",
            "auto" => "breeze-light",
            other => other,
        };
        if self.safe_mode {
            return ("default".into(), Theme::default(), vec![]);
        }
        let Some(dir) = self.layout.theme_dir(name) else {
            return (name.into(), Theme::default(), vec![format!("theme `{name}` not found; using defaults")]);
        };
        match std::fs::read_to_string(dir.join("theme.toml")).map_err(|e| e.to_string()).and_then(|t| Theme::parse(&t)) {
            Ok((theme, warnings)) => (name.into(), theme, warnings),
            Err(e) => (name.into(), Theme::default(), vec![format!("theme.toml: {e}; using defaults")]),
        }
    }

    // ---------------------------------------------------------------- panels

    pub fn rebuild_panels(&self) {
        let started = Instant::now();
        self.clear_popups();
        let monitors = monitor::monitors();
        {
            let mut st = self.state.borrow_mut();
            let st = &mut *st;

            let mut reusable: HashMap<PanelKey, _> =
                std::mem::take(&mut st.panels).into_iter().map(Panel::into_reusable).collect();
            st.statuses.clear();
            st.panel_errors.clear();

            let config = st.config.config().clone();
            let config_error = st.config.error().map(first_line).unwrap_or_default();
            let mut wanted = 0;
            for (ci, pc) in config.panels.iter().enumerate() {
                for mi in geometry::select_monitors(pc.monitor, &monitors) {
                    wanted += 1;
                    let m = &monitors[mi];
                    let key = PanelKey { config_index: ci, device: m.device.clone() };
                    let built = panel::create(panel::Build {
                        composer: &self.composer,
                        registry: &st.registry,
                        config: pc,
                        monitor: m,
                        reuse_appbar: reusable.remove(&key),
                        key,
                        theme: &st.theme,
                        accent: st.accent,
                        safe_mode: self.safe_mode,
                        config_error: &config_error,
                        tasks: PanelTasks::from_config(pc, &m.device),
                        tray: crate::traybar::PanelTray::from_config(pc),
                    });
                    match built {
                        Ok((p, statuses)) => {
                            self.bind_services(&p.instance);
                            st.statuses.push((p.name.clone(), statuses));
                            st.panels.push(p);
                        }
                        Err(e) => {
                            tracing::error!("panel {} on {}: {e:#}", ci + 1, m.device);
                            st.panel_errors.push(format!("panel {} on {}: {e:#}", ci + 1, m.device));
                        }
                    }
                }
            }
            drop(reusable); // app bars no longer needed are removed here
            tracing::info!("{} panel(s) built in {:?}", st.panels.len(), started.elapsed());
            if wanted > 0 && st.panels.is_empty() {
                self.fail_to_supervisor("no panel could be created");
            }
        }
        self.refresh_tasks();
        self.sync_scripts();
        self.sync_services();
        self.sync_tray();
        self.sync_keyhook();
        self.attach_started.set(Some(Instant::now()));
        self.attach_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), || {
            with(|a| a.attach_native_windows());
        });
    }

    /// Style panel windows as their native windows come into existence.
    fn attach_native_windows(&self) {
        let st = self.state.borrow();
        let pending = st.panels.iter().filter(|p| !p.attach_native(&st.theme)).count();
        if pending == 0 {
            self.attach_timer.stop();
        } else if self.attach_started.get().is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
            self.attach_timer.stop();
            let all_failed = pending == st.panels.len();
            drop(st);
            if all_failed {
                self.fail_to_supervisor("panel windows were never created (renderer failure?)");
            } else {
                tracing::error!("{pending} panel window(s) were never created");
            }
        }
    }

    /// Exit as a crash so the supervisor restarts us, escalating to safe mode (software
    /// renderer, default config) if this keeps happening. In safe mode just keep running.
    fn fail_to_supervisor(&self, why: &str) {
        if self.safe_mode {
            tracing::error!("{why} (safe mode; staying up)");
            return;
        }
        tracing::error!("{why}; exiting so the supervisor can recover");
        self.exit_code.set(1);
        let _ = slint::quit_event_loop();
    }

    fn schedule_relayout(&self) {
        if self.relayout_pending.get() {
            return;
        }
        // Our own reservation changes echo back as notifications; ignore the echo.
        if self.last_relayout.get().is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        self.relayout_pending.set(true);
        slint::Timer::single_shot(Duration::from_millis(150), || {
            with(|a| {
                a.relayout_pending.set(false);
                a.relayout();
            });
        });
    }

    /// Monitors moved or changed resolution/DPI: re-place panels; rebuild if the set of
    /// panels that should exist changed.
    fn relayout(&self) {
        let monitors = monitor::monitors();
        let needs_rebuild = {
            let st = self.state.borrow();
            let wanted: HashSet<PanelKey> = st
                .config
                .config()
                .panels
                .iter()
                .enumerate()
                .flat_map(|(ci, pc)| geometry::select_monitors(pc.monitor, &monitors).into_iter().map(move |mi| (ci, mi)))
                .map(|(ci, mi)| PanelKey { config_index: ci, device: monitors[mi].device.clone() })
                .collect();
            let existing: HashSet<PanelKey> = st.panels.iter().map(|p| p.key.clone()).collect();
            wanted != existing
        };
        if needs_rebuild {
            tracing::info!("monitor set changed; rebuilding panels");
            self.rebuild_panels();
        } else {
            let mut st = self.state.borrow_mut();
            let st = &mut *st;
            for p in &mut st.panels {
                if let Some(m) = monitors.iter().find(|m| m.device == p.key.device) {
                    p.monitor = m.clone();
                }
                p.place(&st.theme);
            }
        }
        self.last_relayout.set(Some(Instant::now()));
    }

    fn refresh_theme(&self) {
        let mut st = self.state.borrow_mut();
        let name = st.theme_override.clone().unwrap_or_else(|| st.config.config().theme.clone());
        let (theme_name, theme, _) = self.resolve_theme(&name);
        let accent = theme.system_accent.then(system_accent).flatten();
        if theme_name != st.theme_name {
            // "auto" flipped between light and dark; widgets may be overridden per theme.
            drop(st);
            self.reload();
            return;
        }
        st.theme = theme;
        st.accent = accent;
        for p in &st.panels {
            p.apply_theme(&st.theme, st.accent);
        }
    }

    /// Re-read config, theme and widgets, and rebuild the panels.
    pub fn reload(&self) {
        self.app_indexer.refresh();
        self.load();
        self.rebuild_panels();
        self.sync_pinned();
        self.fingerprints.set(self.fingerprints_now());
    }

    /// The config file changed. Rebuild panels only if something the panels are built from
    /// changed; settings the shell reads itself (pinned apps, grouping) and error-state
    /// changes are applied in place, without flicker.
    pub fn apply_config_change(&self) {
        let (old_cfg, old_theme, old_theme_name) = {
            let st = self.state.borrow();
            (st.registry.strip_shell_only(st.config.config()), st.theme.clone(), st.theme_name.clone())
        };
        self.load();
        let soft = {
            let st = self.state.borrow();
            st.registry.strip_shell_only(st.config.config()) == old_cfg
                && st.theme == old_theme
                && st.theme_name == old_theme_name
                && !st.panels.is_empty()
        };
        if !soft {
            self.rebuild_panels();
            self.sync_pinned();
            return;
        }
        tracing::info!("config change applied without rebuilding panels");
        {
            let mut st = self.state.borrow_mut();
            let st = &mut *st;
            let err = st.config.error().map(first_line).unwrap_or_default();
            let config = st.config.config().clone();
            for p in &mut st.panels {
                p.set_config_error(&err);
                if let Some(pc) = config.panels.get(p.key.config_index) {
                    p.config = pc.clone();
                    let fresh = PanelTasks::from_config(pc, &p.key.device);
                    p.tasks.update_config(fresh);
                    p.tray.update_config(crate::traybar::PanelTray::from_config(pc));
                }
            }
        }
        self.sync_pinned();
        self.sync_scripts();
        self.sync_tray();
        self.sync_keyhook();
        self.refresh_tasks();
    }

    /// Tell the tracker about every pinned app across all panels.
    pub(crate) fn sync_pinned(&self) {
        let specs: Vec<String> = {
            let st = self.state.borrow();
            let mut v: Vec<String> = st.panels.iter().flat_map(|p| p.tasks.pinned_specs.iter().cloned()).collect();
            v.sort();
            v.dedup();
            v
        };
        self.tracker.send(Cmd::SetPinned(specs));
    }

    /// Start/stop/restart widget scripts and plugins to match the widgets on the panels.
    fn sync_scripts(&self) {
        let st = self.state.borrow();
        let mut list = Vec::new();
        let mut plugins: Vec<crate::plugins::PluginSpec> = Vec::new();
        for p in &st.panels {
            let Some((_, statuses)) = st.statuses.iter().find(|(n, _)| n == &p.name) else { continue };
            for (entry, status) in p.config.widgets.iter().zip(statuses) {
                if status.error.is_some() {
                    continue;
                }
                let Ok(pkg) = st.registry.get(&entry.id) else { continue };
                if let Some(plugin) = pkg.manifest.plugin.as_ref().filter(|_| !self.safe_mode) {
                    let instance = (status.instance.clone(), entry.settings.clone());
                    match plugins.iter_mut().find(|p| p.id == entry.id) {
                        Some(p) => p.instances.push(instance),
                        None => plugins.push(crate::plugins::PluginSpec {
                            id: entry.id.clone(),
                            dir: pkg.dir.clone(),
                            exec: plugin.exec.clone(),
                            args: plugin.args.clone(),
                            max_memory_mb: plugin.max_memory_mb,
                            instances: vec![instance],
                        }),
                    }
                }
                if let Some(script) = &pkg.manifest.script {
                    list.push(crate::scripts::ScriptInstance {
                        instance: status.instance.clone(),
                        widget_id: entry.id.clone(),
                        path: pkg.dir.join(&script.file),
                        interval: script.interval,
                        settings: entry.settings.clone(),
                    });
                }
            }
        }
        self.scripts.set(list);
        self.plugins.set(plugins);
    }

    /// A script or plugin published a value; widgets bound to `Shell.source` re-evaluate.
    pub fn set_source(&self, key: String, value: String) {
        let changed = self.sources.borrow_mut().insert(key, value.clone()).as_deref() != Some(value.as_str());
        if changed {
            let rev = self.sources_revision.get().wrapping_add(1);
            self.sources_revision.set(rev);
            for p in &self.state.borrow().panels {
                p.set_sources_revision(rev);
            }
        }
    }

    /// Where overrides of `@ferroshell` library files are looked for, highest priority
    /// first: the user's `library` folder, then the theme's. None in safe mode.
    /// The shell's hidden event window (system broadcasts, power-setting notifications).
    pub(crate) fn events_hwnd(&self) -> fsh_win::Hwnd {
        self._events.hwnd()
    }

    pub(crate) fn library_overrides(&self) -> Vec<std::path::PathBuf> {
        if self.safe_mode {
            return vec![];
        }
        let mut dirs = vec![paths::config_dir().join("library")];
        if let Some(t) = self.layout.theme_dir(&self.state.borrow().theme_name) {
            dirs.push(t.join("library"));
        }
        dirs
    }

    /// The keyboard hook: the Windows key opens our launcher while `[launcher] windows-key`
    /// is on, and the volume and media keys are ours while we replace Explorer. Never in
    /// safe mode.
    fn sync_keyhook(&self) {
        let win = !self.safe_mode && self.state.borrow().config.config().launcher.windows_key;
        let media = crate::osd::media_keys_wanted(self.safe_mode);
        let wanted = win || media;
        let running = self.keyhook.borrow().is_some();
        if wanted && !running {
            match fsh_win::keyhook::KeyHook::start(|e| {
                let _ = slint::invoke_from_event_loop(move || {
                    with(|a| a.on_key_event(e));
                });
            }) {
                Ok(h) => *self.keyhook.borrow_mut() = Some(h),
                Err(e) => tracing::error!("could not install the keyboard hook: {e:#}"),
            }
        } else if !wanted && running {
            drop(self.keyhook.borrow_mut().take());
        }
        if let Some(h) = self.keyhook.borrow().as_ref() {
            h.set_features(win, media);
        }
        let now = (win, media);
        if self.keyhook_features.replace(now) != now {
            tracing::info!(
                "Windows key: {}; volume and media keys: {}",
                if win { "opens the launcher" } else { "Windows" },
                if media { "Ferroshell" } else { "Windows" }
            );
        }
    }

    /// `Shell.invoke` from a widget on a panel: like `actions::invoke`, but actions that
    /// open something next to the widget know where it is.
    pub fn invoke_from_panel(&self, key: &PanelKey, action: &str, arg: &str) {
        if action == "launcher" {
            let v: Vec<f64> = arg.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            let rect = (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]]);
            self.toggle_launcher(crate::launcher::Anchor::Panel { key: key.clone(), rect });
        } else if action == "popup" {
            self.toggle_popup(key, arg);
        } else if action == "close-popup" {
            self.close_popup();
        } else {
            crate::actions::invoke(action, arg);
        }
    }

    pub fn set_theme_override(&self, name: Option<String>) {
        self.state.borrow_mut().theme_override = name;
        self.reload();
    }

    /// (config file, everything else the panels are built from)
    fn fingerprints_now(&self) -> (u64, u64) {
        let theme_dir = self.layout.theme_dir(&self.state.borrow().theme_name);
        let mut assets = vec![self.layout.user_widgets(), self.layout.user_themes()];
        assets.extend(theme_dir);
        (crate::watch::fingerprint(&[config_path()]), crate::watch::fingerprint(&assets))
    }

    fn poll_for_changes(&self) {
        let (cfg, assets) = self.fingerprints_now();
        let (old_cfg, old_assets) = self.fingerprints.get();
        let change = if assets != old_assets {
            Pending::Everything
        } else if cfg != old_cfg {
            Pending::Config
        } else {
            return;
        };
        self.fingerprints.set((cfg, assets));
        let was = self.reload_pending.get();
        self.reload_pending.set(was.max(change));
        if was != Pending::None {
            return;
        }
        // Let editors finish writing (several saves in quick succession) before reloading.
        slint::Timer::single_shot(Duration::from_millis(300), || {
            with(|a| {
                let what = a.reload_pending.replace(Pending::None);
                tracing::info!("changes on disk ({what:?}); reloading");
                match what {
                    Pending::Everything => a.reload(),
                    Pending::Config => a.apply_config_change(),
                    Pending::None => {}
                }
            });
        });
    }

    /// Call after writing config.toml ourselves, so the watcher doesn't reload it again.
    pub(crate) fn config_written(&self) {
        self.fingerprints.set(self.fingerprints_now());
        self.apply_config_change();
    }

    fn tick(&self) {
        let now = chrono::Utc::now().timestamp();
        for p in &self.state.borrow().panels {
            p.set_now(now);
        }
        self.popup_tick(now);
        self.notifications_tick(now);
    }

    // ---------------------------------------------------------------- introspection

    pub fn dump_state(&self) -> Json {
        let st = self.state.borrow();
        let statuses: HashMap<&str, &Vec<WidgetStatus>> = st.statuses.iter().map(|(n, s)| (n.as_str(), s)).collect();
        let tasks = self.tasks.borrow();
        json!({
            "safe_mode": self.safe_mode,
            "replace": self.replace,
            "desktop": self.desktop_state(),
            "identity": fsh_win::identity::package_full_name(),
            "config_error": st.config.error(),
            "config_from_last_good": matches!(st.config, LoadOutcome::Fallback { from_last_good: true, .. }),
            "theme": { "name": st.theme_name, "display_name": st.theme.name, "warnings": st.theme_warnings,
                       "override": st.theme_override, "system_accent": st.accent.map(|a| a.to_string()) },
            "panels": st.panels.iter().map(|p| json!({
                "name": p.name,
                "monitor": p.monitor.device,
                "edge": format!("{:?}", p.config.edge).to_lowercase(),
                "rect": [p.rect.left, p.rect.top, p.rect.right, p.rect.bottom],
                "scale": p.monitor.scale(),
                "fullscreen_app": p.fullscreen_app.get(),
                "tasks": p.tasks.model.row_count(),
                "pinned": p.tasks.pinned_specs,
                "widgets": statuses.get(p.name.as_str()).map(|ws| ws.iter().map(|w| json!({
                    "id": w.id,
                    "origin": w.origin.map(|o| format!("{o:?}").to_lowercase()),
                    "error": w.error,
                    "warnings": w.warnings,
                })).collect::<Vec<_>>()),
            })).collect::<Vec<_>>(),
            "panel_errors": st.panel_errors,
            "windows": tasks.windows.iter().map(|w| json!({
                "hwnd": w.hwnd, "title": w.title, "group": w.group, "icon": w.icon,
                "has_icon": tasks.icons.contains_key(&w.icon), "minimized": w.minimized,
                "attention": w.attention, "monitor": w.monitor,
            })).collect::<Vec<_>>(),
            "foreground": tasks.foreground,
            "sources": *self.sources.borrow(),
            "plugins": self.plugins.status(),
            "launcher": self.launcher_state(),
            "popup": self.popup_state(),
            "services": self.services_state(),
            "tray": {
                "hosting": self.tray_thread.borrow().is_some(),
                "icons": self.tray.borrow().model.icons().iter().map(|i| json!({
                    "id": i.key.id(), "tip": i.tip, "hidden": i.hidden, "version": i.version, "owner": i.owner,
                })).collect::<Vec<_>>(),
            },
            "pinned_resolved": tasks.pinned.values().map(|p| json!({
                "spec": p.spec, "group": p.group, "title": p.title, "launch": p.launch,
                "has_icon": tasks.icons.contains_key(&p.icon),
            })).collect::<Vec<_>>(),
            "widgets_installed": st.registry.packages().map(|p| json!({
                "id": p.manifest.id, "name": p.manifest.name, "version": p.manifest.version,
                "origin": format!("{:?}", p.origin).to_lowercase(), "dir": p.dir,
            })).collect::<Vec<_>>(),
            "widgets_broken": st.registry.broken().map(|(id, e)| json!({"id": id, "error": e})).collect::<Vec<_>>(),
        })
    }
}

pub(crate) fn config_path() -> std::path::PathBuf {
    paths::config_dir().join("config.toml")
}

pub(crate) fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_owned()
}

fn system_accent() -> Option<Rgba> {
    fsh_win::system::accent_color().map(|(r, g, b)| Rgba::rgb(r, g, b))
}

/// Tick at the start of every second so clocks change exactly on time.
fn schedule_tick() {
    let millis = 1000 - (chrono::Utc::now().timestamp_subsec_millis() % 1000) + 5;
    slint::Timer::single_shot(Duration::from_millis(u64::from(millis)), || {
        with(|a| a.tick());
        schedule_tick();
    });
}

fn on_system_message(_hwnd: fsh_win::Hwnd, msg: u32, _wparam: usize, lparam: isize) -> Option<isize> {
    match msg {
        window::WM_DISPLAYCHANGE => {
            tracing::info!("display configuration changed");
            // Let Windows settle (several messages arrive while monitors reconfigure).
            slint::Timer::single_shot(Duration::from_millis(500), || {
                with(|a| a.relayout());
            });
        }
        window::WM_SETTINGCHANGE => {
            if window::setting_change_area(lparam).as_deref() == Some("ImmersiveColorSet") {
                slint::Timer::single_shot(Duration::from_millis(200), || {
                    with(|a| a.refresh_theme());
                });
            }
        }
        fsh_win::power::WM_POWERBROADCAST if _wparam == fsh_win::power::PBT_POWERSETTINGCHANGE as usize => {
            // `lparam` is only valid during this message: parse it now.
            if let Some(setting) = fsh_win::power::parse_power_setting(lparam) {
                with(|a| a.on_power_setting(setting));
            }
            return Some(1);
        }
        window::WM_DWMCOLORIZATIONCOLORCHANGED => {
            slint::Timer::single_shot(Duration::from_millis(200), || {
                with(|a| a.refresh_theme());
            });
        }
        _ => return None,
    }
    Some(0)
}
