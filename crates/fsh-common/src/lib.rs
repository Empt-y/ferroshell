//! Things every Ferroshell process needs: well-known paths, pipe names, exit codes and logging.

use std::path::PathBuf;

pub mod paths {
    use super::*;

    fn env_dir(var: &str) -> PathBuf {
        std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("ferroshell")
    }

    /// `%APPDATA%\ferroshell` — user-editable config, themes and widgets.
    pub fn config_dir() -> PathBuf {
        env_dir("APPDATA")
    }

    /// `%LOCALAPPDATA%\ferroshell` — machine-local state: logs, dumps, runtime state.
    pub fn state_dir() -> PathBuf {
        env_dir("LOCALAPPDATA")
    }

    pub fn log_dir() -> PathBuf {
        state_dir().join("logs")
    }

    pub fn dump_dir() -> PathBuf {
        state_dir().join("dumps")
    }

    /// Records Explorer's original taskbar state while we have it hidden, so any process
    /// (or the next run, after a hard kill) can put it back.
    pub fn taskbar_state_file() -> PathBuf {
        state_dir().join("explorer-taskbar.json")
    }

    /// Directory containing the running executable; sibling binaries live here.
    pub fn exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Named-pipe names (served under `\\.\pipe\`).
pub mod pipes {
    /// Served by `fsh-session`: lifecycle commands (status, restart, safe mode, stop).
    pub const SESSION: &str = "ferroshell-session";
    /// Served by `fsh-shell`: runtime commands (reload, themes, state dumps).
    pub const SHELL: &str = "ferroshell";
}

/// Exit codes `fsh-shell` uses to tell the supervisor what happened.
pub mod exit_codes {
    /// Clean shutdown requested by the user; the supervisor stops too.
    pub const QUIT: i32 = 0;
    /// Restart requested (e.g. `fsh-ctl restart`); not counted as a crash.
    pub const RESTART: i32 = 3;
    /// The supervisor that launched us went away.
    pub const ORPHANED: i32 = 4;
}

/// Command-line flag that starts `fsh-shell` in safe mode.
pub const SAFE_MODE_FLAG: &str = "--safe-mode";
/// Command-line flag carrying the supervisor's PID.
pub const SUPERVISOR_FLAG: &str = "--supervisor";
/// Command-line flag (for both `fsh-session` and `fsh-shell`): Ferroshell is the login
/// shell Winlogon started, in place of Explorer.
pub const REPLACE_FLAG: &str = "--replace";

/// Initialise logging to a daily-rolling file in [`paths::log_dir`] plus stderr.
/// Keep the returned guard alive for the life of the process so buffered lines are flushed.
pub fn init_logging(process_name: &str) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let dir = paths::log_dir();
    std::fs::create_dir_all(&dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(process_name)
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("FERROSHELL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    install_panic_logger();
    Ok(guard)
}

/// Route panics through `tracing` (with a backtrace) before the default hook runs,
/// so they end up in the log file even for GUI processes with no console.
fn install_panic_logger() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{bt}");
        default(info);
    }));
}
