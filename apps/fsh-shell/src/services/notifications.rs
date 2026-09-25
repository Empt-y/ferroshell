//! The "notifications" service: other apps' notifications (Windows' notification centre),
//! on a "notifications" worker thread. Polls every couple of seconds; reports which ones
//! are new since the last look (for banners) and each app's logo as a file.
//!
//! Needs package identity and notification access (see `packaging/identity/`); without
//! them it reports the access state so the applet can explain what to do. Notification
//! text is never logged.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::Duration;

use fsh_win::notifications::{self as notif, Access, Toast};

use super::Update;

const TICK: Duration = Duration::from_secs(2);
const LOGO_SIZE: f32 = 64.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    pub access: Access,
    pub toasts: Vec<Toast>,
    /// Ids that arrived since the previous snapshot, newest first.
    pub arrived: Vec<u32>,
    /// App id → logo file.
    pub logos: HashMap<String, PathBuf>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self { available: false, access: Access::NoIdentity, toasts: vec![], arrived: vec![], logos: HashMap::new() }
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    RequestAccess,
    Remove(u32),
    ClearApp(String),
    ClearAll,
}

pub struct NotificationService {
    tx: Sender<Cmd>,
}

impl NotificationService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("notifications".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("notifications service crashed; notifications are unavailable until the shell restarts");
                on_update(Update::Notifications(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn logo_dir() -> PathBuf {
    fsh_common::paths::state_dir().join("notification-logos")
}

/// A file-name-safe key for an app id (AUMIDs contain `!`, paths contain `\`).
fn logo_file(app_id: &str) -> PathBuf {
    let safe: String = app_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' }).take(120).collect();
    logo_dir().join(format!("{safe}.png"))
}

fn run(rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::mta();
    let mut known: Option<HashSet<u32>> = None;
    let mut logos: HashMap<String, PathBuf> = HashMap::new();
    let mut logo_tried: HashSet<String> = HashSet::new();
    let mut last: Option<Snapshot> = None;
    let mut wait = Duration::ZERO;
    loop {
        match rx.recv_timeout(wait) {
            Ok(cmd) => {
                apply(cmd);
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => apply(cmd),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        wait = TICK;
        let access = notif::access();
        let toasts = if access == Access::Allowed { notif::toasts() } else { vec![] };
        let arrived: Vec<u32> = fsh_core::notifications::arrivals(known.as_ref(), &toasts).into_iter().map(|t| t.id).collect();
        if access == Access::Allowed {
            known = Some(toasts.iter().map(|t| t.id).collect());
        }
        for t in &toasts {
            if t.app_id.is_empty() || !logo_tried.insert(t.app_id.clone()) {
                continue;
            }
            let path = logo_file(&t.app_id);
            if path.is_file() {
                logos.insert(t.app_id.clone(), path);
            } else if let Some(bytes) = notif::app_logo(t.id, LOGO_SIZE) {
                let _ = std::fs::create_dir_all(logo_dir());
                if std::fs::write(&path, bytes).is_ok() {
                    logos.insert(t.app_id.clone(), path);
                }
            }
        }
        if !arrived.is_empty() {
            tracing::debug!("{} new notification(s)", arrived.len());
        }
        let snap = Snapshot { available: true, access, toasts, arrived, logos: logos.clone() };
        // Arrivals are events: always deliver a snapshot that has some.
        if !snap.arrived.is_empty() || last.as_ref() != Some(&snap) {
            on_update(Update::Notifications(snap.clone()));
            last = Some(snap);
        }
    }
}

fn apply(cmd: Cmd) {
    match cmd {
        Cmd::RequestAccess => {
            let a = notif::request_access();
            tracing::info!("notification access: {a:?}");
        }
        Cmd::Remove(id) => notif::remove(id),
        Cmd::ClearApp(app) => {
            for t in notif::toasts().into_iter().filter(|t| t.app_id == app) {
                notif::remove(t.id);
            }
        }
        Cmd::ClearAll => notif::clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_file_names_are_safe() {
        let p = logo_file(r"Microsoft.Windows.SecHealthUI_cw5n1h2txyewy!SecHealthUI");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "Microsoft.Windows.SecHealthUI_cw5n1h2txyewy-SecHealthUI.png");
        let p = logo_file(r"C:\Program Files\App\app.exe");
        assert!(!p.file_name().unwrap().to_str().unwrap().contains(['\\', ':', ' ']));
    }
}
