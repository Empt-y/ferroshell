# config.toml reference

`%APPDATA%\ferroshell\config.toml`. It's created with defaults on first run, and reloaded
automatically whenever it's saved. If it has an error, the shell keeps the last
configuration that loaded and shows a red `!` on the panel. `fsh-ctl shell dump_state`
shows the error text.

```toml
version = 1                # schema version; older files are migrated automatically
theme = "breeze-dark"      # a theme folder name, or "auto" (follows Windows light/dark)

[[panel]]                  # repeat for more panels
monitor = "all"            # "all", "primary", or a monitor number (0 = leftmost)
edge = "bottom"            # "bottom", "top", "left", "right"
thickness = 44             # logical pixels, 20–200 (scaled for the monitor's DPI)
floating = false           # detached from the edge with rounded corners
widgets = [
    { id = "org.ferroshell.launcher-button" },
    { id = "org.ferroshell.taskmanager", pinned = ['C:\Windows\explorer.exe'] },
    { id = "org.ferroshell.clock", format = "%H:%M" },
]
```

Unknown top-level or panel keys are errors, so typos get caught. Unknown widget settings
are warnings: the widget still loads and the setting is ignored.

## Built-in widgets

| id | Settings |
|---|---|
| `org.ferroshell.launcher-button` | `action`: `"launcher"` (default) or `"start-menu"` (Windows Start) |
| `org.ferroshell.taskmanager` | `show-titles`, `max-item-width`, `group`, `only-this-monitor`, `pinned` |
| `org.ferroshell.systemtray` | `icon-scale`, `hidden` (patterns matched against tooltip or program name). Only works when Ferroshell replaces Explorer as the shell; alongside Explorer it stays empty |
| `org.ferroshell.volume` | `scroll-step`, `show-percentage`; popup: `show-apps`, `show-media`, `show-input`. Click for the mixer (devices, per-app volume, now playing), scroll to change the volume, middle-click to mute |
| `org.ferroshell.network` | `show-label`. Click for Wi-Fi/airplane-mode toggles, the network list (click a saved or open network to connect, an unsaved secured one to expand a password field), active VPNs, and a link to Windows' network settings |
| `org.ferroshell.bluetooth` | `show-when-off`. Hidden on machines without Bluetooth. Click for the on/off switch and paired devices with battery levels; audio devices get Connect/Disconnect (others reconnect by themselves when used). Pairing new devices opens Windows' Bluetooth settings |
| `org.ferroshell.clock` | `format`, `date-format` ([strftime](https://docs.rs/chrono/latest/chrono/format/strftime/)), `show-date`; popup (calendar): `first-day` (`"monday"`/`"sunday"`), `week-numbers` |
| `org.ferroshell.show-desktop` | — |
| `org.ferroshell.spacer` | `size` (0 = expand) |
| `org.ferroshell.separator` | — |
| `org.ferroshell.command-output` | `command`, `interval`, `prefix`, `max-text-width` (Rhai script) |
| `org.ferroshell.system-monitor` | `show-text`, `warn-at` (plugin) |

`fsh-settings` shows every widget's settings with descriptions, generated from the
widget's `widget.toml`.

### Pinned apps

`pinned` entries can be:
- an app id (AUMID) such as `"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"`, for
  Store apps;
- a `.lnk` shortcut;
- an `.exe` path. `%VARIABLES%` are expanded.

Right-clicking a task and choosing **Pin to panel** writes the right form for you.

## Launcher

```toml
[launcher]
windows-key = true      # a lone Windows-key press opens Ferroshell's launcher
width = 640             # logical pixels
height = 620
show-recent = true
power-actions = ["lock", "sleep", "restart", "shutdown", "logout"]
favourites = []         # app ids; right-click an app → Add to favourites
web-search = ""         # e.g. "https://duckduckgo.com/?q={}" adds a web result
```

Search finds apps (by name, initials such as "vsc", or program name), Windows
Settings pages ("bluetooth", "wallpaper"), and calculations ("12*(3+4)", `sqrt(2)`;
Enter copies the result). `>command` runs in a command prompt. Paths, `\\server`
shares, URLs and `shell:` locations open directly. Keys: arrows and Page Up/Down move,
Enter opens, Ctrl+Enter runs as administrator, Tab/Shift+Tab switch category, Escape
clears or closes. Changing the launcher section never rebuilds panels.

## Safe mode

After three crashes within a minute, the supervisor restarts the shell in safe mode:
default config, built-in widgets and theme only, no plugins, software rendering. If
that crashes three times too, it gives up and restores Explorer's taskbar.
`fsh-ctl safe-mode off` (or `start`) tries normal mode again.
