//! Pushing a [`Theme`] into a panel's `Theme` global.

use fsh_config::{Rgba, Theme};
use slint_interpreter::{Brush, Color, ComponentInstance, SharedString, Value};

fn color(c: Rgba) -> Value {
    Value::Brush(Brush::SolidColor(Color::from_argb_u8(c.a, c.r, c.g, c.b)))
}

/// Set every token. `accent` overrides the theme's accent (used for "follow system");
/// derived accent tokens (`highlight`) are tinted to match.
pub fn apply(instance: &ComponentInstance, theme: &Theme, accent: Option<Rgba>) {
    let set = |name: &str, value: Value| {
        if let Err(e) = instance.set_global_property("Theme", name, value) {
            tracing::warn!("theme token `{name}`: {e}");
        }
    };
    for (name, value) in &theme.colors {
        let v = match (name.as_str(), accent) {
            ("accent", Some(a)) => a,
            ("highlight", Some(a)) => a.with_alpha(value.a),
            _ => *value,
        };
        set(name, color(v));
    }
    for (name, value) in &theme.metrics {
        set(name, Value::Number(f64::from(*value)));
    }
    set("font-family", Value::String(SharedString::from(theme.font_family.as_str())));
}
