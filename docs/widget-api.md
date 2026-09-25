# Writing widgets

A widget is a folder in `%APPDATA%\ferroshell\widgets\` named after its id:

```
com.example.hello\
  widget.toml     identity and settings schema
  ui.slint        the UI (Slint language)
  logic.rhai      optional script
```

Add it to a panel in `config.toml` (`{ id = "com.example.hello" }`) or with
`fsh-settings`. Saving any of these files reloads the panel. If the widget fails to
compile, it shows as a red `!` placeholder, and hovering it shows the first error. The
full diagnostics are in `%LOCALAPPDATA%\ferroshell\logs\shell.*.log`.

The built-in widgets in `%LOCALAPPDATA%\ferroshell\builtin\widgets` are complete
examples. Copy one into your widgets folder under the same id to override it.

## widget.toml

```toml
id = "com.example.hello"       # letters, digits, '.', '-', '_'; should match the folder
name = "Hello"
description = "Says hello."
version = "1.0.0"
api = 1                        # widget API version

[config.greeting]              # each setting becomes an `in property` of the widget
type = "string"                # bool, int, float, string, enum, color, string-list
default = "Hello"
label = "Greeting"             # shown in fsh-settings
description = "What to say."

[config.size]
type = "int"
default = 3
min = 1                        # int/float: optional min/max (values are clamped)
max = 10

[config.mode]
type = "enum"
default = "short"
options = ["short", "long"]

[config.secret]
type = "string"
default = ""
ui = false                     # not passed to the UI (only scripts/plugins read it);
                               # changing it doesn't rebuild the panel
```

Setting names must be valid Slint property names and must not clash with built-in ones
(`width`, `height`, `x`, `y`, `visible`, …) or `instance-id`.

Applets (widgets with a popup, like the volume or calendar) use three more keys:

```toml
services = ["audio", "media"]   # system services the widget reads (see "System services")

[popup]                         # a popup opened from the widget (see "Popups")
file = "popup.slint"            # the default; it must export a component named Popup
width = 360                     # logical pixels
height = 440

[config.week-numbers]
type = "bool"
default = true
scope = "popup"                 # which component gets this setting as an `in property`:
                                # "widget" (the default), "popup" or "both"
```

## ui.slint

The file must export a component named `Widget`. It gets one `in property` per `ui`
setting, set from the user's config (or the default):

```slint
import { Shell, Theme } from "@ferroshell/api.slint";
import { PanelButton, Label } from "@ferroshell/controls.slint";

export component Widget inherits PanelButton {
    in property <string> greeting: "Hello";
    in property <int> size: 3;
    in property <string> mode: "short";

    horizontal-stretch: 0;        // 1 = take up free space (like the task manager)
    clicked => { Shell.invoke("launch", "notepad.exe"); }

    Label { text: root.greeting + ", it's " + Shell.format-time(Shell.now, "%H:%M"); }
}
```

The panel lays widgets out in a row (or a column for left/right panels) with
`Theme.spacing` between them. A widget is sized by its preferred size unless it sets a
stretch.

### `@ferroshell/api.slint`

`Shell` holds data and actions for the panel the widget is on.

| Member | |
|---|---|
| `vertical: bool`, `edge: string`, `thickness: length` | the panel's orientation and size |
| `tasks: [Task]` | the task list (see `Task` in api.slint) |
| `now: int` | Unix time in seconds, updated every second |
| `format-time(now, fmt) -> string` | strftime formatting in local time |
| `calendar(year, month, monday-first, now) -> [CalendarDay]` | a 6×7 month grid (`day`, `in-month`, `today`, ISO `week`); pass `Shell.now` so "today" moves at midnight |
| `month-title(year, month) -> string` | e.g. "September 2026" |
| `source(name, Shell.sources-revision) -> string` | data published by scripts and plugins |
| `safe-mode: bool`, `config-error: string` | shell state |
| `activate-task(id)`, `task-action(id, action)`, `task-menu(id, x, y)`, `task-hover(...)` | task manager actions |
| `invoke(action, arg)` | shell actions (below) |

`Shell.invoke` actions:
- `"launcher"`: opens Ferroshell's launcher (from a panel widget, pass the widget's
  `"x,y,w,h"` as the argument to open it next to the widget).
- `"start-menu"`: the Windows Start menu.
- `"show-desktop"`
- `"settings"`
- `"launch"`: the argument is a program, file, URL or `shell:` path.
- `"popup"`: opens (or closes) the widget's popup next to it. The argument is
  `root.instance-id + "|x,y,w,h"`, with the widget's rectangle in the panel.
- `"close-popup"`: closes the open popup.
- `"script:<instance-id>/<function>"`: calls `function(arg)` in the widget's script.
- `"plugin:<instance-id>/<action>"`: sends an action to the widget's plugin.

`Theme` has every theme token ([theming.md](theming.md)): `Theme.foreground`,
`Theme.accent`, `Theme.radius`, `Theme.font-size`, and so on.

### `@ferroshell/controls.slint`

- `PanelButton`: a themed, flat button. It has hover and pressed states, plus
  `active`, `attention` and `indicator` properties, and `clicked`, `middle-clicked` and
  `right-clicked(x, y)` callbacks. Children are centred. Set `wheel: true` to get
  `scrolled(notches)` (+1 up, −1 down) for the mouse wheel.
- `Label`, `DimLabel`, `Heading`: themed text (body, secondary, section heading).
- `ErrorWidget`: the error placeholder.

For popups:
- `Slider`: 0 to `maximum` (default 100), with `step` for the wheel and arrow keys. It
  reports changes through `changed(value)` and never writes `value` itself. So `value`
  can stay bound to live data (e.g. `Audio.volume`) without the two fighting while it's
  dragged.
- `Toggle`: an on/off switch, with `checked`, `enabled` and `toggled(on)`.
- `IconButton`: a small round button for an icon, with `clicked`, `checked` and
  `tooltip` (read by screen readers).
- `ListRow`: a clickable row for lists (devices, networks…), with `selected`.

Vector icons that follow the theme's colours (set `tint` to recolour them):
- `SpeakerIcon` (`level`, `muted`)
- `MicIcon` (`muted`)
- `MediaIcon` (`kind`: play/pause/next/previous)
- `WifiIcon` (`bars` 0–4, `off`)
- `EthernetIcon`
- `AirplaneIcon`
- `BluetoothIcon` (`off`)
- `BatteryIcon` (`percent`, `charging`)
- `SunIcon`
- `BellIcon` (`muted`)
- `GlyphIcon` (`kind`: close/refresh/arrow)

Prefer these to symbol characters like ✕ or ⟳: the UI font doesn't have them, so they
show as boxes.

## Popups

A widget with a `[popup]` section can open a window next to itself: the calendar on the
clock, the mixer on the volume applet. `popup.slint` exports a component named `Popup`:

```slint
import { Shell, Theme } from "@ferroshell/api.slint";
import { Label } from "@ferroshell/controls.slint";

