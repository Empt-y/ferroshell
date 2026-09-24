//! Out-of-process plugins: one process per widget package that declares `[plugin]`,
//! speaking newline-delimited JSON-RPC over stdio (see docs/plugin-protocol.md).
//!
//! Stability measures:
//! - every plugin runs in a Job Object: it dies with the shell and has a memory cap;
//! - reading and writing happen on per-plugin threads, so a stuck plugin can't block
//!   the manager or the UI;
//! - plugins are pinged; one that stops answering is killed;
//! - crashed/killed plugins restart with exponential backoff, and a plugin that crashes
//!   `MAX_CRASHES` times within `CRASH_WINDOW` is disabled until the next reload.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fsh_ipc::jsonrpc::{Message, Notification, Request, read_message};
use serde_json::{Value, json};

use crate::app;

const PING_EVERY: Duration = Duration::from_secs(10);
const PING_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_GRACE: Duration = Duration::from_secs(2);
const MAX_CRASHES: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub struct PluginSpec {
    /// The widget package id; also the namespace for its global data sources.
    pub id: String,
    pub dir: PathBuf,
    pub exec: String,
    pub args: Vec<String>,
    pub max_memory_mb: u64,
    /// (instance id, settings) for every widget using this plugin.
    pub instances: Vec<(String, toml::Table)>,
}

