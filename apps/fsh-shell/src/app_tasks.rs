//! Task-manager behaviour: applying tracker updates, and what clicking, middle-clicking,
//! right-clicking and hovering tasks does.

use std::time::Duration;

use fsh_config::{ConfigEditor, Edge};
use fsh_core::tasks::{ClickAction, TaskView, click_action};
use fsh_win::menu::{self, Anchor, MenuItem};
use fsh_win::{Hwnd, winops};

use crate::app::{App, config_path, with};
use crate::panel::PanelKey;
use crate::tracker::{Cmd, Update};

impl App {
    pub(crate) fn on_tracker_update(&self, update: Update) {
        {
            let mut t = self.tasks.borrow_mut();
            for (key, img) in &update.icons {
                t.add_icon(key.clone(), img);
            }
            match update.pinned {
                Some(p) => t.pinned = p,
                None => {
                    t.windows = update.windows;
                    t.foreground = update.foreground;
                }
            }
        }
        for p in &self.state.borrow().panels {
            p.update_fullscreen();
        }
        self.refresh_tasks();
    }

    pub(crate) fn refresh_tasks(&self) {
        let t = self.tasks.borrow();
        for p in &self.state.borrow().panels {
            p.tasks.refresh(&t);
        }
    }

    fn find_task(&self, key: &PanelKey, id: &str) -> Option<TaskView> {
        let st = self.state.borrow();
        st.panels.iter().find(|p| &p.key == key)?.tasks.find(id)
    }

    pub fn activate_task(&self, key: &PanelKey, id: &str) {
        self.hide_preview();
        let Some(task) = self.find_task(key, id) else { return };
        let fg = self.tasks.borrow().foreground;
        match click_action(&task, fg) {
            ClickAction::Launch(target) => crate::actions::launch(target),
            ClickAction::Activate(h) => winops::activate(Hwnd(h)),
            ClickAction::Minimize(hs) => hs.into_iter().for_each(|h| winops::minimize(Hwnd(h))),
            ClickAction::Nothing => {}
        }
        self.tracker.send(Cmd::Refresh);
    }

    pub fn task_action(&self, key: &PanelKey, id: &str, action: &str) {
        let Some(task) = self.find_task(key, id) else { return };
        match action {
            "new-instance" => {
                if let Some(t) = task.launch {
                    crate::actions::launch(t);
                }
            }
            "close" => task.windows.iter().for_each(|h| winops::close(Hwnd(*h))),
            "minimize" => task.windows.iter().for_each(|h| winops::minimize(Hwnd(*h))),
            "restore" => task.windows.iter().for_each(|h| winops::restore(Hwnd(*h))),
            "pin" | "unpin" => {
                if let Some(spec) = &task.pin_spec {
                    self.set_pinned(key, spec, action == "pin");
                }
            }
            other => tracing::warn!("unknown task action `{other}`"),
        }
        self.tracker.send(Cmd::Refresh);
    }

    /// Add or remove a pinned app in this panel's task manager settings in config.toml.
    pub(crate) fn set_pinned(&self, key: &PanelKey, spec: &str, pin: bool) {
        let (widget, mut list) = {
            let st = self.state.borrow();
            let Some(p) = st.panels.iter().find(|p| &p.key == key) else { return };
            (p.tasks.widget_index, p.tasks.pinned_specs.clone())
        };
        let Some(widget) = widget else {
            tracing::warn!("this panel has no task manager to pin to");
            return;
        };
        if pin {
            if !list.iter().any(|s| s.eq_ignore_ascii_case(spec)) {
                list.push(spec.to_owned());
            }
        } else {
            list.retain(|s| s != spec);
        }
        let result = ConfigEditor::load(&config_path())
            .and_then(|mut ed| ed.set_widget_list(key.config_index, widget, "pinned", &list).map(|_| ed))
            .and_then(|ed| ed.save(&config_path()));
        match result {
            Ok(()) => {
                tracing::info!("{} {spec}", if pin { "pinned" } else { "unpinned" });
                self.config_written();
            }
            Err(e) => tracing::error!("could not update pinned apps in config.toml: {e}"),
        }
    }

    pub fn task_menu(&self, key: &PanelKey, id: &str, x: f64, y: f64) {
        self.hide_preview();
        // Show the menu after the click handler returns; the menu runs a modal loop.
        let (key, id) = (key.clone(), id.to_owned());
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(|a| a.show_task_menu(&key, &id, x, y));
        });
    }

    fn show_task_menu(&self, key: &PanelKey, id: &str, x: f64, y: f64) {
        let Some(task) = self.find_task(key, id) else { return };
        let (owner, (sx, sy), edge) = {
            let st = self.state.borrow();
            let Some(p) = st.panels.iter().find(|p| &p.key == key) else { return };
            let Some(hwnd) = p.hwnd.get() else { return };
            (hwnd, p.to_screen(x, y), p.config.edge)
        };
        const NEW: u32 = 1;
        const PIN: u32 = 2;
        const UNPIN: u32 = 3;
        const MINIMIZE: u32 = 4;
        const RESTORE: u32 = 5;
        const CLOSE: u32 = 6;

        let mut items = vec![MenuItem::disabled(truncate(&task.title, 60)), MenuItem::Separator];
        if task.launch.is_some() {
            items.push(MenuItem::new(NEW, if task.running { "Open new window" } else { "Open" }));
        }
        if task.pin_spec.is_some() {
            items.push(if task.pinned { MenuItem::new(UNPIN, "Unpin from panel") } else { MenuItem::new(PIN, "Pin to panel") });
        }
        if task.running {
            items.push(MenuItem::Separator);
            items.push(if task.minimized { MenuItem::new(RESTORE, "Restore") } else { MenuItem::new(MINIMIZE, "Minimize") });
            items.push(MenuItem::new(CLOSE, if task.windows.len() > 1 { "Close all windows" } else { "Close window" }));
        }
        let anchor = match edge {
            Edge::Bottom => Anchor::Above,
            Edge::Top => Anchor::Below,
            Edge::Left => Anchor::RightOf,
            Edge::Right => Anchor::LeftOf,
        };
        let action = match menu::popup(owner, sx, sy, anchor, &items) {
            Some(NEW) => "new-instance",
            Some(PIN) => "pin",
            Some(UNPIN) => "unpin",
            Some(MINIMIZE) => "minimize",
            Some(RESTORE) => "restore",
            Some(CLOSE) => "close",
            _ => return,
        };
        self.task_action(key, id, action);
    }

    /// "Peek at desktop": minimize everything, or put back what we minimized.
    pub(crate) fn toggle_desktop(&self) {
        let visible: Vec<isize> = self.tasks.borrow().windows.iter().filter(|w| !w.minimized).map(|w| w.hwnd).collect();
        let mut shown = self.desktop_shown.borrow_mut();
        if visible.is_empty() {
            if let Some(prev) = shown.take() {
                for h in prev {
                    winops::restore(Hwnd(h));
                }
            }
        } else {
            for h in &visible {
                winops::minimize(Hwnd(*h));
            }
            *shown = Some(visible);
        }
        self.tracker.send(Cmd::Refresh);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
