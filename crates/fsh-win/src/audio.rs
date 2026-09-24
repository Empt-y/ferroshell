//! Core Audio: endpoints (output/input devices), their volume, per-app sessions, and
//! switching the default device.
//!
//! Everything here is COM, so an [`Audio`] belongs to the thread that made it (with COM
//! initialised, see [`crate::com::ComGuard`]).

use anyhow::Context as _;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, DEVICE_STATE_ACTIVE, EDataFlow, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eCapture, eCommunications,
    eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::{GUID, Interface, PCWSTR, PWSTR};

use crate::wide;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Output,
    Input,
}

impl Flow {
    fn edataflow(self) -> EDataFlow {
        match self {
            Flow::Output => eRender,
            Flow::Input => eCapture,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Endpoint id (stable across reboots).
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Volume state of one endpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// 0.0–1.0
    pub volume: f32,
    pub muted: bool,
}

/// A per-app audio session on the default output, merged by process.
#[derive(Debug, Clone, PartialEq)]
pub struct AppSession {
    /// 0 for the system sounds session.
    pub pid: u32,
    /// The session's display name, if the app set one.
    pub display_name: String,
    /// The process's executable, for its name and icon ("" for system sounds).
    pub exe: String,
    pub volume: f32,
    pub muted: bool,
    /// Currently playing (not just open).
    pub active: bool,
}

pub struct Audio {
    enumerator: IMMDeviceEnumerator,
}

fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn device_id(d: &IMMDevice) -> String {
    unsafe { d.GetId() }.map(take_pwstr).unwrap_or_default()
}

fn device_name(d: &IMMDevice) -> String {
    let Ok(store) = (unsafe { d.OpenPropertyStore(STGM_READ) }) else { return String::new() };
    unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.map(|v| v.to_string()).unwrap_or_default()
}

impl Audio {
    pub fn new() -> anyhow::Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.context("MMDeviceEnumerator")?;
        Ok(Self { enumerator })
    }

    fn default_device(&self, flow: Flow) -> Option<IMMDevice> {
        unsafe { self.enumerator.GetDefaultAudioEndpoint(flow.edataflow(), eMultimedia) }.ok()
    }

    fn device(&self, id: &str) -> Option<IMMDevice> {
        let w = wide(id);
        unsafe { self.enumerator.GetDevice(PCWSTR(w.as_ptr())) }.ok()
    }

    /// The default device's id ("" if there is none, e.g. no microphone).
    pub fn default_id(&self, flow: Flow) -> String {
        self.default_device(flow).map(|d| device_id(&d)).unwrap_or_default()
    }

    /// Active devices, sorted by name.
    pub fn devices(&self, flow: Flow) -> Vec<Device> {
        let default = self.default_id(flow);
        let Ok(coll) = (unsafe { self.enumerator.EnumAudioEndpoints(flow.edataflow(), DEVICE_STATE_ACTIVE) }) else {
            return vec![];
        };
        let n = unsafe { coll.GetCount() }.unwrap_or(0);
        let mut out: Vec<Device> = (0..n)
            .filter_map(|i| unsafe { coll.Item(i) }.ok())
            .map(|d| {
                let id = device_id(&d);
                Device { default: id == default, name: device_name(&d), id }
            })
            .collect();
        out.sort_by_key(|d| d.name.to_lowercase());
        out
    }

    /// The volume control of a device (the default one for `None`). Keep it and read it
    /// often; activating is the expensive part.
    pub fn endpoint(&self, flow: Flow, id: Option<&str>) -> Option<Endpoint> {
        let device = match id {
            Some(id) => self.device(id)?,
            None => self.default_device(flow)?,
        };
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }.ok()?;
        Some(Endpoint { id: device_id(&device), volume })
    }

    /// Per-app sessions on the default output, one per process (an app with several streams
    /// shows as one). Expired sessions are left out.
    pub fn sessions(&self) -> Vec<AppSession> {
        let Some(device) = self.default_device(Flow::Output) else { return vec![] };
        let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else { return vec![] };
        let Ok(list) = (unsafe { manager.GetSessionEnumerator() }) else { return vec![] };
        let n = unsafe { list.GetCount() }.unwrap_or(0);
        let mut out: Vec<AppSession> = Vec::new();
        for i in 0..n {
            let Ok(control) = (unsafe { list.GetSession(i) }) else { continue };
            let Ok(c2) = control.cast::<IAudioSessionControl2>() else { continue };
            let Ok(state) = (unsafe { c2.GetState() }) else { continue };
            if state == AudioSessionStateExpired {
                continue;
            }
            let system = unsafe { c2.IsSystemSoundsSession() } == S_OK;
            let pid = if system { 0 } else { unsafe { c2.GetProcessId() }.unwrap_or(0) };
            if !system && pid == 0 {
                continue;
            }
            let Ok(simple) = control.cast::<ISimpleAudioVolume>() else { continue };
            let volume = unsafe { simple.GetMasterVolume() }.unwrap_or(1.0);
            let muted = unsafe { simple.GetMute() }.map(|b| b.as_bool()).unwrap_or(false);
            let active = state == AudioSessionStateActive;
            if let Some(existing) = out.iter_mut().find(|s| s.pid == pid) {
                existing.active |= active;
                continue;
            }
            let display_name = unsafe { c2.GetDisplayName() }.map(take_pwstr).unwrap_or_default();
            // "@%SystemRoot%\..." resource strings aren't useful as names.
            let display_name = if display_name.starts_with('@') { String::new() } else { display_name };
            let exe = if system { String::new() } else { crate::winfo::process_path(pid).unwrap_or_default() };
            out.push(AppSession { pid, display_name, exe, volume, muted, active });
        }
        out
    }

