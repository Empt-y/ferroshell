//! `widget.toml`: identity plus a typed config schema. The schema drives three things: the
//! Slint property values the composer sets on the widget, validation of the user's
//! settings, and the auto-generated form in the settings app.

use fsh_config::Rgba;
use serde::Deserialize;

/// The widget API version this shell implements (`api = 1` in `widget.toml`).
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub api: u32,
    #[serde(default)]
    /// In the order written in `widget.toml` (the settings form uses this order).
    pub config: indexmap::IndexMap<String, ConfigField>,
    /// Optional Rhai script, run once per widget instance.
    #[serde(default)]
    pub script: Option<ScriptSpec>,
    /// Optional out-of-process plugin, run once per widget package.
    #[serde(default)]
    pub plugin: Option<PluginSpec>,
    /// Optional popup (an applet's detail view), opened from the widget.
    #[serde(default)]
    pub popup: Option<PopupSpec>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PopupSpec {
    /// Slint file in the widget folder exporting a component named `Popup`.
    #[serde(default = "default_popup")]
    pub file: String,
    /// Size in logical pixels.
    #[serde(default = "default_popup_width")]
    pub width: u32,
    #[serde(default = "default_popup_height")]
    pub height: u32,
}

fn default_popup() -> String {
    "popup.slint".into()
}
fn default_popup_width() -> u32 {
    360
}
fn default_popup_height() -> u32 {
    440
}

/// Which component a setting is passed to.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    #[default]
    Widget,
    Popup,
    Both,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ScriptSpec {
    /// Script file, relative to the widget folder.
    #[serde(default = "default_script")]
    pub file: String,
    /// Call the script's `tick()` every this many seconds; 0 = never.
    #[serde(default)]
    pub interval: f64,
}

