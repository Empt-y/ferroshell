//! System services behind `@ferroshell/services.slint` (`Audio`, `Media`, ...).
//!
//! Each service owns its Windows APIs on a worker thread, sends snapshots here, and takes
//! commands over a channel. A service only runs while some widget on a panel lists it in
//! `services = [...]`. A failing service shows as `available: false`; it never takes the
//! shell down.

pub mod audio;
pub mod media;
pub mod bluetooth;
pub mod network;
pub mod notifications;
pub mod power;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use fsh_win::audio::{Device, Flow};
use fsh_win::icon::RgbaImage;
use fsh_win::media::Control;
use slint::{Image, Model, ModelRc, SharedPixelBuffer, VecModel};
use slint_interpreter::{ComponentInstance, Struct, Value};

use crate::app::{App, with};

pub enum Update {
    Audio(audio::Snapshot),
    AppIcons(Vec<(String, RgbaImage)>),
    Media(media::Snapshot),
    Network(network::Snapshot),
    Bluetooth(bluetooth::Snapshot),
    Power(power::Snapshot),
    Notifications(notifications::Snapshot),
}

#[derive(Default)]
pub struct Services {
    audio: RefCell<Option<audio::AudioService>>,
    media: RefCell<Option<media::MediaService>>,
    network: RefCell<Option<network::NetworkService>>,
    bluetooth: RefCell<Option<bluetooth::BluetoothService>>,
    power: RefCell<Option<power::PowerService>>,
    // Power-setting notifications to the shell's event window while the power service runs.
    power_notify: RefCell<Option<fsh_win::power::PowerNotifications>>,
    // Last brightness notified: Windows sends the current value on registration, which
    // mustn't pop the OSD.
    last_brightness: std::cell::Cell<Option<u8>>,
    audio_state: RefCell<audio::Snapshot>,
    media_state: RefCell<media::Snapshot>,
    network_state: RefCell<network::Snapshot>,
    bluetooth_state: RefCell<bluetooth::Snapshot>,
    power_state: RefCell<power::Snapshot>,
    art: RefCell<Option<Image>>,
    icons: RefCell<HashMap<String, Image>>,
    // Shared by every instance and updated in place, so a slider being dragged in a list
    // isn't recreated under the pointer.
    outputs: Rc<VecModel<Value>>,
    inputs: Rc<VecModel<Value>>,
    apps: Rc<VecModel<Value>>,
    networks: Rc<VecModel<Value>>,
    vpns: Rc<VecModel<Value>>,
    bt_devices: Rc<VecModel<Value>>,
    displays: Rc<VecModel<Value>>,
    // Notifications (see `crate::notify`).
    pub(crate) notifications: RefCell<Option<notifications::NotificationService>>,
    pub(crate) notif_state: RefCell<notifications::Snapshot>,
    pub(crate) notif_rows: Rc<VecModel<Value>>,
    /// Ids the user has seen (the popup was open while they were there).
    pub(crate) notif_seen: RefCell<std::collections::HashSet<u32>>,
    pub(crate) notif_dnd: std::cell::Cell<bool>,
    pub(crate) logo_images: RefCell<HashMap<std::path::PathBuf, Image>>,
}

type GlobalCallback = Box<dyn Fn(&[Value]) -> Value>;

fn post(u: Update) {
    let _ = slint::invoke_from_event_loop(move || {
        with(|a| a.on_service_update(u));
    });
}

pub(crate) fn strukt(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Struct>())
}

fn num(a: &[Value], i: usize) -> f64 {
    match a.get(i) {
        Some(Value::Number(n)) => *n,
        _ => 0.0,
    }
}

fn flag(a: &[Value], i: usize) -> bool {
    matches!(a.get(i), Some(Value::Bool(true)))
}

fn text(a: &[Value], i: usize) -> String {
    match a.get(i) {
        Some(Value::String(s)) => s.to_string(),
        _ => String::new(),
    }
}

