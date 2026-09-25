//! The desktop when Ferroshell is the login shell: the wallpaper behind everything, and a
//! right-click menu. (Alongside Explorer, Explorer's desktop stays.)

use std::cell::RefCell;
use std::time::Duration;

use fsh_win::desktop::{self, DesktopEvent, DesktopWindow};
use fsh_win::menu::{Anchor, MenuItem};
use serde_json::{Value, json};

use crate::app::{self, App};

#[derive(Default)]
pub struct Desktop {
    window: RefCell<Option<DesktopWindow>>,
    error: RefCell<Option<String>>,
}

const MENU_PERSONALISE: u32 = 1;
const MENU_DISPLAY: u32 = 2;
const MENU_DESKTOP_FOLDER: u32 = 3;
const MENU_FERROSHELL: u32 = 4;
const MENU_TASK_MANAGER: u32 = 5;

impl App {
    /// Creates the desktop window, unless another shell's desktop is already there.
    pub(crate) fn start_desktop(&self) {
        if desktop::desktop_exists() {
            tracing::info!("a desktop already exists (Explorer?); not creating ours");
            *self.desktop.error.borrow_mut() = Some("another desktop exists".into());
            return;
        }
        let created = DesktopWindow::create(Box::new(|event| {
            // Handle it after the window procedure returns: menus run their own modal loop.
            slint::Timer::single_shot(Duration::ZERO, move || {
                app::with(|a| a.on_desktop_event(event));
            });
        }));
        match created {
            Ok(w) => {
                let (shell, taskman) = w.registered();
                tracing::info!("desktop created (shell window: {shell}, task manager window: {taskman})");
                *self.desktop.window.borrow_mut() = Some(w);
            }
            Err(e) => {
                tracing::error!("creating the desktop: {e:#}");
                *self.desktop.error.borrow_mut() = Some(format!("{e:#}"));
            }
        }
    }

    fn on_desktop_event(&self, event: DesktopEvent) {
        match event {
            DesktopEvent::TaskList => self.toggle_launcher(crate::launcher::Anchor::Cursor),
            DesktopEvent::ContextMenu { x, y } => self.desktop_menu(x, y),
        }
    }

    fn desktop_menu(&self, x: i32, y: i32) {
        let Some(owner) = self.desktop.window.borrow().as_ref().map(DesktopWindow::hwnd) else { return };
        let items = [
            MenuItem::new(MENU_PERSONALISE, "Personalise"),
            MenuItem::new(MENU_DISPLAY, "Display settings"),
            MenuItem::Separator,
            MenuItem::new(MENU_DESKTOP_FOLDER, "Open Desktop folder"),
            MenuItem::new(MENU_TASK_MANAGER, "Task Manager"),
            MenuItem::Separator,
            MenuItem::new(MENU_FERROSHELL, "Ferroshell settings"),
        ];
        match fsh_win::menu::popup(owner, x, y, Anchor::Below, &items) {
            Some(MENU_PERSONALISE) => crate::actions::launch("ms-settings:personalization-background".into()),
            Some(MENU_DISPLAY) => crate::actions::launch("ms-settings:display".into()),
            Some(MENU_DESKTOP_FOLDER) => crate::actions::launch("shell:Desktop".into()),
            Some(MENU_TASK_MANAGER) => crate::actions::launch("taskmgr.exe".into()),
            Some(MENU_FERROSHELL) => crate::actions::invoke("settings", ""),
            _ => {}
        }
    }

    pub(crate) fn desktop_state(&self) -> Value {
        match self.desktop.window.borrow().as_ref() {
            Some(w) => {
                let (shell, taskman) = w.registered();
                json!({ "ours": true, "shell_window": shell, "taskman_window": taskman })
            }
            None => json!({ "ours": false, "reason": *self.desktop.error.borrow() }),
        }
    }
}
