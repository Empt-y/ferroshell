//! Starts `fsh-shell`, watches it, and decides what to do when it exits.
//!
//! Everything runs on one supervisor thread that owns the child process. Commands (hotkey,
//! pipe) only flip fields under the mutex and poke the condvar, so a slow or hung shell can
//! never block the hotkey window.

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fsh_common::{REPLACE_FLAG, SAFE_MODE_FLAG, SUPERVISOR_FLAG, exit_codes, paths, pipes};
use fsh_win::taskbar;
use serde_json::{Value, json};

use crate::policy::{CrashPolicy, Decision};

const POLL: Duration = Duration::from_millis(200);
const GRACEFUL_STOP: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct Options {
    pub shell_exe: PathBuf,
    /// Hide Explorer's taskbar while the shell runs (off for side-by-side debugging).
    pub hide_explorer_taskbar: bool,
    /// We are the Winlogon shell: fall back to launching Explorer rather than un-hiding it.
    pub replace: bool,
    /// Start straight into safe mode.
    pub safe_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Run,
    Stop,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Start,
    Stop,
    Toggle,
    Restart,
    SafeMode(bool),
    Quit,
}

struct Inner {
    want: Want,
    policy: CrashPolicy,
    child: Option<Child>,
    child_started: Option<Instant>,
    /// The next exit is one we asked for; don't count it as a crash.
    expected_exit: bool,
    gave_up: bool,
    restarts: u32,
    last_exit: Option<String>,
}

pub struct Supervisor {
    opts: Options,
    hotkey: Mutex<Option<&'static str>>,
    inner: Mutex<Inner>,
    cv: Condvar,
    /// The Windows session is ending (sign-out, shutdown): never launch Explorer now.
    ending: AtomicBool,
    /// Which shell-ready events were signalled (replace mode).
    shell_ready: Arc<Mutex<Vec<&'static str>>>,
}

impl Supervisor {
    pub fn new(opts: Options) -> Self {
        let mut policy = CrashPolicy::new(3, Duration::from_secs(60));
        policy.reset(opts.safe_mode);
        Self {
            opts,
            hotkey: Mutex::new(None),
            inner: Mutex::new(Inner {
                want: Want::Run,
                policy,
                child: None,
                child_started: None,
                expected_exit: false,
                gave_up: false,
                restarts: 0,
                last_exit: None,
            }),
            cv: Condvar::new(),
            ending: AtomicBool::new(false),
            shell_ready: Arc::default(),
        }
    }

    /// The Windows session is ending: put Explorer's taskbar back if we hid it, but don't
    /// start Explorer as a fallback shell on the way out.
    pub fn session_ending(&self) {
        self.ending.store(true, Ordering::SeqCst);
        self.restore_desktop();
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock only means some thread panicked mid-update; the fields are all
        // individually valid, so keep going rather than taking the supervisor down.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn command(&self, cmd: Cmd) {
        let mut g = self.lock();
        tracing::info!("command: {cmd:?}");
        match cmd {
            Cmd::Start => start(&mut g, false),
            Cmd::Stop => g.want = Want::Stop,
            Cmd::Toggle => {
                if g.want == Want::Run {
                    g.want = Want::Stop;
                } else {
                    start(&mut g, false);
                }
            }
            Cmd::Restart => {
                let safe = g.policy.safe_mode;
                start(&mut g, safe);
                request_child_exit(&mut g);
            }
            Cmd::SafeMode(on) => {
                start(&mut g, on);
                request_child_exit(&mut g);
            }
            Cmd::Quit => g.want = Want::Quit,
        }
        self.cv.notify_all();

        fn start(g: &mut Inner, safe_mode: bool) {
            g.want = Want::Run;
            g.gave_up = false;
            g.policy.reset(safe_mode);
        }
        fn request_child_exit(g: &mut Inner) {
            if g.child.is_some() {
                g.expected_exit = true;
            }
        }
    }

    pub fn set_emergency_hotkey(&self, name: Option<&'static str>) {
        if let Ok(mut h) = self.hotkey.lock() {
            *h = name;
        }
    }

