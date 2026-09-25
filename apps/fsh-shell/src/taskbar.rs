//! UI-thread side of the task manager: the latest tracker data, and each panel's task list
//! (panels can have their own pinned apps and options), exposed to Slint as `[Task]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fsh_config::PanelConfig;
use fsh_core::tasks::{PinnedApp, TaskOptions, TaskView, TrackedWindow, build_tasks};
use fsh_win::icon::RgbaImage;
use slint::{Image, Model as _, Rgba8Pixel, SharedPixelBuffer, VecModel};
use slint_interpreter::{Struct, Value};

pub const TASKMANAGER_ID: &str = "org.ferroshell.taskmanager";

/// Everything the tracker has told us.
#[derive(Default)]
pub struct Tasks {
    pub windows: Vec<TrackedWindow>,
    pub foreground: Option<isize>,
    pub icons: HashMap<String, Image>,
    pub pinned: HashMap<String, PinnedApp>,
}

impl Tasks {
    pub fn add_icon(&mut self, key: String, img: &RgbaImage) {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&img.pixels, img.width, img.height);
        self.icons.insert(key, Image::from_rgba8(buf));
    }
}

/// One panel's task list.
pub struct PanelTasks {
    pub model: Rc<VecModel<Value>>,
    views: RefCell<Vec<(TaskView, bool)>>,
    pub opts: TaskOptions,
    pub pinned_specs: Vec<String>,
    /// Index of the task manager widget in the panel's widget list (for pin/unpin edits).
    pub widget_index: Option<usize>,
}

impl PanelTasks {
    pub fn from_config(pc: &PanelConfig, monitor_device: &str) -> Self {
        let widget_index = pc.widgets.iter().position(|w| w.id == TASKMANAGER_ID);
        let settings = widget_index.map(|i| &pc.widgets[i].settings);
        let flag = |k: &str, default: bool| settings.and_then(|s| s.get(k)).and_then(toml::Value::as_bool).unwrap_or(default);
        let pinned_specs = settings
            .and_then(|s| s.get("pinned"))
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        Self {
            model: Rc::new(VecModel::default()),
            views: RefCell::default(),
            opts: TaskOptions {
                group: flag("group", true),
                monitor: flag("only-this-monitor", false).then(|| monitor_device.to_owned()),
            },
            pinned_specs,
            widget_index,
        }
    }

    /// Adopt new options/pins from config while keeping the existing model (and the
    /// widget's bindings to it).
    pub fn update_config(&mut self, from: PanelTasks) {
        self.opts = from.opts;
        self.pinned_specs = from.pinned_specs;
        self.widget_index = from.widget_index;
    }

    /// The id of the nth task as shown (0-based).
    pub fn nth_id(&self, n: usize) -> Option<String> {
        self.views.borrow().get(n).map(|(v, _)| v.id.clone())
    }

    pub fn find(&self, id: &str) -> Option<TaskView> {
        self.views.borrow().iter().find(|(v, _)| v.id == id).map(|(v, _)| v.clone())
    }

    pub fn refresh(&self, tasks: &Tasks) {
        let pinned: Vec<PinnedApp> = self.pinned_specs.iter().filter_map(|s| tasks.pinned.get(s).cloned()).collect();
        let new: Vec<(TaskView, bool)> = build_tasks(&tasks.windows, &pinned, tasks.foreground, &self.opts)
            .into_iter()
            .map(|v| {
                let has_icon = tasks.icons.contains_key(&v.icon);
                (v, has_icon)
            })
            .collect();
        let mut old = self.views.borrow_mut();
        let same_order = old.len() == new.len() && old.iter().zip(&new).all(|(a, b)| a.0.id == b.0.id);
        if same_order {
            // Update changed rows in place so hover state and animations survive.
            for (i, (o, n)) in old.iter().zip(&new).enumerate() {
                if o != n {
                    self.model.set_row_data(i, to_value(&n.0, tasks));
                }
            }
        } else {
            self.model.set_vec(new.iter().map(|(v, _)| to_value(v, tasks)).collect::<Vec<_>>());
        }
        *old = new;
        debug_assert_eq!(self.model.row_count(), old.len());
    }
}

fn to_value(v: &TaskView, tasks: &Tasks) -> Value {
    let s: Struct = [
        ("id", Value::String(v.id.as_str().into())),
        ("title", Value::String(v.title.as_str().into())),
        ("icon", Value::Image(tasks.icons.get(&v.icon).cloned().unwrap_or_default())),
        ("active", Value::Bool(v.active)),
        ("minimized", Value::Bool(v.minimized)),
        ("attention", Value::Bool(v.attention)),
        ("pinned", Value::Bool(v.pinned)),
        ("running", Value::Bool(v.running)),
        ("count", Value::Number(v.windows.len() as f64)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    Value::Struct(s)
}