impl PluginSpec {
    fn same_process(&self, other: &PluginSpec) -> bool {
        self.id == other.id && self.dir == other.dir && self.exec == other.exec && self.args == other.args
            && self.max_memory_mb == other.max_memory_mb
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginStatus {
    pub id: String,
    pub state: String,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub last_error: Option<String>,
}

enum Cmd {
    Set(Vec<PluginSpec>),
    Invoke { instance: String, action: String, arg: String },
    Line { id: String, generation: u64, msg: Result<Message, String> },
    Eof { id: String, generation: u64 },
}

pub struct PluginHost {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Vec<PluginStatus>>>,
}

impl PluginHost {
    pub fn spawn() -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let status = Arc::new(Mutex::new(Vec::new()));
        let (tx2, status2) = (tx.clone(), status.clone());
        std::thread::Builder::new().name("plugins".into()).spawn(move || Manager::new(tx2, status2).run(rx))?;
        Ok(Self { tx, status })
    }

    pub fn set(&self, specs: Vec<PluginSpec>) {
        let _ = self.tx.send(Cmd::Set(specs));
    }

    pub fn invoke(&self, instance: &str, action: &str, arg: &str) {
        let _ = self.tx.send(Cmd::Invoke { instance: instance.into(), action: action.into(), arg: arg.into() });
    }

    pub fn status(&self) -> Vec<PluginStatus> {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

struct Running {
    child: Child,
    _job: Option<fsh_win::job::Job>,
    writer: Sender<String>,
    ping_sent: Option<Instant>,
    last_ping: Instant,
    next_id: u64,
}

struct Plugin {
    spec: PluginSpec,
    generation: u64,
    running: Option<Running>,
    crashes: VecDeque<Instant>,
    restarts: u32,
    next_start: Option<Instant>,
    disabled: bool,
    last_error: Option<String>,
}

struct Manager {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Vec<PluginStatus>>>,
    plugins: HashMap<String, Plugin>,
    generation: u64,
}

impl Manager {
    fn new(tx: Sender<Cmd>, status: Arc<Mutex<Vec<PluginStatus>>>) -> Self {
        Self { tx, status, plugins: HashMap::new(), generation: 0 }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.housekeeping();
            self.publish_status();
        }
        for p in self.plugins.values_mut() {
            stop(p);
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Set(specs) => self.set(specs),
            Cmd::Invoke { instance, action, arg } => {
                let target = self.plugins.values().find(|p| p.spec.instances.iter().any(|(i, _)| *i == instance));
                match target.and_then(|p| p.running.as_ref()) {
                    Some(r) => {
                        let msg = Notification::new("invoke", json!({"instance": instance, "action": action, "arg": arg}));
                        let _ = r.writer.send(Message::from(msg).to_line());
                    }
                    None => tracing::warn!("plugin action {action} for {instance}: plugin not running"),
                }
            }
            Cmd::Line { id, generation, msg } => {
                if let Some(p) = self.plugins.get_mut(&id).filter(|p| p.generation == generation) {
                    on_message(p, msg);
                }
            }
            Cmd::Eof { id, generation } => {
                // Ignore the EOF that follows a kill we already counted.
                if let Some(p) = self.plugins.get_mut(&id).filter(|p| p.generation == generation && p.running.is_some()) {
                    crashed(p, "plugin closed its output (exited?)");
                }
            }
        }
    }

    fn set(&mut self, specs: Vec<PluginSpec>) {
        let wanted: HashMap<String, PluginSpec> = specs.into_iter().map(|s| (s.id.clone(), s)).collect();
        self.plugins.retain(|id, p| {
            let keep = wanted.get(id).is_some_and(|s| s.same_process(&p.spec));
            if !keep {
                tracing::info!("plugin {id} stopped");
                stop(p);
            }
            keep
        });
        for (id, spec) in wanted {
            match self.plugins.get_mut(&id) {
                Some(p) => {
                    if p.spec.instances != spec.instances {
                        p.spec = spec;
                        if let Some(r) = &p.running {
                            let n = Notification::new("instances", json!({"instances": instances_json(&p.spec)}));
                            let _ = r.writer.send(Message::from(n).to_line());
                        }
                    }
                }
                None => {
                    self.plugins.insert(
                        id,
                        Plugin {
                            spec,
                            generation: 0,
                            running: None,
                            crashes: VecDeque::new(),
                            restarts: 0,
                            next_start: Some(Instant::now()),
                            disabled: false,
                            last_error: None,
                        },
                    );
                }
            }
        }
    }

    fn housekeeping(&mut self) {
        let now = Instant::now();
        let ids: Vec<String> = self.plugins.keys().cloned().collect();
        for id in ids {
            let tx = self.tx.clone();
            self.generation += 1;
            let generation = self.generation;
            let Some(p) = self.plugins.get_mut(&id) else { continue };
            if p.disabled {
                continue;
            }
            if let Some(r) = p.running.as_mut() {
                if let Ok(Some(status)) = r.child.try_wait() {
                    crashed(p, &format!("exited with {status}"));
                    continue;
                }
                match r.ping_sent {
                    Some(t) if now.duration_since(t) > PING_TIMEOUT => {
                        tracing::error!("plugin {id} stopped answering; killing it");
                        let _ = r.child.kill();
                        crashed(p, "not responding");
                    }
                    None if now.duration_since(r.last_ping) > PING_EVERY => {
                        r.next_id += 1;
                        let req = Request::new(r.next_id, "ping", Value::Null);
                        let _ = r.writer.send(Message::from(req).to_line());
                        r.ping_sent = Some(now);
                        r.last_ping = now;
                    }
                    _ => {}
                }
            } else if p.next_start.is_some_and(|t| t <= now) {
                p.next_start = None;
                p.generation = generation;
                match start(p, tx) {
                    Ok(r) => {
                        tracing::info!("plugin {id} started (pid {})", r.child.id());
                        p.running = Some(r);
                    }
                    Err(e) => crashed(p, &format!("{e:#}")),
                }
            }
        }
    }

    fn publish_status(&self) {
        let list = self
            .plugins
            .values()
            .map(|p| PluginStatus {
                id: p.spec.id.clone(),
                state: if p.disabled {
                    "disabled".into()
                } else if p.running.is_some() {
                    "running".into()
                } else {
                    "restarting".into()
                },
                pid: p.running.as_ref().map(|r| r.child.id()),
                restarts: p.restarts,
                last_error: p.last_error.clone(),
            })
            .collect();
        if let Ok(mut s) = self.status.lock() {
            *s = list;
        }
    }
}

fn instances_json(spec: &PluginSpec) -> Value {
    Value::Array(
        spec.instances
            .iter()
            .map(|(instance, settings)| json!({"instance": instance, "settings": settings}))
            .collect(),
    )
}

fn resolve_exec(spec: &PluginSpec) -> PathBuf {
    let exec = spec.exec.replace("${EXE_DIR}", &fsh_common::paths::exe_dir().to_string_lossy());
    let p = PathBuf::from(&exec);
    if p.is_absolute() {
        return p;
    }
    let local = spec.dir.join(&p);
    if local.exists() { local } else { p } // otherwise let Windows search PATH
}

fn start(p: &mut Plugin, tx: Sender<Cmd>) -> anyhow::Result<Running> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = resolve_exec(&p.spec);
    let mut child = Command::new(&exe)
        .args(&p.spec.args)
        .current_dir(&p.spec.dir)
        .env("FERROSHELL_PLUGIN", &p.spec.id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| anyhow::anyhow!("starting {}: {e}", exe.display()))?;

    let job = match fsh_win::job::Job::new(Some((p.spec.max_memory_mb as usize) << 20)) {
        Ok(job) => match job.assign(&child) {
            Ok(()) => Some(job),
            Err(e) => {
                tracing::warn!("plugin {}: not in a job object: {e:#}", p.spec.id);
                None
            }
        },
        Err(e) => {
            tracing::warn!("plugin {}: could not create job object: {e:#}", p.spec.id);
            None
        }
    };

    let (id, generation) = (p.spec.id.clone(), p.generation);
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("no stderr"))?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;