    /// PID and uptime of the running shell, if any.
    pub fn running_child(&self) -> Option<(u32, Duration)> {
        let g = self.lock();
        Some((g.child.as_ref()?.id(), g.child_started?.elapsed()))
    }

    /// Kill the shell if it is still process `pid` (used by the hang watchdog). The exit
    /// is then handled like any crash, so repeated hangs escalate to safe mode.
    pub fn kill_hung(&self, pid: u32) {
        let mut g = self.lock();
        if let Some(child) = g.child.as_mut().filter(|c| c.id() == pid) {
            tracing::error!("shell (pid {pid}) is not responding; killing it");
            let _ = child.kill();
        }
    }

    /// True while the shell should be running and Explorer's taskbar is ours to keep hidden.
    pub fn keeping_taskbar_hidden(&self) -> bool {
        self.opts.hide_explorer_taskbar
            && self.lock().want == Want::Run
            && taskbar::needs_restore(&paths::taskbar_state_file())
    }

    pub fn status(&self) -> Value {
        let g = self.lock();
        json!({
            "state": match (g.want, g.gave_up) {
                (_, true) => "gave-up",
                (Want::Run, _) => if g.child.is_some() { "running" } else { "starting" },
                (Want::Stop, _) => "stopped",
                (Want::Quit, _) => "quitting",
            },
            "safe_mode": g.policy.safe_mode,
            "pid": g.child.as_ref().map(Child::id),
            "uptime_secs": g.child_started.map(|t| t.elapsed().as_secs()),
            "restarts": g.restarts,
            "recent_crashes": g.policy.recent_crashes(),
            "last_exit": g.last_exit,
            "replace_mode": self.opts.replace,
            "shell_ready_signalled": *self.shell_ready.lock().unwrap_or_else(|e| e.into_inner()),
            "session_ending": self.ending.load(Ordering::SeqCst),
            "emergency_hotkey": self.hotkey.lock().ok().and_then(|h| *h),
        })
    }

    /// Put the desktop back into a usable state. Called on stop, give-up and quit, and from
    /// emergency paths (logoff, console close) where the supervisor thread may not run again.
    pub fn restore_desktop(&self) {
        let state = paths::taskbar_state_file();
        if taskbar::needs_restore(&state) {
            tracing::info!("restoring Explorer's taskbar");
            taskbar::restore(&state);
        } else if self.opts.replace && !self.ending.load(Ordering::SeqCst) && !taskbar::explorer_taskbar_present() {
            tracing::warn!("launching Explorer as fallback shell");
            if let Err(e) = Command::new("explorer.exe").spawn() {
                tracing::error!("could not launch Explorer: {e}");
            }
        }
    }

