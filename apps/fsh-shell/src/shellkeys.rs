//! What Explorer does besides the taskbar that Ferroshell takes over as the login shell: the
//! Windows-key shortcuts (Win+E, Win+R, Win+D, Win+X, Win+1…9, …) and passing environment
//! changes on to the apps we start.

use std::cell::RefCell;
use std::time::Duration;

use fsh_core::hotkeys::{self, ShellHotkey};
use fsh_win::hotkey::{self, Modifiers};
use fsh_win::menu::{Anchor, MenuItem};
use fsh_win::session::PowerAction;
use fsh_win::{Hwnd, winops};
use serde_json::{Value, json};

use crate::app::{App, with};
use crate::tracker::Cmd;

/// Hotkey ids on the event window: this plus the binding's index.
const HOTKEY_BASE: i32 = 0x5000;

#[derive(Default)]
pub struct ShellKeys {
    registered: RefCell<Vec<i32>>,
    unavailable: RefCell<Vec<String>>,
    /// The user environment as last built from the registry.
    environment: RefCell<Option<Vec<(String, String)>>>,
}

fn describe(b: &hotkeys::Binding) -> String {
    format!("Win+{}{}", if b.shift { "Shift+" } else { "" }, char::from_u32(b.vk).unwrap_or('?'))
}

impl App {
    /// Registers Explorer's shortcuts on the event window. Only as the login shell: with
    /// Explorer running, they're Explorer's.
    pub(crate) fn start_shell_keys(&self) {
        let hwnd = self.events_hwnd();
        for (i, b) in hotkeys::bindings().iter().enumerate() {
            let id = HOTKEY_BASE + i as i32;
            let mods = Modifiers { ctrl: false, alt: false, shift: b.shift, win: true };
            match hotkey::register(hwnd, id, mods, b.vk) {
                Ok(()) => self.shell_keys.registered.borrow_mut().push(id),
                Err(_) => self.shell_keys.unavailable.borrow_mut().push(describe(b)),
            }
        }
        let unavailable = self.shell_keys.unavailable.borrow();
        tracing::info!(
            "Windows-key shortcuts: {} registered{}",
            self.shell_keys.registered.borrow().len(),
            if unavailable.is_empty() { String::new() } else { format!("; in use elsewhere: {}", unavailable.join(", ")) }
        );
    }