    let (reader_id, reader_tx) = (id.clone(), tx.clone());
    std::thread::Builder::new().name(format!("plugin-{id}-out")).spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_message(&mut reader) {
                Ok(Some(msg)) => {
                    let msg = msg.map_err(|e| e.to_string());
                    if reader_tx.send(Cmd::Line { id: reader_id.clone(), generation, msg }).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = reader_tx.send(Cmd::Eof { id: reader_id, generation });
                    return;
                }
            }
        }
    })?;

    let err_id = id.clone();
    std::thread::Builder::new().name(format!("plugin-{id}-err")).spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            tracing::info!("[plugin {err_id} stderr] {line}");
        }
    })?;

    let (writer, rx) = channel::<String>();
    std::thread::Builder::new().name(format!("plugin-{id}-in")).spawn(move || write_loop(stdin, rx))?;

    let init = Request::new(0, "initialize", json!({"api_version": 1, "plugin": id, "instances": instances_json(&p.spec)}));
    let _ = writer.send(Message::from(init).to_line());
    Ok(Running { child, _job: job, writer, ping_sent: None, last_ping: Instant::now(), next_id: 0 })
}

fn write_loop(mut stdin: ChildStdin, rx: Receiver<String>) {
    for mut line in rx {
        line.push('\n');
        if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_err() {
            return;
        }
    }
}

fn on_message(p: &mut Plugin, msg: Result<Message, String>) {
    let id = p.spec.id.clone();
    let msg = match msg {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("plugin {id}: unreadable message: {e}");
            return;
        }
    };
    match msg {
        Message::Response(_) => {
            // Any response (to ping or initialize) proves the plugin is alive.
            if let Some(r) = p.running.as_mut() {
                r.ping_sent = None;
            }
        }
        Message::Notification(n) => handle_notification(&id, &n.method, &n.params),
        Message::Request(req) => {
            // Plugins don't make requests in protocol v1; answer so they don't wait.
            if let Some(r) = &p.running {
                let resp = fsh_ipc::Response::err(req.id, fsh_ipc::RpcError::method_not_found(&req.method));
                let _ = r.writer.send(Message::from(resp).to_line());
            }
        }
    }
}

