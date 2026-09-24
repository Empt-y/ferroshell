//! `fsh-shell`: panels, widgets and core services. Normally started by `fsh-session`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod apps;
mod app;
mod app_tasks;
mod app_tray;
mod control;
mod launcher;
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

use fsh_common::{SAFE_MODE_FLAG, SUPERVISOR_FLAG, exit_codes, paths};
use fsh_win::{crash, process};

pub struct Args {
    pub supervisor: Option<u32>,
    pub safe_mode: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args { supervisor: None, safe_mode: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            SAFE_MODE_FLAG => args.safe_mode = true,
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
        watch_supervisor(pid);
    }

    // Safe mode uses the software renderer in case a GPU driver is what keeps crashing us.
    let renderer = if args.safe_mode { "software" } else { "femtovg" };
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .select()
        .map_err(|e| anyhow::anyhow!("selecting the {renderer} renderer: {e}"))?;

    let app = app::App::start(args.safe_mode)?;
    control::serve()?;

    slint::run_event_loop_until_quit()?;

    let code = app.exit_code();
    tracing::info!("event loop finished; exiting with code {code}");
    drop(app);
    app::shutdown();
    Ok(code)
}

/// If the supervisor dies, nobody else will restore Explorer's taskbar — do it and exit.
fn watch_supervisor(pid: u32) {
    let spawned = std::thread::Builder::new().name("supervisor-watch".into()).spawn(move || {
        let _ = process::wait_for_exit(pid);
        tracing::warn!("supervisor (pid {pid}) is gone; restoring Explorer and exiting");
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
