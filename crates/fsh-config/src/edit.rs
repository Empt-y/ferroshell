//! Changing `config.toml` from code (pinning apps, the settings app) while keeping the
//! user's comments, ordering and formatting intact.

use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

#[derive(Debug)]
pub struct ConfigEditor {
    doc: DocumentMut,
}

impl ConfigEditor {
    pub fn parse(text: &str) -> Result<Self, String> {
        text.parse::<DocumentMut>().map(|doc| Self { doc }).map_err(|e| e.to_string())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// Write atomically (temp file + rename) so a crash mid-write can't corrupt the config.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.doc.to_string()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn to_text(&self) -> String {
        self.doc.to_string()
    }

    fn widget_table(&mut self, panel: usize, widget: usize) -> Result<&mut InlineTable, String> {
        let panels = self.doc.get_mut("panel").and_then(Item::as_array_of_tables_mut).ok_or("config has no [[panel]]")?;
        let p = panels.get_mut(panel).ok_or_else(|| format!("no panel {}", panel + 1))?;
        let widgets = p.get_mut("widgets").and_then(Item::as_array_mut).ok_or("panel has no widgets list")?;
        widgets
            .get_mut(widget)
            .and_then(Value::as_inline_table_mut)
            .ok_or_else(|| format!("no widget {} on panel {}", widget + 1, panel + 1))
    }

    /// Set a widget setting (`{ id = "...", key = value }`) on `panel`'s `widget`-th widget.
    pub fn set_widget_setting(&mut self, panel: usize, widget: usize, key: &str, value: impl Into<Value>) -> Result<(), String> {
        let table = self.widget_table(panel, widget)?;
        table.insert(key, value.into());
        Ok(())
    }

    pub fn set_widget_list(&mut self, panel: usize, widget: usize, key: &str, items: &[String]) -> Result<(), String> {
        let mut arr = Array::new();
        for s in items {
            arr.push(s.as_str());
        }
        self.set_widget_setting(panel, widget, key, arr)
    }

    fn widgets(&mut self, panel: usize) -> Result<&mut Array, String> {
        let panels = self.doc.get_mut("panel").and_then(Item::as_array_of_tables_mut).ok_or("config has no [[panel]]")?;
        let p = panels.get_mut(panel).ok_or_else(|| format!("no panel {}", panel + 1))?;
        if p.get("widgets").is_none() {
            p["widgets"] = toml_edit::value(Array::new());
        }
        p.get_mut("widgets").and_then(Item::as_array_mut).ok_or_else(|| "widgets is not a list".to_owned())
    }

    /// Insert a widget (with default settings) at `index` (clamped to the end).
    pub fn add_widget(&mut self, panel: usize, index: usize, id: &str) -> Result<(), String> {
        let widgets = self.widgets(panel)?;
        let mut t = InlineTable::new();
        t.insert("id", id.into());
        let index = index.min(widgets.len());
        widgets.insert(index, t);
        format_widget_list(widgets);
        Ok(())
    }

    pub fn remove_widget(&mut self, panel: usize, index: usize) -> Result<(), String> {
        let widgets = self.widgets(panel)?;
        if index >= widgets.len() {
            return Err(format!("no widget {}", index + 1));
        }
        widgets.remove(index);
        format_widget_list(widgets);
        Ok(())
    }

    pub fn move_widget(&mut self, panel: usize, from: usize, to: usize) -> Result<(), String> {
        let widgets = self.widgets(panel)?;
        if from >= widgets.len() || to >= widgets.len() {
            return Err("widget index out of range".into());
        }
        let v = widgets.remove(from);
        widgets.insert(to, v);
        format_widget_list(widgets);
        Ok(())
    }

    /// Remove a widget setting so it goes back to its default.
    pub fn reset_widget_setting(&mut self, panel: usize, widget: usize, key: &str) -> Result<(), String> {
        self.widget_table(panel, widget)?.remove(key);
        Ok(())
    }