    /// Set (or mute) every session of process `pid` (0: system sounds).
    pub fn set_session(&self, pid: u32, volume: Option<f32>, muted: Option<bool>) {
        let Some(device) = self.default_device(Flow::Output) else { return };
        let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else { return };
        let Ok(list) = (unsafe { manager.GetSessionEnumerator() }) else { return };
        let n = unsafe { list.GetCount() }.unwrap_or(0);
        for i in 0..n {
            let Ok(control) = (unsafe { list.GetSession(i) }) else { continue };
            let Ok(c2) = control.cast::<IAudioSessionControl2>() else { continue };
            let system = unsafe { c2.IsSystemSoundsSession() } == S_OK;
            let this_pid = if system { 0 } else { unsafe { c2.GetProcessId() }.unwrap_or(0) };
            if this_pid != pid || (pid == 0 && !system) {
                continue;
            }
            let Ok(simple) = control.cast::<ISimpleAudioVolume>() else { continue };
            unsafe {
                if let Some(v) = volume {
                    let _ = simple.SetMasterVolume(v.clamp(0.0, 1.0), &GUID::zeroed());
                }
                if let Some(m) = muted {
                    let _ = simple.SetMute(m, &GUID::zeroed());
                }
            }
        }
    }

    /// Make `id` the default device for every role, like choosing it in Windows' sound
    /// settings. Uses the undocumented (but long-stable) `IPolicyConfig`.
    pub fn set_default(&self, id: &str) -> anyhow::Result<()> {
        let config: IPolicyConfig = unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.context("PolicyConfigClient")?;
        let w = wide(id);
        for role in [eConsole, eMultimedia, eCommunications] {
            policy::set_default_endpoint(&config, PCWSTR(w.as_ptr()), role).ok().context("SetDefaultEndpoint")?;
        }
        Ok(())
    }
}

/// One device's volume control.
pub struct Endpoint {
    pub id: String,
    volume: IAudioEndpointVolume,
}

impl Endpoint {
    pub fn level(&self) -> Option<Level> {
        let volume = unsafe { self.volume.GetMasterVolumeLevelScalar() }.ok()?;
        let muted = unsafe { self.volume.GetMute() }.ok()?.as_bool();
        Some(Level { volume, muted })
    }

    pub fn set_volume(&self, v: f32) {
        let _ = unsafe { self.volume.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), &GUID::zeroed()) };
    }

    pub fn set_muted(&self, m: bool) {
        let _ = unsafe { self.volume.SetMute(m, &GUID::zeroed()) };
    }
}

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

// The Windows 7+ layout, as used by EarTrumpet and SoundSwitch. Only SetDefaultEndpoint
// is called; the rest just fill the vtable.
mod policy {
    #![allow(non_snake_case)]
    use core::ffi::c_void;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::ERole;
    use windows::core::{HRESULT, PCWSTR};

    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: windows_core::IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, default_period: *mut i64, min_period: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(&self, device: PCWSTR, store: i32, key: *const PROPERTYKEY, value: *mut c_void) -> HRESULT;
        fn SetPropertyValue(&self, device: PCWSTR, store: i32, key: *const PROPERTYKEY, value: *mut c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    // The generated methods are private to this module.
    pub fn set_default_endpoint(p: &IPolicyConfig, device: PCWSTR, role: ERole) -> HRESULT {
        unsafe { p.SetDefaultEndpoint(device, role) }
    }
}
use policy::IPolicyConfig;

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads (never changes) the machine's audio setup.
    #[test]
    #[ignore = "reads the live audio devices"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::new();
        let audio = Audio::new().unwrap();
        let outputs = audio.devices(Flow::Output);
        println!("outputs: {outputs:#?}");
        println!("inputs: {:#?}", audio.devices(Flow::Input));
        assert!(outputs.iter().filter(|d| d.default).count() <= 1);
        if let Some(ep) = audio.endpoint(Flow::Output, None) {
            println!("default output level: {:?}", ep.level());
        }
        println!("sessions: {:#?}", audio.sessions());
    }
}
