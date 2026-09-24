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
- `"script:<instance-id>/<function>"`: calls `function(arg)` in the widget's script.
- `"plugin:<instance-id>/<action>"`: sends an action to the widget's plugin.

`Theme` has every theme token ([theming.md](theming.md)): `Theme.foreground`,
`Theme.accent`, `Theme.radius`, `Theme.font-size`, and so on.

### `@ferroshell/controls.slint`

- `PanelButton`: a themed, flat button. It has hover and pressed states, plus
  `active`, `attention` and `indicator` properties, and `clicked`, `middle-clicked` and
  `right-clicked(x, y)` callbacks. Children are centred.
- `Label`, `DimLabel`: themed text.
- `ErrorWidget`: the error placeholder.

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
