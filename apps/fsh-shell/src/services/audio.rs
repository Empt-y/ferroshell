//! The "audio" service: volume, devices and per-app sessions, on an "audio" worker thread.
//!
//! It polls rather than registering Core Audio callbacks: volume every 150 ms (cheap: the
//! endpoint is kept), the default devices every 600 ms, sessions every second and the device
//! lists every few seconds. Snapshots only go to the UI thread when something changed, and
//! commands are applied (and re-read) straight away so sliders don't lag.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use fsh_core::audio as core;
use fsh_win::audio::{Audio, Device, Endpoint, Flow, Level};
use fsh_win::icon::{RgbaImage, shell_item_icon};

use super::Update;

const TICK: Duration = Duration::from_millis(150);
const DEFAULTS_EVERY: Duration = Duration::from_millis(600);
const SESSIONS_EVERY: Duration = Duration::from_secs(1);
const DEVICES_EVERY: Duration = Duration::from_secs(4);
const RETRY_EVERY: Duration = Duration::from_secs(10);
const ICON_SIZE: i32 = 32;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct App {
    pub pid: u32,
    pub name: String,
    /// Executable path, the key for its icon ("" for system sounds).
    pub exe: String,
    /// 0–100
    pub volume: f64,
    pub muted: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    /// 0–100; `None` without an output device.
    pub volume: Option<f64>,
    pub muted: bool,
    pub output_name: String,
    pub outputs: Vec<Device>,
    pub input_volume: Option<f64>,
    pub input_muted: bool,
    pub input_name: String,
    pub inputs: Vec<Device>,
    pub apps: Vec<App>,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    SetVolume(Flow, f64),
    SetMuted(Flow, bool),
    ToggleMute(Flow),
    /// Media keys: `notches` steps of `step` percent on the default output.
    Step { notches: i32, step: f64 },
    SetDefault(String),
    SetApp { pid: u32, volume: Option<f64>, muted: Option<bool> },
}

pub struct AudioService {
    tx: Sender<Cmd>,
}