    /// `WM_HOTKEY` on the event window. Returns false if it isn't one of ours.
    pub(crate) fn on_hotkey_message(&self, id: i32) -> bool {
        let Some(action) = hotkeys::from_id(id - HOTKEY_BASE) else { return false };
        // Run it after the window procedure returns: some open menus with their own loop.
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(|a| a.on_shell_hotkey(action));
        });
        true
    }

    fn on_shell_hotkey(&self, action: ShellHotkey) {
        tracing::debug!("shortcut {action:?}");
        match action {
            ShellHotkey::FileExplorer => crate::actions::launch("explorer.exe".into()),
            ShellHotkey::Run | ShellHotkey::Search => self.toggle_launcher(crate::launcher::Anchor::Cursor),
            ShellHotkey::ShowDesktop => self.toggle_desktop(),
            ShellHotkey::MinimiseAll => self.minimise_all(),
            ShellHotkey::RestoreMinimised => self.restore_minimised(),
            ShellHotkey::Settings => crate::actions::launch("ms-settings:".into()),
            ShellHotkey::Notifications => self.open_widget_popup("org.ferroshell.notifications"),
            ShellHotkey::QuickLinks => self.quick_links_menu(),
            ShellHotkey::ScreenClip => crate::actions::launch("ms-screenclip:".into()),
            ShellHotkey::Task(n) => self.activate_nth_task(usize::from(n)),
        }
    }

    fn minimise_all(&self) {
        let visible: Vec<isize> = self.tasks.borrow().windows.iter().filter(|w| !w.minimized).map(|w| w.hwnd).collect();
        for h in &visible {
            winops::minimize(Hwnd(*h));
        }
        if !visible.is_empty() {
            *self.desktop_shown.borrow_mut() = Some(visible);
        }
        self.tracker.send(Cmd::Refresh);
    }

    fn restore_minimised(&self) {
        if let Some(prev) = self.desktop_shown.borrow_mut().take() {
            for h in prev {
                winops::restore(Hwnd(h));
            }
        }
        self.tracker.send(Cmd::Refresh);
    }

    /// Opens a widget's popup (e.g. Win+N for notifications) on the first panel that has
    /// it, preferring the primary monitor.
    fn open_widget_popup(&self, widget_id: &str) {
        let found = {
            let st = self.state.borrow();
            let mut panels: Vec<_> = st.panels.iter().collect();
            panels.sort_by_key(|p| !p.monitor.primary);
            panels.into_iter().find_map(|p| {
                let (_, statuses) = st.statuses.iter().find(|(name, _)| *name == p.name)?;
                let w = statuses.iter().find(|w| w.id == widget_id && w.error.is_none())?;
                Some((p.key.clone(), w.instance.clone()))
            })
        };
        match found {
            Some((key, instance)) => self.toggle_popup(&key, &instance),
            None => tracing::info!("no {widget_id} widget on any panel"),
        }
    }

    /// Win+1…9: the nth task on the primary monitor's task manager (or the first one).
    fn activate_nth_task(&self, n: usize) {
        let found = {
            let st = self.state.borrow();
            let mut panels: Vec<_> = st.panels.iter().filter(|p| p.tasks.widget_index.is_some()).collect();
            panels.sort_by_key(|p| !p.monitor.primary);
            panels.first().and_then(|p| Some((p.key.clone(), p.tasks.nth_id(n)?)))
        };
        if let Some((key, id)) = found {
            self.activate_task(&key, &id);
        }
    }

    /// Win+X: the "quick links" menu Explorer shows by the Start button.
    fn quick_links_menu(&self) {
        let items: &[(u32, &str, &str)] = &[
            (1, "Installed apps", "ms-settings:appsfeatures"),
            (2, "Power options", "ms-settings:powersleep"),
            (3, "Event Viewer", "eventvwr.msc"),
            (4, "System", "ms-settings:about"),
            (5, "Device Manager", "devmgmt.msc"),
            (6, "Network connections", "ms-settings:network"),
            (7, "Disk Management", "diskmgmt.msc"),
            (8, "Computer Management", "compmgmt.msc"),
            (0, "", ""),
            (9, "Terminal", "wt.exe"),
            (10, "Terminal (Admin)", "wt.exe"),
            (11, "Task Manager", "taskmgr.exe"),
            (12, "Settings", "ms-settings:"),
            (13, "File Explorer", "explorer.exe"),
            (14, "Run", ""),
            (0, "", ""),
            (20, "Sign out", ""),
            (21, "Sleep", ""),
            (22, "Restart", ""),
            (23, "Shut down", ""),
        ];
        let menu: Vec<MenuItem> =
            items.iter().map(|(id, label, _)| if *id == 0 { MenuItem::Separator } else { MenuItem::new(*id, *label) }).collect();
        let (x, y) = fsh_win::system::cursor_pos();
        let Some(chosen) = fsh_win::menu::popup(self.events_hwnd(), x, y, Anchor::Above, &menu) else { return };
        let power = |a: PowerAction| {
            if let Err(e) = fsh_win::session::perform(a) {
                tracing::error!("{a:?}: {e:#}");
            }
        };
        match chosen {
            10 => crate::actions::launch_verb("wt.exe".into(), None, "runas"),
            14 => self.toggle_launcher(crate::launcher::Anchor::Cursor),
            20 => power(PowerAction::Logout),
            21 => power(PowerAction::Sleep),
            22 => power(PowerAction::Restart),
            23 => power(PowerAction::Shutdown),
            id => {
                if let Some((_, _, target)) = items.iter().find(|(i, _, _)| *i == id) {
                    crate::actions::launch((*target).into());
                }
            }
        }
    }

    /// Remember the user environment, to pass on later changes (see [`Self::refresh_environment`]).
    pub(crate) fn snapshot_environment(&self) {
        match fsh_win::env::user_environment() {
            Ok(env) => *self.shell_keys.environment.borrow_mut() = Some(env),
            Err(e) => tracing::warn!("reading the user environment: {e:#}"),
        }
    }

    /// `WM_SETTINGCHANGE("Environment")`: someone changed environment variables (e.g. an
    /// installer added to PATH). Apply the change to our own environment, so apps started
    /// from now on get it, as Explorer does.
    pub(crate) fn refresh_environment(&self) {
        let fresh = match fsh_win::env::user_environment() {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!("reading the user environment: {e:#}");
                return;
            }
        };
        let old = self.shell_keys.environment.borrow_mut().replace(fresh.clone()).unwrap_or_default();
        let (set, remove) = hotkeys::environment_changes(&old, &fresh);
        if set.is_empty() && remove.is_empty() {
            return;
        }
        fsh_win::env::apply(&set, &remove);
        let names: Vec<&str> = set.iter().map(|(k, _)| k.as_str()).chain(remove.iter().map(String::as_str)).collect();
        tracing::info!("environment changed: {}", names.join(", "));
    }

    pub(crate) fn shell_keys_state(&self) -> Value {
        json!({
            "registered": self.shell_keys.registered.borrow().len(),
            "unavailable": *self.shell_keys.unavailable.borrow(),
        })
    }
}