/// Replace a model's rows, changing only rows that differ when the length is unchanged.
pub(crate) fn sync_model(model: &VecModel<Value>, rows: Vec<Value>) {
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

fn device_rows(devices: &[Device]) -> Vec<Value> {
    devices
        .iter()
        .map(|d| strukt(vec![("id", Value::String(d.id.as_str().into())), ("name", Value::String(d.name.as_str().into())), ("default", Value::Bool(d.default))]))
        .collect()
}

fn to_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::clone_from_slice(&img.pixels, img.width, img.height))
}

impl App {
    /// Start the services some widget on a panel asks for; stop the rest.
    pub(crate) fn sync_services(&self) {
        let wanted: BTreeSet<String> = {
            let st = self.state.borrow();
            st.panels
                .iter()
                .flat_map(|p| p.config.widgets.iter())
                .filter_map(|w| st.registry.get(&w.id).ok())
                .flat_map(|pkg| pkg.manifest.services.iter().cloned())
                .chain(
                    // The volume and media keys need both, applet or not.
                    crate::osd::media_keys_wanted(self.safe_mode).then(|| ["audio".to_owned(), "media".to_owned()]).into_iter().flatten(),
                )
                .collect()
        };
        let s = &self.services;
        let running = s.audio.borrow().is_some();
        if wanted.contains("audio") && !running {
            match audio::AudioService::spawn(post) {
                Ok(svc) => *s.audio.borrow_mut() = Some(svc),
                Err(e) => tracing::error!("could not start the audio service: {e:#}"),
            }
        } else if !wanted.contains("audio") && running {
            drop(s.audio.borrow_mut().take());
            self.on_service_update(Update::Audio(audio::Snapshot::default()));
        }
        let running = s.media.borrow().is_some();
        if wanted.contains("media") && !running {
            match media::MediaService::spawn(post) {
                Ok(svc) => *s.media.borrow_mut() = Some(svc),
                Err(e) => tracing::error!("could not start the media service: {e:#}"),
            }
        } else if !wanted.contains("media") && running {
            drop(s.media.borrow_mut().take());
            self.on_service_update(Update::Media(media::Snapshot::default()));
        }
        let running = s.network.borrow().is_some();
        if wanted.contains("network") && !running {
            match network::NetworkService::spawn(post) {
                Ok(svc) => *s.network.borrow_mut() = Some(svc),
                Err(e) => tracing::error!("could not start the network service: {e:#}"),
            }
        } else if !wanted.contains("network") && running {
            drop(s.network.borrow_mut().take());
            self.on_service_update(Update::Network(network::Snapshot::default()));
        }
        let running = s.bluetooth.borrow().is_some();
        if wanted.contains("bluetooth") && !running {
            match bluetooth::BluetoothService::spawn(post) {
                Ok(svc) => *s.bluetooth.borrow_mut() = Some(svc),
                Err(e) => tracing::error!("could not start the bluetooth service: {e:#}"),
            }
        } else if !wanted.contains("bluetooth") && running {
            drop(s.bluetooth.borrow_mut().take());
            self.on_service_update(Update::Bluetooth(bluetooth::Snapshot::default()));
        }
        let running = s.power.borrow().is_some();
        if wanted.contains("power") && !running {
            match power::PowerService::spawn(post) {
                Ok(svc) => {
                    *s.power.borrow_mut() = Some(svc);
                    *s.power_notify.borrow_mut() = Some(fsh_win::power::PowerNotifications::register(self.events_hwnd()));
                }
                Err(e) => tracing::error!("could not start the power service: {e:#}"),
            }
        } else if !wanted.contains("power") && running {
            drop(s.power_notify.borrow_mut().take());
            s.last_brightness.set(None);
            drop(s.power.borrow_mut().take());
            self.on_service_update(Update::Power(power::Snapshot::default()));
        }
        let running = s.notifications.borrow().is_some();
        if wanted.contains("notifications") && !running {
            s.notif_dnd.set(crate::notify::load_dnd());
            match notifications::NotificationService::spawn(post) {
                Ok(svc) => *s.notifications.borrow_mut() = Some(svc),
                Err(e) => tracing::error!("could not start the notifications service: {e:#}"),
            }
        } else if !wanted.contains("notifications") && running {
            drop(s.notifications.borrow_mut().take());
            self.on_service_update(Update::Notifications(notifications::Snapshot::default()));
        }
        for unknown in wanted.iter().filter(|w| !["audio", "media", "network", "bluetooth", "power", "notifications"].contains(&w.as_str())) {
            tracing::warn!("a widget asks for unknown service `{unknown}`");
        }
    }

