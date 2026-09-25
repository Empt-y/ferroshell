//! The desktop background as the login shell: Explorer runs Windows' wallpaper slideshow
//! and Spotlight, so without it we do. Settings > Personalisation stays the place to
//! choose; this follows what it saved.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fsh_common::paths;
use fsh_core::wallpaper::{self as rules, BackgroundType, SpotlightItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, with};

/// How often the engine checks whether it's time for the next picture.
const TICK: Duration = Duration::from_secs(60);
/// Windows' default slideshow interval, when Settings never saved one.
const DEFAULT_INTERVAL_MS: u32 = 30 * 60 * 1000;
/// Spotlight pictures kept on disk.
const SPOTLIGHT_KEEP: usize = 12;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Persisted {
    /// When we last changed the wallpaper (Unix seconds).
    last_change: u64,
    /// The slideshow picture we last set.
    slide: Option<PathBuf>,
    /// The Spotlight picture we last set, with its "about" details.
    spotlight: Option<SpotlightItem>,
    /// Spotlight pictures fetched but not shown yet.
    spotlight_queue: Vec<SpotlightItem>,
}

#[derive(Default)]
pub struct WallpaperEngine {
    timer: slint::Timer,
    busy: Cell<bool>,
    state: RefCell<Persisted>,
    status: RefCell<Value>,
}

fn state_file() -> PathBuf {
    paths::state_dir().join("wallpaper.json")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn spotlight_dir() -> PathBuf {
    paths::state_dir().join("spotlight")
}

/// Every picture in the slideshow folders (and one level of subfolders).
fn slideshow_images() -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth == 0 {
                    walk(&p, depth + 1, out);
                }
            } else if rules::is_slideshow_image(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for pidl in fsh_win::wallpaper::slideshow_folder_pidls() {
        if let Some(dir) = rules::decode_slideshow_pidl(&pidl).and_then(|p| fsh_win::wallpaper::pidl_path(&p)) {
            walk(&dir, 0, &mut out);
        }
    }
    out
}

/// Fetches the Spotlight feed. Blocking; needs COM (MTA).
pub fn fetch_spotlight() -> anyhow::Result<Vec<SpotlightItem>> {
    let url = rules::spotlight_url(&fsh_win::wallpaper::user_locale());
    let items = rules::parse_spotlight(&fsh_win::wallpaper::http_get_text(&url)?);
    anyhow::ensure!(!items.is_empty(), "the Spotlight feed had no pictures");
    Ok(items)
}

/// Downloads a Spotlight picture (once) and returns where it is.
pub fn download_spotlight(item: &SpotlightItem) -> anyhow::Result<PathBuf> {
    let dir = spotlight_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(rules::spotlight_file_name(&item.image_url));
    if !path.is_file() {
        fsh_win::wallpaper::http_download(&item.image_url, &path)?;
    }
    Ok(path)
}

/// Keeps the newest few Spotlight pictures.
fn prune_spotlight(keep_also: &Path) {
    let Ok(entries) = std::fs::read_dir(spotlight_dir()) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> =
        entries.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(SPOTLIGHT_KEEP) {
        if p != keep_also {
            let _ = std::fs::remove_file(p);
        }
    }
}

enum Job {
    Slide(PathBuf),
    Spotlight { queue: Vec<SpotlightItem> },
}

