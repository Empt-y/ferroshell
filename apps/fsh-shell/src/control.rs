//! The `\\.\pipe\ferroshell` control pipe. Requests arrive on pipe threads and are run on
//! the UI thread; if the UI thread doesn't answer in time the request fails, which is also
//! how the supervisor can tell the shell is hung.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use fsh_common::{exit_codes, pipes};
use fsh_ipc::RpcError;
use serde_json::{Value, json};

use crate::app;

const UI_TIMEOUT: Duration = Duration::from_secs(5);

/// Run `f` on the UI thread and wait for its result.
fn on_ui<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, RpcError> {
    let (tx, rx) = mpsc::channel();
    slint::invoke_from_event_loop(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| RpcError::internal(format!("UI thread unavailable: {e}")))?;
    rx.recv_timeout(UI_TIMEOUT).map_err(|_| RpcError::internal("UI thread did not respond (hung?)"))
}

pub fn serve() -> anyhow::Result<()> {
    fsh_ipc::pipe::serve(pipes::SHELL, Arc::new(handle))?;
    Ok(())
}

fn handle(method: &str, params: Value) -> Result<Value, RpcError> {
    match method {
        // Answered on the UI thread on purpose: a reply proves the event loop is alive.
        "ping" => on_ui(|| json!("pong")),
        "status" => on_ui(|| {
            json!({
                "pid": std::process::id(),
                "safe_mode": app::with(|a| a.dump_state()["safe_mode"].clone()).unwrap_or(Value::Null),
                "identity": fsh_win::identity::package_full_name(),
            })
        }),
        "dump_state" | "state" => on_ui(|| app::with(|a| a.dump_state()).unwrap_or(Value::Null)),
        "reload" => on_ui(|| {
            app::with(|a| a.reload());
            json!({"ok": true})
        }),
        "set_theme" | "preview_theme" => {
            let name = params.get("name").and_then(Value::as_str).map(str::to_owned);
            on_ui(move || {
                app::with(|a| a.set_theme_override(name));
                json!({"ok": true})
            })
        }
        "launcher" | "toggle_launcher" => on_ui(|| {
            app::with(|a| a.toggle_launcher(crate::launcher::Anchor::Cursor));
            json!({"ok": true})
        }),
        "shutdown" | "quit" => exit(exit_codes::QUIT),
        "restart" => exit(exit_codes::RESTART),
        "debug.crash" => {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                tracing::warn!("debug.crash requested; aborting");
                std::process::abort();
            });
            Ok(json!({"ok": true}))
        }
        // Act as if a volume/media key was pressed (they're only hooked in replacement mode):
        // {"key": "volume-up" | "volume-down" | "mute" | "play-pause" | "next" | "previous"}.
        "debug.media_key" => {
            use fsh_win::keyhook::{KeyEvent, MediaKey};
            let key = match params.get("key").and_then(Value::as_str).unwrap_or_default() {
                "volume-up" => MediaKey::VolumeUp,
                "volume-down" => MediaKey::VolumeDown,
                "mute" => MediaKey::Mute,
                "play-pause" => MediaKey::PlayPause,
                "next" => MediaKey::Next,
                "previous" => MediaKey::Previous,
                other => return Err(RpcError::invalid_params(format!("unknown key `{other}`"))),
            };
            on_ui(move || {
                app::with(|a| a.on_key_event(KeyEvent::Media(key)));
                json!({"ok": true})
            })
        }
        // Show the brightness OSD as if the screen brightness changed: {"level": 0-100}.
        "debug.brightness_osd" => {
            let level = params.get("level").and_then(Value::as_u64).unwrap_or(50).min(100) as f64;
            on_ui(move || {
                app::with(|a| a.show_osd_level(crate::osd::OsdKind::Brightness, level, false));
                json!({"ok": true})
            })
        }
        "debug.hang" => {
            // Blocks the UI thread to test hang detection.
            let secs = params.get("secs").and_then(Value::as_u64).unwrap_or(30);
            let _ = slint::invoke_from_event_loop(move || std::thread::sleep(Duration::from_secs(secs)));
            Ok(json!({"ok": true}))
        }
        other => Err(RpcError::method_not_found(other)),
    }
}

fn exit(code: i32) -> Result<Value, RpcError> {
    slint::invoke_from_event_loop(move || app::request_exit(code))
        .map_err(|e| RpcError::internal(e.to_string()))?;
    Ok(json!({"ok": true}))
}