    pub(crate) fn audio_cmd(&self, cmd: audio::Cmd) {
        tracing::debug!("audio: {cmd:?}");
        if let Some(a) = self.services.audio.borrow().as_ref() {
            a.send(cmd);
        }
    }

    pub(crate) fn media_cmd(&self, c: Control) {
        if let Some(m) = self.services.media.borrow().as_ref() {
            m.send(c);
        }
    }

    fn network_cmd(&self, cmd: network::Cmd) {
        tracing::debug!("network: {cmd:?}");
        if let Some(n) = self.services.network.borrow().as_ref() {
            n.send(cmd);
        }
    }

    pub(crate) fn power_cmd(&self, cmd: power::Cmd) {
        tracing::debug!("power: {cmd:?}");
        if let Some(p) = self.services.power.borrow().as_ref() {
            p.send(cmd);
        }
    }

    fn bluetooth_cmd(&self, cmd: bluetooth::Cmd) {
        tracing::debug!("bluetooth: {cmd:?}");
        if let Some(b) = self.services.bluetooth.borrow().as_ref() {
            b.send(cmd);
        }
    }

    /// Connect a new panel or popup to the services: callbacks, shared models, current values.
    pub(crate) fn bind_services(&self, instance: &ComponentInstance) {
        let cb = |global: &str, name: &str, f: GlobalCallback| {
            if let Err(e) = instance.set_global_callback(global, name, f) {
                tracing::warn!("{global}.{name}: {e}");
            }
        };
        let audio = |name: &str, f: fn(&[Value]) -> audio::Cmd| {
            cb("Audio", name, Box::new(move |a| {
                let cmd = f(a);
                with(|app| app.audio_cmd(cmd));
                Value::Void
            }));
        };
        audio("set-volume", |a| audio::Cmd::SetVolume(Flow::Output, num(a, 0)));
        audio("set-muted", |a| audio::Cmd::SetMuted(Flow::Output, flag(a, 0)));
        audio("set-input-volume", |a| audio::Cmd::SetVolume(Flow::Input, num(a, 0)));
        audio("set-input-muted", |a| audio::Cmd::SetMuted(Flow::Input, flag(a, 0)));
        audio("set-default-device", |a| audio::Cmd::SetDefault(text(a, 0)));
        audio("set-app-volume", |a| audio::Cmd::SetApp { pid: num(a, 0) as u32, volume: Some(num(a, 1)), muted: None });
        audio("set-app-muted", |a| audio::Cmd::SetApp { pid: num(a, 0) as u32, volume: None, muted: Some(flag(a, 1)) });
        for (name, c) in [("play-pause", Control::PlayPause), ("next", Control::Next), ("previous", Control::Previous)] {
            cb("Media", name, Box::new(move |_| {
                with(|app| app.media_cmd(c));
                Value::Void
            }));
        }
        let net = |name: &str, f: fn(&[Value]) -> network::Cmd| {
            cb("Network", name, Box::new(move |a| {
                let cmd = f(a);
                with(|app| app.network_cmd(cmd));
                Value::Void
            }));
        };
        net("scan", |_| network::Cmd::Scan);
        net("connect", |a| network::Cmd::Connect(text(a, 0)));
        net("connect-new", |a| network::Cmd::ConnectNew { ssid: text(a, 0), password: text(a, 1) });
        net("disconnect", |_| network::Cmd::Disconnect);
        net("forget", |a| network::Cmd::Forget(text(a, 0)));
        net("set-wifi-enabled", |a| network::Cmd::SetWifiEnabled(flag(a, 0)));
        net("set-airplane-mode", |a| network::Cmd::SetAirplaneMode(flag(a, 0)));
        net("vpn-disconnect", |a| network::Cmd::VpnDisconnect(text(a, 0)));
        let bt = |name: &str, f: fn(&[Value]) -> bluetooth::Cmd| {
            cb("Bluetooth", name, Box::new(move |a| {
                let cmd = f(a);
                with(|app| app.bluetooth_cmd(cmd));
                Value::Void
            }));
        };
        bt("set-enabled", |a| bluetooth::Cmd::SetEnabled(flag(a, 0)));
        bt("connect", |a| bluetooth::Cmd::Connect(text(a, 0)));
        bt("disconnect", |a| bluetooth::Cmd::Disconnect(text(a, 0)));
        cb("Power", "set-power-mode", Box::new(|a| {
            let mode = match text(a, 0).as_str() {
                "efficiency" => Some(fsh_win::power::PowerMode::Efficiency),
                "balanced" => Some(fsh_win::power::PowerMode::Balanced),
                "performance" => Some(fsh_win::power::PowerMode::Performance),
                _ => None,
            };
            if let Some(m) = mode {
                with(|app| app.power_cmd(power::Cmd::SetPowerMode(m)));
            }
            Value::Void
        }));
        cb("Power", "set-brightness", Box::new(|a| {
            let cmd = power::Cmd::SetBrightness { id: text(a, 0), percent: num(a, 1).round().clamp(0.0, 100.0) as u8 };
            with(|app| app.power_cmd(cmd));
            Value::Void
        }));
        let s = &self.services;
        let set = |global: &str, name: &str, v: Value| {
            if let Err(e) = instance.set_global_property(global, name, v) {
                tracing::warn!("{global}.{name}: {e}");
            }
        };
        set("Audio", "outputs", Value::Model(ModelRc::from(s.outputs.clone())));
        set("Audio", "inputs", Value::Model(ModelRc::from(s.inputs.clone())));
        set("Audio", "apps", Value::Model(ModelRc::from(s.apps.clone())));
        set("Network", "networks", Value::Model(ModelRc::from(s.networks.clone())));
        set("Network", "vpns", Value::Model(ModelRc::from(s.vpns.clone())));
        set("Bluetooth", "devices", Value::Model(ModelRc::from(s.bt_devices.clone())));
        set("Power", "displays", Value::Model(ModelRc::from(s.displays.clone())));
        set("Notifications", "items", Value::Model(ModelRc::from(s.notif_rows.clone())));
        self.apply_audio(instance);
        self.apply_media(instance);
        self.apply_network(instance);
        self.apply_bluetooth(instance);
        self.apply_power(instance);
        self.bind_notifications(instance);
        self.apply_notifications(instance);
    }

