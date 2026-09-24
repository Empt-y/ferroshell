//! The auto-generated settings form: turning a widget's manifest schema and its current
//! settings into form fields, and form input back into validated config values.

use fsh_config::Rgba;
use fsh_widgets::{FieldKind, Manifest};
use slint::{Color, ModelRc, SharedString, VecModel};

use crate::ui::Field;

pub enum Input {
    Text(String),
    Bool(bool),
    Int(i64),
    Option(usize),
}

pub enum Value {
    One(toml_edit::Value),
    List(Vec<String>),
}

fn kind_name(k: &FieldKind) -> &'static str {
    match k {
        FieldKind::Bool { .. } => "bool",
        FieldKind::Int { .. } => "int",
        FieldKind::Float { .. } => "float",
        FieldKind::String { .. } => "string",
        FieldKind::Enum { .. } => "enum",
        FieldKind::Color { .. } => "color",
        FieldKind::StringList { .. } => "string-list",
    }
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn blank(key: &str, kind: &FieldKind) -> Field {
    Field {
        key: key.into(),
        label: key.into(),
        description: "".into(),
        kind: kind_name(kind).into(),
        text: "".into(),
        checked: false,
        number: 0,
        minimum: i32::MIN,
        maximum: i32::MAX,
        options: ModelRc::new(VecModel::from(Vec::<SharedString>::new())),
        option_index: -1,
        swatch: Color::from_argb_u8(0, 0, 0, 0),
        customised: false,
        problem: "".into(),
    }
}

pub fn fields(manifest: &Manifest, settings: &toml::Table) -> Vec<Field> {
    manifest
        .config
        .iter()
        .map(|(key, cf)| {
            let value = settings.get(key);
            let mut f = blank(key, &cf.kind);
            f.label = cf.label.clone().unwrap_or_else(|| key.replace('-', " ")).into();
            f.description = cf.description.clone().unwrap_or_default().into();
            f.customised = value.is_some();
            f.problem = cf.kind.literal(value).1.unwrap_or_default().into();
            match &cf.kind {
                FieldKind::Bool { default } => f.checked = value.and_then(toml::Value::as_bool).unwrap_or(*default),
                FieldKind::Int { default, min, max } => {
                    f.number = clamp_i32(value.and_then(toml::Value::as_integer).unwrap_or(*default));
                    f.minimum = min.map_or(i32::MIN, clamp_i32);
                    f.maximum = max.map_or(i32::MAX, clamp_i32);
                }
                FieldKind::Float { default, .. } => {
                    let v = value.and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(*default);
                    f.text = v.to_string().into();
                }
                FieldKind::String { default } => {
                    f.text = value.and_then(toml::Value::as_str).unwrap_or(default).into();
                }
                FieldKind::Enum { default, options } => {
                    let current = value.and_then(toml::Value::as_str).unwrap_or(default);
                    f.option_index = options.iter().position(|o| o == current).map_or(-1, |i| i as i32);
                    f.options = ModelRc::new(VecModel::from(options.iter().map(|o| SharedString::from(o.as_str())).collect::<Vec<_>>()));
                }
                FieldKind::Color { default } => {
                    let text = value.and_then(toml::Value::as_str).map(str::to_owned).unwrap_or_else(|| default.to_string());
                    let c = text.parse::<Rgba>().unwrap_or(*default);
                    f.swatch = Color::from_argb_u8(c.a, c.r, c.g, c.b);
                    f.text = text.into();
                }
                FieldKind::StringList { default } => {
                    let items: Vec<String> = match value.and_then(toml::Value::as_array) {
                        Some(a) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                        None => default.clone(),
                    };
                    f.text = items.join("\n").into();
                }
            }
            f
        })
        .collect()
}

/// Validate form input against the field's type before it's written to config.
pub fn to_value(kind: &FieldKind, input: Input) -> Result<Value, String> {
    let one = |v: toml_edit::Value| Ok(Value::One(v));
    match (kind, input) {
        (FieldKind::Bool { .. }, Input::Bool(b)) => one(b.into()),
        (FieldKind::Int { min, max, .. }, Input::Int(i)) => {
            if min.is_some_and(|m| i < m) || max.is_some_and(|m| i > m) {
                return Err(format!("{i} is out of range"));
            }
            one(i.into())
        }
        (FieldKind::Float { min, max, .. }, Input::Text(t)) => {
            let v: f64 = t.trim().parse().map_err(|_| format!("`{t}` is not a number"))?;
            if !v.is_finite() || min.is_some_and(|m| v < m) || max.is_some_and(|m| v > m) {
                return Err(format!("{v} is out of range"));
            }
            one(v.into())
        }
        (FieldKind::String { .. }, Input::Text(t)) => one(t.into()),
        (FieldKind::Color { .. }, Input::Text(t)) => {
            let c: Rgba = t.trim().parse()?;
            one(c.to_string().into())
        }
        (FieldKind::Enum { options, .. }, Input::Option(i)) => {
            let o = options.get(i).ok_or("no such option")?;
            one(o.as_str().into())
        }
        (FieldKind::StringList { .. }, Input::Text(t)) => Ok(Value::List(
            t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect(),
        )),
        _ => Err("that input doesn't fit this setting's type".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model as _;

    const MANIFEST: &str = r##"
        id = "w"
        name = "W"
        api = 1
        [config.flag]
        type = "bool"
        default = true
        label = "A flag"
        [config.count]
        type = "int"
        default = 3
        min = 1
        max = 9
        [config.mode]
        type = "enum"
        default = "b"
        options = ["a", "b"]
        [config.tint]
        type = "color"
        default = "#ff0000"
        [config.apps]
        type = "string-list"
    "##;

    #[test]
    fn builds_fields_in_manifest_order_with_values() {
        let m = Manifest::parse(MANIFEST).unwrap();
        let settings: toml::Table = toml::from_str("count = 12\napps = ['x', 'y']").unwrap();
        let f = fields(&m, &settings);
        let keys: Vec<&str> = f.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["flag", "count", "mode", "tint", "apps"]);
        assert!(f[0].checked && !f[0].customised && f[0].label == "A flag");
        assert_eq!((f[1].number, f[1].minimum, f[1].maximum), (12, 1, 9));
        assert!(f[1].customised && f[1].problem.contains("out of range"));
        assert_eq!((f[2].option_index, f[2].options.row_count()), (1, 2));
        assert_eq!(f[3].text, "#ff0000");
        assert_eq!(f[4].text, "x\ny");
    }

    #[test]
    fn validates_input() {
        let m = Manifest::parse(MANIFEST).unwrap();
        let k = |key: &str| &m.config[key].kind;
        assert!(to_value(k("count"), Input::Int(5)).is_ok());
        assert!(to_value(k("count"), Input::Int(50)).is_err());
        assert!(to_value(k("tint"), Input::Text("#12345".into())).is_err());
        assert!(matches!(to_value(k("mode"), Input::Option(0)), Ok(Value::One(v)) if v.as_str() == Some("a")));
        assert!(matches!(to_value(k("apps"), Input::Text(" a \n\n b".into())), Ok(Value::List(l)) if l == ["a", "b"]));
        assert!(to_value(k("flag"), Input::Text("yes".into())).is_err());
    }
}