fn default_script() -> String {
    "logic.rhai".into()
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PluginSpec {
    /// Executable, relative to the widget folder (or an absolute path / a program on PATH).
    pub exec: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Memory limit for the plugin process, in MiB.
    #[serde(default = "default_memory")]
    pub max_memory_mb: u64,
}

fn default_memory() -> u64 {
    256
}

impl Manifest {
    /// Widgets with a script or plugin get an `instance-id` property to namespace their data.
    pub fn wants_instance_id(&self) -> bool {
        self.script.is_some() || self.plugin.is_some() || self.popup.is_some()
    }
}

fn default_version() -> String {
    "0.0.0".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigField {
    #[serde(flatten)]
    pub kind: FieldKind,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// `false` for settings only the shell reads (e.g. pinned apps). They aren't passed to
    /// the widget's Slint properties, so changing them doesn't rebuild the panel.
    #[serde(default = "yes")]
    pub ui: bool,
    /// For widgets with a popup: pass this setting to the widget (default), the popup,
    /// or both. Each component must declare the settings it receives.
    #[serde(default)]
    pub scope: Scope,
}

impl ConfigField {
    pub fn for_widget(&self) -> bool {
        self.ui && matches!(self.scope, Scope::Widget | Scope::Both)
    }
    pub fn for_popup(&self) -> bool {
        self.ui && matches!(self.scope, Scope::Popup | Scope::Both)
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FieldKind {
    Bool { default: bool },
    Int { default: i64, min: Option<i64>, max: Option<i64> },
    Float { default: f64, min: Option<f64>, max: Option<f64> },
    String { default: String },
    Enum { default: String, options: Vec<String> },
    Color { default: Rgba },
    StringList { #[serde(default)] default: Vec<String> },
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), String> {
        if self.api != API_VERSION {
            return Err(format!(
                "widget needs API version {}, this Ferroshell provides {API_VERSION}",
                self.api
            ));
        }
        let id_ok = !self.id.is_empty()
            && self.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        if !id_ok {
            return Err(format!("invalid widget id `{}` (use letters, digits, '.', '-', '_')", self.id));
        }
        for (key, field) in &self.config {
            if !is_identifier(key) || key == "id" {
                return Err(format!("config key `{key}` is not a valid property name"));
            }
            if RESERVED.contains(&key.as_str()) {
                return Err(format!("config key `{key}` clashes with a built-in property; choose another name"));
            }
            match &field.kind {
                FieldKind::Int { default, min, max } => {
                    if min.zip(*max).is_some_and(|(a, b)| a > b) {
                        return Err(format!("config.{key}: min is greater than max"));
                    }
                    if min.is_some_and(|m| *default < m) || max.is_some_and(|m| *default > m) {
                        return Err(format!("config.{key}: default is out of range"));
                    }
                }
                FieldKind::Float { default, min, max } => {
                    if min.is_some_and(|m| *default < m) || max.is_some_and(|m| *default > m) {
                        return Err(format!("config.{key}: default is out of range"));
                    }
                }
                FieldKind::Enum { default, options } if !options.contains(default) => {
                    return Err(format!("config.{key}: default `{default}` is not one of the options"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Properties every Slint element already has, plus names the shell sets itself.
const RESERVED: &[&str] = &[
    "x", "y", "z", "width", "height", "min-width", "max-width", "min-height", "max-height",
    "preferred-width", "preferred-height", "horizontal-stretch", "vertical-stretch", "opacity",
    "visible", "enabled", "clip", "background", "border-radius", "border-width", "border-color",
    "instance-id",
];

/// A valid Slint property name: letter or `_`, then letters, digits, `-`, `_`.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Escape text for a Slint string literal (`\` starts escapes and `\{` interpolation).
pub fn slint_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn float_literal(v: f64) -> String {
    if v.is_finite() { format!("{v:?}") } else { "0".into() }
}

impl FieldKind {
    /// The Slint literal for `value` (the user's setting), or for the default if it's
    /// missing or invalid — in which case a warning explains why.
    pub fn literal(&self, value: Option<&toml::Value>) -> (String, Option<String>) {
        let mut warning = None;
        let mut bad = |expected: &str| {
            warning = Some(format!("expected {expected}, got {}", value.map(|v| v.to_string()).unwrap_or_default()));
        };
        let lit = match self {
            FieldKind::Bool { default } => {
                let v = match value {
                    None => *default,
                    Some(toml::Value::Boolean(b)) => *b,
                    Some(_) => {
                        bad("true or false");
                        *default
                    }
                };
                v.to_string()
            }
            FieldKind::Int { default, min, max } => {
                let v = match value {
                    None => *default,
                    Some(toml::Value::Integer(i)) => *i,
                    Some(_) => {
                        bad("a whole number");
                        *default
                    }
                };
                let clamped = v.max(min.unwrap_or(i64::MIN)).min(max.unwrap_or(i64::MAX));
                if clamped != v {
                    warning = Some(format!("{v} is out of range, using {clamped}"));
                }
                clamped.to_string()
            }
            FieldKind::Float { default, min, max } => {
                let v = match value {
                    None => *default,
                    Some(toml::Value::Float(f)) => *f,
                    Some(toml::Value::Integer(i)) => *i as f64,
                    Some(_) => {
                        bad("a number");
                        *default
                    }
                };
                let clamped = v.max(min.unwrap_or(f64::MIN)).min(max.unwrap_or(f64::MAX));
                if clamped != v {
                    warning = Some(format!("{v} is out of range, using {clamped}"));
                }
                float_literal(clamped)
            }
            FieldKind::String { default } => match value {
                None => slint_string(default),
                Some(toml::Value::String(s)) => slint_string(s),
                Some(_) => {
                    bad("text");
                    slint_string(default)
                }
            },
            FieldKind::Enum { default, options } => match value {
                None => slint_string(default),
                Some(toml::Value::String(s)) if options.contains(s) => slint_string(s),
                Some(_) => {
                    bad(&format!("one of {options:?}"));
                    slint_string(default)
                }
            },
            FieldKind::Color { default } => {
                let c = match value {
                    None => *default,
                    Some(toml::Value::String(s)) => match s.parse::<Rgba>() {
                        Ok(c) => c,
                        Err(e) => {
                            warning = Some(e);
                            *default
                        }
                    },
                    Some(_) => {
                        bad("a colour like \"#rrggbb\"");
                        *default
                    }
                };
                format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
            }
            FieldKind::StringList { default } => {
                let items: Vec<String> = match value {
                    None => default.clone(),
                    Some(toml::Value::Array(a)) if a.iter().all(toml::Value::is_str) => {
                        a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
                    }
                    Some(_) => {
                        bad("a list of text");
                        default.clone()
                    }
                };
                let inner: Vec<String> = items.iter().map(|s| slint_string(s)).collect();
                format!("[{}]", inner.join(", "))
            }
        };
        (lit, warning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCKISH: &str = r##"
        id = "com.example.w"
        name = "W"
        api = 1
        [config.fmt]
        type = "string"
        default = "%H:%M"
        [config.n]
        type = "int"
        default = 5
        min = 0
        max = 10
        [config.mode]
        type = "enum"
        default = "a"
        options = ["a", "b"]
        [config.tint]
        type = "color"
        default = "#ff0000"
        [config.items]
        type = "string-list"
    "##;

    #[test]
    fn parses_schema() {
        let m = Manifest::parse(CLOCKISH).unwrap();
        assert_eq!(m.config.len(), 5);
        assert_eq!(m.config["n"].kind, FieldKind::Int { default: 5, min: Some(0), max: Some(10) });
        assert_eq!(m.config["items"].kind, FieldKind::StringList { default: vec![] });
    }

    #[test]
    fn rejects_bad_manifests() {
        assert!(Manifest::parse("id='x'\nname='x'\napi=2").unwrap_err().contains("API version"));
        assert!(Manifest::parse("id='a b'\nname='x'\napi=1").unwrap_err().contains("invalid widget id"));
        let bad_key = "id='x'\nname='x'\napi=1\n[config.'9lives']\ntype='bool'\ndefault=true";
        assert!(Manifest::parse(bad_key).unwrap_err().contains("property name"));
        let reserved = "id='x'\nname='x'\napi=1\n[config.max-width]\ntype='int'\ndefault=1";
        assert!(Manifest::parse(reserved).unwrap_err().contains("built-in property"));
        let bad_enum = "id='x'\nname='x'\napi=1\n[config.m]\ntype='enum'\ndefault='z'\noptions=['a']";
        assert!(Manifest::parse(bad_enum).unwrap_err().contains("not one of"));
    }

    #[test]
    fn literals_escape_validate_and_clamp() {
        let m = Manifest::parse(CLOCKISH).unwrap();
        let lit = |k: &str, v: Option<toml::Value>| m.config[k].kind.literal(v.as_ref());

        assert_eq!(lit("fmt", None), ("\"%H:%M\"".into(), None));
        assert_eq!(lit("fmt", Some("a\"b\\c\n{x}".into())).0, r#""a\"b\\c\n{x}""#);
        assert_eq!(lit("n", Some(42.into())), ("10".into(), Some("42 is out of range, using 10".into())));
        assert!(lit("n", Some("five".into())).1.is_some());
        assert_eq!(lit("mode", Some("b".into())).0, "\"b\"");
        assert_eq!(lit("mode", Some("c".into())).0, "\"a\"");
        assert_eq!(lit("tint", Some("#00ff0080".into())).0, "#00ff0080");
        let list = toml::Value::Array(vec!["x".into(), "y\"".into()]);
        assert_eq!(lit("items", Some(list)).0, r#"["x", "y\""]"#);
    }
}