    /// Every instance showing service data: panels and compiled popups.
    pub(crate) fn for_each_instance(&self, f: &dyn Fn(&ComponentInstance)) {
        for p in &self.state.borrow().panels {
            f(&p.instance);
        }
        self.for_each_popup(f);
    }

    pub(crate) fn on_service_update(&self, u: Update) {
        let s = &self.services;
        match u {
            Update::Audio(snap) => {
                sync_model(&s.outputs, device_rows(&snap.outputs));
                sync_model(&s.inputs, device_rows(&snap.inputs));
                *s.audio_state.borrow_mut() = snap;
                self.sync_app_rows();
                self.for_each_instance(&|i| self.apply_audio(i));
                self.osd_audio_changed();
            }
            Update::AppIcons(icons) => {
                {
                    let mut map = s.icons.borrow_mut();
                    for (exe, img) in &icons {
                        map.insert(exe.clone(), to_image(img));
                    }
                }
                self.sync_app_rows();
            }
            Update::Media(snap) => {
                let art = snap.art.as_ref().and_then(|p| Image::load_from_path(p).ok());
                *s.art.borrow_mut() = art;
                *s.media_state.borrow_mut() = snap;
                self.for_each_instance(&|i| self.apply_media(i));
            }
            Update::Network(snap) => {
                let network_rows = snap
                    .networks
                    .iter()
                    .map(|n| {
                        strukt(vec![
                            ("ssid", Value::String(n.ssid.as_str().into())),
                            ("security", Value::String(n.security.as_str().into())),
                            ("needs-password", Value::Bool(n.needs_password)),
                            ("bars", Value::Number(f64::from(n.bars))),
                            ("connected", Value::Bool(n.connected)),
                            ("saved", Value::Bool(n.saved)),
                        ])
                    })
                    .collect();
                let vpn_rows = snap.vpns.iter().map(|v| strukt(vec![("name", Value::String(v.name.as_str().into()))])).collect();
                sync_model(&s.networks, network_rows);
                sync_model(&s.vpns, vpn_rows);
                *s.network_state.borrow_mut() = snap;
                self.for_each_instance(&|i| self.apply_network(i));
            }
            Update::Bluetooth(snap) => {
                let rows = snap
                    .devices
                    .iter()
                    .map(|d| {
                        strukt(vec![
                            ("id", Value::String(d.id.as_str().into())),
                            ("name", Value::String(d.name.as_str().into())),
                            ("kind", Value::String(d.kind.as_str().into())),
                            ("connected", Value::Bool(d.connected)),
                            ("can-connect", Value::Bool(d.can_connect)),
                            ("battery", Value::Number(d.battery.map_or(-1.0, f64::from))),
                            ("status", Value::String(d.status.as_str().into())),
                        ])
                    })
                    .collect();
                sync_model(&s.bt_devices, rows);
                *s.bluetooth_state.borrow_mut() = snap;
                self.for_each_instance(&|i| self.apply_bluetooth(i));
            }
            Update::Power(snap) => {
                let rows = snap
                    .displays
                    .iter()
                    .map(|d| {
                        strukt(vec![
                            ("id", Value::String(d.id.as_str().into())),
                            ("name", Value::String(d.name.as_str().into())),
                            ("internal", Value::Bool(d.internal)),
                            ("brightness", Value::Number(f64::from(d.brightness))),
                        ])
                    })
                    .collect();
                sync_model(&s.displays, rows);
                *s.power_state.borrow_mut() = snap;
                self.for_each_instance(&|i| self.apply_power(i));
            }
            Update::Notifications(snap) => self.on_notifications(snap),
        }
    }

