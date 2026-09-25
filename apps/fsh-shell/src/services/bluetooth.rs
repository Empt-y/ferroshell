//! The "bluetooth" service: the radio and paired devices, on a "bluetooth" worker thread.
//! Polls the radio and devices every few seconds and battery levels less often (each is a
//! PnP query per device).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use fsh_core::bluetooth as core;
use fsh_win::bluetooth::{self as bt, BtDevice};

use super::Update;

const TICK: Duration = Duration::from_secs(3);
const BATTERY_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    /// "audio", "input", "phone", "computer", "wearable" or "other".
    pub kind: String,
    pub connected: bool,
    /// Audio devices: the applet can connect/disconnect them.
    pub can_connect: bool,
    pub battery: Option<u8>,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    /// A Bluetooth radio exists.
    pub present: bool,
    pub enabled: bool,
    pub devices: Vec<DeviceRow>,
    pub error: String,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    SetEnabled(bool),
    Connect(String),
    Disconnect(String),
}

pub struct BluetoothService {
    tx: Sender<Cmd>,
}

impl BluetoothService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("bluetooth".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("bluetooth service crashed; Bluetooth controls are unavailable until the shell restarts");
                on_update(Update::Bluetooth(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Poller {
    devices: Vec<BtDevice>,
    battery: HashMap<String, Option<u8>>,
    last_battery: Option<Instant>,
    error: String,
}

fn run(rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::mta();
    let mut p = Poller { devices: vec![], battery: HashMap::new(), last_battery: None, error: String::new() };
    let mut last: Option<Snapshot> = None;
    let mut wait = Duration::ZERO;
    loop {
        match rx.recv_timeout(wait) {
            Ok(cmd) => {
                p.apply(cmd);
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => p.apply(cmd),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        wait = TICK;
        let snap = p.poll();
        if last.as_ref() != Some(&snap) {
            on_update(Update::Bluetooth(snap.clone()));
            last = Some(snap);
        }
    }
}

impl Poller {
    fn apply(&mut self, cmd: Cmd) {
        self.error.clear();
        let result = match cmd {
            Cmd::SetEnabled(on) => bt::set_radio(on),
            Cmd::Connect(id) => self.audio(&id, true),
            Cmd::Disconnect(id) => self.audio(&id, false),
        };
        if let Err(e) = result {
            self.error = format!("{e:#}");
            tracing::warn!("bluetooth: {}", self.error);
        }
    }

    fn audio(&self, id: &str, connect: bool) -> anyhow::Result<()> {
        let dev = self.devices.iter().find(|d| d.id == id).ok_or_else(|| anyhow::anyhow!("that device is no longer paired"))?;
        let container = dev.container.ok_or_else(|| anyhow::anyhow!("{} can't be connected from here", dev.name))?;
        bt::audio_connect(container, connect).map_err(|e| e.context(format!("{} {}", if connect { "connecting" } else { "disconnecting" }, dev.name)))
    }

    fn poll(&mut self) -> Snapshot {
        let radio = bt::radio_on();
        self.devices = bt::paired_devices();
        if self.last_battery.is_none_or(|t| t.elapsed() >= BATTERY_EVERY) {
            self.last_battery = Some(Instant::now());
            self.battery = self.devices.iter().map(|d| (d.id.clone(), d.container.and_then(bt::battery))).collect();
        }
        Snapshot {
            available: true,
            present: radio.is_some(),
            enabled: radio.unwrap_or(false),
            devices: self
                .devices
                .iter()
                .map(|d| {
                    let kind = core::device_kind(d.major_class, d.le_category);
                    let battery = self.battery.get(&d.id).copied().flatten();
                    DeviceRow {
                        id: d.id.clone(),
                        name: if d.name.is_empty() { core::format_address(d.address) } else { d.name.clone() },
                        kind: kind.to_owned(),
                        connected: d.connected,
                        can_connect: kind == "audio" && d.container.is_some(),
                        battery,
                        status: core::status_text(d.connected, battery),
                    }
                })
                .collect(),
            error: self.error.clone(),
        }
    }
}
