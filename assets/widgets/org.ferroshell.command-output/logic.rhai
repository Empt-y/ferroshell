// Runs `command` every `interval` seconds and publishes the first line as "text".

fn init() {
    let every = setting("interval");
    if every != () {
        set_interval(every);
    }
}

fn tick() {
    let out = shell(setting("command"));
    let first = out.split("\n")[0];
    first.trim();
    let prefix = setting("prefix");
    publish("text", if prefix == () { first } else { prefix + first });
}

// Clicking the widget refreshes it immediately.
fn on_click(arg) {
    tick();
}
