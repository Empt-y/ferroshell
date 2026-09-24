//! Discovering widget packages. A package is a folder containing `widget.toml` and
//! `ui.slint`. Folders are searched in priority order (user, then theme, then built-in),
//! and the first package with a given id wins — that's how overrides work.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::manifest::Manifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    User,
    Theme,
    Builtin,
}

#[derive(Debug, Clone)]
pub struct WidgetPackage {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub origin: Origin,
}

impl WidgetPackage {
    pub fn ui_path(&self) -> PathBuf {
        self.dir.join("ui.slint")
    }
}

#[derive(Debug, Default)]
pub struct Registry {
    packages: BTreeMap<String, WidgetPackage>,
    /// Packages that failed to load, keyed by id (or folder name if the id is unknown).
    broken: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

impl Registry {
    /// Scan `roots` in priority order.
    pub fn scan(roots: &[(PathBuf, Origin)]) -> Self {
        let mut reg = Registry::default();
        for (root, origin) in roots {
            let Ok(entries) = std::fs::read_dir(root) else { continue };
            let mut dirs: Vec<PathBuf> =
                entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for dir in dirs {
                reg.load_package(&dir, *origin);
            }
        }
        reg
    }

    fn load_package(&mut self, dir: &Path, origin: Origin) {
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let manifest_path = dir.join("widget.toml");
        if !manifest_path.is_file() {
            return;
        }
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|t| Manifest::parse(&t));
        let manifest = match manifest {
            Ok(m) => m,
            Err(e) => {
                let msg = format!("{}: {e}", manifest_path.display());
                tracing::warn!("widget package skipped: {msg}");
                // A broken override must not silently fall back to a lower-priority package.
                self.broken.entry(folder).or_insert(msg);
                return;
            }
        };
        if manifest.id != folder {
            self.warnings.push(format!(
                "{}: folder name `{folder}` doesn't match widget id `{}`",
                dir.display(),
                manifest.id
            ));
        }
        if !dir.join("ui.slint").is_file() {
            self.broken.entry(manifest.id.clone()).or_insert_with(|| format!("{}: missing ui.slint", dir.display()));
            return;
        }
        if self.packages.contains_key(&manifest.id) || self.broken.contains_key(&manifest.id) {
            return; // shadowed by a higher-priority package
        }
        self.packages.insert(manifest.id.clone(), WidgetPackage { manifest, dir: dir.to_owned(), origin });
    }

    /// Look up a widget. `Err` explains why it can't be used.
    pub fn get(&self, id: &str) -> Result<&WidgetPackage, String> {
        if let Some(reason) = self.broken.get(id) {
            return Err(reason.clone());
        }
        self.packages.get(id).ok_or_else(|| format!("no widget with id `{id}` is installed"))
    }

    /// A copy of `config` with every shell-only (`ui = false`) widget setting removed. Two
    /// configs that are equal after this only differ in settings that don't need the
    /// panels rebuilt.
    pub fn strip_shell_only(&self, config: &fsh_config::Config) -> fsh_config::Config {
        let mut c = config.clone();
        // The launcher isn't part of any panel.
        c.launcher = Default::default();
        for p in &mut c.panels {
            for w in &mut p.widgets {
                if let Some(pkg) = self.packages.get(&w.id) {
                    w.settings.retain(|k, _| pkg.manifest.config.get(k).is_none_or(|f| f.ui));
                }
            }
        }
        c
    }

    pub fn packages(&self) -> impl Iterator<Item = &WidgetPackage> {
        self.packages.values()
    }

    pub fn broken(&self) -> impl Iterator<Item = (&str, &str)> {
        self.broken.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tests::temp_layout;

    fn write_widget(root: &Path, folder: &str, manifest: &str, ui: Option<&str>) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("widget.toml"), manifest).unwrap();
        if let Some(ui) = ui {
            std::fs::write(dir.join("ui.slint"), ui).unwrap();
        }
    }

    #[test]
    fn finds_builtins_and_user_overrides_win() {
        let layout = temp_layout("registry");
        let user = layout.user_widgets();
        write_widget(&user, "org.ferroshell.clock", "id='org.ferroshell.clock'\nname='My clock'\napi=1", Some(""));
        write_widget(&user, "broken", "this is not toml", None);
        write_widget(&user, "org.ferroshell.spacer", "id='org.ferroshell.spacer'\nname='x'\napi=99", Some(""));

        let reg = Registry::scan(&[(user, Origin::User), (layout.builtin_widgets(), Origin::Builtin)]);
        let clock = reg.get("org.ferroshell.clock").unwrap();
        assert_eq!((clock.origin, clock.manifest.name.as_str()), (Origin::User, "My clock"));
        assert_eq!(reg.get("org.ferroshell.taskmanager").unwrap().origin, Origin::Builtin);
        // A broken user override blocks the built-in rather than silently falling back.
        assert!(reg.get("org.ferroshell.spacer").unwrap_err().contains("API version"));
        assert!(reg.get("nope").unwrap_err().contains("no widget"));
        assert_eq!(reg.broken().count(), 2);
    }
}