fn handle_notification(id: &str, method: &str, params: &Value) {
    let s = |k: &str| params.get(k).and_then(Value::as_str).map(str::to_owned);
    match method {
        "publish" => {
            let Some(name) = s("name") else { return };
            let key = match s("instance") {
                Some(instance) => format!("{instance}/{name}"),
                None => format!("{id}/{name}"),
            };
            let value = match params.get("value") {
                Some(Value::String(v)) => v.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
            let _ = slint::invoke_from_event_loop(move || {
                app::with(|a| a.set_source(key, value));
            });
        }
        "log" => {
            let message = s("message").unwrap_or_default();
            match s("level").as_deref() {
                Some("error") => tracing::error!("[plugin {id}] {message}"),
                Some("warn") => tracing::warn!("[plugin {id}] {message}"),
                Some("debug") => tracing::debug!("[plugin {id}] {message}"),
                _ => tracing::info!("[plugin {id}] {message}"),
            }
        }
        "invoke" => {
            let (action, arg) = (s("action").unwrap_or_default(), s("arg").unwrap_or_default());
            // Plugins may not drive other plugins or scripts through the shell.
            if action.starts_with("plugin:") || action.starts_with("script:") {
                tracing::warn!("plugin {id} tried to invoke {action}; ignored");
                return;
            }
            let _ = slint::invoke_from_event_loop(move || crate::actions::invoke(&action, &arg));
        }
        other => tracing::debug!("plugin {id}: unknown notification {other}"),
    }
}

fn crashed(p: &mut Plugin, why: &str) {
    let id = p.spec.id.clone();
    if let Some(mut r) = p.running.take() {
        let _ = r.child.kill();
        let _ = r.child.wait();
    }
    tracing::warn!("plugin {id}: {why}");
    p.last_error = Some(why.to_owned());
    let now = Instant::now();
    p.crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
    p.crashes.push_back(now);
    p.restarts += 1;
    if p.crashes.len() >= MAX_CRASHES {
        tracing::error!("plugin {id} disabled after {MAX_CRASHES} failures; fix it and reload (fsh-ctl shell reload)");
        p.disabled = true;
        return;
    }
    let delay = Duration::from_secs(1 << (p.crashes.len() - 1).min(6));
    p.next_start = Some(now + delay);
}

fn stop(p: &mut Plugin) {
    let Some(mut r) = p.running.take() else { return };
    let _ = r.writer.send(Message::from(Notification::new("shutdown", Value::Null)).to_line());
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = r.child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = r.child.kill();
    let _ = r.child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, exec: &str, args: &[&str]) -> PluginSpec {
        PluginSpec {
            id: id.into(),
            dir: std::env::temp_dir(),
            exec: exec.into(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            max_memory_mb: 64,
            instances: vec![("panel-0/3".into(), toml::Table::new())],
        }
    }

    fn wait_for(host: &PluginHost, id: &str, pred: impl Fn(&PluginStatus) -> bool, secs: u64) -> PluginStatus {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(s) = host.status().into_iter().find(|s| s.id == id)
                && pred(&s)
            {
                return s;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {id}: {:?}", host.status());
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn real_plugin_runs_and_is_restarted_after_being_killed() {
        let exe = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("fsh-sysmon.exe");
        if !exe.exists() {
            eprintln!("skipping: build fsh-sysmon first ({})", exe.display());
            return;
        }
        let host = PluginHost::spawn().unwrap();
        host.set(vec![spec("sysmon", &exe.to_string_lossy(), &[])]);
        let first = wait_for(&host, "sysmon", |s| s.pid.is_some(), 10).pid.unwrap();

        std::process::Command::new("taskkill").args(["/F", "/PID", &first.to_string()]).output().unwrap();
        let second = wait_for(&host, "sysmon", |s| s.pid.is_some_and(|p| p != first), 15);
        assert_eq!(second.restarts, 1);

        host.set(vec![]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !host.status().is_empty() {
            assert!(Instant::now() < deadline, "plugin not stopped");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn crashing_plugin_backs_off_and_is_disabled() {
        let host = PluginHost::spawn().unwrap();
        host.set(vec![spec("crasher", "cmd.exe", &["/D", "/C", "exit 3"])]);
        // Backoff is 1+2+4+8 s before the fifth failure.
        let s = wait_for(&host, "crasher", |s| s.state == "disabled", 40);
        assert_eq!(s.restarts as usize, MAX_CRASHES);
        assert!(s.last_error.is_some());
    }
}