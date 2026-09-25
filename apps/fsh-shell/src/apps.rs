//! The installed-app index for the launcher, built on an "apps" worker thread.
//!
//! Startup is instant from a cache (`apps.json`); the real scan follows and replaces it.
//! The index is rebuilt when the Start-menu folders change (checked every few seconds)
//! and every 10 minutes (Store installs don't touch those folders). Icons are loaded after
//! each scan and sent in batches.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use fsh_core::launcher::AppEntry;
use fsh_win::appsfolder::ShellApp;
use fsh_win::icon::{RgbaImage, shell_item_icon};

const CHECK_EVERY: Duration = Duration::from_secs(5);
const RESCAN_EVERY: Duration = Duration::from_secs(600);
const ICON_SIZE: i32 = 64;
const ICON_BATCH: usize = 24;

pub enum Update {
    Apps(Vec<AppEntry>),
    Icons(Vec<(String, RgbaImage)>),
}

pub struct AppIndexer {
    tx: Sender<()>,
}

impl AppIndexer {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("apps".into()).spawn(move || run(&rx, &on_update))?;
        Ok(Self { tx })
    }

    /// Rescan now (e.g. the user asked for a refresh).
    pub fn refresh(&self) {
        let _ = self.tx.send(());
    }
}

fn cache_path() -> PathBuf {
    fsh_common::paths::state_dir().join("apps.json")
}

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for (var, sub) in [("ProgramData", r"Microsoft\Windows\Start Menu\Programs"), ("APPDATA", r"Microsoft\Windows\Start Menu\Programs")] {
        if let Some(base) = std::env::var_os(var) {
            dirs.push(PathBuf::from(base).join(sub));
        }
    }
    dirs
}