impl App {
    /// As the login shell: follow Windows' slideshow / Spotlight setting.
    pub(crate) fn start_wallpaper_engine(&self) {
        if let Some(saved) = std::fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            *self.wallpaper.state.borrow_mut() = saved;
        }
        self.wallpaper.timer.start(slint::TimerMode::Repeated, TICK, || {
            with(|a| a.wallpaper_tick(false));
        });
        // A first look shortly after sign-in, once the desktop is up.
        slint::Timer::single_shot(Duration::from_secs(5), || {
            with(|a| a.wallpaper_tick(false));
        });
    }

    fn background_type(&self) -> Option<BackgroundType> {
        fsh_win::wallpaper::background_type().and_then(BackgroundType::from_registry)
    }

    /// Changes the picture if it's due (or now, with `next`).
    pub(crate) fn wallpaper_tick(&self, next: bool) {
        if self.wallpaper.busy.get() {
            return;
        }
        let kind = self.background_type();
        let st = self.wallpaper.state.borrow().clone();
        let elapsed = now().saturating_sub(st.last_change);
        let job = match kind {
            Some(BackgroundType::Slideshow) => {
                let interval = u64::from(fsh_win::wallpaper::slideshow_options().0.unwrap_or(DEFAULT_INTERVAL_MS)) / 1000;
                if !next && elapsed < interval.max(60) {
                    return;
                }
                let (_, shuffle) = fsh_win::wallpaper::slideshow_options();
                let random = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
                match rules::next_slide(&slideshow_images(), st.slide.as_deref(), shuffle, random) {
                    Some(p) => Job::Slide(p),
                    None => {
                        *self.wallpaper.status.borrow_mut() = json!("slideshow: no pictures found in its folders");
                        return;
                    }
                }
            }
            Some(BackgroundType::Spotlight) => {
                // Explorer's Spotlight changes about once a day.
                if !next && st.spotlight.is_some() && elapsed < 24 * 3600 {
                    return;
                }
                Job::Spotlight { queue: st.spotlight_queue.clone() }
            }
            _ => return,
        };
        self.wallpaper.busy.set(true);
        let spawned = std::thread::Builder::new().name("wallpaper".into()).spawn(move || {
            let _com = fsh_win::com::ComGuard::mta();
            let result: anyhow::Result<(PathBuf, Option<SpotlightItem>, Vec<SpotlightItem>)> = (|| match job {
                Job::Slide(p) => {
                    fsh_win::wallpaper::set_wallpaper(&p)?;
                    Ok((p, None, vec![]))
                }
                Job::Spotlight { mut queue } => {
                    if queue.is_empty() {
                        queue = fetch_spotlight()?;
                    }
                    let item = queue.remove(0);
                    let path = download_spotlight(&item)?;
                    fsh_win::wallpaper::set_wallpaper(&path)?;
                    prune_spotlight(&path);
                    Ok((path, Some(item), queue))
                }
            })();
            let _ = slint::invoke_from_event_loop(move || {
                with(|a| a.wallpaper_done(result));
            });
        });
        if let Err(e) = spawned {
            self.wallpaper.busy.set(false);
            tracing::warn!("could not start the wallpaper thread: {e}");
        }
    }

    fn wallpaper_done(&self, result: anyhow::Result<(PathBuf, Option<SpotlightItem>, Vec<SpotlightItem>)>) {
        self.wallpaper.busy.set(false);
        match result {
            Ok((path, spotlight, queue)) => {
                tracing::info!("wallpaper: {}", path.display());
                let mut st = self.wallpaper.state.borrow_mut();
                st.last_change = now();
                if spotlight.is_some() {
                    st.spotlight = spotlight;
                    st.spotlight_queue = queue;
                } else {
                    st.slide = Some(path.clone());
                }
                if let Ok(text) = serde_json::to_string_pretty(&*st) {
                    let _ = std::fs::write(state_file(), text);
                }
                *self.wallpaper.status.borrow_mut() = json!({ "current": path });
            }
            Err(e) => {
                // Offline or refused: keep the current picture and try again next tick.
                tracing::warn!("wallpaper: {e:#}");
                *self.wallpaper.status.borrow_mut() = json!({ "error": format!("{e:#}") });
            }
        }
    }

    /// For the desktop menu: whether "Next desktop background" applies, and the current
    /// Spotlight picture's title (for "About this picture").
    pub(crate) fn wallpaper_menu(&self) -> (bool, Option<String>) {
        let kind = self.background_type();
        let next = matches!(kind, Some(BackgroundType::Slideshow | BackgroundType::Spotlight));
        let about = (kind == Some(BackgroundType::Spotlight))
            .then(|| self.wallpaper.state.borrow().spotlight.as_ref().map(|s| s.title.clone()))
            .flatten()
            .filter(|t| !t.is_empty());
        (next, about)
    }

    /// "About this picture": the Spotlight picture's page.
    pub(crate) fn wallpaper_about(&self) {
        let url = self.wallpaper.state.borrow().spotlight.as_ref().and_then(|s| s.learn_more.clone());
        if let Some(url) = url {
            crate::actions::launch(url);
        }
    }

    pub(crate) fn wallpaper_state(&self) -> Value {
        let st = self.wallpaper.state.borrow();
        json!({
            "background": self.background_type(),
            "status": *self.wallpaper.status.borrow(),
            "last_change": st.last_change,
            "spotlight": st.spotlight.as_ref().map(|s| json!({ "title": s.title, "copyright": s.copyright })),
            "spotlight_queued": st.spotlight_queue.len(),
        })
    }
}
