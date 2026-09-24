//! Which windows get a task entry, and how windows and pinned apps combine into the list
//! the task manager shows. Pure functions over plain data, so all of it is unit-tested.

use std::collections::HashMap;

use fsh_win::winfo::WindowInfo;

/// Window classes that are never user-facing tasks.
const EXCLUDED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "ApplicationManager_ImmersiveShellWindow",
    "Windows.Internal.Shell.TabProxyWindow",
];

/// Does this top-level window belong in the task manager? Mirrors Explorer's rules:
/// visible, not cloaked (other virtual desktops, suspended UWP), not owned, not a tool
/// window — unless the app explicitly asked for a taskbar button (`WS_EX_APPWINDOW`).
pub fn is_task_window(w: &WindowInfo, own_pid: u32) -> bool {
    if !w.visible || w.cloaked || w.pid == own_pid {
        return false;
    }
    if EXCLUDED_CLASSES.contains(&w.class.as_str()) {
        return false;
    }
    if w.app_window {
        return true;
    }
    if w.tool_window || w.no_activate || w.has_owner {
        return false;
    }
    !w.title.trim().is_empty()
}

/// Normalised grouping key: explicit/package app id, else executable path.
pub fn group_key(app_id: Option<&str>, exe: Option<&str>, hwnd: isize) -> String {
    match (app_id, exe) {
        (Some(id), _) if !id.is_empty() => format!("app:{}", id.to_lowercase()),
        (_, Some(path)) if !path.is_empty() => format!("exe:{}", path.to_lowercase()),
        _ => format!("hwnd:{hwnd}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackedWindow {
    pub hwnd: isize,
    pub title: String,
    pub group: String,
    /// How to start another instance (app id target or exe path).
    pub launch: Option<String>,
    /// What to write to config to pin this app.
    pub pin_spec: Option<String>,
    pub icon: String,
    pub minimized: bool,
    pub attention: bool,
    pub monitor: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PinnedApp {
    /// The entry exactly as written in config.
    pub spec: String,
    pub group: String,
    pub launch: String,
    pub title: String,
    pub icon: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub active: bool,
    pub minimized: bool,
    pub attention: bool,
    pub pinned: bool,
    pub running: bool,
    /// Windows in this entry, oldest first.
    pub windows: Vec<isize>,
    pub launch: Option<String>,
    pub pin_spec: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskOptions {
    pub group: bool,
    /// Only windows on this monitor (device name).
    pub monitor: Option<String>,
}

fn view(id: String, wins: &[&TrackedWindow], pinned: Option<&PinnedApp>, foreground: Option<isize>) -> TaskView {
    let running = !wins.is_empty();
    let title = match (wins, pinned) {
        ([only], _) => only.title.clone(),
        (_, Some(p)) => p.title.clone(),
        ([first, ..], None) => first.title.clone(),
        ([], None) => String::new(),
    };
    TaskView {
        id,
        title,
        icon: pinned.map(|p| p.icon.clone()).or_else(|| wins.first().map(|w| w.icon.clone())).unwrap_or_default(),
        active: wins.iter().any(|w| Some(w.hwnd) == foreground),
        minimized: running && wins.iter().all(|w| w.minimized),
        attention: wins.iter().any(|w| w.attention),
        pinned: pinned.is_some(),
        running,
        windows: wins.iter().map(|w| w.hwnd).collect(),
        launch: pinned.map(|p| p.launch.clone()).or_else(|| wins.iter().find_map(|w| w.launch.clone())),
        pin_spec: pinned.map(|p| p.spec.clone()).or_else(|| wins.iter().find_map(|w| w.pin_spec.clone())),
    }
}

/// Combine windows (in first-seen order) and pinned apps into task entries: pinned apps
/// first in their configured order, then other apps in the order they were opened.
pub fn build_tasks(
    windows: &[TrackedWindow],
    pinned: &[PinnedApp],
    foreground: Option<isize>,
    opts: &TaskOptions,
) -> Vec<TaskView> {
    let windows: Vec<&TrackedWindow> =
        windows.iter().filter(|w| opts.monitor.as_ref().is_none_or(|m| &w.monitor == m)).collect();

    let mut by_group: HashMap<&str, Vec<&TrackedWindow>> = HashMap::new();
    let mut group_order: Vec<&str> = Vec::new();
    for w in &windows {
        let e = by_group.entry(w.group.as_str()).or_default();
        if e.is_empty() {
            group_order.push(w.group.as_str());
        }
        e.push(w);
    }

    let mut out = Vec::new();
    let mut placed: std::collections::HashSet<&str> = Default::default();
    for p in pinned {
        if !placed.insert(p.group.as_str()) {
            continue; // the same app pinned twice
        }
        let wins = by_group.get(p.group.as_str()).cloned().unwrap_or_default();
        if opts.group || wins.is_empty() {
            out.push(view(format!("g:{}", p.group), &wins, Some(p), foreground));
        } else {
            for w in wins {
                out.push(view(format!("w:{}", w.hwnd), &[w], Some(p), foreground));
            }
        }
    }
    for g in group_order {
        if placed.contains(g) {
            continue;
        }
        let wins = &by_group[g];
        if opts.group {
            out.push(view(format!("g:{g}"), wins, None, foreground));
        } else {
            for w in wins {
                out.push(view(format!("w:{}", w.hwnd), &[*w], None, foreground));
            }
        }
    }
    out
}

/// What clicking a task should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickAction {
    Launch(String),
    Activate(isize),
    Minimize(Vec<isize>),
    Nothing,
}

/// Plasma/Windows behaviour: a launcher starts the app; a single window toggles between
/// focused and minimized; a group cycles through its windows, starting with the most
/// recent one if none is focused.
pub fn click_action(task: &TaskView, foreground: Option<isize>) -> ClickAction {
    if !task.running {
        return task.launch.clone().map_or(ClickAction::Nothing, ClickAction::Launch);
    }
    let fg_index = task.windows.iter().position(|w| Some(*w) == foreground);
    match (task.windows.as_slice(), fg_index) {
        ([only], Some(_)) => ClickAction::Minimize(vec![*only]),
        ([only], None) => ClickAction::Activate(*only),
        (wins, Some(i)) => ClickAction::Activate(wins[(i + 1) % wins.len()]),
        (wins, None) => wins.last().map_or(ClickAction::Nothing, |w| ClickAction::Activate(*w)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsh_win::Hwnd;

    fn info(title: &str) -> WindowInfo {
        WindowInfo {
            hwnd: Hwnd(1),
            pid: 100,
            title: title.into(),
            class: "Notepad".into(),
            visible: true,
            cloaked: false,
            minimized: false,
            tool_window: false,
            app_window: false,
            no_activate: false,
            has_owner: false,
        }
    }

    #[test]
    fn classification() {
        assert!(is_task_window(&info("Untitled - Notepad"), 1));
        assert!(!is_task_window(&info("x"), 100), "our own windows");
        assert!(!is_task_window(&info(""), 1), "untitled helper windows");
        assert!(!is_task_window(&WindowInfo { visible: false, ..info("x") }, 1));
        assert!(!is_task_window(&WindowInfo { cloaked: true, ..info("x") }, 1), "other desktops");
        assert!(!is_task_window(&WindowInfo { tool_window: true, ..info("x") }, 1));
        assert!(!is_task_window(&WindowInfo { has_owner: true, ..info("dialog") }, 1));
        assert!(!is_task_window(&WindowInfo { class: "Progman".into(), ..info("Program Manager") }, 1));
        // WS_EX_APPWINDOW forces a button even for tool/owned/untitled windows.
        assert!(is_task_window(&WindowInfo { app_window: true, tool_window: true, has_owner: true, ..info("") }, 1));
    }

    #[test]
    fn group_keys() {
        assert_eq!(group_key(Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Some("x"), 1), "app:microsoft.windowscalculator_8wekyb3d8bbwe!app");
        assert_eq!(group_key(None, Some(r"C:\Windows\notepad.exe"), 1), r"exe:c:\windows\notepad.exe");
        assert_eq!(group_key(Some(""), None, 7), "hwnd:7");
    }

    fn win(hwnd: isize, group: &str, title: &str) -> TrackedWindow {
        TrackedWindow {
            hwnd,
            title: title.into(),
            group: group.into(),
            launch: Some(format!("launch-{group}")),
            pin_spec: Some(format!("spec-{group}")),
            icon: format!("icon-{hwnd}"),
            minimized: false,
            attention: false,
            monitor: "M1".into(),
        }
    }

    fn pin(group: &str) -> PinnedApp {
        PinnedApp { spec: format!("pin-{group}"), group: group.into(), launch: format!("run-{group}"), title: format!("App {group}"), icon: format!("pinned-icon-{group}") }
    }

    #[test]
    fn grouped_pinned_first_then_by_first_appearance() {
        let windows = [win(1, "b", "B1"), win(2, "c", "C1"), win(3, "b", "B2"), win(4, "a", "A1")];
        let tasks = build_tasks(&windows, &[pin("a"), pin("z")], Some(3), &TaskOptions { group: true, monitor: None });
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["g:a", "g:z", "g:b", "g:c"]);

        let a = &tasks[0];
        assert!(a.pinned && a.running && !a.active);
        assert_eq!((a.title.as_str(), a.icon.as_str()), ("A1", "pinned-icon-a"));
        let z = &tasks[1];
        assert!(z.pinned && !z.running);
        assert_eq!((z.title.as_str(), z.launch.as_deref()), ("App z", Some("run-z")));
        let b = &tasks[2];
        assert_eq!(b.windows, vec![1, 3]);
        assert!(b.active && !b.pinned);
        assert_eq!(b.pin_spec.as_deref(), Some("spec-b"));
    }

    #[test]
    fn ungrouped_and_monitor_filter() {
        let mut windows = vec![win(1, "b", "B1"), win(2, "a", "A1"), win(3, "b", "B2")];
        windows[2].monitor = "M2".into();
        let tasks = build_tasks(&windows, &[pin("b")], None, &TaskOptions { group: false, monitor: None });
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["w:1", "w:3", "w:2"]);
        assert!(tasks[0].pinned && tasks[1].pinned && !tasks[2].pinned);

        let only_m2 = build_tasks(&windows, &[], None, &TaskOptions { group: true, monitor: Some("M2".into()) });
        assert_eq!(only_m2.len(), 1);
        assert_eq!(only_m2[0].windows, vec![3]);
    }

    #[test]
    fn minimized_and_attention_flags() {
        let mut windows = vec![win(1, "a", "A1"), win(2, "a", "A2")];
        windows[0].minimized = true;
        let t = &build_tasks(&windows, &[], None, &TaskOptions { group: true, monitor: None })[0];
        assert!(!t.minimized, "only when every window is minimized");
        windows[1].minimized = true;
        windows[1].attention = true;
        let t = &build_tasks(&windows, &[], None, &TaskOptions { group: true, monitor: None })[0];
        assert!(t.minimized && t.attention);
    }

    #[test]
    fn click_behaviour() {
        let tasks = build_tasks(&[win(1, "a", "A"), win(2, "b", "B1"), win(3, "b", "B2")], &[pin("z")], Some(1), &TaskOptions { group: true, monitor: None });
        let find = |id: &str| tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(click_action(find("g:a"), Some(1)), ClickAction::Minimize(vec![1]));
        assert_eq!(click_action(find("g:a"), Some(99)), ClickAction::Activate(1));
        assert_eq!(click_action(find("g:b"), Some(99)), ClickAction::Activate(3), "most recent first");
        assert_eq!(click_action(find("g:b"), Some(2)), ClickAction::Activate(3), "cycle");
        assert_eq!(click_action(find("g:b"), Some(3)), ClickAction::Activate(2), "wrap");
        assert_eq!(click_action(find("g:z"), None), ClickAction::Launch("run-z".into()));
    }
}
