//! The desktop when Ferroshell is the login shell: the wallpaper behind everything, and a
//! right-click menu. (Alongside Explorer, Explorer's desktop stays.)

use std::cell::RefCell;
use std::time::Duration;

use fsh_win::desktop::{self, DesktopEvent, DesktopWindow};
use fsh_core::desktop_icons::SortBy;
use fsh_win::menu::{Anchor, MenuItem};

use crate::desktop_icons::ViewCommand;
use serde_json::{Value, json};

use crate::app::{self, App};

#[derive(Default)]
pub struct Desktop {
    pub(crate) window: RefCell<Option<DesktopWindow>>,
    error: RefCell<Option<String>>,
}

const MENU_PERSONALISE: u32 = 1;
const MENU_DISPLAY: u32 = 2;
const MENU_DESKTOP_FOLDER: u32 = 3;
const MENU_FERROSHELL: u32 = 4;
const MENU_TASK_MANAGER: u32 = 5;
const MENU_NEXT_BACKGROUND: u32 = 6;
const MENU_ABOUT_PICTURE: u32 = 7;
const MENU_LARGE: u32 = 10;
const MENU_MEDIUM: u32 = 11;
const MENU_SMALL: u32 = 12;
const MENU_AUTO_ARRANGE: u32 = 13;
const MENU_SHOW_ICONS: u32 = 14;
const MENU_SORT_NAME: u32 = 15;
const MENU_SORT_SIZE: u32 = 16;
const MENU_SORT_TYPE: u32 = 17;
const MENU_SORT_DATE: u32 = 18;
const MENU_REFRESH: u32 = 19;
const ITEM_OPEN: u32 = 1;
const ITEM_RENAME: u32 = 2;
const ITEM_DELETE: u32 = 3;
const ITEM_PROPERTIES: u32 = 4;

fn checked(id: u32, label: &str, on: bool) -> MenuItem {
    MenuItem::Item { id, label: label.into(), enabled: true, checked: on }
}

impl App {
    /// Creates the desktop window, unless another shell's desktop is already there.
    pub(crate) fn start_desktop(&self) {
        if desktop::desktop_exists() {
            tracing::info!("a desktop already exists (Explorer?); not creating ours");
            *self.desktop.error.borrow_mut() = Some("another desktop exists".into());
            return;
        }
        desktop::hide_minimized_windows();
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
            DesktopEvent::ItemsChanged => self.desktop_items_changed(),
        }
    }

    pub(crate) fn desktop_menu(&self, x: i32, y: i32) {
        let Some(owner) = self.desktop.window.borrow().as_ref().map(DesktopWindow::hwnd) else { return };
        let (next, about) = self.wallpaper_menu();
        let mut items = Vec::new();
        if next {
            items.push(MenuItem::new(MENU_NEXT_BACKGROUND, "Next desktop background"));
        }
        if let Some(title) = about {
            items.push(MenuItem::new(MENU_ABOUT_PICTURE, format!("About this picture: {title}")));
        }
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        let view = fsh_win::desktop_items::view_settings();
        items.extend([
            checked(MENU_LARGE, "Large icons", view.icon_size > 72),
            checked(MENU_MEDIUM, "Medium icons", (41..=72).contains(&view.icon_size)),
            checked(MENU_SMALL, "Small icons", view.icon_size <= 40),
            checked(MENU_AUTO_ARRANGE, "Auto arrange icons", view.auto_arrange),
            checked(MENU_SHOW_ICONS, "Show desktop icons", view.show_icons),
            MenuItem::Separator,
            MenuItem::new(MENU_SORT_NAME, "Sort by name"),
            MenuItem::new(MENU_SORT_SIZE, "Sort by size"),
            MenuItem::new(MENU_SORT_TYPE, "Sort by item type"),
            MenuItem::new(MENU_SORT_DATE, "Sort by date modified"),
            MenuItem::new(MENU_REFRESH, "Refresh"),
            MenuItem::Separator,
            MenuItem::new(MENU_PERSONALISE, "Personalise"),
            MenuItem::new(MENU_DISPLAY, "Display settings"),
            MenuItem::Separator,
            MenuItem::new(MENU_DESKTOP_FOLDER, "Open Desktop folder"),
            MenuItem::new(MENU_TASK_MANAGER, "Task Manager"),
            MenuItem::Separator,
            MenuItem::new(MENU_FERROSHELL, "Ferroshell settings"),
        ]);
        match fsh_win::menu::popup(owner, x, y, Anchor::Below, &items) {
            Some(MENU_PERSONALISE) => crate::actions::launch("ms-settings:personalization-background".into()),
            Some(MENU_DISPLAY) => crate::actions::launch("ms-settings:display".into()),
            Some(MENU_DESKTOP_FOLDER) => crate::actions::launch("shell:Desktop".into()),
            Some(MENU_TASK_MANAGER) => crate::actions::launch("taskmgr.exe".into()),
            Some(MENU_FERROSHELL) => crate::actions::invoke("settings", ""),
            Some(MENU_NEXT_BACKGROUND) => self.wallpaper_tick(true),
            Some(MENU_ABOUT_PICTURE) => self.wallpaper_about(),
            Some(MENU_LARGE) => self.desktop_view_command(ViewCommand::Size(96)),
            Some(MENU_MEDIUM) => self.desktop_view_command(ViewCommand::Size(48)),
            Some(MENU_SMALL) => self.desktop_view_command(ViewCommand::Size(32)),
            Some(MENU_AUTO_ARRANGE) => self.desktop_view_command(ViewCommand::AutoArrange),
            Some(MENU_SHOW_ICONS) => self.desktop_view_command(ViewCommand::ShowIcons),
            Some(MENU_SORT_NAME) => self.desktop_view_command(ViewCommand::Sort(SortBy::Name)),
            Some(MENU_SORT_SIZE) => self.desktop_view_command(ViewCommand::Sort(SortBy::Size)),
            Some(MENU_SORT_TYPE) => self.desktop_view_command(ViewCommand::Sort(SortBy::Type)),
            Some(MENU_SORT_DATE) => self.desktop_view_command(ViewCommand::Sort(SortBy::Modified)),
            Some(MENU_REFRESH) => self.desktop_view_command(ViewCommand::Refresh),
            _ => {}
        }
    }

    /// Right-click on icons.
    pub(crate) fn desktop_item_menu(&self, selection: &[usize], x: i32, y: i32) {
        let Some(owner) = self.desktop.window.borrow().as_ref().map(DesktopWindow::hwnd) else { return };
        let keys = self.desktop_selection_keys(selection);
        let files = keys.iter().filter(|(_, p)| p.is_some()).count();
        let mut items = vec![MenuItem::new(ITEM_OPEN, "Open")];
        if keys.len() == 1 && files == 1 {
            items.push(MenuItem::new(ITEM_RENAME, "Rename"));
        }
        if files > 0 {
            items.push(MenuItem::new(ITEM_DELETE, "Delete"));
        }
        if keys.len() == 1 {
            items.extend([MenuItem::Separator, MenuItem::new(ITEM_PROPERTIES, "Properties")]);
        }
        match fsh_win::menu::popup(owner, x, y, Anchor::Below, &items) {
            Some(ITEM_OPEN) => self.open_desktop_items(selection),
            Some(ITEM_RENAME) => {
                if let Some(&i) = selection.first() {
                    self.begin_desktop_rename(i);
                }
            }
            Some(ITEM_DELETE) => self.delete_desktop_items(selection, false),
            Some(ITEM_PROPERTIES) => {
                if let Some((key, _)) = keys.first() {
                    fsh_win::winops::show_properties(key, Some(owner));
                }
            }
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
