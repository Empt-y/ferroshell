//! Starting apps at sign-in the way Explorer does, when Ferroshell is the login shell. Pure
//! rules; `fsh_win::startup` reads the registry, folders and packages, and launches.

/// `StartupApproved` data (Task Manager's "Startup apps" switches): the first byte is even
/// (02, 06) when enabled and odd (03, 07) when turned off. No entry means enabled.
pub fn approved(data: Option<&[u8]>) -> bool {
    data.and_then(|d| d.first()).is_none_or(|b| b % 2 == 0)
}

/// A RunOnce value's name prefixes: `!` defers deleting the value until the program has
/// been started (so it runs again if that fails), `*` also runs in safe mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOnceFlags {
    pub delete_after: bool,
    pub in_safe_mode: bool,
}

pub fn runonce_flags(name: &str) -> RunOnceFlags {
    let prefix: String = name.chars().take_while(|c| *c == '!' || *c == '*').collect();
    RunOnceFlags { delete_after: prefix.contains('!'), in_safe_mode: prefix.contains('*') }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    RunOnce,
    Run,
    StartupFolder,
    StartupTask,
}

/// Something that would start at sign-in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Candidate {
    pub source: Source,
    /// "HKLM", "HKLM (32-bit)", "HKCU", "all users", "you", or the package family name.
    pub scope: String,
    /// The Run value name, Startup-folder file name, or startup task id.
    pub name: String,
    /// The command line, file path, or `shell:AppsFolder\<aumid>` to start.
    pub target: String,
    /// Turned on in Task Manager / Settings (always true where there's no switch).
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "verdict", content = "reason")]
pub enum Verdict {
    Start,
    Skip(String),
}

/// Whether to start it: not excluded (case-insensitive, matching the entry name, a
/// Startup-folder file name without its extension, or the scope, e.g. a package family
/// name) and turned on.
pub fn decide(c: &Candidate, exclude: &[String]) -> Verdict {
    let stem = match c.source {
        Source::StartupFolder => c.name.rsplit_once('.').map_or(c.name.as_str(), |(s, _)| s),
        _ => c.name.as_str(),
    };
    let excluded = |x: &String| x.eq_ignore_ascii_case(&c.name) || x.eq_ignore_ascii_case(stem) || x.eq_ignore_ascii_case(&c.scope);
    if exclude.iter().any(excluded) {
        return Verdict::Skip("excluded by [session] startup-exclude".into());
    }
    if !c.enabled {
        return Verdict::Skip("turned off in Startup apps".into());
    }
    if c.target.trim().is_empty() {
        return Verdict::Skip("nothing to run".into());
    }
    Verdict::Start
}

/// A `windows.startupTask` declared in a package manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTask {
    /// The `<Application Id>` it belongs to (AUMID = `<family name>!<app id>`).
    pub app_id: String,
    pub task_id: String,
    /// The manifest's `Enabled`, used until the user flips the switch.
    pub enabled_by_default: bool,
}

/// Startup tasks in an `AppxManifest.xml` (desktop and UWP `StartupTask` extensions, any
/// namespace prefix). Malformed XML gives none.
pub fn manifest_startup_tasks(xml: &str) -> Vec<ManifestTask> {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return vec![] };
    let mut out = Vec::new();
    for app in doc.descendants().filter(|n| n.tag_name().name() == "Application") {
        let Some(app_id) = app.attribute("Id") else { continue };
        for ext in app.descendants().filter(|n| n.tag_name().name() == "Extension" && n.attribute("Category") == Some("windows.startupTask")) {
            for task in ext.children().filter(|n| n.tag_name().name() == "StartupTask") {
                if let Some(task_id) = task.attribute("TaskId") {
                    out.push(ManifestTask {
                        app_id: app_id.to_owned(),
                        task_id: task_id.to_owned(),
                        enabled_by_default: task.attribute("Enabled").is_some_and(|v| v.eq_ignore_ascii_case("true")),
                    });
                }
            }
        }
    }
    out
}

/// A startup task's `State` (Windows' `StartupTaskState`): 2 Enabled and 4 EnabledByPolicy
/// start; 0 Disabled, 1 DisabledByUser and 3 DisabledByPolicy don't. No state yet: the
/// manifest's default.
pub fn task_enabled(state: Option<u32>, enabled_by_default: bool) -> bool {
    match state {
        Some(s) => matches!(s, 2 | 4),
        None => enabled_by_default,
    }
}

