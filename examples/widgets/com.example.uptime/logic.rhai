// Rhai functions can't see top-level variables; keep state with state_set/state_get.

fn init() {
    state_set("started", now());
}

fn tick() {
    let secs = now() - state_get("started");
    let text = if secs < 60 { `${secs}s` } else { `${secs / 60}m ${secs % 60}s` };
    publish("text", text);
}

fn on_click(arg) {
    state_set("started", now());
    tick();
}
