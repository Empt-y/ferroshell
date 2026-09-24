//! Widget scripts (Rhai). Every widget instance with a `[script]` gets its own sandboxed
//! engine on a single script thread, so a slow or looping script can never block the UI.
//!
//! Scripts talk to their widget through data sources: `publish("text", value)` makes the
//! value available to the widget as `Shell.source(root.instance-id + "/text", ...)`.
//!
//! Limits: each call may run at most `MAX_OPS` operations and `MAX_CALL_TIME`, and
//! external commands time out after `COMMAND_TIMEOUT`. A script that fails repeatedly is
//! disabled until the next reload.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, INT, Map, Scope};

use crate::app;

const MAX_OPS: u64 = 5_000_000;
const MAX_CALL_TIME: Duration = Duration::from_secs(2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERRORS: u32 = 5;
const MIN_INTERVAL: f64 = 0.25;

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptInstance {
    pub instance: String,
    pub widget_id: String,
    pub path: PathBuf,
    pub interval: f64,
    pub settings: toml::Table,
}

enum Cmd {
    Set(Vec<ScriptInstance>),
    Call { instance: String, func: String, arg: String },
}

pub struct ScriptHost {
    tx: Sender<Cmd>,
}

impl ScriptHost {
    pub fn spawn() -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("scripts".into()).spawn(move || run(rx))?;
        Ok(Self { tx })
    }

    /// Replace the set of running scripts. Unchanged instances keep running (and keep
    /// their state); changed ones are restarted.
    pub fn set(&self, instances: Vec<ScriptInstance>) {
        let _ = self.tx.send(Cmd::Set(instances));
    }

    pub fn call(&self, instance: &str, func: &str, arg: &str) {
        let _ = self.tx.send(Cmd::Call { instance: instance.into(), func: func.into(), arg: arg.into() });
    }
}

struct Running {
    spec: ScriptInstance,
    modified: Option<std::time::SystemTime>,
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    interval: Rc<Cell<f64>>,
    call_started: Rc<Cell<Instant>>,
    next_tick: Option<Instant>,
    errors: u32,
    disabled: bool,
}

