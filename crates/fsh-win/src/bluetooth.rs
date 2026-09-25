//! Bluetooth: the radio, paired devices (classic and LE) with connection state and battery
//! where Windows knows it, and connect/disconnect for audio devices.
//!
//! Connecting is only offered for audio devices, through the in-box Bluetooth audio
//! drivers' "one-shot reconnect/disconnect" kernel-streaming property — what Windows' own
//! quick settings uses. Headphones sit paired but idle until asked; keyboards, mice and
//! controllers reconnect by themselves when used, so there's nothing to do for them. We
//! never toggle a device's Bluetooth *services* (`BluetoothSetServiceState`): that
//! uninstalls and reinstalls its drivers.
//!
//! WinRT calls block on async operations: use from a worker thread in a multi-threaded
//! COM apartment ([`crate::com::ComGuard::mta`]).

use anyhow::Context as _;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::IPropertyValue;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_ContainerId;
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATEMASK_ALL, IConnector, IDeviceTopology, IMMDeviceEnumerator, IPart, MMDeviceEnumerator, eAll,
};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT, KSPROPERTY_ONESHOT_RECONNECT,
    KSPROPERTY_TYPE_GET, KSPROPSETID_BtAudio,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToGUID;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ};
use windows::core::{GUID, HSTRING, IInspectable, Interface};
use windows_collections::IIterable;

/// Windows' battery level for Bluetooth devices (`DEVPKEY_Bluetooth_Battery`), set on the
/// device's PnP node by the in-box hands-free and LE battery drivers.
const BATTERY_PROPERTY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const CONTAINER_PROPERTY: &str = "System.Devices.Aep.ContainerId";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    /// WinRT device id (stable while paired).
    pub id: String,
    pub name: String,
    pub address: u64,
    pub connected: bool,
    /// Bluetooth LE (otherwise classic).
    pub le: bool,
    /// Classic devices' major class (see `fsh_core::bluetooth::device_kind`).
    pub major_class: Option<i32>,
    /// LE devices' appearance category.
    pub le_category: Option<u16>,
    /// Links the device to its other Windows faces (PnP nodes, audio endpoints).
    pub container: Option<GUID>,
}

fn radio() -> Option<Radio> {
    Radio::GetRadiosAsync()
        .and_then(|op| op.join())
        .ok()?
        .into_iter()
        .find(|r| r.Kind().ok() == Some(RadioKind::Bluetooth))
}

/// Is the Bluetooth radio on? `None` without Bluetooth hardware.
pub fn radio_on() -> Option<bool> {
    radio().and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
}

pub fn set_radio(on: bool) -> anyhow::Result<()> {
    let r = radio().context("no Bluetooth radio")?;
    let status = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|op| op.join())?;
    // RadioAccessStatus::Allowed == 1
    anyhow::ensure!(status.0 == 1, "Windows refused to switch Bluetooth (access status {})", status.0);
    Ok(())
}

fn guid_property(info: &DeviceInformation, name: &str) -> Option<GUID> {
    let value: IInspectable = info.Properties().ok()?.Lookup(&HSTRING::from(name)).ok()?;
    value.cast::<IPropertyValue>().ok()?.GetGuid().ok()
}

fn find_all(selector: &HSTRING, extra: &[&str]) -> Vec<DeviceInformation> {
    let props: Vec<HSTRING> = extra.iter().map(|p| HSTRING::from(*p)).collect();
    let props = IIterable::<HSTRING>::from(props);
    DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(selector, &props)
        .and_then(|op| op.join())
        .map(|c| c.into_iter().collect())
        .unwrap_or_default()
}

