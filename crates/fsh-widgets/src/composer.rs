//! Turns a panel's widget list into one Slint component.
//!
//! Each widget is first compiled on its own (with its settings applied), so a widget that
//! fails — bad Slint, a setting its UI doesn't declare — is swapped for an `ErrorWidget`
//! placeholder while the rest of the panel keeps working. The panel itself is then
//! generated as Slint source that imports each widget, and compiled.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use fsh_config::{Edge, WidgetEntry};
use slint_interpreter::{Compiler, ComponentDefinition, DiagnosticLevel};

use crate::assets::Layout;
use crate::manifest::slint_string;
use crate::registry::{Origin, Registry};

/// What to build: which edge (decides the layout direction) and which widgets.
#[derive(Debug, Clone)]
pub struct PanelSpec {
    pub edge: Edge,
    pub floating: bool,
    pub widgets: Vec<WidgetEntry>,
}

/// Outcome for one widget on the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetStatus {
    pub id: String,
    /// Unique per panel slot and stable across rebuilds, e.g. `panel-0-DISPLAY1/2`. Scripts
    /// and plugins use it to address this instance's data.
    pub instance: String,
    pub origin: Option<Origin>,
    /// Set when the widget was replaced by an error placeholder.
    pub error: Option<String>,
    /// Non-fatal problems, e.g. an invalid setting that fell back to its default.
    pub warnings: Vec<String>,
}

enum Slot {
    Widget { ui: PathBuf, props: Vec<(String, String)> },
    Error { id: String, message: String },
}

pub struct Composer {
    layout: Layout,
}

/// Name of the root component in generated panel sources.
pub const PANEL_COMPONENT: &str = "Panel";

impl Composer {
    pub fn new(layout: Layout) -> Self {
        Self { layout }
    }

    fn compiler(&self) -> Compiler {
        let mut c = Compiler::default();
        c.set_library_paths(HashMap::from([("ferroshell".to_owned(), self.layout.library_dir())]));
        c
    }

    /// Compile a component from a file in the `@ferroshell` library (e.g. popups).
    pub fn compile_library(&self, file: &str, component: &str) -> Result<ComponentDefinition, String> {
        let path = self.layout.library_dir().join(file);
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.compile(source, path, component)
    }

    fn compile(&self, source: String, virtual_path: PathBuf, component: &str) -> Result<ComponentDefinition, String> {
        let result = spin_on::spin_on(self.compiler().build_from_source(source, virtual_path));
        let errors: Vec<String> = result
            .diagnostics()
            .filter(|d| d.level() == DiagnosticLevel::Error)
            .map(|d| {
                let (line, col) = d.line_column();
                match d.source_file() {
                    Some(f) => format!("{}:{line}:{col}: {}", short_path(f), d.message()),
                    None => d.message().to_owned(),
                }
            })
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        result.component(component).ok_or_else(|| format!("no component `{component}` was produced"))
    }

    /// Resolve and check one widget: its package, its settings, and that it compiles.
    fn prepare(&self, registry: &Registry, entry: &WidgetEntry, instance: String) -> (Slot, WidgetStatus) {
        let mut status = WidgetStatus { id: entry.id.clone(), instance, origin: None, error: None, warnings: vec![] };
        let fail = |mut status: WidgetStatus, message: String| {
            tracing::warn!("widget {}: {message}", status.id);
            status.error = Some(message.clone());
            (Slot::Error { id: status.id.clone(), message }, status)
        };

        let pkg = match registry.get(&entry.id) {
            Ok(p) => p,
            Err(e) => return fail(status, e),
        };
        status.origin = Some(pkg.origin);

        let mut props = Vec::new();
        for (key, field) in &pkg.manifest.config {
            let (lit, warning) = field.kind.literal(entry.settings.get(key));
            if let Some(w) = warning {
                status.warnings.push(format!("{key}: {w}"));
            }
            if field.ui {
                props.push((key.clone(), lit));
            }
        }
        if pkg.manifest.wants_instance_id() {
            props.push(("instance-id".into(), slint_string(&status.instance)));
        }
        for key in entry.settings.keys() {
            if !pkg.manifest.config.contains_key(key) {
                status.warnings.push(format!("unknown setting `{key}` (ignored)"));
            }
        }

        let ui = pkg.ui_path();
        let mut check = format!("import {{ Widget }} from {};\n", slint_string(&slint_path(&ui)));
        check.push_str("export component Check inherits Window {\n    Widget {\n");
        for (k, v) in &props {
            let _ = writeln!(check, "        {k}: {v};");
        }
        check.push_str("    }\n}\n");
        if let Err(e) = self.compile(check, self.generated_dir().join("check.slint"), "Check") {
            return fail(status, e);
        }
        for w in &status.warnings {
            tracing::info!("widget {}: {w}", status.id);
        }
        (Slot::Widget { ui, props }, status)
    }

