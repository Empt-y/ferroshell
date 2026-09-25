use std::fmt;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Schema version written by this build. Older files are migrated on load.
pub const CURRENT_VERSION: u32 = 1;

/// Shipped defaults, also written out (with comments) on first run.
pub const DEFAULT_CONFIG: &str = include_str!("../default-config.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub version: u32,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default, rename = "panel")]
    pub panels: Vec<PanelConfig>,
    #[serde(default)]
    pub launcher: LauncherConfig,
    #[serde(default)]
    pub session: SessionConfig,
}

/// Sign-in behaviour when Ferroshell is the login shell (`fsh-session --replace`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", default)]
pub struct SessionConfig {
    /// Start the apps Windows would (Run/RunOnce keys, Startup folders), as Explorer does.
    pub startup_apps: bool,
    /// Also start Store/packaged apps' startup tasks (Terminal, Spotify, ...).
    pub startup_tasks: bool,
    /// Seconds to wait after the desktop is up before starting them.
    pub startup_delay: u32,
    /// Names to skip: a Run value name, a Startup-folder file name, or a package family
    /// name, matched case-insensitively.
    pub startup_exclude: Vec<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { startup_apps: true, startup_tasks: true, startup_delay: 2, startup_exclude: vec![] }
    }
}

/// The application launcher (start menu).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", default)]
pub struct LauncherConfig {
    /// A lone Windows-key press opens the launcher instead of the Windows Start menu.
    pub windows_key: bool,
    pub width: u32,
    pub height: u32,
    /// App ids (as shown by the launcher), in order.
    pub favourites: Vec<String>,
    pub show_recent: bool,
    pub power_actions: Vec<String>,
    /// Web search URL with `{}` for the query; empty disables web results.
    pub web_search: String,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            windows_key: true,
            width: 640,
            height: 620,
            favourites: vec![],
            show_recent: true,
            power_actions: POWER_ACTIONS.iter().map(|s| (*s).to_owned()).collect(),
            web_search: String::new(),
        }
    }
}

pub const POWER_ACTIONS: &[&str] = &["lock", "sleep", "restart", "shutdown", "logout"];
pub const LAUNCHER_SIZE_RANGE: std::ops::RangeInclusive<u32> = 300..=2000;

fn default_theme() -> String {
    "breeze-dark".to_owned()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PanelConfig {
    #[serde(default)]
    pub monitor: MonitorSel,
    #[serde(default)]
    pub edge: Edge,
    #[serde(default = "default_thickness")]
    pub thickness: u32,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub widgets: Vec<WidgetEntry>,
}

fn default_thickness() -> u32 {
    44
}

pub const THICKNESS_RANGE: std::ops::RangeInclusive<u32> = 20..=200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorSel {
    Primary,
    #[default]
    All,
    Index(u32),
}

impl Serialize for MonitorSel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MonitorSel::Primary => s.serialize_str("primary"),
            MonitorSel::All => s.serialize_str("all"),
            MonitorSel::Index(i) => s.serialize_u32(*i),
        }
    }
}

