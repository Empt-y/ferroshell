//! The window tracker thread. It owns the WinEvent and shell hooks, and does every query
//! that touches other applications' windows (titles, icons, app ids), so a hung
//! application can at worst delay task updates — never freeze the panel.
//!
//! Events only mark the list dirty; a short timer then rescans all top-level windows
//! (cheap, and immune to missed or out-of-order events). A slower periodic rescan catches
//! anything events don't report, such as icon changes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use fsh_core::tasks::{PinnedApp, TrackedWindow, group_key, is_task_window};
use fsh_win::icon::{self, RgbaImage};
use fsh_win::shellhook::{self, ShellHook};
use fsh_win::window::{self, MessageWindow, WM_APP, WM_TIMER};
use fsh_win::winevent::{WinEvent, WinEventHooks};
use fsh_win::{Hwnd, com::ComGuard, winfo, winops};

const TIMER_FLUSH: usize = 1;
const TIMER_RESCAN: usize = 2;
const WM_WAKE: u32 = WM_APP + 1;
const FLUSH_DELAY_MS: u32 = 40;
const RESCAN_MS: u32 = 3000;
const ICON_TIMEOUT_MS: u32 = 150;
const ICON_SIZE: i32 = 64;

pub enum Cmd {
    /// Resolve these pinned-app specs (shortcuts, paths, app ids) and group windows with them.
    SetPinned(Vec<String>),
    /// Rescan now (e.g. after we activated or minimized something).
    Refresh,
}

#[derive(Debug, Default)]
pub struct Update {
    pub windows: Vec<TrackedWindow>,
    pub foreground: Option<isize>,
    /// Newly available icons, by key.
    pub icons: Vec<(String, RgbaImage)>,
    /// Set when the pinned list was (re)resolved.
    pub pinned: Option<HashMap<String, PinnedApp>>,
}

pub struct Tracker {
    tx: Sender<Cmd>,
    wake: Hwnd,
}

impl Tracker {
    /// Start the thread. `on_update` runs on the tracker thread.
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let (hwnd_tx, hwnd_rx) = channel();
        std::thread::Builder::new().name("window-tracker".into()).spawn(move || {
            let _com = ComGuard::new();
            match run(rx, Box::new(on_update), &hwnd_tx) {
                Ok(()) => tracing::info!("window tracker stopped"),
                Err(e) => tracing::error!("window tracker failed: {e:#}"),
            }
        })?;
        let wake = hwnd_rx.recv()?;
        Ok(Self { tx, wake })
    }

    pub fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            self.wake.post(WM_WAKE, 0, 0);
        }
    }
}

struct Known {
    seq: u64,
    /// Group from the app's own identity; `group` may instead be a pinned app's group.
    base_group: String,
    group: String,
    exe: Option<String>,
    launch: Option<String>,
    pin_spec: Option<String>,
    icon: String,
}

struct State {
    on_update: Box<dyn Fn(Update)>,
    own_pid: u32,
    seq: u64,
    known: HashMap<isize, Known>,
    exe_paths: HashMap<u32, Option<String>>,
    attention: HashSet<isize>,
    sent_icons: HashSet<String>,
    pinned: HashMap<String, PinnedApp>,
    /// Pinned groups keyed by executable path, so an app pinned by path still matches
    /// windows that set an explicit app id.
    pinned_by_exe: HashMap<String, String>,
    last: Option<(Vec<TrackedWindow>, Option<isize>)>,
    flush_pending: bool,
}

fn run(rx: Receiver<Cmd>, on_update: Box<dyn Fn(Update)>, hwnd_tx: &Sender<Hwnd>) -> anyhow::Result<()> {
    let state = Rc::new(RefCell::new(State {
        on_update,
        own_pid: std::process::id(),
        seq: 0,
        known: HashMap::new(),
        exe_paths: HashMap::new(),
        attention: HashSet::new(),
        sent_icons: HashSet::new(),
        pinned: HashMap::new(),
        pinned_by_exe: HashMap::new(),
        last: None,
        flush_pending: false,
    }));

    let shell_msg = Rc::new(std::cell::Cell::new(0u32));
    let (st, sm) = (state.clone(), shell_msg.clone());
    let win = MessageWindow::new(
        "Ferroshell Tracker",
        Box::new(move |hwnd, msg, wparam, lparam| {
            match msg {
                WM_TIMER if wparam == TIMER_FLUSH => {
                    hwnd.kill_timer(TIMER_FLUSH);
                    flush(&st);
                }
                WM_TIMER if wparam == TIMER_RESCAN => flush(&st),
                WM_WAKE => {
                    while let Ok(cmd) = rx.try_recv() {
                        match cmd {
                            Cmd::SetPinned(specs) => resolve_pinned(&st, &specs),
                            Cmd::Refresh => {}
                        }
                    }
                    schedule(&st, hwnd, FLUSH_DELAY_MS);
                }
                m if m != 0 && m == sm.get() => {
                    let target = lparam;
                    match wparam {
                        shellhook::HSHELL_FLASH => {
                            st.borrow_mut().attention.insert(target);
                        }
                        shellhook::HSHELL_WINDOWACTIVATED | shellhook::HSHELL_RUDEAPPACTIVATED => {
                            st.borrow_mut().attention.remove(&target);
                        }
                        _ => {}
                    }
                    schedule(&st, hwnd, FLUSH_DELAY_MS);
                }
                _ => return None,
            }
            Some(0)
        }),
    )?;
    let hwnd = win.hwnd();
    let hook = ShellHook::register(hwnd);
    if let Some(h) = &hook {
        shell_msg.set(h.message);
    } else {
        tracing::warn!("shell hook unavailable; attention highlighting disabled");
    }

    let st = state.clone();
    let _hooks = WinEventHooks::install(move |ev, _target| {
        // Foreground changes should feel instant; everything else can batch. Never scan
        // directly from a hook callback: scans wait on other windows and could re-enter.
        schedule(&st, hwnd, if ev == WinEvent::Foreground { 1 } else { FLUSH_DELAY_MS });
    });

    // Hand our window back before the first scan, which may be slow.
    let _ = hwnd_tx.send(hwnd);
    hwnd.set_timer(TIMER_RESCAN, RESCAN_MS);
    schedule(&state, hwnd, 1);
    window::run_message_loop();
    Ok(())
}

