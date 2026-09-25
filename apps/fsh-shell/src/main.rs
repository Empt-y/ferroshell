//! `fsh-shell`: panels, widgets and core services. Normally started by `fsh-session`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod apps;
mod app;
mod app_tasks;
mod app_tray;
mod banner;
mod control;
mod desktop;
mod launcher;
mod notify;
mod osd;
mod panel;
mod plugins;
mod popup;
mod scripts;
mod services;
mod taskbar;
mod thumbs;
mod timefmt;
mod tracker;
mod tray;
mod traybar;
mod watch;

use std::process::ExitCode;
use std::time::Duration;

use fsh_common::{REPLACE_FLAG, SAFE_MODE_FLAG, SUPERVISOR_FLAG, exit_codes, paths};
use fsh_win::{crash, process};

pub struct Args {
    pub supervisor: Option<u32>,
    pub safe_mode: bool,
    /// Ferroshell is the login shell (the supervisor was started by Winlogon).
    pub replace: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args { supervisor: None, safe_mode: false, replace: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            SAFE_MODE_FLAG => args.safe_mode = true,
            REPLACE_FLAG => args.replace = true,
            SUPERVISOR_FLAG => {
                let pid = it.next().ok_or_else(|| anyhow::anyhow!("{SUPERVISOR_FLAG} needs a pid"))?;
                args.supervisor = Some(pid.parse()?);
            }
            other => anyhow::bail!("unknown argument `{other}`"),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    // `fsh-shell --identity`: report the package identity (packaging/identity/) and exit,
    // without starting the shell. Exit code 0 with identity, 2 without.
    if std::env::args().nth(1).as_deref() == Some("--identity") {
        let _com = fsh_win::com::ComGuard::mta();
        let name = fsh_win::identity::package_full_name();
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        // Never prompts: the consent prompt comes from the notifications applet.
        let access = fsh_win::notifications::access();
        // Which apps, and whether text was found: never the notifications' content.
        let toasts = (access == fsh_win::notifications::Access::Allowed).then(fsh_win::notifications::toasts).unwrap_or_default();
        let summary: Vec<_> = toasts
            .iter()
            .map(|t| serde_json::json!({ "app": t.app_name, "has_app_id": !t.app_id.is_empty(), "has_title": !t.title.is_empty(), "has_body": !t.body.is_empty(), "created": t.created }))
            .collect();
        println!("{}", serde_json::json!({ "identity": name, "exe": exe, "notification_access": format!("{access:?}"), "notifications": summary }));
        return if name.is_some() { ExitCode::SUCCESS } else { ExitCode::from(2) };
    }
    // `fsh-shell --test-notification [count]`: raise test notifications as Ferroshell (needs
    // package identity) to try out the notifications applet and banners, then exit.
    if std::env::args().nth(1).as_deref() == Some("--test-notification") {
        let _com = fsh_win::com::ComGuard::mta();
        let count: u32 = std::env::args().nth(2).and_then(|n| n.parse().ok()).unwrap_or(1).clamp(1, 10);
        for i in 1..=count {
            let title = format!("Ferroshell test {i}");
            if let Err(e) = fsh_win::notifications::send_test(&title, "A test notification for the notifications applet. Safe to dismiss.") {
                println!("could not send: {e} (does fsh-shell.exe have package identity? try --identity)");
                return ExitCode::FAILURE;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        println!("sent {count}");
        return ExitCode::SUCCESS;
    }
    let _log = fsh_common::init_logging("shell").ok();
    crash::install_minidump_handler(&paths::dump_dir(), "fsh-shell");
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<i32> {
    // COM on the UI thread (the same apartment winit/OLE use later) for the process's
    // lifetime, and process-wide COM security before any other thread touches COM.
    let _com = fsh_win::com::ComGuard::new();
    if let Err(e) = fsh_win::com::init_process_security() {
        tracing::warn!("COM security not set (brightness control may be unavailable): {e}");
    }
    let args = parse_args()?;
    let Some(_instance) = process::SingleInstance::acquire("ferroshell-shell")? else {
        anyhow::bail!("fsh-shell is already running");
    };
    tracing::info!(
        "fsh-shell {} starting{}",
        env!("CARGO_PKG_VERSION"),
        if args.safe_mode { " in SAFE MODE" } else { "" }
    );

    if let Some(pid) = args.supervisor {
        watch_supervisor(pid, args.replace);
    }

    // Safe mode uses the software renderer in case a GPU driver is what keeps crashing us.
    let renderer = if args.safe_mode { "software" } else { "femtovg" };
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .select()
        .map_err(|e| anyhow::anyhow!("selecting the {renderer} renderer: {e}"))?;

    let app = app::App::start(args.safe_mode, args.replace)?;
    control::serve()?;

    slint::run_event_loop_until_quit()?;

    let code = app.exit_code();
    tracing::info!("event loop finished; exiting with code {code}");
    drop(app);
    app::shutdown();
    Ok(code)
}

/// If the supervisor dies, nobody else will restore Explorer's taskbar — do it and exit.
fn watch_supervisor(pid: u32, replace: bool) {
    let spawned = std::thread::Builder::new().name("supervisor-watch".into()).spawn(move || {
        let _ = process::wait_for_exit(pid);
        if replace {
            // We're the login shell: nothing else will bring a desktop back.
            tracing::warn!("supervisor (pid {pid}) is gone; relaunching it");
            relaunch_session();
        } else {
            tracing::warn!("supervisor (pid {pid}) is gone; restoring Explorer and exiting");
        }
        fsh_win::taskbar::restore(&paths::taskbar_state_file());
        // Give the UI thread a moment to remove app bars cleanly, then exit regardless.
        let _ = slint::invoke_from_event_loop(|| app::request_exit(exit_codes::ORPHANED));
        std::thread::sleep(Duration::from_secs(2));
        std::process::exit(exit_codes::ORPHANED);
    });
    if let Err(e) = spawned {
        tracing::warn!("could not watch supervisor: {e}");
    }
}

/// Start a new `fsh-session --replace` next to this exe (it then starts a fresh shell once
/// this one has exited), or Explorer if the session keeps dying.
fn relaunch_session() {
    let file = paths::state_dir().join("session-relaunches.txt");
    let now = chrono::Utc::now().timestamp();
    let history: Vec<i64> = std::fs::read_to_string(&file).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).collect();
    let allowed = fsh_core::session::relaunch_allowed(&history, now, 60, 3);
    let mut kept: Vec<String> = history.iter().filter(|&&t| now - t < 60).map(i64::to_string).collect();
    kept.push(now.to_string());
    let _ = std::fs::write(&file, kept.join("\n"));
    let session = paths::exe_dir().join("fsh-session.exe");
    let result = if allowed && session.is_file() {
        std::process::Command::new(&session).arg(REPLACE_FLAG).spawn().map(|_| ())
    } else {
        tracing::error!("the session keeps dying (or is missing); starting Explorer instead");
        std::process::Command::new("explorer.exe").spawn().map(|_| ())
    };
    if let Err(e) = result {
        tracing::error!("could not relaunch: {e}");
    }
}