    /// Append a panel with default settings and the given widgets.
    pub fn add_panel(&mut self, edge: &str, widgets: &[&str]) {
        if self.doc.get("panel").and_then(Item::as_array_of_tables).is_none() {
            self.doc["panel"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
        }
        let mut t = toml_edit::Table::new();
        t["monitor"] = toml_edit::value("all");
        t["edge"] = toml_edit::value(edge);
        t["thickness"] = toml_edit::value(44);
        t["floating"] = toml_edit::value(false);
        let mut arr = Array::new();
        for id in widgets {
            let mut w = InlineTable::new();
            w.insert("id", (*id).into());
            arr.push(w);
        }
        format_widget_list(&mut arr);
        t["widgets"] = toml_edit::value(arr);
        if let Some(panels) = self.doc.get_mut("panel").and_then(Item::as_array_of_tables_mut) {
            panels.push(t);
        }
    }

    pub fn remove_panel(&mut self, panel: usize) -> Result<(), String> {
        let panels = self.doc.get_mut("panel").and_then(Item::as_array_of_tables_mut).ok_or("config has no [[panel]]")?;
        if panel >= panels.len() {
            return Err(format!("no panel {}", panel + 1));
        }
        panels.remove(panel);
        Ok(())
    }

    fn launcher_table(&mut self) -> &mut toml_edit::Table {
        if self.doc.get("launcher").and_then(Item::as_table).is_none() {
            self.doc["launcher"] = Item::Table(toml_edit::Table::new());
        }
        // Just ensured it's a table.
        self.doc["launcher"].as_table_mut().expect("launcher is a table")
    }

    /// Set a `[launcher]` key, creating the section if needed.
    pub fn set_launcher(&mut self, key: &str, value: impl Into<Value>) {
        self.launcher_table()[key] = toml_edit::value(value.into());
    }

    pub fn set_launcher_list(&mut self, key: &str, items: &[String]) {
        let mut arr = Array::new();
        for s in items {
            arr.push(s.as_str());
        }
        self.set_launcher(key, arr);
    }

    /// Set a top-level key such as `theme`.
    pub fn set_top(&mut self, key: &str, value: impl Into<Value>) {
        self.doc[key] = toml_edit::value(value.into());
    }

    /// Set a panel key such as `edge` or `thickness`.
    pub fn set_panel(&mut self, panel: usize, key: &str, value: impl Into<Value>) -> Result<(), String> {
        let panels = self.doc.get_mut("panel").and_then(Item::as_array_of_tables_mut).ok_or("config has no [[panel]]")?;
        let p = panels.get_mut(panel).ok_or_else(|| format!("no panel {}", panel + 1))?;
        p[key] = toml_edit::value(value.into());
        Ok(())
    }
}

/// One widget per line, like the default config, so edits stay readable.
fn format_widget_list(arr: &mut Array) {
    for v in arr.iter_mut() {
        v.decor_mut().set_prefix("\n    ");
        v.decor_mut().set_suffix("");
    }
    arr.set_trailing("\n");
    arr.set_trailing_comma(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn edits_keep_comments_and_stay_valid() {
        let mut ed = ConfigEditor::parse(crate::config::DEFAULT_CONFIG).unwrap();
        ed.set_widget_list(0, 1, "pinned", &[r"C:\Windows\explorer.exe".into(), "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()])
            .unwrap();
        ed.set_widget_setting(0, 2, "format", "%H:%M:%S").unwrap();
        ed.set_top("theme", "breeze-light");
        ed.set_panel(0, "thickness", 36).unwrap();
        let text = ed.to_text();
        assert!(text.contains("# Ferroshell configuration."), "comments preserved");
        assert!(text.contains("# \"bottom\", \"top\", \"left\" or \"right\"."));

        let c = Config::parse(&text).unwrap();
        assert_eq!(c.theme, "breeze-light");
        assert_eq!(c.panels[0].thickness, 36);
        let tm = &c.panels[0].widgets[1];
        assert_eq!(tm.id, "org.ferroshell.taskmanager");
        assert_eq!(tm.settings["pinned"].as_array().unwrap().len(), 2);
        assert_eq!(c.panels[0].widgets[2].settings["format"].as_str(), Some("%H:%M:%S"));
    }

    #[test]
    fn widget_and_panel_list_operations() {
        let mut ed = ConfigEditor::parse(crate::config::DEFAULT_CONFIG).unwrap();
        let ids = |ed: &ConfigEditor, p: usize| -> Vec<String> {
            Config::parse(&ed.to_text()).unwrap().panels[p].widgets.iter().map(|w| w.id.clone()).collect()
        };
        ed.add_widget(0, 2, "org.ferroshell.spacer").unwrap();
        assert_eq!(ids(&ed, 0)[2], "org.ferroshell.spacer");
        ed.move_widget(0, 2, 0).unwrap();
        assert_eq!(ids(&ed, 0)[0], "org.ferroshell.spacer");
        let before = ids(&ed, 0).len();
        ed.remove_widget(0, 0).unwrap();
        assert_eq!(ids(&ed, 0).len(), before - 1);
        assert!(ed.remove_widget(0, 10).is_err());

        ed.set_widget_setting(0, 2, "format", "%H").unwrap();
        ed.reset_widget_setting(0, 2, "format").unwrap();
        assert!(!Config::parse(&ed.to_text()).unwrap().panels[0].widgets[2].settings.contains_key("format"));

        ed.add_panel("top", &["org.ferroshell.clock"]);
        let c = Config::parse(&ed.to_text()).unwrap();
        assert_eq!(c.panels.len(), 2);
        assert_eq!(c.panels[1].edge, crate::Edge::Top);
        assert_eq!(ids(&ed, 1), ["org.ferroshell.clock"]);
        ed.remove_panel(0).unwrap();
        assert_eq!(Config::parse(&ed.to_text()).unwrap().panels.len(), 1);
        // One widget per line.
        assert!(ed.to_text().contains("widgets = [\n    { id = \"org.ferroshell.clock\" },\n]"), "{}", ed.to_text());
    }

    #[test]
    fn launcher_section_is_created_and_edited() {
        let mut ed = ConfigEditor::parse(crate::config::DEFAULT_CONFIG).unwrap();
        ed.set_launcher_list("favourites", &["x".into()]);
        ed.set_launcher("windows-key", false);
        let c = Config::parse(&ed.to_text()).unwrap();
        assert_eq!(c.launcher.favourites, ["x"]);
        assert!(!c.launcher.windows_key);
        assert!(ed.to_text().contains("# RustShell configuration.") || ed.to_text().contains("# Ferroshell configuration."));
    }

    #[test]
    fn reports_missing_targets() {
        let mut ed = ConfigEditor::parse("version = 1").unwrap();
        assert!(ed.set_widget_setting(0, 0, "x", 1).is_err());
        let mut ed = ConfigEditor::parse(crate::config::DEFAULT_CONFIG).unwrap();
        assert!(ed.set_widget_setting(0, 99, "x", 1).unwrap_err().contains("no widget 100"));
    }

    #[test]
    fn save_is_atomic_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("fsh-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, crate::config::DEFAULT_CONFIG).unwrap();
        let mut ed = ConfigEditor::load(&path).unwrap();
        ed.set_top("theme", "x");
        ed.save(&path).unwrap();
        assert!(!path.with_extension("toml.tmp").exists());
        assert_eq!(Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap().theme, "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
