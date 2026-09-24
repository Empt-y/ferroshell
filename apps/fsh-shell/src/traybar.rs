//! UI-thread side of the system tray: the icon list, and each panel's view of it
//! (panels can hide different icons), exposed to Slint as `[TrayItem]`.

use std::collections::HashMap;
use std::rc::Rc;

use fsh_config::PanelConfig;
use fsh_core::tray::{TrayIcon, TrayModel, matches_hidden};
use fsh_win::{Hwnd, winfo};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, VecModel};
use slint_interpreter::{Struct, Value};

pub const SYSTEMTRAY_ID: &str = "org.ferroshell.systemtray";

#[derive(Default)]
pub struct TrayState {
    pub model: TrayModel,
    pub icons: HashMap<String, Image>,
    exe_names: HashMap<isize, String>,
}

impl TrayState {
    pub fn set_icon(&mut self, key: String, img: &fsh_win::icon::RgbaImage) {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&img.pixels, img.width, img.height);
        self.icons.insert(key, Image::from_rgba8(buf));
    }

    /// Forget images no icon uses any more.
    pub fn prune_icons(&mut self) {
        let live: std::collections::HashSet<String> = self.model.icons().iter().map(TrayIcon::icon_key).collect();
        self.icons.retain(|k, _| live.contains(k));
        let owners: std::collections::HashSet<isize> = self.model.icons().iter().map(|i| i.owner).collect();
        self.exe_names.retain(|o, _| owners.contains(o));
    }

    /// File name of the program owning an icon, e.g. "OneDrive.exe".
    pub fn exe_name(&mut self, owner: isize) -> String {
        self.exe_names
            .entry(owner)
            .or_insert_with(|| {
                winfo::process_path(winfo::pid(Hwnd(owner)))
                    .and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default()
            })
            .clone()
    }

    /// Text for an icon's tooltip: its own tip, or the program name.
    pub fn label(&mut self, icon: &TrayIcon) -> String {
        if icon.tip.trim().is_empty() {
            let exe = self.exe_name(icon.owner);
            exe.strip_suffix(".exe").unwrap_or(&exe).to_owned()
        } else {
            icon.tip.clone()
        }
    }
}

/// One panel's tray.
pub struct PanelTray {
    pub model: Rc<VecModel<Value>>,
    pub enabled: bool,
    hidden: Vec<String>,
}

impl PanelTray {
    pub fn from_config(pc: &PanelConfig) -> Self {
        let widget = pc.widgets.iter().find(|w| w.id == SYSTEMTRAY_ID);
        let hidden = widget
            .and_then(|w| w.settings.get("hidden"))
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        Self { model: Rc::new(VecModel::default()), enabled: widget.is_some(), hidden }
    }

    pub fn update_config(&mut self, from: PanelTray) {
        self.enabled = from.enabled;
        self.hidden = from.hidden;
    }

    pub fn refresh(&self, state: &mut TrayState) {
        if !self.enabled {
            if slint::Model::row_count(&*self.model) > 0 {
                self.model.set_vec(Vec::new());
            }
            return;
        }
        let icons: Vec<TrayIcon> = state.model.icons().to_vec();
        let mut items = Vec::new();
        for icon in &icons {
            if icon.hidden {
                continue;
            }
            let exe = state.exe_name(icon.owner);
            if matches_hidden(&self.hidden, &icon.tip, &exe) {
                continue;
            }
            let label = state.label(icon);
            let s: Struct = [
                ("id", Value::String(icon.key.id().into())),
                ("icon", Value::Image(state.icons.get(&icon.icon_key()).cloned().unwrap_or_default())),
                ("tooltip", Value::String(label.into())),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
            items.push(Value::Struct(s));
        }
        // Update in place when only contents changed, so hover state survives (tooltips
        // like a volume level change often).
        use slint::Model as _;
        let id_of = |v: &Value| match v {
            Value::Struct(s) => s.get_field("id").cloned(),
            _ => None,
        };
        let same_ids = self.model.row_count() == items.len()
            && items.iter().enumerate().all(|(i, v)| self.model.row_data(i).as_ref().and_then(id_of) == id_of(v));
        if same_ids {
            for (i, v) in items.into_iter().enumerate() {
                if self.model.row_data(i).as_ref() != Some(&v) {
                    self.model.set_row_data(i, v);
                }
            }
        } else {
            self.model.set_vec(items);
        }
    }
}
