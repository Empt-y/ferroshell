# Ferroshell

A KDE Plasma-style desktop shell for Windows, written in Rust. Panels, a task manager
and widgets are built from plain files you can edit, and it's designed to fail safely: a
crash or hang never leaves you without a taskbar.

Beyond the taskbar, it replaces what Windows' `ShellExperienceHost` provides, as
Plasma-style applets you can add, remove and restyle one by one:
- **Volume:** devices, per-app mixer and now playing.
- **Network:** Wi-Fi, Ethernet, VPN and airplane mode.
- **Bluetooth.**
- **Battery:** power mode and brightness.
- **Notifications:** with banners.
- **Calendar:** on the clock.

There's also a Kickoff-style launcher.

It currently runs **alongside Explorer**, hiding Explorer's taskbar while it runs.
Replacing Explorer as the login shell comes later (see [Roadmap](#roadmap)).

## Running it

```bash
cargo build --release
target\release\fsh-session.exe
```

`fsh-session` is the supervisor. It starts the shell, restarts it if it crashes or
hangs, and puts Explorer's taskbar back when it stops. To try it without hiding
Explorer's taskbar, add `--keep-explorer-taskbar`.

**If anything goes wrong:** press **Ctrl+Alt+Shift+E** (if another program already uses that, Ferroshell falls back to Ctrl+Alt+Shift+F12, then Ctrl+Alt+Shift+Q; `fsh-ctl status` shows which). That stops Ferroshell and restores
Explorer's taskbar; press it again to start Ferroshell back up. From a terminal,
`fsh-ctl restore-explorer` does the same even if nothing is running.

`fsh-settings.exe` is a graphical editor for the configuration. `fsh-ctl` controls a
running shell:

```bash
fsh-ctl status
```

Other `fsh-ctl` commands: `reload`, `restart`, `safe-mode on|off`, `stop`, `start`,
`quit`, `shell dump_state` (everything the shell knows, as JSON), and `debug crash` /
`debug hang` to exercise recovery.

## Customising

Everything lives in `%APPDATA%\ferroshell` and is reloaded automatically when saved:

| What | Where | Docs |
|---|---|---|
| Panels, widgets, pinned apps | `config.toml` | [config reference](docs/config-reference.md) |
| Themes (colours, sizes, backdrop) | `themes\<name>\theme.toml` | [theming](docs/theming.md) |
| Your own widgets, or overrides of built-in ones | `widgets\<id>\` | [widget API](docs/widget-api.md) |
| Widget behaviour in any language | a plugin executable | [plugin protocol](docs/plugin-protocol.md) |
| Applet popups, system services (audio, network, power…) | `widgets\<id>\popup.slint` | [widget API](docs/widget-api.md#popups) |
| Launcher, OSD, banners, previews | `library\*.slint` | [theming](docs/theming.md#restyling-the-launcher-osd-banners-and-previews) |
| Notifications (one-time setup) | package identity | [packaging/identity](packaging/identity/README.md) |

Built-in widgets and themes are extracted to `%LOCALAPPDATA%\ferroshell\builtin`. Copy
one into your own folder to change it: a widget in `%APPDATA%\ferroshell\widgets` with
the same id replaces the built-in one.

A broken config falls back to the last one that worked, and a broken widget becomes a
red `!` placeholder while the rest of the panel keeps working. Hover it to see why.

## How it's built

```
fsh-session   supervisor: restarts, crash-loop → safe mode → Explorer, hang watchdog,
 │            emergency hotkey, restores Explorer's taskbar however things end
 └─ fsh-shell  panels (Slint), popups, window tracker thread, script thread, plugin host,
     │         a worker thread per system service (audio, media, network, Bluetooth,
     │         power, notifications), started only when a widget uses it
     └─ plugins  one process per plugin widget, in a Job Object (memory cap, dies with shell)
fsh-settings  settings GUI (edits config.toml, keeping comments)
fsh-ctl       command-line control over \\.\pipe\ferroshell and \\.\pipe\ferroshell-session
```

| Crate | Purpose |
|---|---|
| `fsh-win` | the only crate with `unsafe`: safe wrappers over Win32 and WinRT (Core Audio, WLAN, Bluetooth, power, notifications…) |
| `fsh-core` | pure, unit-tested shell logic: window classification, task grouping, panel geometry, launcher search, calendar, applet helpers |
| `fsh-config` | config/theme schemas, validation, migrations, comment-preserving edits |
| `fsh-widgets` | widget packages, the panel composer (slint-interpreter), built-in assets |
| `fsh-ipc` | JSON-RPC 2.0 over named pipes and stdio |
| `fsh-plugin-sdk` | write plugins in Rust (`fsh-sysmon` is the reference example) |

Stability rules the code follows:
- Nothing on the UI thread waits on another application. Window queries run on the
  tracker thread, and anything that messages another app's window first checks the app
  is responsive.
- Scripts and plugins can't block the UI. Scripts run on their own thread with
  operation and time limits; plugins are separate processes.
- Every failure has a fallback: last-good config, default theme, error placeholders,
  safe mode (default config and software rendering), then Explorer.

## Development

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets
```

`scripts\test-recovery.ps1` runs the supervisor through crash → safe mode → give up →
recover, and `scripts\screenshot.ps1` captures the panel for visual checks.

## Roadmap

- [x] Supervisor, safe mode, hang watchdog, emergency hotkey
- [x] Panels as app bars, multi-monitor, per-monitor DPI, themes with acrylic/mica
- [x] Task manager: grouping, pinning, live previews, attention, context menus
- [x] Hot reload, error isolation, Rhai scripts, out-of-process plugins
- [x] Settings GUI
- [x] System tray (active in replacement mode only)
- [x] Kickoff-style application launcher (Windows key, search, calculator, run, Settings pages)
- [x] Applet popups and system services: calendar, volume/media (+ OSD and media keys in
      replacement mode), network, Bluetooth, battery/power mode/brightness
- [x] Notifications applet and banners (needs the one-time package identity setup)
- [ ] Full Explorer replacement at login (startup apps, `SetShellWindow`)
- [ ] Virtual desktop pager

## License

[MIT](LICENSE)
