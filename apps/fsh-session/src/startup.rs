//! Starting the user's startup apps when Ferroshell is the login shell (Explorer does this
//! otherwise): Run/RunOnce keys, Startup folders and packaged startup tasks, honouring the
//! switches in Task Manager's Startup apps and `[session]` in config.toml.

use std::time::Duration;

use fsh_common::paths;
use fsh_config::{Config, SessionConfig};
use fsh_core::startup::{self as rules, Candidate, RunOnceFlags, Source, Verdict};
use fsh_win::startup::{self as win, Hive};
use serde_json::{Value, json};

/// Pause between launches, so sign-in doesn't start everything at once.
const STAGGER: Duration = Duration::from_millis(300);

enum Launch {
    /// A Run-key command line.
    Command(String),
    /// A file or `shell:AppsFolder\<aumid>`, opened through the shell.
    Open(String),
}

struct Planned {
    candidate: Candidate,
    verdict: Verdict,
    launch: Launch,
    run_once: Option<(Hive, String, RunOnceFlags)>,
}

/// `[session]` from config.toml, read without writing anything (the shell owns the file).
pub fn session_config() -> SessionConfig {
    std::fs::read_to_string(paths::config_dir().join("config.toml"))
        .ok()
        .and_then(|text| Config::parse(&text).map_err(|e| tracing::warn!("config.toml: {e}; using default [session]")).ok())
        .map(|c| c.session)
        .unwrap_or_default()
}

fn plan(cfg: &SessionConfig) -> (Vec<Planned>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let safe_mode = win::windows_safe_mode();
    let hives = [Hive::LocalMachine, Hive::LocalMachine32, Hive::CurrentUser];
    let mut push = |candidate: Candidate, launch: Launch, run_once: Option<(Hive, String, RunOnceFlags)>, allowed: bool, why: &str| {
        let verdict = if !allowed {
            Verdict::Skip(why.to_owned())
        } else {
            rules::decide(&candidate, &cfg.startup_exclude)
        };
        out.push(Planned { candidate, verdict, launch, run_once });
    };

    for once in [true, false] {
        for hive in hives {
            for (name, command) in win::run_entries(hive, once) {
                let flags = rules::runonce_flags(&name);
                let enabled = once || rules::approved(win::run_approval(hive, &name).as_deref());
                let (allowed, why) = match () {
                    _ if !cfg.startup_apps => (false, "[session] startup-apps = false"),
                    _ if safe_mode && !(once && flags.in_safe_mode) => (false, "Windows is in safe mode"),
                    _ => (true, ""),
                };
                let candidate = Candidate {
                    source: if once { Source::RunOnce } else { Source::Run },
                    scope: hive.label().to_owned(),
                    name: name.clone(),
                    target: command.clone(),
                    enabled,
                };
                push(candidate, Launch::Command(command), once.then_some((hive, name, flags)), allowed, why);
            }
        }
    }

    for common in [true, false] {
        for path in win::startup_folder(common) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let target = path.to_string_lossy().into_owned();
            let candidate = Candidate {
                source: Source::StartupFolder,
                scope: if common { "all users" } else { "you" }.to_owned(),
                enabled: rules::approved(win::folder_approval(common, &name).as_deref()),
                name,
                target: target.clone(),
            };
            let why = if safe_mode { "Windows is in safe mode" } else { "[session] startup-apps = false" };
            push(candidate, Launch::Open(target), None, cfg.startup_apps && !safe_mode, why);
        }
    }

    match win::user_packages() {
        Ok(packages) => {
            for pkg in packages {
                let Ok(xml) = std::fs::read_to_string(pkg.install_path.join("AppxManifest.xml")) else { continue };
                for task in rules::manifest_startup_tasks(&xml) {
                    let state = win::startup_task_state(&pkg.family_name, &task.task_id);
                    let target = format!(r"shell:AppsFolder\{}!{}", pkg.family_name, task.app_id);
                    let candidate = Candidate {
                        source: Source::StartupTask,
                        scope: pkg.family_name.clone(),
                        name: task.task_id.clone(),
                        target: target.clone(),
                        enabled: rules::task_enabled(state, task.enabled_by_default),
                    };
                    let why = if safe_mode { "Windows is in safe mode" } else { "[session] startup-tasks = false" };
                    push(candidate, Launch::Open(target), None, cfg.startup_tasks && !safe_mode, why);
                }
            }
        }
        Err(e) => problems.push(format!("listing app packages: {e:#}")),
    }
    (out, problems)
}

