//! System tray behaviour: applying icon updates from the tray thread, starting/stopping
//! the host with the widget, and delivering clicks to icon owners.

use std::time::Duration;

use fsh_win::icon::RgbaImage;
use fsh_win::tray::{IconTarget, Mouse, NotifyCommand, send_mouse};
use fsh_win::{Hwnd, winfo};

use crate::app::{App, with};
use crate::panel::PanelKey;
use crate::tray::TrayThread;

const DEAD_ICON_CHECK: Duration = Duration::from_secs(5);

impl App {
    /// Host the tray while some panel shows a system tray widget and Explorer's taskbar
    /// isn't running (never in safe mode).
    pub(crate) fn sync_tray(&self) {
        let has_widget = !self.safe_mode && self.state.borrow().panels.iter().any(|p| p.tray.enabled);
        // Apps send icons to whichever tray window Windows finds first, and while Explorer's
        // taskbar runs that is Explorer's. So the tray only works when Ferroshell replaces
        // Explorer as the shell; alongside Explorer the widget stays empty.
        let explorer = fsh_win::taskbar::explorer_taskbar_present();
        if has_widget && explorer && self.tray_thread.borrow().is_none() {
            tracing::info!("system tray widget is inactive: it needs Explorer's taskbar not to be running (replacement mode)");
        }
        let wanted = has_widget && !explorer;
        let running = self.tray_thread.borrow().is_some();
        if wanted && !running {
            match TrayThread::start() {
                Ok(t) => {
                    *self.tray_thread.borrow_mut() = Some(t);
                    self.tray_timer.start(slint::TimerMode::Repeated, DEAD_ICON_CHECK, || {
                        with(|a| a.remove_dead_tray_icons());
                    });
                }
                Err(e) => tracing::error!("could not start the system tray: {e:#}"),
            }
        } else if !wanted && running {
            drop(self.tray_thread.borrow_mut().take());
            self.tray_timer.stop();
            *self.tray.borrow_mut() = Default::default();
        }
        self.refresh_tray();
    }

    pub(crate) fn on_tray_command(&self, cmd: NotifyCommand, icon: Option<RgbaImage>) {
        {
            let mut tray = self.tray.borrow_mut();
            if !tray.model.apply(&cmd) {
                return;
            }
            if let Some(img) = icon {
                let key = fsh_core::tray::TrayKey::of(&cmd);
                if let Some(i) = tray.model.icons().iter().find(|i| i.key == key) {
                    let icon_key = i.icon_key();
                    tray.set_icon(icon_key, &img);
                }
            }
            tray.prune_icons();
        }
        self.refresh_tray();
    }

    fn remove_dead_tray_icons(&self) {
        let removed = self.tray.borrow_mut().model.remove_dead(|owner| Hwnd(owner).exists());
        if removed {
            self.tray.borrow_mut().prune_icons();
            self.refresh_tray();
        }
    }

    pub(crate) fn refresh_tray(&self) {
        let mut tray = self.tray.borrow_mut();
        for p in &self.state.borrow().panels {
            p.tray.refresh(&mut tray);
        }
    }

    pub fn tray_click(&self, key: &PanelKey, id: &str, button: &str, rect: [f64; 4]) {
        self.hide_preview();
        let Some(icon) = self.tray.borrow().model.find(id).cloned() else { return };
        if !Hwnd(icon.owner).exists() {
            self.remove_dead_tray_icons();
            return;
        }
        let anchor = {
            let st = self.state.borrow();
            let Some(p) = st.panels.iter().find(|p| &p.key == key) else { return };
            let [x, y, w, h] = rect;
            p.to_screen(x + w / 2.0, y + h / 2.0)
        };
        let mouse = match button {
            "right" => Mouse::Right,
            "middle" => Mouse::Middle,
            "double" => Mouse::Double,
            _ => Mouse::Left,
        };
        tracing::debug!("tray {button} click on {} ({})", icon.tip, winfo::pid(Hwnd(icon.owner)));
        send_mouse(
            IconTarget { owner: icon.owner, uid: icon.uid, callback: icon.callback, version: icon.version },
            mouse,
            anchor.0,
            anchor.1,
        );
    }
}
