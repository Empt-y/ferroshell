//! Generic actions widgets trigger with `Shell.invoke(action, arg)`, and app launching.

use crate::app;

pub fn invoke(action: &str, arg: &str) {
    tracing::debug!("invoke {action} {arg:?}");
    if let Some(target) = action.strip_prefix("plugin:") {
        // plugin:<instance>/<action>
        match target.rsplit_once('/') {
            Some((instance, act)) => {
                app::with(|a| a.plugins.invoke(instance, act, arg));
            }
            None => tracing::warn!("bad plugin action `{action}`; use plugin:<instance>/<action>"),
        }
        return;
    }
    if let Some(target) = action.strip_prefix("script:") {
        // script:<instance>/<function>
        match target.rsplit_once('/') {
            Some((instance, func)) => {
                app::with(|a| a.scripts.call(instance, func, arg));
            }
            None => tracing::warn!("bad script action `{action}`; use script:<instance>/<function>"),
        }
        return;
    }
    match action {
        "start-menu" => fsh_win::system::tap_windows_key(),
        "show-desktop" => {
            app::with(|a| a.toggle_desktop());
        }
        "launch" if !arg.is_empty() => launch(arg.to_owned()),
        "settings" => spawn_sibling("fsh-settings.exe", &[]),
        "calendar" => {}
        other => tracing::warn!("unknown action `{other}`"),
    }
}

/// Open an app, file or `shell:` target on a worker thread (the shell can block while it
/// talks to the target app).
pub fn launch(target: String) {
    let spawned = std::thread::Builder::new().name("launch".into()).spawn(move || {
        let _com = fsh_win::com::ComGuard::new();
        match fsh_win::winops::launch(&target, None) {
            Ok(()) => tracing::info!("launched {target}"),
            Err(e) => tracing::warn!("{e:#}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("could not start launch thread: {e}");
    }
}

/// Start another Ferroshell executable from our own folder.
fn spawn_sibling(exe: &str, args: &[&str]) {
    let path = fsh_common::paths::exe_dir().join(exe);
    if let Err(e) = std::process::Command::new(&path).args(args).spawn() {
        tracing::warn!("could not start {}: {e}", path.display());
    }
}
