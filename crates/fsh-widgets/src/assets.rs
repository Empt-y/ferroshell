//! Built-in assets (the `@ferroshell` Slint library, widgets and themes) are compiled into
//! the binary and extracted to `%LOCALAPPDATA%\ferroshell\builtin` at startup. Keeping them
//! as real files means the interpreter loads them like any user file, and users can read
//! them as a starting point for their own widgets and themes.

use std::path::{Path, PathBuf};

use include_dir::{Dir, DirEntry, include_dir};

static ASSETS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets");

/// Where everything lives on disk.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Extracted built-in assets.
    pub builtin: PathBuf,
    /// `%APPDATA%\ferroshell` — the user's config, widgets and themes.
    pub user: PathBuf,
}

impl Layout {
    pub fn new(builtin: PathBuf, user: PathBuf) -> Self {
        Self { builtin, user }
    }

    /// Directory mapped to the `@ferroshell` import prefix.
    pub fn library_dir(&self) -> PathBuf {
        self.builtin.join("ferroshell")
    }
    pub fn builtin_widgets(&self) -> PathBuf {
        self.builtin.join("widgets")
    }
    pub fn builtin_themes(&self) -> PathBuf {
        self.builtin.join("themes")
    }
    pub fn user_widgets(&self) -> PathBuf {
        self.user.join("widgets")
    }
    pub fn user_themes(&self) -> PathBuf {
        self.user.join("themes")
    }

    /// Theme directory for `name`: the user's first, then built-in.
    pub fn theme_dir(&self, name: &str) -> Option<PathBuf> {
        [self.user_themes(), self.builtin_themes()]
            .into_iter()
            .map(|d| d.join(name))
            .find(|d| d.join("theme.toml").is_file())
    }

    /// Write the built-in assets to [`Self::builtin`], replacing files whose content differs.
    pub fn extract_builtin(&self) -> std::io::Result<usize> {
        extract(&ASSETS, &self.builtin)
    }
}

fn extract(dir: &Dir<'_>, to: &Path) -> std::io::Result<usize> {
    let mut written = 0;
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(d) => written += extract(d, to)?,
            DirEntry::File(f) => {
                let dest = to.join(f.path());
                if std::fs::read(&dest).ok().as_deref() != Some(f.contents()) {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, f.contents())?;
                    written += 1;
                }
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A layout with built-ins extracted into a fresh temp dir.
    pub(crate) fn temp_layout(tag: &str) -> Layout {
        let root = std::env::temp_dir().join(format!("fsh-widgets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::new(root.join("builtin"), root.join("user"));
        layout.extract_builtin().unwrap();
        layout
    }

    #[test]
    fn extracts_and_is_idempotent() {
        let layout = temp_layout("extract");
        assert!(layout.library_dir().join("api.slint").is_file());
        assert!(layout.builtin_widgets().join("org.ferroshell.clock").join("widget.toml").is_file());
        assert_eq!(layout.extract_builtin().unwrap(), 0, "second extraction writes nothing");
        assert!(layout.theme_dir("breeze-dark").is_some());
        assert!(layout.theme_dir("nope").is_none());
    }
}
