//! The "network" service: Wi-Fi, Ethernet, overall connectivity, airplane mode and VPN
//! status, on a "network" worker thread.
//!
//! Cheap facts (connectivity, Ethernet, radio state) are polled every couple of seconds;
//! a Wi-Fi scan is kicked off less often (scanning is what actually costs time and battery)
//! and after the user asks for one.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use fsh_core::network::SecurityExt;
use fsh_win::network::{self, Connectivity, Interface, Security, Wlan};

use super::Update;

const TICK: Duration = Duration::from_millis(800);
const SCAN_EVERY: Duration = Duration::from_secs(15);
const RETRY_EVERY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub struct WifiRow {
    pub ssid: String,
    pub security: String,
    pub needs_password: bool,
    /// 0–4
    pub bars: u8,
    pub connected: bool,
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VpnRow {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    None,
    Wifi,
    Ethernet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    pub wifi_present: bool,
    pub wifi_enabled: bool,
    pub airplane_mode: bool,
    pub connected: bool,
    pub limited: bool,
    pub kind: Kind,
    pub ssid: String,
    /// 0–4
    pub bars: u8,
    pub ethernet_name: String,
    pub networks: Vec<WifiRow>,
    pub vpns: Vec<VpnRow>,
    pub error: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            available: false,
            wifi_present: false,
            wifi_enabled: false,
            airplane_mode: false,
            connected: false,
            limited: false,
            kind: Kind::None,
            ssid: String::new(),
            bars: 0,
            ethernet_name: String::new(),
            networks: vec![],
            vpns: vec![],
            error: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    Scan,
    /// Connect to a saved network, or one with no password.
    Connect(String),
    /// Save a profile with `password` for `ssid`, then connect.
    ConnectNew { ssid: String, password: String },
    Disconnect,
    Forget(String),
    SetWifiEnabled(bool),
    SetAirplaneMode(bool),
    VpnDisconnect(String),
}

pub struct NetworkService {
    tx: Sender<Cmd>,
}

impl NetworkService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("network".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("network service crashed; network controls are unavailable until the shell restarts");
                on_update(Update::Network(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn run(rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::mta();
    loop {
        match Wlan::open() {
            Ok(wlan) => {
                let mut p = Poller { wlan, iface: None, cached: vec![], last_scan: None, error: String::new() };
                p.poll_loop(rx, on_update);
                return;
            }
            Err(e) => {
                tracing::warn!("Wi-Fi (WLAN service) unavailable: {e:#}");
                // Wi-Fi hardware/service missing doesn't mean no networking at all: still
                // report Ethernet and overall connectivity.
                on_update(Update::Network(read_wifi_less_snapshot()));
                match rx.recv_timeout(RETRY_EVERY) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    _ => continue,
                }
            }
        }
    }
}

fn read_wifi_less_snapshot() -> Snapshot {
    let (level, is_wifi) = network::connectivity();
    let eth = network::ethernet_status();
    Snapshot {
        available: true,
        wifi_present: false,
        connected: level != Connectivity::None,
        limited: level == Connectivity::Limited || level == Connectivity::LocalOnly,
        kind: if !is_wifi && eth.as_ref().is_some_and(|e| e.connected) { Kind::Ethernet } else { Kind::None },
        ethernet_name: eth.map(|e| e.name).unwrap_or_default(),
        vpns: network::vpn_connections().into_iter().map(|v| VpnRow { name: v.name }).collect(),
        ..Snapshot::default()
    }
}

struct Poller {
    wlan: Wlan,
    iface: Option<Interface>,
    /// The last scan, so `ConnectNew` doesn't need the UI to round-trip security details.
    cached: Vec<network::WifiNetwork>,
    last_scan: Option<Instant>,
    error: String,
}

fn security_label(s: Security) -> &'static str {
    s.label()
}

impl Poller {
    fn poll_loop(&mut self, rx: &Receiver<Cmd>, on_update: &dyn Fn(Update)) {
        self.iface = self.wlan.interfaces().into_iter().next();
        let mut last: Option<Snapshot> = None;
        let mut force_scan = true;
        loop {
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    self.apply(cmd, &mut force_scan);
                    loop {
                        match rx.try_recv() {
                            Ok(cmd) => self.apply(cmd, &mut force_scan),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if let Some(iface) = self.iface {
                if force_scan || self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_EVERY) {
                    self.wlan.scan(iface);
                    self.last_scan = Some(Instant::now());
                    force_scan = false;
                }
                self.cached = self.wlan.available_networks(iface);
            }
            let snap = self.snapshot();
            if last.as_ref() != Some(&snap) {
                on_update(Update::Network(snap.clone()));
                last = Some(snap);
            }
        }
    }

    fn apply(&mut self, cmd: Cmd, force_scan: &mut bool) {
        let Some(iface) = self.iface else { return };
        self.error.clear();
        match cmd {
            Cmd::Scan => *force_scan = true,
            Cmd::Connect(ssid) => {
                if let Err(e) = self.wlan.connect_profile(iface, &ssid) {
                    self.error = format!("could not connect to {ssid}: {e:#}");
                    tracing::warn!("{}", self.error);
                }
            }
            Cmd::ConnectNew { ssid, password } => {
                let Some(net) = self.cached.iter().find(|n| n.ssid == ssid).cloned() else {
                    self.error = format!("{ssid} is no longer in range");
                    return;
                };
                let xml = fsh_core::network::wifi_profile_xml(&net.ssid_hex, &net.ssid, net.security, &password);
                match xml {
                    Some(xml) => match self.wlan.set_profile(iface, &xml).and_then(|()| self.wlan.connect_profile(iface, &net.ssid)) {
                        Ok(()) => {}
                        Err(e) => {
                            self.error = format!("could not connect to {ssid}: {e:#}");
                            tracing::warn!("{}", self.error);
                        }
                    },
                    None => self.error = format!("{ssid} needs Windows' own network settings (enterprise security)"),
                }
            }
            Cmd::Disconnect => self.wlan.disconnect(iface),
            Cmd::Forget(profile) => {
                self.wlan.forget(iface, &profile);
                *force_scan = true;
            }
            Cmd::SetWifiEnabled(on) => network::set_wifi_radio(on),
            Cmd::SetAirplaneMode(on) => network::set_airplane_mode(on),
            Cmd::VpnDisconnect(name) => {
                if let Some(v) = network::vpn_connections().into_iter().find(|v| v.name == name) {
                    network::vpn_disconnect(&v);
                }
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        let (level, is_wifi) = network::connectivity();
        let eth = network::ethernet_status();
        let current = self.cached.iter().find(|n| n.connected);
        let kind = if is_wifi && current.is_some() {
            Kind::Wifi
        } else if eth.as_ref().is_some_and(|e| e.connected) {
            Kind::Ethernet
        } else if current.is_some() {
            Kind::Wifi
        } else {
            Kind::None
        };
        Snapshot {
            available: true,
            wifi_present: self.iface.is_some(),
            wifi_enabled: network::wifi_radio_on().unwrap_or(false),
            airplane_mode: network::airplane_mode_on(),
            connected: level != Connectivity::None,
            limited: matches!(level, Connectivity::Limited | Connectivity::LocalOnly),
            kind,
            ssid: current.map(|n| n.ssid.clone()).unwrap_or_default(),
            bars: current.map(|n| fsh_core::network::signal_bars(n.signal)).unwrap_or(0),
            ethernet_name: eth.map(|e| e.name).unwrap_or_default(),
            networks: self
                .cached
                .iter()
                .map(|n| WifiRow {
                    ssid: n.ssid.clone(),
                    security: security_label(n.security).to_owned(),
                    needs_password: n.security.needs_password() && !n.has_profile,
                    bars: fsh_core::network::signal_bars(n.signal),
                    connected: n.connected,
                    saved: n.has_profile,
                })
                .collect(),
            vpns: network::vpn_connections().into_iter().map(|v| VpnRow { name: v.name }).collect(),
            error: self.error.clone(),
        }
    }
}