    /// Body of the supervisor thread. Returns once a quit has been requested and the
    /// shell is stopped and the desktop restored.
    pub fn run(&self) {
        let mut g = self.lock();
        loop {
            match g.want {
                Want::Quit => break,
                Want::Stop => {
                    if let Some(child) = g.child.take() {
                        g = self.stop_child(g, child);
                    }
                    self.restore_desktop();
                    while g.want == Want::Stop {
                        g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    continue;
                }
                Want::Run => {}
            }

            if g.child.is_none() {
                g = self.spawn(g);
                continue;
            }

            g = self.cv.wait_timeout(g, POLL).unwrap_or_else(|e| e.into_inner()).0;
            if g.want != Want::Run {
                continue;
            }
            if g.expected_exit && g.child.is_some() {
                if let Some(child) = g.child.take() {
                    g = self.stop_child(g, child);
                }
                g.expected_exit = false;
                continue;
            }
            let exited = g.child.as_mut().and_then(|c| c.try_wait().ok().flatten());
            if let Some(status) = exited {
                g.child = None;
                g = self.on_exit(g, status);
            }
        }

        if let Some(child) = g.child.take() {
            g = self.stop_child(g, child);
        }
        drop(g);
        self.restore_desktop();
    }

    fn spawn<'a>(&'a self, mut g: MutexGuard<'a, Inner>) -> MutexGuard<'a, Inner> {
        g.expected_exit = false;
        if self.opts.hide_explorer_taskbar
            && taskbar::explorer_taskbar_present()
            && let Err(e) = taskbar::hide(&paths::taskbar_state_file())
        {
            tracing::warn!("could not hide Explorer's taskbar: {e:#}");
        }
        let mut cmd = Command::new(&self.opts.shell_exe);
        cmd.arg(SUPERVISOR_FLAG).arg(std::process::id().to_string());
        if g.policy.safe_mode {
            cmd.arg(SAFE_MODE_FLAG);
        }
        if self.opts.replace {
            cmd.arg(REPLACE_FLAG);
        }
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!(
                    "started shell (pid {}){}",
                    child.id(),
                    if g.policy.safe_mode { " in SAFE MODE" } else { "" }
                );
                if self.opts.replace {
                    signal_ready_when_up(self.shell_ready.clone());
                }
                g.child = Some(child);
                g.child_started = Some(Instant::now());
                g
            }
            Err(e) => {
                tracing::error!("could not start {}: {e}", self.opts.shell_exe.display());
                g.last_exit = Some(format!("spawn failed: {e}"));
                self.on_crash(g)
            }
        }
    }

    fn on_exit<'a>(&'a self, mut g: MutexGuard<'a, Inner>, status: ExitStatus) -> MutexGuard<'a, Inner> {
        let code = status.code();
        let desc = match code {
            Some(c) => format!("exit code {c} ({:#010x})", c as u32),
            None => "no exit code".to_owned(),
        };
        g.last_exit = Some(desc.clone());
        match code {
            Some(exit_codes::QUIT) => {
                tracing::info!("shell quit at the user's request");
                g.want = Want::Quit;
                g
            }
            Some(exit_codes::RESTART) => {
                tracing::info!("shell asked to be restarted");
                g.restarts += 1;
                g
            }
            _ => {
                tracing::error!("shell crashed: {desc}");
                self.on_crash(g)
            }
        }
    }

    fn on_crash<'a>(&'a self, mut g: MutexGuard<'a, Inner>) -> MutexGuard<'a, Inner> {
        g.restarts += 1;
        match g.policy.on_crash(Instant::now()) {
            Decision::Restart(delay) => {
                tracing::info!("restarting in {delay:?}");
                // Wait on the condvar so a stop/quit command still gets through immediately.
                let deadline = Instant::now() + delay;
                while g.want == Want::Run {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    g = self.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
                }
            }
            Decision::EnterSafeMode => {
                tracing::warn!("shell keeps crashing; restarting in safe mode");
            }
            Decision::GiveUp => {
                tracing::error!(
                    "shell crashes even in safe mode; giving up. Press Ctrl+Alt+Shift+E or run `fsh-ctl start` to retry"
                );
                g.gave_up = true;
                g.want = Want::Stop;
            }
        }
        g
    }

    /// Ask the shell to exit cleanly, then kill it if it doesn't. The lock is released while
    /// waiting so status queries keep working.
    fn stop_child<'a>(&'a self, g: MutexGuard<'a, Inner>, mut child: Child) -> MutexGuard<'a, Inner> {
        drop(g);
        let pid = child.id();
        let asked = fsh_ipc::pipe::call(pipes::SHELL, "shutdown", Value::Null, Duration::from_secs(1)).is_ok();
        let deadline = Instant::now() + if asked { GRACEFUL_STOP } else { Duration::ZERO };
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                tracing::info!("shell (pid {pid}) stopped");
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!("shell (pid {pid}) did not exit in time; killing it");
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut g = self.lock();
        g.child_started = None;
        g
    }
}

/// Once the shell answers a ping (or after 10 s regardless), tell Windows the desktop is
/// ready so the sign-in screen goes away. Runs on its own thread; never blocks supervision.
fn signal_ready_when_up(record: Arc<Mutex<Vec<&'static str>>>) {
    let spawned = std::thread::Builder::new().name("shell-ready".into()).spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if fsh_ipc::pipe::call(pipes::SHELL, "ping", Value::Null, Duration::from_millis(500)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let signalled = fsh_win::session::signal_shell_ready();
        tracing::info!("shell ready; signalled {signalled:?}");
        *record.lock().unwrap_or_else(|e| e.into_inner()) = signalled;
    });
    if let Err(e) = spawned {
        tracing::warn!("could not start the shell-ready thread: {e}");
    }
}