export component Popup inherits Rectangle {
    in property <string> instance-id;
    in property <bool> week-numbers: true;   // a setting with scope = "popup" or "both"

    Label { text: "Hello from the popup"; }
}
```

Open it from the widget (which must declare `in property <string> instance-id;`):

```slint
clicked => {
    Shell.invoke("popup", root.instance-id + "|" + (self.absolute-position.x / 1px) + ","
        + (self.absolute-position.y / 1px) + "," + (self.width / 1px) + "," + (self.height / 1px));
}
```

The shell handles the window:
- It themes it, rounds its corners and places it against the panel's edge, lined up
  with the widget and kept on screen.
- Only one popup is open at a time, and clicking the widget again toggles it.
- It closes on Escape or when you click elsewhere. Call `Shell.invoke("close-popup", "")`
  to close it yourself.

The popup gets the same `Shell` and `Theme` globals and the same services as the panel.
Overriding the widget (a copy in your widgets folder or a theme) overrides its popup too.

## System services

`@ferroshell/services.slint` gives widgets live system state and controls:

```slint
import { Audio, Media } from "@ferroshell/services.slint";
```

A service only runs while some widget on a panel lists it in `services = [...]`. Until
then, or if it fails, its `available` is false and everything else keeps its default:
design for that. Services run on their own threads, so they never slow down the panel.
Lists (devices, networks…) update in place, so a row being dragged isn't recreated
under the pointer.

### `audio`: `Audio`

| Member | |
|---|---|
| `volume: float`, `muted: bool`, `output-name` | the default output, 0–100 |
| `outputs: [AudioDevice]`, `inputs: [AudioDevice]` | `id`, `name`, `default` |
| `has-input`, `input-volume`, `input-muted`, `input-name` | the default microphone |
| `apps: [AudioApp]` | per-app sessions: `pid` (0 = system sounds), `name`, `icon`, `volume`, `muted`, `active` |
| `set-volume(v)`, `set-muted(b)`, `set-input-volume(v)`, `set-input-muted(b)` | |
| `set-default-device(id)` | make an output or input the Windows default |
| `set-app-volume(pid, v)`, `set-app-muted(pid, b)` | |

### `media`: `Media`

The current media session (Spotify, a browser tab…).

| Member | |
|---|---|
| `has-session`, `playing` | |
| `title`, `artist`, `album`, `app-name`, `app-icon: image` | |
| `art: image`, `has-art` | cover art |
| `can-play-pause`, `can-next`, `can-previous` | what the app allows |
| `play-pause()`, `next()`, `previous()` | |

### `network`: `Network`

| Member | |
|---|---|
| `kind: string` | `"wifi"`, `"ethernet"` or `"none"`: the primary connection |
| `connected`, `limited` | any connectivity / without full internet |
| `ssid`, `bars: int` (0–4), `ethernet-name` | |
| `wifi-present`, `wifi-enabled`, `airplane-mode` | |
| `networks: [WifiNetwork]` | scan results: `ssid`, `security`, `needs-password`, `bars`, `connected`, `saved` |
| `vpns: [VpnConnection]` | active VPN connections (`name`) |
| `error: string` | the last failed action, or `""` |
| `scan()`, `connect(ssid)`, `connect-new(ssid, password)`, `disconnect()`, `forget(ssid)` | `connect` is for saved or open networks, `connect-new` for secured ones without a saved profile |
| `set-wifi-enabled(b)`, `set-airplane-mode(b)`, `vpn-disconnect(name)` | |

### `bluetooth`: `Bluetooth`

| Member | |
|---|---|
| `present`, `enabled`, `connected-count` | `present` is false on PCs without Bluetooth |
| `devices: [BluetoothDevice]` | paired devices: `id`, `name`, `kind` (audio, input, phone, computer, wearable, other), `connected`, `can-connect`, `battery` (−1 = unknown), `status` |
| `error: string` | |
| `set-enabled(b)`, `connect(id)`, `disconnect(id)` | connect/disconnect works for audio devices (`can-connect`); others reconnect by themselves when used |

### `power`: `Power`

| Member | |
|---|---|
| `has-battery`, `percent`, `level` (0–5, for icons), `charging`, `on-ac` | |
| `status: string` | e.g. "2 h 15 min left", "Charging · 45 min until full" |
| `health: int` | battery wear, 0–100 (−1 = unknown) |
| `power-mode: string`, `mode-available` | `"efficiency"`, `"balanced"`, `"performance"` or `"custom"`; only changeable on the Balanced plan |
| `energy-saver: string` | `"on"`, `"off"` or `"unavailable"` (plugged in) |
| `displays: [Display]` | `id`, `name`, `internal`, `brightness` (0–100): laptop panels and DDC/CI monitors |
| `set-power-mode(mode)`, `set-brightness(id, v)` | |

### `notifications`: `Notifications`

Other apps' notifications from Windows' notification centre. This needs Ferroshell's
package identity ([packaging/identity](../packaging/identity/README.md)), and Windows'
notifications switch must be on.

| Member | |
|---|---|
| `access: string` | `"allowed"`, `"unspecified"` (not asked yet), `"denied"` or `"no-identity"` |
| `count`, `unread`, `dnd` | `unread`: not yet seen in the notifications popup |
| `items: [Notification]` | grouped by app (`first-in-group` starts each group), newest first: `id`, `app-id`, `app`, `icon`, `has-icon`, `title`, `body`, `time`, `unread` |
| `request-access()` | shows Windows' consent prompt |
| `open(id)` | starts the sending app and dismisses the notification |
| `dismiss(id)`, `clear-app(app-id)`, `clear-all()`, `set-dnd(b)` | |

## Scripts (Rhai)

Add a `[script]` section to run [Rhai](https://rhai.rs/book/) code for each instance of
the widget. The widget then receives an `instance-id` property, which it must declare as
`in property <string> instance-id;`.

```toml
[script]
file = "logic.rhai"   # default
interval = 5          # call tick() every 5 s; 0 = never
```

```rhai
fn init() { }                          // optional, runs once
fn tick() {                            // runs every `interval` seconds
    let out = shell("ver");            // run a command line (cmd.exe), get its stdout
    publish("text", out);              // → Shell.source(root.instance-id + "/text", ...)
}
fn on_click(arg) { tick(); }           // Shell.invoke("script:" + root.instance-id + "/on_click", "")
```

Functions available to scripts:

| Function | What it does |
|---|---|
| `publish(name, value)` | Publishes a value for this widget instance. |
| `setting(name)` | Returns this instance's setting, including `ui = false` ones. |
| `state_set(key, value)`, `state_get(key)` | Keep state between calls. Rhai functions can't see top-level `let` variables. |
| `set_interval(seconds)` | Changes how often `tick()` runs. |
| `shell(cmdline)` | Runs a command line with `cmd.exe` and returns its stdout. Times out after 5 s. |
| `run(exe, [args])` | Runs a program and returns its stdout. Times out after 5 s. |
| `invoke(action, arg)` | Runs a shell action (see the `Shell.invoke` actions above). |
| `now()` | Unix time in seconds. |
| `format_time(fmt)` | Formats the current local time with strftime codes. |
| `env(name)` | Reads an environment variable. |
| `print(x)` | Writes to the shell log. |

Scripts are sandboxed: there's no file or network access except through the commands
you run.
- **Time limit:** each call may run for at most 2 s and about 5 million operations.
- **Errors:** the latest error is published as `error`. After 5 failures in a row the
  script is disabled until the next reload.
- **Threading:** all scripts share one thread, separate from the UI, so a slow script
  delays other scripts but never the panel.

## Plugins

For anything a script can't do, a widget can ship a plugin: any executable that speaks
JSON-RPC over stdio. See [plugin-protocol.md](plugin-protocol.md).