impl AudioService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("audio".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("audio service crashed; audio controls are unavailable until the shell restarts");
                on_update(Update::Audio(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Poller {
    audio: Audio,
    output: Option<Endpoint>,
    input: Option<Endpoint>,
    outputs: Vec<Device>,
    inputs: Vec<Device>,
    apps: Vec<App>,
    descriptions: HashMap<String, String>,
    icons_sent: HashSet<String>,
    last_defaults: Option<Instant>,
    last_sessions: Option<Instant>,
    last_devices: Option<Instant>,
}

fn run(rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::new();
    loop {
        match Audio::new() {
            Ok(audio) => {
                let mut p = Poller {
                    audio,
                    output: None,
                    input: None,
                    outputs: vec![],
                    inputs: vec![],
                    apps: vec![],
                    descriptions: HashMap::new(),
                    icons_sent: HashSet::new(),
                    last_defaults: None,
                    last_sessions: None,
                    last_devices: None,
                };
                p.poll_loop(rx, on_update);
                return;
            }
            Err(e) => {
                tracing::warn!("audio unavailable: {e:#}");
                on_update(Update::Audio(Snapshot::default()));
                match rx.recv_timeout(RETRY_EVERY) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    _ => continue,
                }
            }
        }
    }
}

fn due(last: Option<Instant>, every: Duration) -> bool {
    last.is_none_or(|t| t.elapsed() >= every)
}

impl Poller {
    fn poll_loop(&mut self, rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
        let mut last: Option<Snapshot> = None;
        loop {
            let mut force = false;
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    self.apply(cmd);
                    loop {
                        match rx.try_recv() {
                            Ok(cmd) => self.apply(cmd),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                    force = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let snap = self.poll(force, on_update);
            if last.as_ref() != Some(&snap) {
                on_update(Update::Audio(snap.clone()));
                last = Some(snap);
            }
        }
    }

    fn endpoint(&self, flow: Flow) -> Option<&Endpoint> {
        match flow {
            Flow::Output => self.output.as_ref(),
            Flow::Input => self.input.as_ref(),
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetVolume(flow, v) => {
                if let Some(ep) = self.endpoint(flow) {
                    ep.set_volume(core::from_percent(v));
                }
            }
            Cmd::SetMuted(flow, m) => {
                if let Some(ep) = self.endpoint(flow) {
                    ep.set_muted(m);
                }
            }
            Cmd::ToggleMute(flow) => {
                if let Some(ep) = self.endpoint(flow)
                    && let Some(l) = ep.level()
                {
                    ep.set_muted(!l.muted);
                }
            }
            Cmd::Step { notches, step } => {
                if let Some(ep) = self.output.as_ref()
                    && let Some(l) = ep.level()
                {
                    ep.set_volume(core::from_percent(core::step(core::to_percent(l.volume), notches, step)));
                    // Like Windows: changing the volume unmutes.
                    if l.muted {
                        ep.set_muted(false);
                    }
                }
            }
            Cmd::SetDefault(id) => {
                if let Err(e) = self.audio.set_default(&id) {
                    tracing::warn!("could not switch the default audio device: {e:#}");
                }
                self.last_defaults = None;
                self.last_devices = None;
            }
            Cmd::SetApp { pid, volume, muted } => {
                self.audio.set_session(pid, volume.map(core::from_percent), muted);
                self.last_sessions = None;
            }
        }
    }

    fn poll(&mut self, force: bool, on_update: &dyn Fn(Update)) -> Snapshot {
        if force || due(self.last_defaults, DEFAULTS_EVERY) {
            self.last_defaults = Some(Instant::now());
            for flow in [Flow::Output, Flow::Input] {
                let default = self.audio.default_id(flow);
                let current = self.endpoint(flow).map(|e| e.id.clone()).unwrap_or_default();
                if default != current {
                    let ep = if default.is_empty() { None } else { self.audio.endpoint(flow, None) };
                    match flow {
                        Flow::Output => self.output = ep,
                        Flow::Input => self.input = ep,
                    }
                    self.last_devices = None;
                    self.last_sessions = None;
                }
            }
        }
        if due(self.last_devices, DEVICES_EVERY) {
            self.last_devices = Some(Instant::now());
            self.outputs = self.audio.devices(Flow::Output);
            self.inputs = self.audio.devices(Flow::Input);
        }
        if force || due(self.last_sessions, SESSIONS_EVERY) {
            self.last_sessions = Some(Instant::now());
            self.apps = self.sessions(on_update);
        }
        let level = |ep: Option<&Endpoint>| ep.and_then(Endpoint::level);
        let (out, inp): (Option<Level>, Option<Level>) = (level(self.output.as_ref()), level(self.input.as_ref()));
        let name_of = |list: &[Device]| list.iter().find(|d| d.default).map(|d| d.name.clone()).unwrap_or_default();
        Snapshot {
            available: true,
            volume: out.map(|l| core::to_percent(l.volume)),
            muted: out.is_some_and(|l| l.muted),
            output_name: name_of(&self.outputs),
            outputs: self.outputs.clone(),
            input_volume: inp.map(|l| core::to_percent(l.volume)),
            input_muted: inp.is_some_and(|l| l.muted),
            input_name: name_of(&self.inputs),
            inputs: self.inputs.clone(),
            apps: self.apps.clone(),
        }
    }

    fn sessions(&mut self, on_update: &dyn Fn(Update)) -> Vec<App> {
        let mut apps: Vec<App> = self
            .audio
            .sessions()
            .into_iter()
            .map(|s| {
                let description = if s.exe.is_empty() {
                    String::new()
                } else {
                    self.descriptions
                        .entry(s.exe.clone())
                        .or_insert_with(|| fsh_win::process::file_description(&s.exe).unwrap_or_default())
                        .clone()
                };
                App {
                    pid: s.pid,
                    name: core::app_name(s.pid, &s.display_name, &description, &s.exe),
                    exe: s.exe,
                    volume: core::to_percent(s.volume),
                    muted: s.muted,
                    active: s.active,
                }
            })
            .collect();
        // System sounds last; the rest by name.
        apps.sort_by(|a, b| (a.pid == 0).cmp(&(b.pid == 0)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        let new_icons: Vec<(String, RgbaImage)> = apps
            .iter()
            .filter(|a| !a.exe.is_empty() && self.icons_sent.insert(a.exe.clone()))
            .filter_map(|a| shell_item_icon(&a.exe, ICON_SIZE).map(|i| (a.exe.clone(), i)))
            .collect();
        if !new_icons.is_empty() {
            on_update(Update::AppIcons(new_icons));
        }
        apps
    }
}
