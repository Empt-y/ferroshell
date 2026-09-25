//! Screen brightness: built-in laptop panels through WMI (`WmiMonitorBrightness`, what
//! Settings' slider drives) and external monitors through DDC/CI (dxva2), for those that
//! support it.
//!
//! WMI is COM: keep a [`Brightness`] on one worker thread in a multi-threaded apartment
//! ([`crate::com::ComGuard::mta`]), in a process that called
//! [`crate::com::init_process_security`] (the brightness provider refuses calls otherwise).
//! DDC/CI calls are slow (tens of milliseconds): never on the UI thread.

use anyhow::Context as _;
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW, HMONITOR};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_I4, VT_UI1};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE,
    WBEM_INFINITE, WbemLocator,
};
use windows::core::{BSTR, PCWSTR, w};

use crate::wide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// `wmi:<instance name>` or `ddc:<\\.\DISPLAYn>#<index>`.
    pub id: String,
    pub name: String,
    pub internal: bool,
    /// 0–100
    pub brightness: u8,
}

/// The hardware id in a monitor's device path: `BOE08B3` in both
/// `DISPLAY\BOE08B3\4&15...` (WMI) and `MONITOR\BOE08B3\{4d36...}\0001` (display devices).
fn hardware_id(path: &str) -> Option<&str> {
    path.split('\\').nth(1).filter(|s| !s.is_empty())
}

fn variant(vt: VARENUM, set: impl FnOnce(&mut windows::Win32::System::Variant::VARIANT_0_0_0)) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = vt;
        set(&mut inner.Anonymous);
    }
    v
}

fn prop(obj: &IWbemClassObject, name: &str) -> Option<VARIANT> {
    let mut v = VARIANT::default();
    let n = wide(name);
    unsafe { obj.Get(PCWSTR(n.as_ptr()), 0, &mut v, None, None) }.ok()?;
    Some(v)
}

fn prop_string(obj: &IWbemClassObject, name: &str) -> Option<String> {
    prop(obj, name).and_then(|v| BSTR::try_from(&v).ok()).map(|b| b.to_string())
}

fn prop_u32(obj: &IWbemClassObject, name: &str) -> Option<u32> {
    prop(obj, name).and_then(|v| u32::try_from(&v).ok())
}

pub struct Brightness {
    wmi: Option<IWbemServices>,
}

impl Brightness {
    /// Never fails: without WMI brightness (desktops) only DDC/CI monitors are offered.
    pub fn new() -> Self {
        let wmi = (|| -> windows::core::Result<IWbemServices> {
            let locator: IWbemLocator = unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }?;
            unsafe { locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None) }
        })()
        .map_err(|e| tracing::debug!("WMI brightness unavailable: {e}"))
        .ok();
        Self { wmi }
    }

    fn query(&self, wql: &str) -> Vec<IWbemClassObject> {
        let Some(svc) = &self.wmi else { return vec![] };
        let Ok(rows) = (unsafe { svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(wql), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None) }) else {
            return vec![];
        };
        let mut out = Vec::new();
        loop {
            let mut row = [None];
            let mut n = 0u32;
            let _ = unsafe { rows.Next(WBEM_INFINITE, &mut row, &mut n) };
            match (n, row[0].take()) {
                (1, Some(obj)) => out.push(obj),
                _ => break,
            }
        }
        out
    }

    /// Built-in panels (WMI) first, then DDC/CI monitors.
    pub fn displays(&self) -> Vec<Display> {
        let mut out: Vec<Display> = self
            .query("SELECT InstanceName, CurrentBrightness, Active FROM WmiMonitorBrightness")
            .iter()
            .filter(|o| prop(o, "Active").and_then(|v| bool::try_from(&v).ok()).unwrap_or(true))
            .filter_map(|o| {
                let instance = prop_string(o, "InstanceName")?;
                let level = prop_u32(o, "CurrentBrightness")?;
                Some(Display { id: format!("wmi:{instance}"), name: "Built-in display".into(), internal: true, brightness: level.min(100) as u8 })
            })
            .collect();
        let builtin: Vec<String> = out.iter().filter_map(|d| hardware_id(d.id.trim_start_matches("wmi:")).map(str::to_owned)).collect();
        let multiple = out.len() > 1;
        if multiple {
            for (i, d) in out.iter_mut().enumerate() {
                d.name = format!("Built-in display {}", i + 1);
            }
        }
        for m in crate::monitor::monitors() {
            let (name, hwid) = monitor_device(&m.device).unwrap_or_default();
            if builtin.iter().any(|b| hwid.eq_ignore_ascii_case(b)) {
                continue;
            }
            with_physical_monitors(m.handle, |physical| {
                for (i, pm) in physical.iter().enumerate() {
                    if let Some(level) = ddc_get(pm) {
                        out.push(Display {
                            id: format!("ddc:{}#{i}", m.device),
                            name: if name.is_empty() { m.device.trim_start_matches(r"\\.\").to_owned() } else { name.clone() },
                            internal: false,
                            brightness: level,
                        });
                    }
                }
            });
        }
        out
    }

    pub fn set(&self, id: &str, percent: u8) -> anyhow::Result<()> {
        let percent = percent.min(100);
        if let Some(instance) = id.strip_prefix("wmi:") {
            return self.set_wmi(instance, percent);
        }
        let (device, index) = id.strip_prefix("ddc:").and_then(|r| r.rsplit_once('#')).context("unknown display id")?;
        let index: usize = index.parse().context("unknown display id")?;
        let m = crate::monitor::monitors().into_iter().find(|m| m.device == device).context("that display is no longer connected")?;
        let mut result = Err(anyhow::anyhow!("that display is no longer connected"));
        with_physical_monitors(m.handle, |physical| {
            if let Some(pm) = physical.get(index) {
                result = ddc_set(pm, percent);
            }
        });
        result
    }

    fn set_wmi(&self, instance: &str, percent: u8) -> anyhow::Result<()> {
        let svc = self.wmi.as_ref().context("WMI brightness is unavailable")?;
        let target = self
            .query("SELECT * FROM WmiMonitorBrightnessMethods")
            .into_iter()
            .find(|o| prop_string(o, "InstanceName").as_deref() == Some(instance))
            .context("that display no longer supports brightness control")?;
        let path = prop_string(&target, "__PATH").context("no WMI path")?;
        unsafe {
            let mut class = None;
            svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut class), None)?;
            let class: IWbemClassObject = class.context("no WmiMonitorBrightnessMethods class")?;
            let mut signature = None;
            class.GetMethod(w!("WmiSetBrightness"), 0, &mut signature, std::ptr::null_mut())?;
            let params = signature.context("no WmiSetBrightness signature")?.SpawnInstance(0)?;
            params.Put(w!("Timeout"), 0, &variant(VT_I4, |a| a.lVal = 0), 0)?;
            params.Put(w!("Brightness"), 0, &variant(VT_UI1, |a| a.bVal = percent), 0)?;
            svc.ExecMethod(&BSTR::from(path), &BSTR::from("WmiSetBrightness"), WBEM_GENERIC_FLAG_TYPE(0), None, &params, None, None)
                .context("WmiSetBrightness")?;
        }
        Ok(())
    }
}

