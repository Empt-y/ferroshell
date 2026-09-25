//! Launcher logic: the app list, usage history and search across providers (apps,
//! calculator, run commands, Settings pages, web). Pure, so it's all unit-tested; the
//! shell supplies the app index and does the launching.

pub mod calc;
pub mod fuzzy;
pub mod settings;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// An installed app, as shown in the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// AppsFolder parsing name (an AUMID or a path); launched as `shell:AppsFolder\<id>`.
    pub id: String,
    pub name: String,
    /// Extra search terms: executable and shortcut file names.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Start-menu folder ("Steam", "Accessories"...), or "" for none.
    #[serde(default)]
    pub category: String,
    /// Shortcut or executable path, if known (for "Open file location").
    #[serde(default)]
    pub path: Option<String>,
    /// Can be started elevated: desktop apps, packaged or not (Windows Terminal), but not
    /// UWP apps. `None` when unknown (e.g. an index cached by an older version).
    #[serde(default)]
    pub elevatable: Option<bool>,
}

impl AppEntry {
    pub fn launch_target(&self) -> String {
        format!(r"shell:AppsFolder\{}", self.id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub count: u32,
    /// Unix seconds.
    pub last: i64,
}

/// How often and how recently things were launched ("frecency").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub entries: HashMap<String, Usage>,
}

const MAX_HISTORY: usize = 500;

impl History {
    pub fn record(&mut self, id: &str, now: i64) {
        let u = self.entries.entry(id.to_owned()).or_default();
        u.count = u.count.saturating_add(1);
        u.last = now;
        if self.entries.len() > MAX_HISTORY {
            // Forget the least recently used.
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, u)| u.last).map(|(k, _)| k.clone()) {
                self.entries.remove(&oldest);
            }
        }
    }

    pub fn forget(&mut self, id: &str) {
        self.entries.remove(id);
    }

    /// Ranking bonus (0..=200): launches, decaying with a one-week half-life-ish curve.
    pub fn bonus(&self, id: &str, now: i64) -> u32 {
        let Some(u) = self.entries.get(id) else { return 0 };
        let days = ((now - u.last).max(0) as f64) / 86_400.0;
        let weight = f64::from(u.count.min(50)) / (1.0 + days / 7.0);
        (weight * 25.0).min(200.0) as u32
    }

    /// Most recently used ids, newest first.
    pub fn recent(&self, n: usize) -> Vec<String> {
        let mut v: Vec<(&String, &Usage)> = self.entries.iter().collect();
        v.sort_by(|a, b| b.1.last.cmp(&a.1.last).then_with(|| a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(k, _)| k.clone()).collect()
    }
}

/// What typed text asks to run, if it isn't a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunKind {
    /// `>command args`: run through cmd.exe.
    Command(String),
    /// A path, URL or shell location to open.
    Open(String),
}

pub fn classify_run(input: &str) -> Option<RunKind> {
    let s = input.trim();
    if let Some(cmd) = s.strip_prefix('>') {
        let cmd = cmd.trim();
        return (!cmd.is_empty()).then(|| RunKind::Command(cmd.to_owned()));
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(RunKind::Open(s.to_owned()));
    }
    if lower.starts_with("www.") && s.contains('.') && !s.contains(' ') {
        return Some(RunKind::Open(format!("https://{s}")));
    }
    let b = s.as_bytes();
    let drive_path = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if lower.starts_with("shell:") || lower.starts_with("ms-settings:") || s.starts_with(r"\\") || drive_path || s.starts_with('%') || s.starts_with('~') {
        return Some(RunKind::Open(s.to_owned()));
    }
    None
}

/// A single word that might be a program on PATH ("notepad", "regedit").
pub fn bare_command(input: &str) -> Option<&str> {
    let s = input.trim();
    (!s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))).then_some(s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    App,
    Calculator,
    Run,
    Setting,
    Web,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub kind: ResultKind,
    /// App id, calculator value, command, URI or URL, depending on `kind`.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub score: u32,
}

#[derive(Clone, Default)]
pub struct SearchOptions<'a> {
    /// Web search URL with `{}` for the query; empty disables the web result.
    pub web_search: &'a str,
    /// Called for bare words: does this program exist on PATH? (Supplied by the shell.)
    pub on_path: Option<&'a dyn Fn(&str) -> bool>,
}

pub const MAX_RESULTS: usize = 50;

fn app_score(q: &str, app: &AppEntry) -> Option<u32> {
    let name = fuzzy::score(q, &app.name);
    let kw = app.keywords.iter().filter_map(|k| fuzzy::score(q, k)).max().map(|s| s * 4 / 5);
    name.max(kw)
}