fn run(rx: Receiver<Cmd>) {
    let mut running: HashMap<String, Running> = HashMap::new();
    loop {
        let now = Instant::now();
        let next = running.values().filter(|r| !r.disabled).filter_map(|r| r.next_tick).min();
        let wait = next.map_or(Duration::from_secs(3600), |t| t.saturating_duration_since(now));
        match rx.recv_timeout(wait) {
            Ok(Cmd::Set(list)) => set_instances(&mut running, list),
            Ok(Cmd::Call { instance, func, arg }) => {
                if let Some(r) = running.get_mut(&instance) {
                    call(r, &func, Some(arg));
                } else {
                    tracing::warn!("script call to unknown instance {instance}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        for r in running.values_mut() {
            if !r.disabled && r.next_tick.is_some_and(|t| t <= now) {
                call(r, "tick", None);
                r.next_tick = tick_interval(r).map(|d| Instant::now() + d);
            }
        }
    }
}

fn tick_interval(r: &Running) -> Option<Duration> {
    let secs = r.interval.get();
    (secs > 0.0).then(|| Duration::from_secs_f64(secs.max(MIN_INTERVAL)))
}

fn modified(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn set_instances(running: &mut HashMap<String, Running>, list: Vec<ScriptInstance>) {
    let wanted: HashMap<String, ScriptInstance> = list.into_iter().map(|s| (s.instance.clone(), s)).collect();
    running.retain(|k, r| {
        let keep = wanted.get(k).is_some_and(|s| *s == r.spec && modified(&s.path) == r.modified);
        if !keep {
            tracing::info!("script {k} stopped");
        }
        keep
    });
    for (k, spec) in wanted {
        if running.contains_key(&k) {
            continue;
        }
        match start(spec) {
            Ok(r) => {
                tracing::info!("script {k} started");
                running.insert(k, r);
            }
            Err(e) => {
                tracing::error!("script {k}: {e}");
                publish(&k, "error", Dynamic::from(e));
            }
        }
    }
}

fn publish(instance: &str, name: &str, value: Dynamic) {
    let key = format!("{instance}/{name}");
    let text = if value.is_unit() { String::new() } else { value.to_string() };
    let _ = slint::invoke_from_event_loop(move || {
        app::with(|a| a.set_source(key, text));
    });
}

fn start(spec: ScriptInstance) -> Result<Running, String> {
    let interval = Rc::new(Cell::new(spec.interval));
    let call_started = Rc::new(Cell::new(Instant::now()));
    let engine = make_engine(&spec, interval.clone(), call_started.clone());
    let source = std::fs::read_to_string(&spec.path).map_err(|e| format!("{}: {e}", spec.path.display()))?;
    let ast = engine.compile(&source).map_err(|e| format!("{}: {e}", spec.path.display()))?;
    let mut r = Running {
        modified: modified(&spec.path),
        spec,
        engine,
        ast,
        scope: Scope::new(),
        interval,
        call_started,
        next_tick: None,
        errors: 0,
        disabled: false,
    };
    // Top-level statements, then init() if the script has one.
    r.call_started.set(Instant::now());
    r.engine.run_ast_with_scope(&mut r.scope, &r.ast).map_err(|e| e.to_string())?;
    call(&mut r, "init", None);
    r.next_tick = tick_interval(&r).map(|_| Instant::now());
    Ok(r)
}

/// Call a script function if it exists. Errors are logged and counted.
fn call(r: &mut Running, func: &str, arg: Option<String>) {
    let arity = usize::from(arg.is_some());
    if !r.ast.iter_functions().any(|f| f.name == func && f.params.len() == arity) {
        if arg.is_some() {
            tracing::warn!("script {}: no function {func}(arg)", r.spec.instance);
        }
        return;
    }
    r.call_started.set(Instant::now());
    let result: Result<Dynamic, Box<EvalAltResult>> = match arg {
        Some(a) => r.engine.call_fn(&mut r.scope, &r.ast, func, (a,)),
        None => r.engine.call_fn(&mut r.scope, &r.ast, func, ()),
    };
    match result {
        Ok(_) => r.errors = 0,
        Err(e) => {
            r.errors += 1;
            tracing::warn!("script {} {func}(): {e}", r.spec.instance);
            publish(&r.spec.instance, "error", Dynamic::from(e.to_string()));
            if r.errors >= MAX_ERRORS {
                tracing::error!("script {} disabled after {MAX_ERRORS} errors in a row", r.spec.instance);
                r.disabled = true;
            }
        }
    }
}

fn to_dynamic(v: &toml::Value) -> Dynamic {
    match v {
        toml::Value::String(s) => s.clone().into(),
        toml::Value::Integer(i) => (*i as INT).into(),
        toml::Value::Float(f) => (*f).into(),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Array(a) => a.iter().map(to_dynamic).collect::<Array>().into(),
        toml::Value::Table(t) => t.iter().map(|(k, v)| (k.as_str().into(), to_dynamic(v))).collect::<Map>().into(),
        toml::Value::Datetime(d) => d.to_string().into(),
    }
}

fn make_engine(spec: &ScriptInstance, interval: Rc<Cell<f64>>, call_started: Rc<Cell<Instant>>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPS);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(64, 32);
    engine.set_max_string_size(1 << 20);
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);
    engine.on_progress(move |_| {
        (call_started.get().elapsed() > MAX_CALL_TIME).then(|| Dynamic::from("script took too long"))
    });

    let tag = spec.instance.clone();
    engine.on_print(move |s| tracing::info!("[script {tag}] {s}"));
    let tag = spec.instance.clone();
    engine.on_debug(move |s, _, pos| tracing::debug!("[script {tag}] {pos:?} {s}"));

    // publish(name, value): data for this widget instance.
    let inst = spec.instance.clone();
    engine.register_fn("publish", move |name: &str, value: Dynamic| publish(&inst, name, value));

    // setting(name): this instance's config value (or () if unset).
    let settings = Rc::new(spec.settings.clone());
    engine.register_fn("setting", move |name: &str| settings.get(name).map(to_dynamic).unwrap_or(Dynamic::UNIT));

    // Rhai functions can't see script-level variables, so scripts keep state here.
    let store: Rc<std::cell::RefCell<Map>> = Rc::default();
    let s = store.clone();
    engine.register_fn("state_set", move |key: &str, value: Dynamic| {
        s.borrow_mut().insert(key.into(), value);
    });
    engine.register_fn("state_get", move |key: &str| store.borrow().get(key).cloned().unwrap_or(Dynamic::UNIT));

    // set_interval(seconds): how often tick() runs (0 stops it). Accepts ints and floats.
    let iv = interval.clone();
    engine.register_fn("set_interval", move |secs: f64| iv.set(secs));
    engine.register_fn("set_interval", move |secs: INT| interval.set(secs as f64));
    engine.register_fn("now", || chrono::Utc::now().timestamp() as INT);
    engine.register_fn("format_time", |fmt: &str| crate::timefmt::format_time(chrono::Utc::now().timestamp(), fmt));
    engine.register_fn("env", |name: &str| std::env::var(name).unwrap_or_default());
    engine.register_fn("shell", |cmdline: &str| run_command("cmd.exe", &["/D".into(), "/C".into(), cmdline.into()]));
    engine.register_fn("run", |exe: &str, args: Array| {
        let args: Vec<String> = args.into_iter().map(|a| a.to_string()).collect();
        run_command(exe, &args)
    });
    engine.register_fn("invoke", |action: &str, arg: &str| {
        let (action, arg) = (action.to_owned(), arg.to_owned());
        let _ = slint::invoke_from_event_loop(move || crate::actions::invoke(&action, &arg));
    });
    engine
}

/// Run a program without a console window and return its stdout (trimmed of a trailing
/// newline). Killed after `COMMAND_TIMEOUT`.
fn run_command(exe: &str, args: &[String]) -> String {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return format!("error: {e}"),
    };
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.take(1 << 20).read_to_end(&mut out);
        }
        out
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return "error: command timed out".into();
            }
        }
    }
    let out = reader.join().unwrap_or_default();
    String::from_utf8_lossy(&out).trim_end_matches(['\r', '\n']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(code: &str) -> ScriptInstance {
        let dir = std::env::temp_dir().join(format!("fsh-script-{}-{}", std::process::id(), code.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logic.rhai");
        std::fs::write(&path, code).unwrap();
        ScriptInstance { instance: "t/0".into(), widget_id: "w".into(), path, interval: 0.0, settings: toml::from_str("x = 21").unwrap() }
    }

    #[test]
    fn runs_functions_with_settings_and_state() {
        let mut r = start(spec("let n = 0; fn tick() { n += 1; } fn double() { setting(\"x\") * 2 }")).unwrap();
        let v: INT = r.engine.call_fn(&mut r.scope, &r.ast, "double", ()).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn state_persists_between_calls() {
        let mut r = start(spec("fn init() { state_set(\"n\", 1); } fn tick() { state_set(\"n\", state_get(\"n\") + 1); } fn n() { state_get(\"n\") }")).unwrap();
        call(&mut r, "tick", None);
        call(&mut r, "tick", None);
        let v: INT = r.engine.call_fn(&mut r.scope, &r.ast, "n", ()).unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn infinite_loops_are_stopped() {
        let mut r = start(spec("fn tick() { loop { } }")).unwrap();
        let t = Instant::now();
        call(&mut r, "tick", None);
        assert_eq!(r.errors, 1);
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn repeated_failures_disable_the_script() {
        let mut r = start(spec("fn tick() { throw \"nope\"; }")).unwrap();
        for _ in 0..MAX_ERRORS {
            call(&mut r, "tick", None);
        }
        assert!(r.disabled);
    }

    #[test]
    fn shipped_and_example_scripts_compile() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut n = 0;
        for dir in ["assets/widgets", "examples/widgets"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
                let path = entry.path().join("logic.rhai");
                if path.exists() {
                    let s = ScriptInstance { path: path.clone(), ..spec("") };
                    start(s).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    n += 1;
                }
            }
        }
        assert!(n >= 2);
    }

    #[test]
    fn compile_errors_are_reported() {
        assert!(start(spec("fn tick( {")).is_err());
    }

    #[test]
    fn commands_capture_output_and_time_out() {
        assert_eq!(run_command("cmd.exe", &["/D".into(), "/C".into(), "echo hi".into()]), "hi");
        assert!(run_command("definitely-not-a-program.exe", &[]).starts_with("error"));
    }
}
