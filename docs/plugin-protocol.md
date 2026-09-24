# Plugin protocol (v1)

A widget can declare a plugin: a program Ferroshell runs **once per widget package**,
shared by every instance of that widget. Plugins can be written in any language. Rust
plugins can use the `fsh-plugin-sdk` crate; `apps/fsh-sysmon` is a complete example.

```toml
# widget.toml
[plugin]
exec = "plugin.exe"        # relative to the widget folder, absolute, or a program on PATH;
                           # ${EXE_DIR} = Ferroshell's own folder
args = []
max-memory-mb = 256        # the process is killed if it uses more
```

Declaring a plugin also gives the widget an `instance-id` property.

## How plugins are run

- They are started with the widget folder as the working directory, no console window,
  and `FERROSHELL_PLUGIN=<widget id>` set.
- They run in a Job Object: a plugin can't outlive the shell, even if the shell is
  killed.
- **stdin/stdout** carry the protocol: one JSON-RPC 2.0 message per line, UTF-8. Never
  print anything else to stdout.
- **stderr** is copied into the shell's log.
- **Pings:** the shell sends `ping` every 10 s. A plugin that doesn't answer within 8 s
  is killed.
- **Restarts:** a plugin that exits or is killed restarts after 1, 2, 4, 8… seconds.
  After 5 failures in 5 minutes it's disabled until the next reload. The status page in
  `fsh-settings` and `fsh-ctl shell dump_state` show why.

## Shell → plugin

| Message | Kind | Params | Reply |
|---|---|---|---|
| `initialize` | request | `{ "api_version": 1, "plugin": "<widget id>", "instances": [Instance] }` | any (e.g. `{ "api_version": 1 }`) |
| `ping` | request | none | any (e.g. `"pong"`) |
| `instances` | notification | `{ "instances": [Instance] }`: widgets were added, removed or reconfigured | none |
| `invoke` | notification | `{ "instance", "action", "arg" }`: from `Shell.invoke("plugin:<instance-id>/<action>", arg)` | none |
| `shutdown` | notification | none; exit within 2 s or be killed | none |

`Instance` is `{ "instance": "<instance-id>", "settings": { ...that widget's settings from config.toml } }`.

## Plugin → shell

| Notification | Params | Effect |
|---|---|---|
| `publish` | `{ "name", "value" }` | Readable by every instance as `Shell.source("<widget id>/<name>", Shell.sources-revision)`. |
| `publish` | `{ "instance", "name", "value" }` | Readable by that instance only: `Shell.source(root.instance-id + "/<name>", ...)`. |
| `log` | `{ "level": "debug" \| "info" \| "warn" \| "error", "message" }` | Writes to the shell log. |
| `invoke` | `{ "action", "arg" }` | Runs a shell action (`launch`, `show-desktop`, …). `plugin:` and `script:` actions are refused. |

Values are shown to widgets as text: strings as-is, anything else as JSON. In Slint,
`.to-float()` turns numeric text into a number.

## Minimal plugin (Python)

```python
import json, sys, threading, time

def send(method, params=None, id=None):
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None: msg["params"] = params
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()

def ticker():
    while True:
        send("publish", {"name": "time", "value": time.strftime("%H:%M:%S")})
        time.sleep(1)

for line in sys.stdin:
    msg = json.loads(line)
    if "id" in msg:  # request: answer it
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": "ok"}) + "\n")
        sys.stdout.flush()
        if msg["method"] == "initialize":
            threading.Thread(target=ticker, daemon=True).start()
    elif msg.get("method") == "shutdown":
        break
```

The full version is in `examples/widgets/com.example.hello-python`.