fn run(rx: &Receiver<()>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::new();
    let mut current: Vec<AppEntry> =
        std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !current.is_empty() {
        on_update(Update::Apps(current.clone()));
    }
    let mut sent_icons: HashSet<String> = HashSet::new();
    let mut fingerprint = crate::watch::fingerprint(&start_menu_dirs());
    let mut last_scan: Option<Instant> = None;
    let mut force = true;
    loop {
        if force || last_scan.is_none_or(|t| t.elapsed() >= RESCAN_EVERY) {
            force = false;
            let started = Instant::now();
            match index() {
                Ok(apps) => {
                    tracing::info!("indexed {} apps in {:?}", apps.len(), started.elapsed());
                    if apps != current {
                        current = apps;
                        if let Ok(json) = serde_json::to_vec(&current) {
                            let _ = std::fs::write(cache_path(), json);
                        }
                        on_update(Update::Apps(current.clone()));
                    }
                }
                Err(e) => tracing::warn!("app index failed: {e:#}"),
            }
            last_scan = Some(Instant::now());
            send_icons(&current, &mut sent_icons, on_update);
        }
        match rx.recv_timeout(CHECK_EVERY) {
            Ok(()) => force = true,
            Err(RecvTimeoutError::Timeout) => {
                let fp = crate::watch::fingerprint(&start_menu_dirs());
                if fp != fingerprint {
                    fingerprint = fp;
                    // Installers write several shortcuts; let them finish.
                    std::thread::sleep(Duration::from_secs(2));
                    force = true;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn send_icons(apps: &[AppEntry], sent: &mut HashSet<String>, on_update: &dyn Fn(Update)) {
    let mut batch = Vec::new();
    for app in apps {
        if !sent.insert(app.id.clone()) {
            continue;
        }
        if let Some(img) = shell_item_icon(&app.launch_target(), ICON_SIZE) {
            batch.push((app.id.clone(), img));
        }
        if batch.len() >= ICON_BATCH {
            on_update(Update::Icons(std::mem::take(&mut batch)));
        }
    }
    if !batch.is_empty() {
        on_update(Update::Icons(batch));
    }
}

/// Shortcut name (lowercase) → (Start-menu folder, shortcut path).
fn shortcut_folders() -> HashMap<String, (String, PathBuf)> {
    let mut map = HashMap::new();
    for root in start_menu_dirs() {
        walk(&root, &root, 0, &mut map);
    }
    map
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut HashMap<String, (String, PathBuf)>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(root, &path, depth + 1, out);
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            let folder = path
                .strip_prefix(root)
                .ok()
                .and_then(|rel| rel.components().next())
                .filter(|_| path.parent() != Some(root))
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            if folder.eq_ignore_ascii_case("startup") {
                continue;
            }
            if let Some(stem) = path.file_stem() {
                out.entry(stem.to_string_lossy().to_lowercase()).or_insert((folder, path.clone()));
            }
        }
    }
}

/// Is this something to list? Skips documentation links and uninstallers, which Start
/// menus are full of.
fn wanted(app: &ShellApp) -> bool {
    let name = app.name.to_lowercase();
    let id = app.parsing_name.to_lowercase();
    if name.contains("uninstall") || id.starts_with("http:") || id.starts_with("https:") {
        return false;
    }
    const DOCS: &[&str] = &[".html", ".htm", ".txt", ".chm", ".pdf", ".url", ".rtf", ".md", ".ini", ".log", ".xml"];
    !DOCS.iter().any(|ext| id.ends_with(ext))
}

fn to_entry(app: ShellApp, folders: &HashMap<String, (String, PathBuf)>) -> AppEntry {
    let elevatable = app.elevatable();
    let id = app.parsing_name;
    let packaged = id.contains('!') && !id.contains('\\');
    let mut keywords = Vec::new();
    let mut path = None;
    if packaged {
        // "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App" → "WindowsCalculator"
        if let Some(pkg) = id.split('_').next().and_then(|p| p.rsplit('.').next()) {
            keywords.push(pkg.to_owned());
        }
    } else if let Some(stem) = Path::new(&id).file_stem() {
        keywords.push(stem.to_string_lossy().into_owned());
        if Path::new(&id).is_absolute() {
            path = Some(id.clone());
        }
    }
    let shortcut = folders.get(&app.name.to_lowercase());
    let category = match (shortcut, packaged) {
        (Some((folder, _)), _) if !folder.is_empty() => folder.clone(),
        (_, true) => "Apps".to_owned(),
        _ => String::new(),
    };
    if let Some((_, lnk)) = shortcut {
        path = Some(lnk.to_string_lossy().into_owned());
        if let Some(stem) = lnk.file_stem() {
            let s = stem.to_string_lossy().into_owned();
            if !keywords.contains(&s) && s != app.name {
                keywords.push(s);
            }
        }
    }
    AppEntry { id, name: app.name, keywords, category, path, elevatable }
}

fn index() -> anyhow::Result<Vec<AppEntry>> {
    let folders = shortcut_folders();
    let mut seen = HashSet::new();
    let mut apps: Vec<AppEntry> = fsh_win::appsfolder::enumerate()?
        .into_iter()
        .filter(wanted)
        .filter(|a| seen.insert(a.parsing_name.to_lowercase()))
        .map(|a| to_entry(a, &folders))
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, id: &str) -> ShellApp {
        ShellApp { name: name.into(), parsing_name: id.into(), host_environment: None }
    }

    /// Indexes the real apps on this machine and runs real searches; opt-in (no UI).
    #[test]
    #[ignore = "reads the live app list"]
    fn live_index_and_search() {
        use fsh_core::launcher::{History, SearchOptions, search};
        let _com = fsh_win::com::ComGuard::new();
        let t = Instant::now();
        let apps = index().unwrap();
        eprintln!("indexed {} apps in {:?}", apps.len(), t.elapsed());
        let cats = fsh_core::launcher::categories(&apps);
        eprintln!("categories: {cats:?}");
        let top = |q: &str| search(q, &apps, &History::default(), 0, &SearchOptions::default()).into_iter().next().map(|r| r.title);
        for q in ["calc", "note", "settings", "term", "12*(3+4)", "bluetooth"] {
            eprintln!("{q:>10} -> {:?}", top(q));
        }
        assert_eq!(top("calc").as_deref(), Some("Calculator"));
        assert_eq!(top("12*(3+4)").as_deref(), Some("= 84"));
        assert!(apps.iter().all(|a| !a.name.to_lowercase().contains("uninstall")));
        let t = Instant::now();
        let mut n = 0;
        for app in apps.iter().take(20) {
            n += usize::from(shell_item_icon(&app.launch_target(), ICON_SIZE).is_some());
        }
        eprintln!("{n}/20 icons in {:?}", t.elapsed());
        assert!(n >= 15);
    }

    #[test]
    fn filters_docs_and_uninstallers() {
        assert!(wanted(&app("Firefox", r"C:\Program Files\Mozilla Firefox\firefox.exe")));
        assert!(wanted(&app("Calculator", "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")));
        assert!(!wanted(&app("Python Manuals", r"C:\Python314\Doc\html\index.html")));
        assert!(!wanted(&app("Uninstall Foo", r"C:\Foo\unins000.exe")));
        assert!(!wanted(&app("Website", "https://example.com")));
    }

    #[test]
    fn builds_entries_with_keywords_and_categories() {
        let mut folders = HashMap::new();
        folders.insert("steam".to_owned(), ("Steam".to_owned(), PathBuf::from(r"C:\SM\Steam\Steam.lnk")));
        let steam = to_entry(app("Steam", r"C:\Program Files (x86)\Steam\steam.exe"), &folders);
        assert_eq!(steam.category, "Steam");
        assert_eq!(steam.keywords, ["steam"]);
        assert_eq!(steam.path.as_deref(), Some(r"C:\SM\Steam\Steam.lnk"));

        let calc = to_entry(app("Calculator", "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), &folders);
        assert_eq!((calc.category.as_str(), calc.keywords[0].as_str()), ("Apps", "WindowsCalculator"));
        assert_eq!(calc.path, None);

        let loose = to_entry(app("Tool", r"C:\Tools\tool.exe"), &folders);
        assert_eq!(loose.category, "");
        assert_eq!(loose.path.as_deref(), Some(r"C:\Tools\tool.exe"));
    }
}
