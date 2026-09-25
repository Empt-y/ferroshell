//! The "power" service: battery, power mode, energy saver and screen brightness, on a
//! "power" worker thread.
//!
//! Polls slowly (battery every 10 s, brightness every 5 s); the shell's power-setting
//! notifications ask for an immediate refresh when something actually changes. Brightness
//! slider drags are coalesced: only the newest value per display is applied, since DDC/CI
//! monitors take tens of milliseconds per write.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use fsh_core::power as core;
use fsh_win::brightness::Brightness;
use fsh_win::power::{self, EnergySaver, PowerMode};

use super::Update;

const TICK: Duration = Duration::from_secs(5);
const BATTERY_EVERY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRow {
    pub id: String,
    pub name: String,
    pub internal: bool,
    /// 0–100
    pub brightness: u8,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    pub has_battery: bool,
    pub percent: u8,
    pub charging: bool,
    pub on_ac: bool,
    pub status: String,
    /// Battery wear, 0–100.
    pub health: Option<u8>,
    pub power_mode: Option<PowerMode>,
    pub mode_available: bool,
    pub energy_saver: Option<EnergySaver>,
    pub displays: Vec<DisplayRow>,
    pub error: String,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    SetPowerMode(PowerMode),
    SetBrightness { id: String, percent: u8 },
    /// Something changed (a power-setting notification): re-read everything now.
    Refresh,
}

pub struct PowerService {
    tx: Sender<Cmd>,
}

impl PowerService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("power".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("power service crashed; power controls are unavailable until the shell restarts");
                on_update(Update::Power(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Poller {
    brightness: Brightness,
    snap: Snapshot,
    last_battery: Option<Instant>,
}

fn run(rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::mta();
    let mut p = Poller { brightness: Brightness::new(), snap: Snapshot { available: true, ..Snapshot::default() }, last_battery: None };
    let mut last: Option<Snapshot> = None;
    let mut wait = Duration::ZERO;
    loop {
        let mut cmds = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(cmd) => {
                cmds.push(cmd);
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => cmds.push(cmd),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let refresh_all = p.apply(cmds);
        p.poll(refresh_all);
        if last.as_ref() != Some(&p.snap) {
            on_update(Update::Power(p.snap.clone()));
            last = Some(p.snap.clone());
        }
        wait = TICK;
    }
}

impl Poller {
    /// Apply a batch of commands; returns whether battery/mode should be re-read now.
    fn apply(&mut self, cmds: Vec<Cmd>) -> bool {
        let mut refresh = false;
        // Newest brightness per display only.
        let mut brightness: BTreeMap<String, u8> = BTreeMap::new();
        for cmd in cmds {
            match cmd {
                Cmd::SetBrightness { id, percent } => {
                    brightness.insert(id, percent);
                }
                Cmd::SetPowerMode(mode) => {
                    self.snap.error.clear();
                    if let Err(e) = power::set_power_mode(mode) {
                        self.snap.error = format!("could not change the power mode: {e:#}");
                        tracing::warn!("{}", self.snap.error);
                    }
                    refresh = true;
                }
                Cmd::Refresh => refresh = true,
            }
        }
        for (id, percent) in brightness {
            if let Err(e) = self.brightness.set(&id, percent) {
                self.snap.error = format!("could not change the brightness: {e:#}");
                tracing::warn!("{}", self.snap.error);
            } else if let Some(d) = self.snap.displays.iter_mut().find(|d| d.id == id) {
                // Show the new value straight away; the next read confirms it.
                d.brightness = percent;
            }
        }
        refresh
    }

    fn poll(&mut self, refresh_all: bool) {
        if refresh_all || self.last_battery.is_none_or(|t| t.elapsed() >= BATTERY_EVERY) {
            self.last_battery = Some(Instant::now());
            let s = &mut self.snap;
            match power::battery() {
                Some(b) => {
                    let to_full = match (b.remaining_mwh, b.full_mwh, b.charge_rate_mw) {
                        (Some(r), Some(f), Some(rate)) => core::time_to_full(r, f, rate),
                        _ => None,
                    };
                    s.has_battery = true;
                    s.percent = b.percent;
                    s.charging = b.charging;
                    s.on_ac = b.on_ac;
                    s.status = core::status_line(b.percent, b.on_ac, b.charging, b.seconds_left, to_full);
                    s.health = match (b.full_mwh, b.design_mwh) {
                        (Some(f), Some(d)) => core::health_percent(f, d),
                        _ => None,
                    };
                }
                None => {
                    s.has_battery = false;
                    s.status.clear();
                    s.health = None;
                }
            }
            s.mode_available = power::power_mode_available();
            s.power_mode = power::power_mode();
            s.energy_saver = Some(power::energy_saver());
        }
        self.snap.displays = self
            .brightness
            .displays()
            .into_iter()
            .map(|d| DisplayRow { id: d.id, name: d.name, internal: d.internal, brightness: d.brightness })
            .collect();
    }
}