pub fn search(query: &str, apps: &[AppEntry], history: &History, now: i64, opts: &SearchOptions<'_>) -> Vec<SearchResult> {
    let q = query.trim();
    if q.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();

    if calc::looks_like_math(q)
        && let Some(v) = calc::evaluate(q)
    {
        out.push(SearchResult {
            kind: ResultKind::Calculator,
            id: calc::format(v),
            title: format!("= {}", calc::format(v)),
            subtitle: "Press Enter to copy".into(),
            score: 5000,
        });
    }

    match classify_run(q) {
        Some(RunKind::Command(cmd)) => out.push(SearchResult {
            kind: ResultKind::Run,
            id: format!(">{cmd}"),
            title: format!("Run: {cmd}"),
            subtitle: "Command prompt".into(),
            score: 4000,
        }),
        Some(RunKind::Open(target)) => out.push(SearchResult {
            kind: ResultKind::Run,
            title: format!("Open {target}"),
            id: target,
            subtitle: "Location".into(),
            score: 4000,
        }),
        None => {}
    }

    for app in apps {
        if let Some(s) = app_score(q, app) {
            out.push(SearchResult {
                kind: ResultKind::App,
                id: app.id.clone(),
                title: app.name.clone(),
                subtitle: if app.category.is_empty() { "Application".into() } else { app.category.clone() },
                score: s + history.bonus(&app.id, now),
            });
        }
    }

    for (title, uri, keywords) in settings::PAGES {
        let s = fuzzy::score(q, title)
            .max(keywords.split(' ').filter_map(|k| fuzzy::score(q, k)).max().map(|s| s * 3 / 4));
        // Settings rank a little below apps with a similar match.
        if let Some(s) = s.filter(|s| *s >= 500) {
            out.push(SearchResult {
                kind: ResultKind::Setting,
                id: (*uri).to_owned(),
                title: (*title).to_owned(),
                subtitle: "Settings".into(),
                score: s * 9 / 10 + history.bonus(uri, now),
            });
        }
    }

    let has_app = out.iter().any(|r| r.kind == ResultKind::App);
    if !has_app
        && let (Some(word), Some(on_path)) = (bare_command(q), opts.on_path)
        && on_path(word)
    {
        out.push(SearchResult {
            kind: ResultKind::Run,
            id: word.to_owned(),
            title: format!("Run {word}"),
            subtitle: "Program".into(),
            score: 3000,
        });
    }

    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    out.truncate(MAX_RESULTS);

    if !opts.web_search.is_empty() && opts.web_search.contains("{}") {
        let encoded: String = q
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                b' ' => "+".into(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        out.push(SearchResult {
            kind: ResultKind::Web,
            id: opts.web_search.replace("{}", &encoded),
            title: format!("Search the web for \"{q}\""),
            subtitle: "Web".into(),
            score: 0,
        });
    }
    out
}

/// Categories for the sidebar, sorted, with "Other" last. Apps without a folder are in "Other".
pub fn categories(apps: &[AppEntry]) -> Vec<String> {
    let mut cats: Vec<String> = apps.iter().map(|a| category_of(a).to_owned()).collect();
    cats.sort_by_key(|c| (c == "Other", c.to_lowercase()));
    cats.dedup();
    cats
}

pub fn category_of(app: &AppEntry) -> &str {
    if app.category.is_empty() { "Other" } else { &app.category }
}

/// Apps sorted for "All applications": by name, case-insensitively, ignoring leading
/// punctuation.
pub fn sorted_apps(apps: &[AppEntry]) -> Vec<&AppEntry> {
    let mut v: Vec<&AppEntry> = apps.iter().collect();
    v.sort_by_cached_key(|a| fuzzy::fold(a.name.trim_start_matches(|c: char| !c.is_alphanumeric())));
    v
}

/// The A–Z section letter for an app name ("#" for digits and symbols).
pub fn section_letter(name: &str) -> String {
    match fuzzy::fold(name).chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        _ => "#".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, cat: &str, kw: &[&str]) -> AppEntry {
        AppEntry { id: id.into(), name: name.into(), keywords: kw.iter().map(|s| (*s).to_owned()).collect(), category: cat.into(), path: None, elevatable: None }
    }

    fn apps() -> Vec<AppEntry> {
        vec![
            app("code", "Visual Studio Code", "Visual Studio Code", &["Code.exe"]),
            app("vs", "Visual Studio 2022", "", &["devenv.exe"]),
            app("calc", "Calculator", "", &[]),
            app("ff", "Firefox", "", &["firefox.exe"]),
            app("ffdev", "Firefox Developer Edition", "", &[]),
            app("np", "Notepad", "Accessories", &["notepad.exe"]),
        ]
    }

    fn ids(r: &[SearchResult]) -> Vec<&str> {
        r.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn ranks_apps_by_match_quality_and_history() {
        let h = History::default();
        let r = search("vsc", &apps(), &h, 0, &SearchOptions::default());
        assert_eq!(r[0].id, "code");
        let r = search("fire", &apps(), &h, 0, &SearchOptions::default());
        assert_eq!(ids(&r)[..2], ["ff", "ffdev"]);

        // Using Developer Edition a lot moves it up.
        let mut h = History::default();
        for _ in 0..10 {
            h.record("ffdev", 1000);
        }
        let r = search("fire", &apps(), &h, 1000, &SearchOptions::default());
        assert_eq!(r[0].id, "ffdev");
        // Keywords find apps by executable name.
        assert_eq!(search("devenv", &apps(), &History::default(), 0, &SearchOptions::default())[0].id, "vs");
    }

    #[test]
    fn calculator_run_settings_and_web() {
        let h = History::default();
        let opts = SearchOptions { web_search: "https://duckduckgo.com/?q={}", on_path: None };
        let r = search("12*(3+4)", &apps(), &h, 0, &opts);
        assert_eq!((r[0].kind, r[0].id.as_str()), (ResultKind::Calculator, "84"));
        assert_eq!(r.last().unwrap().kind, ResultKind::Web);

        let r = search(">cmd /c echo hi", &apps(), &h, 0, &SearchOptions::default());
        assert_eq!((r[0].kind, r[0].id.as_str()), (ResultKind::Run, ">cmd /c echo hi"));

        let r = search("bluetooth", &apps(), &h, 0, &SearchOptions::default());
        assert_eq!((r[0].kind, r[0].id.as_str()), (ResultKind::Setting, "ms-settings:bluetooth"));
        let r = search("wallpaper", &apps(), &h, 0, &SearchOptions::default());
        assert!(r.iter().any(|r| r.id == "ms-settings:personalization-background"));

        let web = search("a b&c", &[], &h, 0, &opts);
        assert_eq!(web[0].id, "https://duckduckgo.com/?q=a+b%26c");
    }

    #[test]
    fn bare_commands_only_when_no_app_matches() {
        let on_path = |w: &str| w == "regedit";
        let opts = SearchOptions { web_search: "", on_path: Some(&on_path) };
        let r = search("regedit", &apps(), &History::default(), 0, &opts);
        assert_eq!((r[0].kind, r[0].id.as_str()), (ResultKind::Run, "regedit"));
        let r = search("notepad", &apps(), &History::default(), 0, &opts);
        assert_eq!(r[0].id, "np");
    }

    #[test]
    fn run_classification() {
        assert_eq!(classify_run(">ping 1.1.1.1"), Some(RunKind::Command("ping 1.1.1.1".into())));
        assert_eq!(classify_run(">  "), None);
        assert_eq!(classify_run("https://example.com"), Some(RunKind::Open("https://example.com".into())));
        assert_eq!(classify_run("www.rust-lang.org"), Some(RunKind::Open("https://www.rust-lang.org".into())));
        assert_eq!(classify_run(r"C:\Windows"), Some(RunKind::Open(r"C:\Windows".into())));
        assert_eq!(classify_run(r"\\nas\share"), Some(RunKind::Open(r"\\nas\share".into())));
        assert_eq!(classify_run("shell:startup"), Some(RunKind::Open("shell:startup".into())));
        assert_eq!(classify_run("firefox"), None);
        assert_eq!(bare_command("notepad"), Some("notepad"));
        assert_eq!(bare_command("two words"), None);
    }

    #[test]
    fn history_recent_and_decay() {
        let mut h = History::default();
        h.record("a", 100);
        h.record("b", 200);
        h.record("a", 300);
        assert_eq!(h.recent(5), ["a", "b"]);
        assert!(h.bonus("a", 300) > h.bonus("b", 300));
        assert!(h.bonus("a", 300) > h.bonus("a", 300 + 60 * 86_400), "decays over time");
        h.forget("a");
        assert_eq!(h.recent(5), ["b"]);
        for i in 0..(MAX_HISTORY as i64 + 10) {
            h.record(&format!("x{i}"), 1000 + i);
        }
        assert!(h.entries.len() <= MAX_HISTORY + 1);
    }

    #[test]
    fn categories_and_sorting() {
        let a = apps();
        assert_eq!(categories(&a), ["Accessories", "Visual Studio Code", "Other"]);
        let names: Vec<&str> = sorted_apps(&a).iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names[0], "Calculator");
        assert_eq!(section_letter("7-Zip"), "#");
        assert_eq!(section_letter("éditeur"), "E");
    }
}
