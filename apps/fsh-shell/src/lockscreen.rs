//! `[lock-screen] sync-image`: keep Windows' lock screen picture the same as the wallpaper.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use fsh_common::paths;
use serde_json::{Value, json};

use crate::app::{App, with};

#[derive(Default)]
pub struct LockScreenSync {
    running: Cell<bool>,
    status: RefCell<Value>,
}

fn state_file() -> PathBuf {
    paths::state_dir().join("lockscreen.json")
}

/// Identifies a wallpaper file's current contents, so an unchanged one isn't set again.
fn fingerprint(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{}|{}|{modified}", path.display(), meta.len()))
}

fn last_synced() -> Option<String> {
    let text = std::fs::read_to_string(state_file()).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.get("synced")?.as_str().map(str::to_owned)
}

/// Sets the lock screen picture from the current wallpaper, blocking (needs COM, MTA).
pub fn sync_now() -> Result<String, String> {
    let wallpaper = fsh_win::lockscreen::current_wallpaper().ok_or("no wallpaper picture is set")?;
    // Windows' own copy (TranscodedWallpaper) has no extension, which the lock screen
    // refuses: hand it a copy that has one.
    let image = if wallpaper.extension().is_some() {
        wallpaper.clone()
    } else {
        let copy = paths::state_dir().join("lockscreen-image.jpg");
        std::fs::copy(&wallpaper, &copy).map_err(|e| format!("copying {}: {e}", wallpaper.display()))?;
        copy
    };
    let how = fsh_win::lockscreen::set_image(&image).map_err(|e| format!("{e:#}"))?;
    if let Some(fp) = fingerprint(&wallpaper) {
        let _ = std::fs::write(state_file(), json!({ "synced": fp }).to_string());
    }
    Ok(format!("set from {} ({how})", wallpaper.display()))
}

impl App {
    /// If `[lock-screen] sync-image` is on and the wallpaper changed since it was last
    /// synced, set it as the lock screen picture (on a worker thread).
    pub(crate) fn sync_lock_screen(&self) {
        let enabled = self.state.borrow().config.config().lock_screen.sync_image;
        if !enabled || self.safe_mode || self.lock_screen.running.get() {
            if !enabled {
                *self.lock_screen.status.borrow_mut() = Value::Null;
            }
            return;
        }
        let Some(wallpaper) = fsh_win::lockscreen::current_wallpaper() else {
            *self.lock_screen.status.borrow_mut() = json!("no wallpaper picture is set");
            return;
        };
        if fingerprint(&wallpaper).is_some() && fingerprint(&wallpaper) == last_synced() {
            *self.lock_screen.status.borrow_mut() = json!("up to date");
            return;
        }
        self.lock_screen.running.set(true);
        let spawned = std::thread::Builder::new().name("lock-screen".into()).spawn(|| {
            let _com = fsh_win::com::ComGuard::mta();
            let result = sync_now();
            match &result {
                Ok(msg) => tracing::info!("lock screen picture {msg}"),
                Err(e) => tracing::warn!("could not set the lock screen picture: {e}"),
            }
            let _ = slint::invoke_from_event_loop(move || {
                with(|a| {
                    a.lock_screen.running.set(false);
                    *a.lock_screen.status.borrow_mut() = match result {
                        Ok(msg) => json!(msg),
                        Err(e) => json!({ "error": e }),
                    };
                });
            });
        });
        if let Err(e) = spawned {
            self.lock_screen.running.set(false);
            tracing::warn!("could not start the lock-screen thread: {e}");
        }
    }

    pub(crate) fn lock_screen_state(&self) -> Value {
        self.lock_screen.status.borrow().clone()
    }
}