impl<'de> Deserialize<'de> for MonitorSel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Index(u32),
        }
        match Raw::deserialize(d)? {
            Raw::Index(i) => Ok(MonitorSel::Index(i)),
            Raw::Name(n) => match n.as_str() {
                "primary" => Ok(MonitorSel::Primary),
                "all" => Ok(MonitorSel::All),
                other => Err(serde::de::Error::custom(format!(
                    "monitor must be \"primary\", \"all\" or a number, not \"{other}\""
                ))),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Edge {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub fn is_vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

/// A widget on a panel: its id plus whatever settings that widget's manifest declares.
/// Settings are validated against the widget's schema by the widget runtime, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetEntry {
    pub id: String,
    #[serde(flatten)]
    pub settings: toml::Table,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parse, migrate and validate config text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut table: toml::Table = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
        migrate(&mut table)?;
        let config: Config =
            Config::deserialize(toml::Value::Table(table)).map_err(|e| ConfigError(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn defaults() -> Self {
        // The shipped default is covered by a unit test, so this cannot fail in practice;
        // fall back to a bare config rather than panicking just in case.
        Self::parse(DEFAULT_CONFIG).unwrap_or(Config {
            version: CURRENT_VERSION,
            theme: default_theme(),
            panels: vec![],
            launcher: LauncherConfig::default(),
            session: SessionConfig::default(),
        })
    }

    fn validate(&self) -> Result<(), ConfigError> {
        for (i, p) in self.panels.iter().enumerate() {
            let n = i + 1;
            if !THICKNESS_RANGE.contains(&p.thickness) {
                return Err(ConfigError(format!(
                    "panel {n}: thickness {} is outside {}..={}",
                    p.thickness,
                    THICKNESS_RANGE.start(),
                    THICKNESS_RANGE.end()
                )));
            }
            for w in &p.widgets {
                if w.id.trim().is_empty() {
                    return Err(ConfigError(format!("panel {n}: a widget has an empty id")));
                }
            }
        }
        let l = &self.launcher;
        if !LAUNCHER_SIZE_RANGE.contains(&l.width) || !LAUNCHER_SIZE_RANGE.contains(&l.height) {
            return Err(ConfigError(format!(
                "launcher: width and height must be within {}..={}",
                LAUNCHER_SIZE_RANGE.start(),
                LAUNCHER_SIZE_RANGE.end()
            )));
        }
        if let Some(bad) = l.power_actions.iter().find(|a| !POWER_ACTIONS.contains(&a.as_str())) {
            return Err(ConfigError(format!("launcher: unknown power action `{bad}` (use {POWER_ACTIONS:?})")));
        }
        if self.theme.trim().is_empty() {
            return Err(ConfigError("theme must not be empty".into()));
        }
        Ok(())
    }
}

type Migration = fn(&mut toml::Table);

/// `MIGRATIONS[i]` upgrades a version `i + 1` file to version `i + 2`.
const MIGRATIONS: &[Migration] = &[];

fn migrate(table: &mut toml::Table) -> Result<(), ConfigError> {
    let version = match table.get("version") {
        None => 1,
        Some(toml::Value::Integer(v)) if *v >= 1 => *v as u32,
        Some(other) => return Err(ConfigError(format!("version must be a positive integer, not {other}"))),
    };
    if version > CURRENT_VERSION {
        return Err(ConfigError(format!(
            "config version {version} is newer than this Ferroshell understands ({CURRENT_VERSION}); update Ferroshell"
        )));
    }
    for (i, m) in MIGRATIONS.iter().enumerate().skip(version as usize - 1) {
        tracing::info!("migrating config from version {} to {}", i + 1, i + 2);
        m(table);
    }
    table.insert("version".into(), toml::Value::Integer(CURRENT_VERSION.into()));
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    Loaded(Config),
    /// The file was broken; `config` came from the last-known-good copy or the defaults.
    Fallback { config: Config, error: String, from_last_good: bool },
}

impl LoadOutcome {
    pub fn config(&self) -> &Config {
        match self {
            LoadOutcome::Loaded(c) | LoadOutcome::Fallback { config: c, .. } => c,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            LoadOutcome::Loaded(_) => None,
            LoadOutcome::Fallback { error, .. } => Some(error),
        }
    }
}

/// Load `path`, creating it from the defaults if missing. A config that loads cleanly is
/// copied to `last_good`; a broken one falls back to `last_good`, then to the defaults.
pub fn load(path: &Path, last_good: &Path) -> LoadOutcome {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, DEFAULT_CONFIG) {
            tracing::warn!("could not write default config to {}: {e}", path.display());
        }
    }

    let result = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("reading {}: {e}", path.display())))
        .and_then(|text| Config::parse(&text).map(|c| (c, text)));

    match result {
        Ok((config, text)) => {
            if let Some(dir) = last_good.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(last_good, text) {
                tracing::warn!("could not save last-known-good config: {e}");
            }
            LoadOutcome::Loaded(config)
        }
        Err(error) => {
            tracing::error!("config error in {}: {error}", path.display());
            let last = std::fs::read_to_string(last_good).ok().and_then(|t| Config::parse(&t).ok());
            let from_last_good = last.is_some();
            LoadOutcome::Fallback {
                config: last.unwrap_or_else(Config::defaults),
                error: error.0,
                from_last_good,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.version, CURRENT_VERSION);
        assert_eq!(c.panels.len(), 1);
        let p = &c.panels[0];
        assert_eq!((p.edge, p.monitor, p.thickness), (Edge::Bottom, MonitorSel::All, 44));
        let clock = p.widgets.iter().find(|w| w.id == "org.ferroshell.clock").unwrap();
        assert_eq!(clock.settings["format"].as_str(), Some("%H:%M"));
    }

    #[test]
    fn minimal_config_gets_defaults() {
        let c = Config::parse("[[panel]]\nedge = \"left\"\nmonitor = 1").unwrap();
        assert_eq!(c.version, CURRENT_VERSION);
        assert_eq!(c.theme, "breeze-dark");
        assert_eq!(c.panels[0].edge, Edge::Left);
        assert_eq!(c.panels[0].monitor, MonitorSel::Index(1));
        assert_eq!(c.panels[0].thickness, 44);
    }

    #[test]
    fn launcher_section() {
        let c = Config::parse("[launcher]\nwindows-key = false\nfavourites = ['a', 'b']").unwrap();
        assert!(!c.launcher.windows_key);
        assert_eq!(c.launcher.favourites, ["a", "b"]);
        assert_eq!(c.launcher.width, 640, "defaults fill the rest");
        assert_eq!(Config::parse("").unwrap().launcher, LauncherConfig::default());
        assert!(Config::parse("[launcher]\nwidth = 10").unwrap_err().0.contains("width"));
        assert!(Config::parse("[launcher]\npower-actions = ['explode']").unwrap_err().0.contains("explode"));
        assert!(Config::parse("[launcher]\nwinkey = true").is_err(), "typos are caught");
    }

    #[test]
    fn reports_useful_errors() {
        let e = Config::parse("[[panel]]\nedge = \"middle\"").unwrap_err().0;
        assert!(e.contains("middle"), "{e}");
        let e = Config::parse("[[panel]]\nthickness = 5").unwrap_err().0;
        assert!(e.contains("thickness 5"), "{e}");
        let e = Config::parse("[[panel]]\nmonitor = \"left\"").unwrap_err().0;
        assert!(e.contains("primary"), "{e}");
        let e = Config::parse("tehme = \"x\"").unwrap_err().0;
        assert!(e.contains("tehme"), "{e}");
        let e = Config::parse("version = 99").unwrap_err().0;
        assert!(e.contains("newer"), "{e}");
    }

    #[test]
    fn falls_back_to_last_good_then_defaults() {
        let dir = std::env::temp_dir().join(format!("fsh-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (path, good) = (dir.join("config.toml"), dir.join("state").join("config.lastgood.toml"));

        // First run writes the defaults and records them as last-good.
        assert!(matches!(load(&path, &good), LoadOutcome::Loaded(_)));
        assert!(good.exists());

        std::fs::write(&path, "theme = \"custom\"").unwrap();
        assert_eq!(load(&path, &good).config().theme, "custom");

        // Breaking the file keeps the last good config.
        std::fs::write(&path, "theme = ").unwrap();
        let out = load(&path, &good);
        assert!(matches!(&out, LoadOutcome::Fallback { from_last_good: true, .. }));
        assert_eq!(out.config().theme, "custom");

        // With no last-good either, defaults are used.
        std::fs::remove_file(&good).unwrap();
        let out = load(&path, &good);
        assert!(matches!(&out, LoadOutcome::Fallback { from_last_good: false, .. }));
        assert_eq!(out.config(), &Config::defaults());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
