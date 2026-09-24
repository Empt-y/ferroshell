//! Write Ferroshell plugins in Rust.
//!
//! A plugin is a separate process that Ferroshell starts for a widget package. It talks
//! newline-delimited JSON-RPC 2.0 over stdin/stdout (see `docs/plugin-protocol.md`); this
//! crate handles the protocol so a plugin is just a [`Plugin`] implementation:
//!
//! ```no_run
//! use fsh_plugin_sdk::{Context, Instance, Plugin};
//!
//! struct Hello;
//! impl Plugin for Hello {
//!     fn initialize(&mut self, ctx: &Context, _instances: Vec<Instance>) {
//!         ctx.publish("greeting", "Hello from a plugin");
//!     }
//! }
//!
//! fn main() -> std::io::Result<()> {
//!     fsh_plugin_sdk::run(Hello)
//! }
//! ```
//!
//! Anything written to stdout that isn't protocol breaks the connection — log with
//! [`Context::log`] (or to stderr) instead of `println!`.

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use fsh_ipc::jsonrpc::{Message, Notification, Response, RpcError, read_message, write_message};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Protocol version this SDK speaks.
pub const API_VERSION: u32 = 1;

/// One widget instance using this plugin, with its settings from `config.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Instance {
    pub instance: String,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
}

impl Instance {
    pub fn setting_str(&self, key: &str) -> Option<&str> {
        self.settings.get(key).and_then(Value::as_str)
    }
    pub fn setting_f64(&self, key: &str) -> Option<f64> {
        self.settings.get(key).and_then(Value::as_f64)
    }
    pub fn setting_bool(&self, key: &str) -> Option<bool> {
        self.settings.get(key).and_then(Value::as_bool)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

/// Talks back to the shell. Cheap to clone and usable from any thread, so background
/// threads (timers, watchers) can publish too.
#[derive(Clone)]
pub struct Context {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Context {
    fn send(&self, msg: Message) {
        if let Ok(mut out) = self.out.lock() {
            let _ = write_message(&mut *out, &msg);
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(Notification::new(method, params).into());
    }

    /// Publish a value every instance's widget can read as `Shell.source("<widget-id>/<name>", ...)`.
    pub fn publish(&self, name: &str, value: impl Serialize) {
        self.notify("publish", json!({ "name": name, "value": value }));
    }

    /// Publish a value for one instance; its widget reads it as
    /// `Shell.source(root.instance-id + "/<name>", ...)`.
    pub fn publish_for(&self, instance: &str, name: &str, value: impl Serialize) {
        self.notify("publish", json!({ "instance": instance, "name": name, "value": value }));
    }

    pub fn log(&self, level: Level, message: impl Into<String>) {
        let level = match level {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };
        self.notify("log", json!({ "level": level, "message": message.into() }));
    }

    /// Ask the shell to run an action, e.g. `invoke("launch", "notepad.exe")`.
    pub fn invoke(&self, action: &str, arg: &str) {
        self.notify("invoke", json!({ "action": action, "arg": arg }));
    }
}

/// Implement the methods you need; all have empty defaults.
pub trait Plugin {
    /// Called once at startup with the widget instances using this plugin.
    fn initialize(&mut self, _ctx: &Context, _instances: Vec<Instance>) {}
    /// Widgets were added/removed or their settings changed.
    fn instances_changed(&mut self, _ctx: &Context, _instances: Vec<Instance>) {}
    /// A widget called `Shell.invoke("plugin:" + root.instance-id + "/<action>", arg)`.
    fn invoke(&mut self, _ctx: &Context, _instance: &str, _action: &str, _arg: &str) {}
    /// The shell is stopping the plugin; the process should exit soon after.
    fn shutdown(&mut self, _ctx: &Context) {}
}

fn instances(params: &Value) -> Vec<Instance> {
    params.get("instances").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Run the plugin on stdin/stdout until the shell disconnects or asks it to shut down.
pub fn run(plugin: impl Plugin) -> io::Result<()> {
    let stdin = io::stdin();
    run_with(plugin, &mut stdin.lock(), Box::new(io::stdout()))
}

/// [`run`] with explicit streams (used by tests).
pub fn run_with(mut plugin: impl Plugin, input: &mut impl BufRead, output: Box<dyn Write + Send>) -> io::Result<()> {
    let ctx = Context { out: Arc::new(Mutex::new(output)) };
    loop {
        let msg = match read_message(input)? {
            None => return Ok(()),
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                ctx.log(Level::Warn, format!("bad message from shell: {e}"));
                continue;
            }
        };
        match msg {
            Message::Request(req) => {
                let result = match req.method.as_str() {
                    "ping" => Ok(json!("pong")),
                    "initialize" => {
                        plugin.initialize(&ctx, instances(&req.params));
                        Ok(json!({ "api_version": API_VERSION }))
                    }
                    other => Err(RpcError::method_not_found(other)),
                };
                let resp = match result {
                    Ok(v) => Response::ok(req.id, v),
                    Err(e) => Response::err(req.id, e),
                };
                ctx.send(resp.into());
            }
            Message::Notification(n) => match n.method.as_str() {
                "instances" => plugin.instances_changed(&ctx, instances(&n.params)),
                "invoke" => {
                    let s = |k: &str| n.params.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                    plugin.invoke(&ctx, &s("instance"), &s("action"), &s("arg"));
                }
                "shutdown" => {
                    plugin.shutdown(&ctx);
                    return Ok(());
                }
                _ => {}
            },
            Message::Response(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        seen: Vec<String>,
    }

    impl Plugin for Recorder {
        fn initialize(&mut self, ctx: &Context, instances: Vec<Instance>) {
            self.seen.push(format!("init {}", instances.len()));
            ctx.publish("hello", 42);
            ctx.publish_for(&instances[0].instance, "x", "y");
        }
        fn invoke(&mut self, _ctx: &Context, instance: &str, action: &str, arg: &str) {
            self.seen.push(format!("{instance} {action} {arg}"));
        }
    }

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn speaks_the_protocol() {
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"api_version":1,"instances":[{"instance":"p/0","settings":{"a":1}}]}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","method":"invoke","params":{"instance":"p/0","action":"refresh","arg":"now"}}"#,
            r#"{"jsonrpc":"2.0","method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
        ]
        .join("\n");
        let out = Shared::default();
        let mut recorder = Recorder::default();
        run_with(&mut recorder, &mut io::Cursor::new(input), Box::new(out.clone())).unwrap();
        assert_eq!(recorder.seen, ["init 1", "p/0 refresh now"]);

        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["params"], json!({"name": "hello", "value": 42}));
        assert_eq!(lines[1]["params"], json!({"instance": "p/0", "name": "x", "value": "y"}));
        assert_eq!(lines[2]["result"]["api_version"], 1);
        assert_eq!(lines[3], json!({"jsonrpc": "2.0", "id": 2, "result": "pong"}));
        assert_eq!(lines.len(), 4, "nothing is answered after shutdown");
    }

    impl Plugin for &mut Recorder {
        fn initialize(&mut self, ctx: &Context, i: Vec<Instance>) {
            (**self).initialize(ctx, i)
        }
        fn invoke(&mut self, ctx: &Context, a: &str, b: &str, c: &str) {
            (**self).invoke(ctx, a, b, c)
        }
    }
}
