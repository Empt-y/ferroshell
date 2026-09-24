//! `fsh-session`: the Ferroshell supervisor.
//!
//! Keeps the shell alive, escalates to safe mode and then back to Explorer if it keeps
//! crashing, owns the emergency hotkey (Ctrl+Alt+Shift+E), and makes sure Explorer's
//! taskbar comes back however things end.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod policy;
mod supervisor;
mod watchdog;

use std::process::ExitCode;
use std::sync::Arc;

use fsh_common::{paths, pipes};
use fsh_ipc::RpcError;
use fsh_win::hotkey::{self, Modifiers, WM_HOTKEY};
use fsh_win::window::{self, MessageWindow, WM_APP};
use fsh_win::{console, crash, process::SingleInstance, taskbar};
use serde_json::{Value, json};

use supervisor::{Cmd, Options, Supervisor};

const HOTKEY_EMERGENCY: i32 = 1;
const TIMER_ENFORCE_HIDDEN: usize = 1;
const WM_SUPERVISOR_DONE: u32 = WM_APP + 1;
const WM_TIMER: u32 = 0x0113;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;

const USAGE: &str = "\
fsh-session [options]
  --safe-mode             start the shell in safe mode
  --keep-explorer-taskbar don't hide Explorer's taskbar (side-by-side debugging)
  --replace               running as the Winlogon shell: fall back to launching Explorer";

fn main() -> ExitCode {
    let _log = match fsh_common::init_logging("session") {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("logging unavailable: {e:#}");
            None
        }
    };
    crash::install_minidump_handler(&paths::dump_dir(), "fsh-session");

    let mut opts = Options {
        shell_exe: std::env::var_os("FERROSHELL_SHELL_EXE")
            .map(Into::into)
            .unwrap_or_else(|| paths::exe_dir().join("fsh-shell.exe")),
        hide_explorer_taskbar: true,
        replace: false,
        safe_mode: false,
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--safe-mode" => opts.safe_mode = true,
            "--keep-explorer-taskbar" => opts.hide_explorer_taskbar = false,
            "--replace" => opts.replace = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument `{other}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    match run(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            // Whatever went wrong, never leave the user without a taskbar.
            taskbar::restore(&paths::taskbar_state_file());
            ExitCode::FAILURE
        }
    }
}

fn run(opts: Options) -> anyhow::Result<()> {
    let Some(_instance) = SingleInstance::acquire("ferroshell-session")? else {
        anyhow::bail!("fsh-session is already running (use `fsh-ctl status`)");
    };
    tracing::info!("fsh-session {} starting: {opts:?}", env!("CARGO_PKG_VERSION"));
    if taskbar::needs_restore(&paths::taskbar_state_file()) {
        tracing::warn!("previous session didn't restore Explorer's taskbar; its saved state will be reused");
    }

    let sup = Arc::new(Supervisor::new(opts));

    let taskbar_created = window::register_window_message("TaskbarCreated");
    let sup_w = sup.clone();
    let win = MessageWindow::new(
        "Ferroshell Session",
        Box::new(move |_hwnd, msg, wparam, _lparam| {
            match msg {
                WM_HOTKEY if wparam as i32 == HOTKEY_EMERGENCY => sup_w.command(Cmd::Toggle),
                WM_TIMER if wparam == TIMER_ENFORCE_HIDDEN => {
                    if sup_w.keeping_taskbar_hidden() && taskbar::enforce_hidden() > 0 {
                        tracing::debug!("re-hid Explorer's taskbar");
                    }
                }
                m if m == taskbar_created && m != 0 => {
                    if sup_w.keeping_taskbar_hidden() {
                        tracing::info!("Explorer restarted; hiding its taskbar again");
                        let _ = taskbar::hide(&paths::taskbar_state_file());
                    }
                }
                WM_QUERYENDSESSION => return Some(1),
                WM_ENDSESSION if wparam != 0 => {
                    tracing::info!("session ending");
                    sup_w.restore_desktop();
                    return Some(0);
                }
                WM_SUPERVISOR_DONE => window::post_quit(0),
                _ => return None,
            }
            Some(0)
        }),
    )?;
    win.set_timer(TIMER_ENFORCE_HIDDEN, 1000);

    // The emergency hotkey must work, so fall back if another program already uses it.
    let mods = Modifiers { ctrl: true, alt: true, shift: true, win: false };
    let candidates: [(u32, &str); 3] = [(u32::from(b'E'), "Ctrl+Alt+Shift+E"), (0x7B, "Ctrl+Alt+Shift+F12"), (u32::from(b'Q'), "Ctrl+Alt+Shift+Q")];
    let mut active = None;
    for (vk, name) in candidates {
        match hotkey::register(win.hwnd(), HOTKEY_EMERGENCY, mods, vk) {
            Ok(()) => {
                active = Some(name);
                break;
            }
            Err(e) => tracing::warn!("emergency hotkey {name} unavailable: {e:#}"),
        }
    }
    match active {
        Some(name) => tracing::info!("emergency hotkey: {name}"),
        None => tracing::error!("no emergency hotkey could be registered; use `fsh-ctl restore-explorer`, or `fsh-ctl stop`"),
    }
    sup.set_emergency_hotkey(active);

    let sup_c = sup.clone();
    console::on_console_close(move || sup_c.restore_desktop());

    let sup_p = sup.clone();
    fsh_ipc::pipe::serve(
        pipes::SESSION,
        Arc::new(move |method: &str, params: Value| {
            let cmd = match method {
                "status" => return Ok(sup_p.status()),
                "start" => Cmd::Start,
                "stop" => Cmd::Stop,
                "restart" => Cmd::Restart,
                "safe_mode" => Cmd::SafeMode(params.get("on").and_then(Value::as_bool).unwrap_or(true)),
                "quit" => Cmd::Quit,
                other => return Err(RpcError::method_not_found(other)),
            };
            sup_p.command(cmd);
            Ok(json!({"ok": true}))
        }),
    )?;

    watchdog::spawn(sup.clone())?;

    let hwnd = win.hwnd();
    let sup_t = sup.clone();
    let worker = std::thread::Builder::new().name("supervisor".into()).spawn(move || {
        sup_t.run();
        hwnd.post(WM_SUPERVISOR_DONE, 0, 0);
    })?;

    window::run_message_loop();
    hotkey::unregister(win.hwnd(), HOTKEY_EMERGENCY);
    let _ = worker.join();
    tracing::info!("fsh-session exiting");
    Ok(())
}