fn schedule(st: &Rc<RefCell<State>>, hwnd: Hwnd, delay_ms: u32) {
    let Ok(mut s) = st.try_borrow_mut() else {
        // Mid-scan: the scan in progress will pick the change up, and the periodic rescan
        // is the backstop.
        return;
    };
    if !s.flush_pending || delay_ms < FLUSH_DELAY_MS {
        s.flush_pending = true;
        hwnd.set_timer(TIMER_FLUSH, delay_ms);
    }
}

/// Rescan windows and report if anything changed.
fn flush(st: &Rc<RefCell<State>>) {
    let Ok(mut guard) = st.try_borrow_mut() else { return };
    let s = &mut *guard;
    s.flush_pending = false;
    let foreground = winfo::foreground().map(|h| h.0);
    if let Some(fg) = foreground {
        s.attention.remove(&fg);
    }

    let mut seen = HashSet::new();
    let mut windows = Vec::new();
    for hwnd in winfo::top_level_windows() {
        let Some(info) = winfo::info(hwnd) else { continue };
        if !is_task_window(&info, s.own_pid) {
            continue;
        }
        seen.insert(hwnd.0);
        if !s.known.contains_key(&hwnd.0) {
            s.seq += 1;
            let known = identify(s, &info);
            s.known.insert(hwnd.0, known);
        }
        let k = &s.known[&hwnd.0];
        windows.push((
            k.seq,
            TrackedWindow {
                hwnd: hwnd.0,
                title: info.title,
                group: k.group.clone(),
                launch: k.launch.clone(),
                pin_spec: k.pin_spec.clone(),
                icon: k.icon.clone(),
                minimized: info.minimized,
                attention: s.attention.contains(&hwnd.0),
                monitor: winfo::monitor_device(hwnd).unwrap_or_default(),
            },
        ));
    }
    s.known.retain(|h, _| seen.contains(h));
    s.attention.retain(|h| seen.contains(h));
    windows.sort_by_key(|(seq, _)| *seq);
    let windows: Vec<TrackedWindow> = windows.into_iter().map(|(_, w)| w).collect();

    // Fetch icons we haven't sent yet. Window icons can fail (hung app, no icon), in which
    // case the executable's icon is used instead.
    let mut icons = Vec::new();
    for w in &windows {
        if s.sent_icons.contains(&w.icon) {
            continue;
        }
        let img = if w.icon.starts_with("win:") {
            let responsive = winfo::is_responsive(Hwnd(w.hwnd), 100);
            responsive.then(|| icon::window_icon(Hwnd(w.hwnd), ICON_TIMEOUT_MS)).flatten().or_else(|| {
                s.known.get(&w.hwnd).and_then(|k| k.exe.as_deref()).and_then(|exe| icon::shell_item_icon(exe, ICON_SIZE))
            })
        } else {
            w.icon.strip_prefix("item:").and_then(|p| icon::shell_item_icon(p, ICON_SIZE))
        };
        s.sent_icons.insert(w.icon.clone());
        if let Some(img) = img {
            icons.push((w.icon.clone(), img));
        }
    }
    // Forget icons of closed windows so a reused handle gets a fresh icon.
    let live: HashSet<&str> = windows.iter().map(|w| w.icon.as_str()).collect();
    let pinned_icons: HashSet<&str> = s.pinned.values().map(|p| p.icon.as_str()).collect();
    s.sent_icons.retain(|k| live.contains(k.as_str()) || pinned_icons.contains(k.as_str()));

    let snapshot = (windows, foreground);
    if icons.is_empty() && s.last.as_ref() == Some(&snapshot) {
        return;
    }
    s.last = Some(snapshot.clone());
    let update = Update { windows: snapshot.0, foreground: snapshot.1, icons, pinned: None };
    drop(guard);
    (st.borrow().on_update)(update);
}

