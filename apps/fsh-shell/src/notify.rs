//! Notifications on the UI thread: the `Notifications` global (list, unread count, access
//! state, Do Not Disturb), marking them read when the applet's popup opens, and deciding
//! when to show a banner. The service itself is `services::notifications`.

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use fsh_win::notifications::{Access, Toast};
use slint::Image;
use slint_interpreter::{ComponentInstance, Value};

use crate::app::{App, with};
use crate::services::notifications::{Cmd, Snapshot};
use crate::services::{strukt, sync_model};

/// Rows are rebuilt this often so "5 min ago" keeps up.
const ROWS_EVERY_SECS: i64 = 30;
const WIDGET_ID: &str = "org.ferroshell.notifications";

type GlobalCallback = Box<dyn Fn(&[Value]) -> Value>;

fn state_file() -> std::path::PathBuf {
    fsh_common::paths::state_dir().join("notifications.json")
}

/// Do Not Disturb survives restarts.
pub(crate) fn load_dnd() -> bool {
    std::fs::read(state_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("dnd").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn save_dnd(on: bool) {
    let json = serde_json::json!({ "dnd": on }).to_string();
    std::thread::spawn(move || {
        let _ = std::fs::write(state_file(), json);
    });
}

fn access_name(a: Access) -> &'static str {
    match a {
        Access::NoIdentity => "no-identity",
        Access::Unspecified => "unspecified",
        Access::Allowed => "allowed",
        Access::Denied => "denied",
    }
}

/// How banners are configured on the notifications applet (the first one on a panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BannerMode {
    /// Only while Ferroshell replaces Explorer (alongside it, Windows shows its own).
    Auto,
    Always,
    Never,
}

impl App {
    fn notification_cmd(&self, cmd: Cmd) {
        if let Some(s) = self.services.notifications.borrow().as_ref() {
            s.send(cmd);
        }
    }

    pub(crate) fn bind_notifications(&self, instance: &ComponentInstance) {
        let cb = |name: &str, f: GlobalCallback| {
            if let Err(e) = instance.set_global_callback("Notifications", name, f) {
                tracing::warn!("Notifications.{name}: {e}");
            }
        };
        let id = |a: &[Value]| match a.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let text = |a: &[Value]| match a.first() {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        };
        cb("request-access", Box::new(|_| {
            with(|app| app.notification_cmd(Cmd::RequestAccess));
            Value::Void
        }));
        cb("dismiss", Box::new(move |a| {
            let n = id(a);
            with(|app| app.notification_cmd(Cmd::Remove(n)));
            Value::Void
        }));
        cb("clear-app", Box::new(move |a| {
            let app_id = text(a);
            with(|app| app.notification_cmd(Cmd::ClearApp(app_id)));
            Value::Void
        }));
        cb("clear-all", Box::new(|_| {
            with(|app| app.notification_cmd(Cmd::ClearAll));
            Value::Void
        }));
        cb("open", Box::new(move |a| {
            let n = id(a);
            with(|app| app.open_notification(n));
            Value::Void
        }));
        cb("set-dnd", Box::new(|a| {
            let on = matches!(a.first(), Some(Value::Bool(true)));
            with(|app| app.set_dnd(on));
            Value::Void
        }));
    }

    /// Start the app that sent notification `id` and dismiss it (like clicking a toast;
    /// the toast's own launch arguments aren't available to listeners).
    pub(crate) fn open_notification(&self, id: u32) {
        let app_id = self.services.notif_state.borrow().toasts.iter().find(|t| t.id == id).map(|t| t.app_id.clone());
        if let Some(app_id) = app_id.filter(|a| !a.is_empty()) {
            crate::actions::launch(format!(r"shell:AppsFolder\{app_id}"));
        }
        self.notification_cmd(Cmd::Remove(id));
        self.close_popup();
        self.hide_banner();
    }

    fn set_dnd(&self, on: bool) {
        self.services.notif_dnd.set(on);
        save_dnd(on);
        if on {
            self.hide_banner();
        }
        self.for_each_instance(&|i| self.apply_notifications(i));
    }

