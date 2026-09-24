//! Themes: named design tokens (colours and metrics) that widgets read through the `Theme`
//! Slint global. A theme only overrides the tokens it mentions; everything else keeps the
//! built-in default, so a theme can be as small as one line.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::Rgba;

/// Every colour token, with its default (Breeze Dark). Order is the documentation order.
pub const COLOR_TOKENS: &[(&str, &str, &str)] = &[
    ("panel-background", "#202326e6", "Panel fill (alpha lets the backdrop show through)"),
    ("panel-border", "#ffffff14", "Hairline along the panel's inner edge"),
    ("foreground", "#fcfcfc", "Primary text and icons"),
    ("foreground-dim", "#a1a9b1", "Secondary text"),
    ("accent", "#3daee9", "Accent colour; `accent = \"system\"` follows Windows"),
    ("hover", "#ffffff14", "Background of hovered items"),
    ("pressed", "#ffffff24", "Background of pressed items"),
    ("highlight", "#3daee940", "Background of the active task"),
    ("attention", "#f67400", "Tasks asking for attention"),
    ("popup-background", "#202326f5", "Tooltips, menus and thumbnail popups"),
    ("error", "#da4453", "Error badges and broken widgets"),
];

/// Every metric token (logical pixels), with its default.
pub const METRIC_TOKENS: &[(&str, f32, &str)] = &[
    ("radius", 4.0, "Corner radius of items"),
    ("panel-radius", 8.0, "Corner radius of floating panels"),
    ("floating-margin", 6.0, "Gap between a floating panel and the screen edge"),
    ("spacing", 2.0, "Gap between widgets"),
    ("padding", 3.0, "Space inside the panel edge"),
    ("font-size", 13.0, "Base font size"),
    ("icon-size", 24.0, "Task and launcher icon size"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backdrop {
    /// Plain `panel-background` fill.
    None,
    /// Windows 11 acrylic ("transient window" material) behind the panel.
    #[default]
    Acrylic,
    /// Windows 11 mica (tinted by the wallpaper).
    Mica,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub backdrop: Backdrop,
    /// Follow the Windows accent colour instead of the `accent` token.
    pub system_accent: bool,
    pub font_family: String,
    pub colors: BTreeMap<String, Rgba>,
    pub metrics: BTreeMap<String, f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ThemeFile {
    name: Option<String>,
    backdrop: Option<Backdrop>,
    font: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    metrics: BTreeMap<String, f32>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "Breeze Dark".into(),
            backdrop: Backdrop::default(),
            system_accent: false,
            font_family: String::new(),
            colors: COLOR_TOKENS
                .iter()
                .filter_map(|(k, v, _)| Some(((*k).to_owned(), v.parse().ok()?)))
                .collect(),
            metrics: METRIC_TOKENS.iter().map(|(k, v, _)| ((*k).to_owned(), *v)).collect(),
        }
    }
}

impl Theme {
    /// Parse `theme.toml` over the defaults. Unknown tokens aren't fatal — they're returned
    /// as warnings so a typo doesn't throw the whole theme away.
    pub fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        let file: ThemeFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut theme = Theme::default();
        let mut warnings = Vec::new();
        if let Some(n) = file.name {
            theme.name = n;
        }
        if let Some(b) = file.backdrop {
            theme.backdrop = b;
        }
        if let Some(f) = file.font {
            theme.font_family = f;
        }
        for (key, value) in file.colors {
            if !COLOR_TOKENS.iter().any(|(k, ..)| *k == key) {
                warnings.push(format!("unknown colour token `{key}`"));
                continue;
            }
            if key == "accent" && value == "system" {
                theme.system_accent = true;
                continue;
            }
            match value.parse() {
                Ok(c) => {
                    theme.colors.insert(key, c);
                }
                Err(e) => warnings.push(format!("colors.{key}: {e}")),
            }
        }
        for (key, value) in file.metrics {
            if !METRIC_TOKENS.iter().any(|(k, ..)| *k == key) {
                warnings.push(format!("unknown metric token `{key}`"));
            } else if !(0.0..=1000.0).contains(&value) {
                warnings.push(format!("metrics.{key} = {value} is out of range"));
            } else {
                theme.metrics.insert(key, value);
            }
        }
        Ok((theme, warnings))
    }

    pub fn color(&self, token: &str) -> Rgba {
        self.colors.get(token).copied().unwrap_or(Rgba::rgb(255, 0, 255))
    }

    pub fn metric(&self, token: &str) -> f32 {
        self.metrics.get(token).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_token() {
        let t = Theme::default();
        assert_eq!(t.colors.len(), COLOR_TOKENS.len());
        assert_eq!(t.metrics.len(), METRIC_TOKENS.len());
    }

    #[test]
    fn partial_theme_overrides_only_what_it_names() {
        let (t, warnings) = Theme::parse(
            r##"
            name = "Test"
            backdrop = "mica"
            [colors]
            foreground = "#000"
            accent = "system"
            bogus = "#fff"
            hover = "not a colour"
            [metrics]
            radius = 10
            "##,
        )
        .unwrap();
        assert_eq!(t.name, "Test");
        assert_eq!(t.backdrop, Backdrop::Mica);
        assert!(t.system_accent);
        assert_eq!(t.color("foreground"), Rgba::rgb(0, 0, 0));
        assert_eq!(t.color("hover"), Theme::default().color("hover"));
        assert_eq!(t.metric("radius"), 10.0);
        assert_eq!(t.metric("spacing"), Theme::default().metric("spacing"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn structural_errors_are_fatal() {
        assert!(Theme::parse("backdrop = \"glass\"").is_err());
        assert!(Theme::parse("colours = {}").is_err());
    }
}