    fn generated_dir(&self) -> PathBuf {
        self.layout.builtin.join("generated")
    }

    /// Build the panel component. Only fails if even a panel of error placeholders won't
    /// compile, which would mean the built-in library itself is broken.
    pub fn build_panel(
        &self,
        registry: &Registry,
        spec: &PanelSpec,
        name: &str,
    ) -> anyhow::Result<(ComponentDefinition, Vec<WidgetStatus>)> {
        let (slots, mut statuses): (Vec<Slot>, Vec<WidgetStatus>) =
            spec.widgets.iter().enumerate().map(|(i, w)| self.prepare(registry, w, format!("{name}/{i}"))).unzip();

        let source = generate_panel_source(spec, &slots);
        let path = self.generated_dir().join(format!("{name}.slint"));
        // Written for debugging and for users curious how panels are put together.
        let _ = std::fs::create_dir_all(self.generated_dir());
        let _ = std::fs::write(&path, &source);

        match self.compile(source, path.clone(), PANEL_COMPONENT) {
            Ok(def) => Ok((def, statuses)),
            Err(e) => {
                // Individually fine widgets can still clash when combined; fall back to
                // placeholders for all of them so the panel itself survives.
                tracing::error!("panel {name} failed to compile:\n{e}");
                let slots: Vec<Slot> = spec
                    .widgets
                    .iter()
                    .map(|w| Slot::Error { id: w.id.clone(), message: format!("panel failed to compile: {e}") })
                    .collect();
                for s in &mut statuses {
                    s.error.get_or_insert_with(|| format!("panel failed to compile: {e}"));
                }
                let def = self
                    .compile(generate_panel_source(spec, &slots), path, PANEL_COMPONENT)
                    .map_err(|e| anyhow::anyhow!("fallback panel failed to compile: {e}"))?;
                Ok((def, statuses))
            }
        }
    }
}