/// Work out a new window's app identity: grouping key, how to launch another instance,
/// what to pin, and where its icon comes from.
fn identify(s: &mut State, info: &winfo::WindowInfo) -> Known {
    let hwnd = info.hwnd;
    // Store apps live in ApplicationFrameHost; the real app is its CoreWindow's process.
    let app_pid = if info.class == "ApplicationFrameWindow" {
        winfo::uwp_core_window(hwnd).map(winfo::pid).unwrap_or(info.pid)
    } else {
        info.pid
    };
    let package_id = winfo::process_app_id(app_pid);
    // Looking up a window's app id can message it; skip unresponsive windows (they're
    // grouped by executable instead).
    let explicit_id = if winfo::is_responsive(hwnd, 100) { winfo::window_app_id(hwnd) } else { None };
    let exe = s.exe_paths.entry(app_pid).or_insert_with(|| winfo::process_path(app_pid)).clone();

    let app_id = package_id.clone().or(explicit_id);
    let base_group = group_key(app_id.as_deref(), exe.as_deref(), hwnd.0);
    let group = pinned_group(&s.pinned_by_exe, exe.as_deref()).unwrap_or_else(|| base_group.clone());
    match package_id {
        Some(id) => Known {
            seq: s.seq,
            base_group,
            group,
            exe,
            launch: Some(format!(r"shell:AppsFolder\{id}")),
            pin_spec: Some(id.clone()),
            icon: format!(r"item:shell:AppsFolder\{id}"),
        },
        None => Known {
            seq: s.seq,
            base_group,
            group,
            launch: exe.clone(),
            pin_spec: exe.clone(),
            exe,
            icon: format!("win:{}", hwnd.0),
        },
    }
}

fn pinned_group(by_exe: &HashMap<String, String>, exe: Option<&str>) -> Option<String> {
    exe.and_then(|e| by_exe.get(&e.to_lowercase())).cloned()
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
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

fn stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_owned())
}

/// Turn config pinned entries into app identities. Shortcuts are read for their target
/// and app id; bare app ids are Store/packaged apps.
fn resolve_pinned(st: &Rc<RefCell<State>>, specs: &[String]) {
    let mut resolved = HashMap::new();
    let mut by_exe = HashMap::new();
    let mut icons = Vec::new();
    for spec in specs {
        if resolved.contains_key(spec) {
            continue;
        }
        let path = expand_env(spec.trim());
        let lower = path.to_lowercase();
        let (group, launch, title, icon_src, exe) = if lower.ends_with(".lnk") {
            let sc = winops::resolve_shortcut(&path);
            let target = sc.as_ref().and_then(|s| s.target.clone());
            let group = group_key(sc.as_ref().and_then(|s| s.app_id.as_deref()), target.as_deref(), 0);
            (group, path.clone(), stem(&path), path.clone(), target)
        } else if lower.contains('\\') || lower.ends_with(".exe") {
            (group_key(None, Some(&path), 0), path.clone(), stem(&path), path.clone(), Some(path.clone()))
        } else {
            let launch = format!(r"shell:AppsFolder\{path}");
            let title = winops::display_name(&launch).unwrap_or_else(|| path.clone());
            (group_key(Some(&path), None, 0), launch.clone(), title, launch, None)
        };
        if let Some(exe) = exe {
            by_exe.insert(exe.to_lowercase(), group.clone());
        }
        let icon = format!("item:{icon_src}");
        if let Some(img) = icon::shell_item_icon(&icon_src, ICON_SIZE) {
            icons.push((icon.clone(), img));
        }
        resolved.insert(spec.clone(), PinnedApp { spec: spec.clone(), group, launch, title, icon });
    }

    {
        let mut guard = st.borrow_mut();
        let s = &mut *guard;
        s.pinned = resolved.clone();
        s.pinned_by_exe = by_exe;
        for (k, _) in &icons {
            s.sent_icons.insert(k.clone());
        }
        // Windows of an app pinned by path join the pinned entry's group.
        for k in s.known.values_mut() {
            k.group = pinned_group(&s.pinned_by_exe, k.exe.as_deref()).unwrap_or_else(|| k.base_group.clone());
        }
        s.last = None;
    }
    (st.borrow().on_update)(Update { icons, pinned: Some(resolved), ..Default::default() });
    flush(st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_environment_variables() {
        // SAFETY-free: set_var is unsafe in edition 2024, so use a variable Windows always has.
        let windir = std::env::var("WINDIR").unwrap();
        assert_eq!(expand_env(r"%WINDIR%\explorer.exe"), format!(r"{windir}\explorer.exe"));
        assert_eq!(expand_env("100% sure"), "100% sure");
        assert_eq!(expand_env("%NOPE_NOT_SET%\\x"), "%NOPE_NOT_SET%\\x");
    }
}