/// Paired devices, classic and LE, sorted connected-first then by name. Devices paired
/// over both transports (e.g. some headsets) are listed once.
pub fn paired_devices() -> Vec<BtDevice> {
    let mut out: Vec<BtDevice> = Vec::new();
    if let Ok(selector) = BluetoothDevice::GetDeviceSelectorFromPairingState(true) {
        for info in find_all(&selector, &[CONTAINER_PROPERTY]) {
            let Ok(id) = info.Id() else { continue };
            let Ok(dev) = BluetoothDevice::FromIdAsync(&id).and_then(|op| op.join()) else { continue };
            out.push(BtDevice {
                id: id.to_string(),
                name: dev.Name().map(|n| n.to_string()).unwrap_or_default(),
                address: dev.BluetoothAddress().unwrap_or_default(),
                connected: dev.ConnectionStatus().ok() == Some(BluetoothConnectionStatus::Connected),
                le: false,
                major_class: dev.ClassOfDevice().and_then(|c| c.MajorClass()).ok().map(|m| m.0),
                le_category: None,
                container: guid_property(&info, CONTAINER_PROPERTY),
            });
        }
    }
    if let Ok(selector) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true) {
        for info in find_all(&selector, &[CONTAINER_PROPERTY]) {
            let Ok(id) = info.Id() else { continue };
            let Ok(dev) = BluetoothLEDevice::FromIdAsync(&id).and_then(|op| op.join()) else { continue };
            let address = dev.BluetoothAddress().unwrap_or_default();
            let connected = dev.ConnectionStatus().ok() == Some(BluetoothConnectionStatus::Connected);
            if let Some(existing) = out.iter_mut().find(|d| d.address == address && address != 0) {
                existing.connected |= connected;
                continue;
            }
            out.push(BtDevice {
                id: id.to_string(),
                name: dev.Name().map(|n| n.to_string()).unwrap_or_default(),
                address,
                connected,
                le: true,
                major_class: None,
                le_category: dev.Appearance().and_then(|a| a.Category()).ok(),
                container: guid_property(&info, CONTAINER_PROPERTY),
            });
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// The battery level Windows shows for a device (0–100), if any of its PnP nodes has one.
pub fn battery(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let props = IIterable::<HSTRING>::from(vec![HSTRING::from(BATTERY_PROPERTY)]);
    let nodes = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device)
        .and_then(|op| op.join())
        .ok()?;
    nodes.into_iter().find_map(|n| {
        let v: IInspectable = n.Properties().ok()?.Lookup(&HSTRING::from(BATTERY_PROPERTY)).ok()?;
        v.cast::<IPropertyValue>().ok()?.GetUInt8().ok()
    })
}

/// Connect (or disconnect) a paired Bluetooth audio device: find its audio endpoints by
/// container id and send the Bluetooth audio driver's one-shot reconnect/disconnect.
/// Errors if the device has no audio endpoint, or its driver doesn't support the property.
pub fn audio_connect(container: GUID, connect: bool) -> anyhow::Result<()> {
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
    // Every state: a disconnected headset's endpoints are "unplugged", not active.
    let endpoints = unsafe { enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)) }?;
    let mut last_err: Option<anyhow::Error> = None;
    let mut matched = false;
    for i in 0..unsafe { endpoints.GetCount() }? {
        let Ok(dev) = (unsafe { endpoints.Item(i) }) else { continue };
        let Ok(store) = (unsafe { dev.OpenPropertyStore(STGM_READ) }) else { continue };
        let Ok(value) = (unsafe { store.GetValue(&PKEY_Device_ContainerId) }) else { continue };
        if unsafe { PropVariantToGUID(&value) }.ok() != Some(container) {
            continue;
        }
        matched = true;
        match send_oneshot(&dev, connect) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    match (matched, last_err) {
        (false, _) => anyhow::bail!("this device has no Bluetooth audio endpoint"),
        (true, Some(e)) => Err(e),
        (true, None) => anyhow::bail!("no endpoint accepted the request"),
    }
}

fn send_oneshot(dev: &windows::Win32::Media::Audio::IMMDevice, connect: bool) -> anyhow::Result<()> {
    unsafe {
        let topology: IDeviceTopology = dev.Activate(CLSCTX_ALL, None).context("IDeviceTopology")?;
        let connector: IConnector = topology.GetConnector(0).context("GetConnector")?;
        let other: IConnector = connector.GetConnectedTo().context("GetConnectedTo")?;
        let part: IPart = other.cast().context("IPart")?;
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        part.Activate(CLSCTX_ALL.0, &IKsControl::IID, Some(&mut raw)).context("IKsControl")?;
        let ks = IKsControl::from_raw(raw);
        let id = if connect { KSPROPERTY_ONESHOT_RECONNECT } else { KSPROPERTY_ONESHOT_DISCONNECT };
        let prop = KSIDENTIFIER {
            Anonymous: KSIDENTIFIER_0 { Anonymous: KSIDENTIFIER_0_0 { Set: KSPROPSETID_BtAudio, Id: id.0 as u32, Flags: KSPROPERTY_TYPE_GET } },
        };
        let mut returned = 0u32;
        ks.KsProperty(&prop, std::mem::size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned)
            .context("the Bluetooth audio driver rejected the request")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: the radio's state and paired devices with battery levels.
    #[test]
    #[ignore = "reads the live Bluetooth state"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::mta();
        println!("radio on: {:?}", radio_on());
        // `paired_devices` hides errors; make sure "none" really means none.
        for selector in [BluetoothDevice::GetDeviceSelectorFromPairingState(true).unwrap(), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).unwrap()] {
            let found = DeviceInformation::FindAllAsyncAqsFilter(&selector).and_then(|op| op.join()).expect("paired-device query failed");
            println!("query ok: {} paired", found.Size().unwrap());
        }
        // The battery query's AQS syntax: this PC's own container matches many PnP nodes.
        let this_pc = GUID::from_u128(0x00000000_0000_0000_ffff_ffffffffffff);
        let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{this_pc:?}}}\""));
        let props = IIterable::<HSTRING>::from(vec![HSTRING::from(BATTERY_PROPERTY)]);
        let nodes = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device)
            .and_then(|op| op.join())
            .expect("battery query syntax");
        println!("container query ok: {} nodes ({aqs})", nodes.Size().unwrap());
        assert!(nodes.Size().unwrap() > 0);
        // `audio_connect` matches endpoints by container id: check those are readable.
        unsafe {
            let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let all = e.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)).unwrap();
            let n = all.GetCount().unwrap();
            let with_container = (0..n)
                .filter_map(|i| all.Item(i).ok())
                .filter_map(|d| d.OpenPropertyStore(STGM_READ).ok())
                .filter(|s| s.GetValue(&PKEY_Device_ContainerId).is_ok_and(|v| PropVariantToGUID(&v).is_ok()))
                .count();
            println!("audio endpoints: {n}, with a container id: {with_container}");
            assert!(with_container > 0);
        }
        for d in paired_devices() {
            let battery = d.container.and_then(battery);
            println!("{d:?} battery={battery:?}");
        }
    }
}