fn slint_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn short_path(p: &Path) -> String {
    let parts: Vec<_> = p.components().rev().take(2).collect();
    parts.into_iter().rev().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn generate_panel_source(spec: &PanelSpec, slots: &[Slot]) -> String {
    let mut s = String::new();
    s.push_str("// Generated by Ferroshell. Changes are overwritten; edit config.toml instead.\n");
    s.push_str("import { Shell, Theme } from \"@ferroshell/api.slint\";\n");
    s.push_str("import { ErrorWidget } from \"@ferroshell/controls.slint\";\n");
    for (i, slot) in slots.iter().enumerate() {
        if let Slot::Widget { ui, .. } = slot {
            let _ = writeln!(s, "import {{ Widget as W{i} }} from {};", slint_string(&slint_path(ui)));
        }
    }
    s.push_str("export { Shell, Theme }\n\n");
    let _ = writeln!(s, "export component {PANEL_COMPONENT} inherits Window {{");
    s.push_str(
        "    title: \"Ferroshell Panel\";\n    no-frame: true;\n    always-on-top: true;\n    \
         background: transparent;\n    min-width: 1px;\n    min-height: 1px;\n    \
         default-font-size: Theme.font-size;\n    default-font-family: Theme.font-family;\n\n",
    );
    s.push_str("    Rectangle {\n        background: Theme.panel-background;\n");

    // Hairline along the inner edge for docked panels (floating ones get DWM's border).
    if !spec.floating {
        let line = match spec.edge {
            Edge::Bottom => "x: 0; y: 0; width: parent.width; height: 1px;",
            Edge::Top => "x: 0; y: parent.height - 1px; width: parent.width; height: 1px;",
            Edge::Left => "x: parent.width - 1px; y: 0; width: 1px; height: parent.height;",
            Edge::Right => "x: 0; y: 0; width: 1px; height: parent.height;",
        };
        let _ = writeln!(s, "        Rectangle {{ background: Theme.panel-border; {line} }}");
    }

    let layout = if spec.edge.is_vertical() { "VerticalLayout" } else { "HorizontalLayout" };
    let _ = writeln!(s, "        {layout} {{\n            padding: Theme.padding;\n            spacing: Theme.spacing;");
    for (i, slot) in slots.iter().enumerate() {
        match slot {
            Slot::Widget { props, .. } => {
                let _ = write!(s, "            W{i} {{");
                for (k, v) in props {
                    let _ = write!(s, " {k}: {v};");
                }
                s.push_str(" }\n");
            }
            Slot::Error { id, message } => {
                let _ = writeln!(
                    s,
                    "            ErrorWidget {{ widget-id: {}; message: {}; }}",
                    slint_string(id),
                    slint_string(&first_line(message))
                );
            }
        }
    }
    s.push_str(
        "            if Shell.config-error != \"\": ErrorWidget { widget-id: \"config.toml\"; message: Shell.config-error; }\n",
    );
    s.push_str("        }\n    }\n}\n");
    s
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or_default();
    if s.lines().count() > 1 { format!("{line} (+ more, see log)") } else { line.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tests::temp_layout;
    use crate::registry::Origin;

    fn entry(id: &str, settings: &str) -> WidgetEntry {
        WidgetEntry { id: id.into(), settings: toml::from_str(settings).unwrap() }
    }

    fn setup(tag: &str) -> (Composer, Registry, Layout) {
        let layout = temp_layout(tag);
        let registry = Registry::scan(&[
            (layout.user_widgets(), Origin::User),
            (layout.builtin_widgets(), Origin::Builtin),
        ]);
        (Composer::new(layout.clone()), registry, layout)
    }

    #[test]
    fn every_builtin_widget_compiles_in_both_orientations() {
        let (composer, registry, _) = setup("builtins");
        let widgets: Vec<WidgetEntry> = registry.packages().map(|p| entry(&p.manifest.id, "")).collect();
        assert!(widgets.len() >= 6);
        for edge in [Edge::Bottom, Edge::Left] {
            let spec = PanelSpec { edge, floating: false, widgets: widgets.clone() };
            let (def, statuses) = composer.build_panel(&registry, &spec, "test").unwrap();
            for s in &statuses {
                assert_eq!(s.error, None, "{} failed", s.id);
                assert!(s.warnings.is_empty(), "{}: {:?}", s.id, s.warnings);
            }
            assert!(def.globals().any(|g| g == "Shell"));
            assert!(def.globals().any(|g| g == "Theme"));
        }
    }

    #[test]
    fn example_widgets_compile() {
        let (composer, _, layout) = setup("examples");
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/widgets");
        let registry = Registry::scan(&[(examples, Origin::User), (layout.builtin_widgets(), Origin::Builtin)]);
        let widgets: Vec<WidgetEntry> =
            registry.packages().filter(|p| p.origin == Origin::User).map(|p| entry(&p.manifest.id, "")).collect();
        assert!(widgets.len() >= 2, "examples not found");
        let spec = PanelSpec { edge: Edge::Bottom, floating: false, widgets };
        let (_, statuses) = composer.build_panel(&registry, &spec, "examples").unwrap();
        for s in statuses {
            assert_eq!(s.error, None, "{} failed", s.id);
        }
    }

    #[test]
    fn library_popups_compile() {
        let (composer, _, _) = setup("popups");
        let def = composer.compile_library("popups.slint", "PreviewPopup").unwrap();
        assert!(def.globals().any(|g| g == "Theme"));
    }

    #[test]
    fn default_config_panel_builds_cleanly() {
        let (composer, registry, _) = setup("default");
        let cfg = fsh_config::Config::defaults();
        let p = &cfg.panels[0];
        let spec = PanelSpec { edge: p.edge, floating: p.floating, widgets: p.widgets.clone() };
        let (_, statuses) = composer.build_panel(&registry, &spec, "default").unwrap();
        assert!(statuses.iter().all(|s| s.error.is_none() && s.warnings.is_empty()), "{statuses:?}");
    }

    #[test]
    fn broken_widgets_are_isolated() {
        let (composer, registry, layout) = setup("isolation");
        let bad = layout.user_widgets().join("com.example.bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("widget.toml"), "id='com.example.bad'\nname='Bad'\napi=1").unwrap();
        std::fs::write(bad.join("ui.slint"), "export component Widget { oops }").unwrap();
        let registry = if registry.get("com.example.bad").is_err() {
            Registry::scan(&[(layout.user_widgets(), Origin::User), (layout.builtin_widgets(), Origin::Builtin)])
        } else {
            registry
        };

        let spec = PanelSpec {
            edge: Edge::Bottom,
            floating: false,
            widgets: vec![
                entry("org.ferroshell.clock", "format = 42\nbogus = 1"),
                entry("com.example.bad", ""),
                entry("com.example.missing", ""),
                entry("org.ferroshell.spacer", "size = 99999"),
            ],
        };
        let (_, st) = composer.build_panel(&registry, &spec, "iso").unwrap();
        // Bad settings are warnings, not failures.
        assert_eq!(st[0].error, None);
        assert_eq!(st[0].warnings.len(), 2, "{:?}", st[0].warnings);
        // Broken and missing widgets become placeholders.
        assert!(st[1].error.as_deref().unwrap().contains("ui.slint"), "{:?}", st[1].error);
        assert!(st[2].error.as_deref().unwrap().contains("no widget"));
        assert_eq!(st[3].error, None);
        assert!(st[3].warnings[0].contains("out of range"));
    }
}
