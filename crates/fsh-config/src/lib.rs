//! User configuration (`config.toml`) and themes (`theme.toml`): schema, parsing,
//! validation, migrations and the last-known-good fallback.

pub mod color;
pub mod config;
pub mod edit;
pub mod theme;

pub use color::Rgba;
pub use config::{Config, Edge, LauncherConfig, LoadOutcome, MonitorSel, PanelConfig, WidgetEntry};
pub use edit::ConfigEditor;
pub use theme::{Backdrop, Theme};