    pub(crate) fn apply_notifications(&self, i: &ComponentInstance) {
        let s = &self.services;
        let st = s.notif_state.borrow();
        let seen = s.notif_seen.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Notifications", name, v);
        };
        set("available", Value::Bool(st.available));
        set("access", Value::String(access_name(st.access).into()));
        set("count", Value::Number(st.toasts.len() as f64));
        set("unread", Value::Number(st.toasts.iter().filter(|t| !seen.contains(&t.id)).count() as f64));
        set("dnd", Value::Bool(s.notif_dnd.get()));
    }

    fn logo(&self, app_id: &str) -> Option<Image> {
        let s = &self.services;
        let path = s.notif_state.borrow().logos.get(app_id).cloned()?;
        let mut cache = s.logo_images.borrow_mut();
        if let Some(img) = cache.get(&path) {
            return Some(img.clone());
        }
        let img = Image::load_from_path(&path).ok()?;
        cache.insert(path, img.clone());
        Some(img)
    }

    fn sync_notification_rows(&self) {
        let s = &self.services;
        let now = Local::now().naive_local();
        let rows: Vec<Value> = {
            let st = s.notif_state.borrow();
            let seen = s.notif_seen.borrow();
            fsh_core::notifications::grouped(&st.toasts)
                .into_iter()
                .map(|(first, t)| {
                    let created = Local.timestamp_opt(t.created, 0).single().map(|d| d.naive_local()).unwrap_or(now);
                    let logo = self.logo(&t.app_id);
                    strukt(vec![
                        ("id", Value::Number(f64::from(t.id))),
                        ("app-id", Value::String(t.app_id.as_str().into())),
                        ("app", Value::String(if t.app_name.is_empty() { "App".into() } else { t.app_name.as_str().into() })),
                        ("has-icon", Value::Bool(logo.is_some())),
                        ("icon", Value::Image(logo.unwrap_or_default())),
                        ("title", Value::String(t.title.as_str().into())),
                        ("body", Value::String(t.body.as_str().into())),
                        ("time", Value::String(fsh_core::notifications::time_text(created, now).into())),
                        ("first-in-group", Value::Bool(first)),
                        ("unread", Value::Bool(!seen.contains(&t.id))),
                    ])
                })
                .collect()
        };
        sync_model(&s.notif_rows, rows);
    }

    pub(crate) fn on_notifications(&self, snap: Snapshot) {
        let arrived: Vec<Toast> = snap.toasts.iter().filter(|t| snap.arrived.contains(&t.id)).cloned().collect();
        {
            // Forget ids that are gone, so the seen set doesn't grow forever.
            let live: HashSet<u32> = snap.toasts.iter().map(|t| t.id).collect();
            self.services.notif_seen.borrow_mut().retain(|id| live.contains(id));
        }
        *self.services.notif_state.borrow_mut() = snap;
        self.sync_notification_rows();
        self.for_each_instance(&|i| self.apply_notifications(i));
        if let Some(newest) = arrived.first()
            && let Some(seconds) = self.banner_seconds()
        {
            let logo = self.logo(&newest.app_id);
            self.show_banner(newest, arrived.len() - 1, logo, seconds);
        }
    }

    /// A popup opened: if it's the notifications applet's, everything listed is now read.
    pub(crate) fn services_popup_opened(&self, services: &[String]) {
        if !services.iter().any(|s| s == "notifications") {
            return;
        }
        let ids: HashSet<u32> = self.services.notif_state.borrow().toasts.iter().map(|t| t.id).collect();
        *self.services.notif_seen.borrow_mut() = ids;
        self.sync_notification_rows();
        self.for_each_instance(&|i| self.apply_notifications(i));
    }

    pub(crate) fn notifications_tick(&self, now: i64) {
        if now % ROWS_EVERY_SECS == 0 && !self.services.notif_state.borrow().toasts.is_empty() {
            self.sync_notification_rows();
        }
    }

    /// How long to show a banner, or `None` for no banner (Do Not Disturb, banners off, or
    /// "auto" alongside Explorer, which shows its own).
    fn banner_seconds(&self) -> Option<u64> {
        if self.services.notif_dnd.get() {
            return None;
        }
        let (mode, seconds) = {
            let st = self.state.borrow();
            let entry = st.panels.iter().flat_map(|p| p.config.widgets.iter()).find(|w| w.id == WIDGET_ID);
            let mode = match entry.and_then(|e| e.settings.get("banners")).and_then(|v| v.as_str()) {
                Some("always") => BannerMode::Always,
                Some("never") => BannerMode::Never,
                _ => BannerMode::Auto,
            };
            let seconds = entry.and_then(|e| e.settings.get("banner-seconds")).and_then(toml::Value::as_integer).unwrap_or(6).clamp(2, 60);
            (mode, seconds as u64)
        };
        let show = match mode {
            BannerMode::Always => true,
            BannerMode::Never => false,
            BannerMode::Auto => crate::osd::media_keys_wanted(self.safe_mode),
        };
        show.then_some(seconds)
    }

    /// `fsh-ctl shell debug.banner`: show the newest notification as a banner (or a sample),
    /// whatever the banner settings.
    pub(crate) fn debug_banner(&self) {
        let newest = {
            let st = self.services.notif_state.borrow();
            st.toasts.iter().max_by_key(|t| t.created).cloned()
        };
        let toast = newest.unwrap_or_else(|| Toast {
            id: 0,
            app_id: String::new(),
            app_name: "Ferroshell".into(),
            created: chrono::Utc::now().timestamp(),
            title: "Sample notification".into(),
            body: "This is what a notification banner looks like.".into(),
        });
        let logo = self.logo(&toast.app_id);
        self.show_banner(&toast, 0, logo, 6);
    }

    /// For `fsh-ctl shell dump_state`: counts only, never notification content.
    pub(crate) fn notifications_state(&self) -> serde_json::Value {
        let s = &self.services;
        let st = s.notif_state.borrow();
        let seen = s.notif_seen.borrow();
        serde_json::json!({
            "running": s.notifications.borrow().is_some(),
            "available": st.available,
            "access": access_name(st.access),
            "count": st.toasts.len(),
            "unread": st.toasts.iter().filter(|t| !seen.contains(&t.id)).count(),
            "apps": st.toasts.iter().map(|t| t.app_name.as_str()).collect::<std::collections::BTreeSet<_>>(),
            "dnd": s.notif_dnd.get(),
            "banners": self.banner_seconds(),
        })
    }
}