    /// Default output (0–100, muted).
    pub(crate) fn audio_level(&self) -> (f64, bool) {
        let st = self.services.audio_state.borrow();
        (st.volume.unwrap_or(0.0), st.muted)
    }

    fn sync_app_rows(&self) {
        let s = &self.services;
        let st = s.audio_state.borrow();
        let icons = s.icons.borrow();
        let rows = st
            .apps
            .iter()
            .map(|a| {
                strukt(vec![
                    ("pid", Value::Number(f64::from(a.pid))),
                    ("name", Value::String(a.name.as_str().into())),
                    ("icon", Value::Image(icons.get(&a.exe).cloned().unwrap_or_default())),
                    ("volume", Value::Number(a.volume)),
                    ("muted", Value::Bool(a.muted)),
                    ("active", Value::Bool(a.active)),
                ])
            })
            .collect();
        sync_model(&s.apps, rows);
    }

    fn apply_audio(&self, i: &ComponentInstance) {
        let st = self.services.audio_state.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Audio", name, v);
        };
        set("available", Value::Bool(st.available && st.volume.is_some()));
        set("volume", Value::Number(st.volume.unwrap_or(0.0)));
        set("muted", Value::Bool(st.muted));
        set("output-name", Value::String(st.output_name.as_str().into()));
        set("has-input", Value::Bool(st.input_volume.is_some()));
        set("input-volume", Value::Number(st.input_volume.unwrap_or(0.0)));
        set("input-muted", Value::Bool(st.input_muted));
        set("input-name", Value::String(st.input_name.as_str().into()));
    }

    fn apply_media(&self, i: &ComponentInstance) {
        let st = self.services.media_state.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Media", name, v);
        };
        let now = st.now.clone().unwrap_or_default();
        // The app's name and icon, if the launcher's index knows it.
        let (app_name, app_icon) = {
            let l = self.launcher.state.borrow();
            let known = l.apps.iter().find(|a| a.id.eq_ignore_ascii_case(&now.app_id));
            (
                known.map(|a| a.name.clone()).unwrap_or_else(|| fsh_core::audio::media_app_name(&now.app_id)),
                known.and_then(|a| l.icons.get(&a.id).cloned()).unwrap_or_default(),
            )
        };
        let art = self.services.art.borrow().clone();
        set("available", Value::Bool(st.available));
        set("has-session", Value::Bool(st.now.is_some()));
        set("title", Value::String(now.title.as_str().into()));
        set("artist", Value::String(now.artist.as_str().into()));
        set("album", Value::String(now.album.as_str().into()));
        set("app-name", Value::String(app_name.as_str().into()));
        set("app-icon", Value::Image(app_icon));
        set("has-art", Value::Bool(art.is_some()));
        set("art", Value::Image(art.unwrap_or_default()));
        set("playing", Value::Bool(now.playing));
        set("can-play-pause", Value::Bool(now.can_play_pause));
        set("can-next", Value::Bool(now.can_next));
        set("can-previous", Value::Bool(now.can_previous));
    }

    fn apply_network(&self, i: &ComponentInstance) {
        let st = self.services.network_state.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Network", name, v);
        };
        set("available", Value::Bool(st.available));
        set("wifi-present", Value::Bool(st.wifi_present));
        set("wifi-enabled", Value::Bool(st.wifi_enabled));
        set("airplane-mode", Value::Bool(st.airplane_mode));
        set("connected", Value::Bool(st.connected));
        set("limited", Value::Bool(st.limited));
        set(
            "kind",
            Value::String(match st.kind {
                network::Kind::Wifi => "wifi",
                network::Kind::Ethernet => "ethernet",
                network::Kind::None => "none",
            }.into()),
        );
        set("ssid", Value::String(st.ssid.as_str().into()));
        set("bars", Value::Number(f64::from(st.bars)));
        set("ethernet-name", Value::String(st.ethernet_name.as_str().into()));
        set("error", Value::String(st.error.as_str().into()));
    }

    fn apply_power(&self, i: &ComponentInstance) {
        use fsh_win::power::{EnergySaver, PowerMode};
        let st = self.services.power_state.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Power", name, v);
        };
        set("available", Value::Bool(st.available));
        set("has-battery", Value::Bool(st.has_battery));
        set("percent", Value::Number(f64::from(st.percent)));
        set("level", Value::Number(f64::from(fsh_core::power::icon_level(st.percent))));
        set("charging", Value::Bool(st.charging));
        set("on-ac", Value::Bool(st.on_ac));
        set("status", Value::String(st.status.as_str().into()));
        set("health", Value::Number(st.health.map_or(-1.0, f64::from)));
        set(
            "power-mode",
            Value::String(
                match st.power_mode {
                    Some(PowerMode::Efficiency) => "efficiency",
                    Some(PowerMode::Balanced) => "balanced",
                    Some(PowerMode::Performance) => "performance",
                    None => "custom",
                }
                .into(),
            ),
        );
        set("mode-available", Value::Bool(st.mode_available));
        set(
            "energy-saver",
            Value::String(
                match st.energy_saver {
                    Some(EnergySaver::On) => "on",
                    Some(EnergySaver::Off) => "off",
                    _ => "unavailable",
                }
                .into(),
            ),
        );
        set("error", Value::String(st.error.as_str().into()));
    }

    /// The built-in screen's brightness changed (power-setting notification): refresh the
    /// applet, and in replacement mode show the OSD (Explorer's isn't there).
    pub(crate) fn on_power_setting(&self, setting: fsh_win::power::PowerSetting) {
        self.power_cmd(power::Cmd::Refresh);
        if let fsh_win::power::PowerSetting::Brightness(level) = setting {
            tracing::debug!("brightness changed to {level}");
            let previous = self.services.last_brightness.replace(Some(level));
            if previous.is_some_and(|p| p != level) && crate::osd::media_keys_wanted(self.safe_mode) {
                self.show_osd_level(crate::osd::OsdKind::Brightness, f64::from(level), false);
            }
        }
    }

    fn apply_bluetooth(&self, i: &ComponentInstance) {
        let st = self.services.bluetooth_state.borrow();
        let set = |name: &str, v: Value| {
            let _ = i.set_global_property("Bluetooth", name, v);
        };
        set("available", Value::Bool(st.available));
        set("present", Value::Bool(st.present));
        set("enabled", Value::Bool(st.enabled));
        set("connected-count", Value::Number(st.devices.iter().filter(|d| d.connected).count() as f64));
        set("error", Value::String(st.error.as_str().into()));
    }

    /// For `fsh-ctl shell dump_state`.
    pub(crate) fn services_state(&self) -> serde_json::Value {
        let s = &self.services;
        let a = s.audio_state.borrow();
        let m = s.media_state.borrow();
        let n = s.network_state.borrow();
        let b = s.bluetooth_state.borrow();
        let p = s.power_state.borrow();
        serde_json::json!({
            "audio": {
                "running": s.audio.borrow().is_some(),
                "available": a.available,
                "volume": a.volume,
                "muted": a.muted,
                "output": a.output_name,
                "outputs": a.outputs.iter().map(|d| &d.name).collect::<Vec<_>>(),
                "input": a.input_name,
                "apps": a.apps.iter().map(|x| serde_json::json!({"name": x.name, "volume": x.volume, "muted": x.muted})).collect::<Vec<_>>(),
            },
            "media": {
                "running": s.media.borrow().is_some(),
                "available": m.available,
                "title": m.now.as_ref().map(|n| &n.title),
                "artist": m.now.as_ref().map(|n| &n.artist),
                "app": m.now.as_ref().map(|n| &n.app_id),
                "playing": m.now.as_ref().is_some_and(|n| n.playing),
                "art": m.art.is_some(),
            },
            "network": {
                "running": s.network.borrow().is_some(),
                "available": n.available,
                "wifi_present": n.wifi_present,
                "wifi_enabled": n.wifi_enabled,
                "airplane_mode": n.airplane_mode,
                "connected": n.connected,
                "limited": n.limited,
                "kind": match n.kind { network::Kind::Wifi => "wifi", network::Kind::Ethernet => "ethernet", network::Kind::None => "none" },
                "ssid": n.ssid,
                "bars": n.bars,
                "ethernet": n.ethernet_name,
                "networks": n.networks.iter().map(|w| serde_json::json!({"ssid": w.ssid, "security": w.security, "bars": w.bars, "connected": w.connected, "saved": w.saved})).collect::<Vec<_>>(),
                "vpns": n.vpns.iter().map(|v| &v.name).collect::<Vec<_>>(),
                "error": n.error,
            },
            "bluetooth": {
                "running": s.bluetooth.borrow().is_some(),
                "available": b.available,
                "present": b.present,
                "enabled": b.enabled,
                "devices": b.devices.iter().map(|d| serde_json::json!({"name": d.name, "kind": d.kind, "status": d.status, "can_connect": d.can_connect})).collect::<Vec<_>>(),
                "error": b.error,
            },
            "power": {
                "running": s.power.borrow().is_some(),
                "available": p.available,
                "has_battery": p.has_battery,
                "percent": p.percent,
                "charging": p.charging,
                "on_ac": p.on_ac,
                "status": p.status,
                "health": p.health,
                "power_mode": p.power_mode.map(|m| format!("{m:?}")),
                "mode_available": p.mode_available,
                "energy_saver": p.energy_saver.map(|e| format!("{e:?}")),
                "displays": p.displays.iter().map(|d| serde_json::json!({"name": d.name, "brightness": d.brightness, "internal": d.internal})).collect::<Vec<_>>(),
                "error": p.error,
            },
            "notifications": self.notifications_state(),
        })
    }
}