impl Default for Brightness {
    fn default() -> Self {
        Self::new()
    }
}

/// A monitor's display name and hardware id, from its display device.
fn monitor_device(device: &str) -> Option<(String, String)> {
    let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    let name = wide(device);
    unsafe { EnumDisplayDevicesW(PCWSTR(name.as_ptr()), 0, &mut dd, 0) }.as_bool().then_some(())?;
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    let id = text(&dd.DeviceID);
    Some((text(&dd.DeviceString), hardware_id(&id).unwrap_or_default().to_owned()))
}

fn with_physical_monitors(hmonitor: isize, f: impl FnOnce(&[PHYSICAL_MONITOR])) {
    let h = HMONITOR(hmonitor as _);
    let mut n = 0u32;
    if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(h, &mut n) }.is_err() || n == 0 {
        return;
    }
    let mut physical = vec![PHYSICAL_MONITOR::default(); n as usize];
    if unsafe { GetPhysicalMonitorsFromHMONITOR(h, &mut physical) }.is_err() {
        return;
    }
    f(&physical);
    unsafe {
        let _ = DestroyPhysicalMonitors(&physical);
    }
}

fn ddc_get(pm: &PHYSICAL_MONITOR) -> Option<u8> {
    let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
    (unsafe { GetMonitorBrightness(pm.hPhysicalMonitor, &mut min, &mut cur, &mut max) } != 0 && max > min)
        .then(|| (((cur.clamp(min, max) - min) * 100 + (max - min) / 2) / (max - min)) as u8)
}

fn ddc_set(pm: &PHYSICAL_MONITOR, percent: u8) -> anyhow::Result<()> {
    let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
    anyhow::ensure!(unsafe { GetMonitorBrightness(pm.hPhysicalMonitor, &mut min, &mut cur, &mut max) } != 0 && max > min, "the monitor stopped answering DDC/CI");
    let value = min + (u32::from(percent) * (max - min) + 50) / 100;
    anyhow::ensure!(unsafe { SetMonitorBrightness(pm.hPhysicalMonitor, value) } != 0, "the monitor rejected the brightness change");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_ids_match_across_apis() {
        assert_eq!(hardware_id(r"DISPLAY\BOE08B3\4&155567f9&0&UID8388688_0"), Some("BOE08B3"));
        assert_eq!(hardware_id(r"MONITOR\BOE08B3\{4d36e96e-e325-11ce-bfc1-08002be10318}\0001"), Some("BOE08B3"));
        assert_eq!(hardware_id("nonsense"), None);
    }

    #[test]
    #[ignore = "reads the live brightness"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::mta();
        crate::com::init_process_security().expect("COM security");
        let displays = Brightness::new().displays();
        println!("{displays:#?}");
        assert!(!displays.is_empty(), "this laptop's built-in panel should be listed");
    }
}