fn entry_json(p: &Planned, result: Option<Result<(), String>>) -> Value {
    let mut v = json!(p.candidate);
    let obj = v.as_object_mut().expect("a candidate serialises as an object");
    match &p.verdict {
        Verdict::Start => obj.insert("start".into(), json!(true)),
        Verdict::Skip(why) => {
            obj.insert("start".into(), json!(false));
            obj.insert("skipped".into(), json!(why))
        }
    };
    if let Some(r) = result {
        obj.insert("result".into(), match r {
            Ok(()) => json!("started"),
            Err(e) => json!(e),
        });
    }
    v
}

/// What would start at sign-in and why the rest wouldn't; starts nothing.
pub fn dry_run() -> Value {
    let _com = fsh_win::com::ComGuard::mta();
    let cfg = session_config();
    let (planned, problems) = plan(&cfg);
    json!({
        "delay_seconds": cfg.startup_delay,
        "would_start": planned.iter().filter(|p| p.verdict == Verdict::Start).count(),
        "entries": planned.iter().map(|p| entry_json(p, None)).collect::<Vec<_>>(),
        "problems": problems,
    })
}

fn launch(p: &Planned) -> Result<(), String> {
    // RunOnce: delete first so a crashing entry can't run at every sign-in, unless it's
    // `!`-prefixed (delete once it has started). Unwritable (HKLM without admin): skip,
    // as Explorer does.
    if let Some((hive, name, flags)) = &p.run_once
        && !flags.delete_after
    {
        win::delete_run_once(*hive, name).map_err(|e| format!("{e:#}; not run"))?;
    }
    let started = match &p.launch {
        Launch::Command(cmd) => win::run_command_line(cmd).map(drop),
        Launch::Open(target) => fsh_win::winops::launch_quiet(target),
    };
    started.map_err(|e| format!("{e:#}"))?;
    if let Some((hive, name, flags)) = &p.run_once
        && flags.delete_after
        && let Err(e) = win::delete_run_once(*hive, name)
    {
        tracing::warn!("RunOnce {name}: started, but {e:#}");
    }
    Ok(())
}

/// Starts the startup apps, once per sign-in (a restarted session or shell doesn't start
/// them again). Blocks for the configured delay and the launches; run it on its own thread.
/// Returns a summary for `fsh-ctl status`.
pub fn run_once_per_sign_in() -> Value {
    let cfg = session_config();
    match win::claim_startup_run() {
        Ok(true) => {}
        Ok(false) => return json!({ "ran": false, "reason": "already started this sign-in" }),
        Err(e) => return json!({ "ran": false, "reason": format!("{e:#}") }),
    }
    std::thread::sleep(Duration::from_secs(u64::from(cfg.startup_delay)));
    // Shell launches (shortcuts, AppsFolder) need a single-threaded apartment.
    let _com = fsh_win::com::ComGuard::new();
    let (planned, problems) = plan(&cfg);
    let mut entries = Vec::new();
    let (mut started, mut failed) = (0, 0);
    for p in &planned {
        let result = (p.verdict == Verdict::Start).then(|| {
            let r = launch(p);
            match &r {
                Ok(()) => {
                    started += 1;
                    tracing::info!("startup: started {} ({})", p.candidate.name, p.candidate.target);
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!("startup: {} failed: {e}", p.candidate.name);
                }
            }
            std::thread::sleep(STAGGER);
            r
        });
        entries.push(entry_json(p, result));
    }
    for problem in &problems {
        tracing::warn!("startup: {problem}");
    }
    tracing::info!("startup apps: {started} started, {failed} failed, {} skipped", planned.len() - started - failed);
    json!({ "ran": true, "started": started, "failed": failed, "entries": entries, "problems": problems })
}
