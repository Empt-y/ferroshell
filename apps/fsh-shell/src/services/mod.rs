//! System services behind `@ferroshell/services.slint` (`Audio`, `Media`, ...).
//!
//! Each service owns its Windows APIs on a worker thread, sends snapshots here, and takes
//! commands over a channel. A service only runs while some widget on a panel lists it in
//! `services = [...]`. A failing service shows as `available: false`; it never takes the
//! shell down.

pub mod audio;
pub mod media;

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
}

#[derive(Default)]
pub struct Services {
    audio: RefCell<Option<audio::AudioService>>,
    media: RefCell<Option<media::MediaService>>,
    audio_state: RefCell<audio::Snapshot>,
    media_state: RefCell<media::Snapshot>,
    art: RefCell<Option<Image>>,
    icons: RefCell<HashMap<String, Image>>,
    // Shared by every instance and updated in place, so a slider being dragged in a list
    // isn't recreated under the pointer.
    outputs: Rc<VecModel<Value>>,
    inputs: Rc<VecModel<Value>>,
    apps: Rc<VecModel<Value>>,
}

type GlobalCallback = Box<dyn Fn(&[Value]) -> Value>;

fn post(u: Update) {
    let _ = slint::invoke_from_event_loop(move || {
        with(|a| a.on_service_update(u));
    });
}

fn strukt(fields: Vec<(&str, Value)>) -> Value {
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
fn sync_model(model: &VecModel<Value>, rows: Vec<Value>) {
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
        for unknown in wanted.iter().filter(|w| !["audio", "media"].contains(&w.as_str())) {
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
        let s = &self.services;
        let set = |global: &str, name: &str, v: Value| {
            if let Err(e) = instance.set_global_property(global, name, v) {
                tracing::warn!("{global}.{name}: {e}");
            }
        };
        set("Audio", "outputs", Value::Model(ModelRc::from(s.outputs.clone())));
        set("Audio", "inputs", Value::Model(ModelRc::from(s.inputs.clone())));
        set("Audio", "apps", Value::Model(ModelRc::from(s.apps.clone())));
        self.apply_audio(instance);
        self.apply_media(instance);
    }

    /// Every instance showing service data: panels and compiled popups.
    fn for_each_instance(&self, f: &dyn Fn(&ComponentInstance)) {
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

    /// For `fsh-ctl shell dump_state`.
    pub(crate) fn services_state(&self) -> serde_json::Value {
        let s = &self.services;
        let a = s.audio_state.borrow();
        let m = s.media_state.borrow();
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
        })
    }
}