/// The program part of a Run command line (for display and exclusion by file name): the
/// quoted first token, or everything up to the first space.
pub fn program_of(command: &str) -> &str {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    c.split_whitespace().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, enabled: bool) -> Candidate {
        Candidate { source: Source::Run, scope: "HKCU".into(), name: name.into(), target: "app.exe".into(), enabled }
    }

    #[test]
    fn approval_bytes() {
        assert!(approved(None));
        assert!(approved(Some(&[])));
        assert!(approved(Some(&[0x02, 0, 0, 0])));
        assert!(approved(Some(&[0x06])));
        assert!(!approved(Some(&[0x03, 0x12, 0x34])));
        assert!(!approved(Some(&[0x07])));
    }

    #[test]
    fn runonce_prefixes() {
        assert_eq!(runonce_flags("Setup"), RunOnceFlags { delete_after: false, in_safe_mode: false });
        assert_eq!(runonce_flags("!Setup"), RunOnceFlags { delete_after: true, in_safe_mode: false });
        assert_eq!(runonce_flags("*Setup"), RunOnceFlags { delete_after: false, in_safe_mode: true });
        assert_eq!(runonce_flags("*!Setup"), RunOnceFlags { delete_after: true, in_safe_mode: true });
        assert_eq!(runonce_flags("Set!up"), RunOnceFlags { delete_after: false, in_safe_mode: false });
    }

    #[test]
    fn decisions() {
        assert_eq!(decide(&cand("Discord", true), &[]), Verdict::Start);
        assert!(matches!(decide(&cand("Discord", false), &[]), Verdict::Skip(r) if r.contains("turned off")));
        assert!(matches!(decide(&cand("Discord", true), &["discord".into()]), Verdict::Skip(r) if r.contains("excluded")));
        let mut link = cand("Ollama.lnk", true);
        link.source = Source::StartupFolder;
        assert!(matches!(decide(&link, &["ollama".into()]), Verdict::Skip(_)));
        assert_eq!(decide(&cand("Ollama.lnk", true), &["ollama".into()]), Verdict::Start, "only folder items match by stem");
        let mut empty = cand("x", true);
        empty.target = " ".into();
        assert!(matches!(decide(&empty, &[]), Verdict::Skip(_)));
    }

    #[test]
    fn manifest_tasks() {
        let xml = r#"<?xml version="1.0"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
         xmlns:uap5="http://schemas.microsoft.com/appx/manifest/uap/windows10/5">
  <Applications>
    <Application Id="App" Executable="app.exe">
      <Extensions>
        <desktop:Extension Category="windows.startupTask" Executable="app.exe" EntryPoint="Windows.FullTrustApplication">
          <desktop:StartupTask TaskId="AppStartup" Enabled="true" DisplayName="App" />
        </desktop:Extension>
        <uap5:Extension Category="windows.appExecutionAlias"><uap5:AppExecutionAlias /></uap5:Extension>
      </Extensions>
    </Application>
    <Application Id="Other">
      <Extensions>
        <uap5:Extension Category="windows.startupTask">
          <uap5:StartupTask TaskId="OtherTask" Enabled="false" />
        </uap5:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>"#;
        let tasks = manifest_startup_tasks(xml);
        assert_eq!(
            tasks,
            vec![
                ManifestTask { app_id: "App".into(), task_id: "AppStartup".into(), enabled_by_default: true },
                ManifestTask { app_id: "Other".into(), task_id: "OtherTask".into(), enabled_by_default: false },
            ]
        );
        assert!(manifest_startup_tasks("<not xml").is_empty());
    }

    #[test]
    fn task_states() {
        assert!(task_enabled(Some(2), false));
        assert!(task_enabled(Some(4), false));
        assert!(!task_enabled(Some(1), true), "the user turned it off");
        assert!(!task_enabled(Some(0), true));
        assert!(!task_enabled(Some(3), true));
        assert!(task_enabled(None, true));
        assert!(!task_enabled(None, false));
    }

    #[test]
    fn programs() {
        assert_eq!(program_of(r#""C:\Program Files\App\app.exe" --min"#), r"C:\Program Files\App\app.exe");
        assert_eq!(program_of(r"C:\Tools\tool.exe /quiet"), r"C:\Tools\tool.exe");
        assert_eq!(program_of("  notepad  "), "notepad");
    }
}
