//! `fsh-ctl`: command-line control for Ferroshell.

use std::process::ExitCode;
use std::time::Duration;

use fsh_common::{paths, pipes};
use serde_json::{Value, json};

const USAGE: &str = "\
fsh-ctl <command>

Session (supervisor):
  status                 show supervisor and shell status
  start | stop           start or stop the shell (stop restores Explorer's taskbar)
  restart                restart the shell
  safe-mode [on|off]     restart in (or out of) safe mode
  quit                   stop everything and exit the supervisor

Shell:
  ping                   check the shell is responsive
  launcher               open or close the application launcher
  shell <method> [json]  send any method to the shell, e.g. `fsh-ctl shell status`
  debug crash            make the shell crash (tests recovery)
  debug hang [secs]      freeze the shell's UI thread (tests the hang watchdog)

Recovery (works even if nothing is running):
  restore-explorer       bring Explorer's taskbar back";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(v) => {
            if !v.is_null() {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> anyhow::Result<Value> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let session = |method: &str, params: Value| call(pipes::SESSION, method, params);
    let shell = |method: &str, params: Value| call(pipes::SHELL, method, params);

    match (arg(0), arg(1)) {
        (Some("status"), _) => {
            let session = session("status", Value::Null)
                .unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
            let shell = shell("status", Value::Null)
                .unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
            Ok(json!({"session": session, "shell": shell}))
        }
        (Some(m @ ("start" | "stop" | "restart" | "quit")), _) => session(m, Value::Null),
        (Some("safe-mode"), on) => session("safe_mode", json!({"on": on != Some("off")})),
        (Some("ping"), _) => shell("ping", Value::Null),
        (Some("launcher"), _) => shell("launcher", Value::Null),
        (Some("debug"), Some("crash")) => shell("debug.crash", Value::Null),
        (Some("debug"), Some("hang")) => {
            let secs: u64 = arg(2).map(str::parse).transpose()?.unwrap_or(60);
            shell("debug.hang", json!({"secs": secs}))
        }
        (Some("shell"), Some(method)) => {
            let params = match arg(2) {
                Some(p) => serde_json::from_str(p)?,
                None => Value::Null,
            };
            shell(method, params)
        }
        (Some("restore-explorer"), _) => {
            fsh_win::taskbar::restore(&paths::taskbar_state_file());
            Ok(json!({"ok": true}))
        }
        (Some("-h" | "--help" | "help") | None, _) => {
            println!("{USAGE}");
            Ok(Value::Null)
        }
        (Some(other), _) => anyhow::bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn call(pipe: &str, method: &str, params: Value) -> anyhow::Result<Value> {
    fsh_ipc::pipe::call(pipe, method, params, Duration::from_secs(10))
}
