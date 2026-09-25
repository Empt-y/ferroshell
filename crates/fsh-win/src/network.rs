//! Networking: Wi-Fi (Native Wifi/WLAN API), wired Ethernet, overall internet
//! connectivity, radios (Wi-Fi on/off, airplane mode) and active VPN connections.
//!
//! The `Wlan` handle is COM-free but still per-thread by convention (`WlanOpenHandle` isn't
//! documented as thread-safe); keep one on the network service's worker thread.

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{GET_ADAPTERS_ADDRESSES_FLAGS, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::Rras::{HRASCONN, RASCONNW, RasEnumConnectionsW, RasHangUpW};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_AUTH_ALGO_80211_OPEN, DOT11_AUTH_ALGO_80211_SHARED_KEY, DOT11_AUTH_ALGO_RSNA, DOT11_AUTH_ALGO_RSNA_PSK,
    DOT11_AUTH_ALGO_WPA, DOT11_AUTH_ALGO_WPA3, DOT11_AUTH_ALGO_WPA3_ENT, DOT11_AUTH_ALGO_WPA3_SAE, DOT11_AUTH_ALGO_WPA_NONE,
    DOT11_AUTH_ALGO_WPA_PSK, WLAN_AVAILABLE_NETWORK, WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_LIST,
    WLAN_CONNECTION_ATTRIBUTES, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WLAN_PROFILE_GET_PLAINTEXT_KEY,
    WlanCloseHandle, WlanConnect, WlanDeleteProfile, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory,
    WlanGetAvailableNetworkList, WlanOpenHandle, WlanQueryInterface, WlanScan, WlanSetProfile, dot11_BSS_type_infrastructure,
    wlan_connection_mode_profile, wlan_interface_state_connected, wlan_intf_opcode_current_connection,
};
use windows::core::{GUID, PCWSTR, PWSTR};

use crate::wide;

/// A network's authentication, as far as the applet cares. `fsh_core::network` (which may
/// depend on this crate, not the other way round) builds a connection profile from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    Wep,
    Wpa,
    Wpa2,
    Wpa3,
    /// 802.1X or anything else we don't build a profile for: connecting opens Windows'
    /// network settings instead.
    Enterprise,
}

/// A visible Wi-Fi interface (most machines have exactly one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interface {
    pub guid: GUID,
}

/// A network from a scan, or the one currently connected.
#[derive(Debug, Clone, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// The SSID's raw bytes as hex, for building a profile without retyping the name.
    pub ssid_hex: String,
    /// 0–100.
    pub signal: u32,
    pub security: Security,
    pub connected: bool,
    /// A saved profile exists: connecting doesn't need a password.
    pub has_profile: bool,
    pub profile_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthernetStatus {
    pub name: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    None,
    LocalOnly,
    Limited,
    Internet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpnConnection {
    pub name: String,
    handle: isize,
}

/// UTF-8/raw bytes as uppercase hex, the form a WLAN profile's `<hex>` element wants.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() { String::new() } else { unsafe { p.to_string() }.unwrap_or_default() }
}

fn wstr(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn security_of(net: &WLAN_AVAILABLE_NETWORK) -> Security {
    if !net.bSecurityEnabled.as_bool() {
        return Security::Open;
    }
    match net.dot11DefaultAuthAlgorithm {
        DOT11_AUTH_ALGO_80211_OPEN | DOT11_AUTH_ALGO_WPA_NONE => Security::Open,
        DOT11_AUTH_ALGO_80211_SHARED_KEY => Security::Wep,
        DOT11_AUTH_ALGO_WPA | DOT11_AUTH_ALGO_WPA_PSK => Security::Wpa,
        DOT11_AUTH_ALGO_WPA3 | DOT11_AUTH_ALGO_WPA3_SAE => Security::Wpa3,
        DOT11_AUTH_ALGO_RSNA_PSK => Security::Wpa2,
        DOT11_AUTH_ALGO_RSNA | DOT11_AUTH_ALGO_WPA3_ENT => Security::Enterprise,
        _ => Security::Enterprise,
    }
}

/// A handle to the WLAN service. One per thread; not `Send`.
pub struct Wlan {
    handle: HANDLE,
}

impl Wlan {
    pub fn open() -> anyhow::Result<Self> {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        let rc = unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) };
        if rc != ERROR_SUCCESS.0 {
            anyhow::bail!("WlanOpenHandle failed ({rc})");
        }
        Ok(Self { handle })
    }

    /// The machine's Wi-Fi interfaces (usually one, none on a desktop with no Wi-Fi card).
    pub fn interfaces(&self) -> Vec<Interface> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let rc = unsafe { WlanEnumInterfaces(self.handle, None, &mut list) };
        if rc != ERROR_SUCCESS.0 || list.is_null() {
            return vec![];
        }
        let out = unsafe {
            let l = &*list;
            std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize)
                .iter()
                .map(|i| Interface { guid: i.InterfaceGuid })
                .collect()
        };
        unsafe { WlanFreeMemory(list.cast()) };
        out
    }

    /// Kick off a scan; results land in [`Wlan::available_networks`] a moment later (Windows
    /// doesn't say how long — the service just polls).
    pub fn scan(&self, iface: Interface) {
        unsafe {
            let _ = WlanScan(self.handle, &iface.guid, None, None, None);
        }
    }

    /// The SSID and signal quality Windows is actually associated with right now, if any.
    /// More reliable than `WLAN_AVAILABLE_NETWORK_CONNECTED` in the scan list, which some
    /// drivers leave unset on the currently-connected entry.
    pub fn current_connection(&self, iface: Interface) -> Option<(String, u32)> {
        let mut size = 0u32;
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        let rc = unsafe { WlanQueryInterface(self.handle, &iface.guid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) };
        if rc != ERROR_SUCCESS.0 || data.is_null() {
            return None;
        }
        let result = unsafe {
            let attrs = &*data.cast::<WLAN_CONNECTION_ATTRIBUTES>();
            (attrs.isState == wlan_interface_state_connected).then(|| {
                let assoc = &attrs.wlanAssociationAttributes;
                let ssid_bytes = &assoc.dot11Ssid.ucSSID[..(assoc.dot11Ssid.uSSIDLength as usize).min(32)];
                (String::from_utf8_lossy(ssid_bytes).into_owned(), assoc.wlanSignalQuality)
            })
        };
        unsafe { WlanFreeMemory(data) };
        result
    }

    pub fn available_networks(&self, iface: Interface) -> Vec<WifiNetwork> {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let rc = unsafe { WlanGetAvailableNetworkList(self.handle, &iface.guid, 0, None, &mut list) };
        if rc != ERROR_SUCCESS.0 || list.is_null() {
            return vec![];
        }
        let mut networks = unsafe {
            let l = &*list;
            std::slice::from_raw_parts(l.Network.as_ptr(), l.dwNumberOfItems as usize)
                .iter()
                .map(|n| {
                    let ssid_bytes = &n.dot11Ssid.ucSSID[..(n.dot11Ssid.uSSIDLength as usize).min(32)];
                    WifiNetwork {
                        ssid: String::from_utf8_lossy(ssid_bytes).into_owned(),
                        ssid_hex: hex_encode(ssid_bytes),
                        signal: n.wlanSignalQuality,
                        security: security_of(n),
                        connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                        has_profile: n.strProfileName[0] != 0,
                        profile_name: wstr(&n.strProfileName),
                    }
                })
                // Windows can list the same SSID once per BSSID/profile; keep the
                // strongest and prefer whichever entry already has a saved profile.
                .fold(Vec::<WifiNetwork>::new(), |mut acc, n| {
                    match acc.iter_mut().find(|e| e.ssid == n.ssid) {
                        Some(existing) if n.has_profile || n.signal > existing.signal => *existing = n,
                        Some(_) => {}
                        None => acc.push(n),
                    }
                    acc
                })
        };
        unsafe { WlanFreeMemory(list.cast()) };
        if let Some((ssid, quality)) = self.current_connection(iface) {
            match networks.iter_mut().find(|n| n.ssid == ssid) {
                Some(n) => {
                    n.connected = true;
                    n.signal = quality;
                }
                // Connected to a network the last scan didn't happen to include (e.g. it
                // dropped out of range briefly): still show it as connected.
                None if !ssid.is_empty() => networks.push(WifiNetwork {
                    ssid: ssid.clone(),
                    ssid_hex: hex_encode(ssid.as_bytes()),
                    signal: quality,
                    security: Security::Wpa2,
                    connected: true,
                    has_profile: true,
                    profile_name: ssid,
                }),
                None => {}
            }
        }
        networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
        networks
    }

    /// Connect using an already-saved profile (an existing network, or one just created by
    /// [`Wlan::set_profile`]).
    pub fn connect_profile(&self, iface: Interface, profile_name: &str) -> anyhow::Result<()> {
        let name = wide(profile_name);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(name.as_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        let rc = unsafe { WlanConnect(self.handle, &iface.guid, &params, None) };
        (rc == ERROR_SUCCESS.0).then_some(()).ok_or_else(|| anyhow::anyhow!("WlanConnect failed ({rc})"))
    }

    /// Save a connection profile (built by `fsh_core::network::wifi_profile_xml`) so
    /// [`Wlan::connect_profile`] can use it.
    pub fn set_profile(&self, iface: Interface, profile_xml: &str) -> anyhow::Result<()> {
        let xml_w = wide(profile_xml);
        let mut reason = 0u32;
        let rc = unsafe { WlanSetProfile(self.handle, &iface.guid, WLAN_PROFILE_GET_PLAINTEXT_KEY, PCWSTR(xml_w.as_ptr()), None, true, None, &mut reason) };
        if rc != ERROR_SUCCESS.0 {
            anyhow::bail!("WlanSetProfile failed ({rc}, reason {reason}) — check the password");
        }
        Ok(())
    }

    pub fn disconnect(&self, iface: Interface) {
        unsafe {
            let _ = WlanDisconnect(self.handle, &iface.guid, None);
        }
    }

    pub fn forget(&self, iface: Interface, profile_name: &str) {
        let name = wide(profile_name);
        unsafe {
            let _ = WlanDeleteProfile(self.handle, &iface.guid, PCWSTR(name.as_ptr()), None);
        }
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe {
            let _ = WlanCloseHandle(self.handle, None);
        }
    }
}

// SAFETY-adjacent: the handle is a kernel-object-backed WLAN client handle; Microsoft's own
// docs example uses it from whichever thread issues the call, and we only ever touch it
// from the network service's single worker thread at a time.
unsafe impl Send for Wlan {}

/// Ethernet adapters that are actually up, by IANA ifType (6 = ethernetCsmacd). This misses
/// some virtualised NICs Windows also reports as type 6; good enough for "is a cable
/// plugged in", which is all the applet needs.
pub fn ethernet_status() -> Option<EthernetStatus> {
    const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
    const AF_UNSPEC: u32 = 0;

    let mut size = 0u32;
    unsafe { GetAdaptersAddresses(AF_UNSPEC, GET_ADAPTERS_ADDRESSES_FLAGS(0), None, None, &mut size) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe { GetAdaptersAddresses(AF_UNSPEC, GET_ADAPTERS_ADDRESSES_FLAGS(0), None, Some(buf.as_mut_ptr().cast()), &mut size) };
    if rc != ERROR_SUCCESS.0 {
        return None;
    }
    let mut cur: *const IP_ADAPTER_ADDRESSES_LH = buf.as_ptr().cast();
    let mut best: Option<EthernetStatus> = None;
    while !cur.is_null() {
        let a = unsafe { &*cur };
        if a.IfType == IF_TYPE_ETHERNET_CSMACD {
            let up = a.OperStatus == IfOperStatusUp;
            let name = take_pwstr(a.FriendlyName);
            if up || best.is_none() {
                best = Some(EthernetStatus { name, connected: up });
            }
            if up {
                break;
            }
        }
        cur = a.Next;
    }
    best
}

/// Active VPN (RAS) connections. Windows' VPN entries dial through `rasphone.exe`/Settings
/// (stored credentials, MFA, ...); we only report status and let the applet disconnect,
/// pointing to Windows' own settings to connect.
pub fn vpn_connections() -> Vec<VpnConnection> {
    let mut size = std::mem::size_of::<RASCONNW>() as u32;
    let mut count = 0u32;
    let mut buf = vec![0u8; size as usize];
    unsafe { (*buf.as_mut_ptr().cast::<RASCONNW>()).dwSize = size };
    let mut rc = unsafe { RasEnumConnectionsW(Some(buf.as_mut_ptr().cast()), &mut size, &mut count) };
    // ERROR_BUFFER_TOO_SMALL: `size` now holds what's needed.
    if rc == 603 {
        buf = vec![0u8; size as usize];
        unsafe { (*buf.as_mut_ptr().cast::<RASCONNW>()).dwSize = std::mem::size_of::<RASCONNW>() as u32 };
        rc = unsafe { RasEnumConnectionsW(Some(buf.as_mut_ptr().cast()), &mut size, &mut count) };
    }
    if rc != 0 {
        return vec![];
    }
    let entries = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<RASCONNW>(), count as usize) };
    entries.iter().map(|c| VpnConnection { name: wstr(&c.szEntryName), handle: c.hrasconn.0 as isize }).collect()
}

pub fn vpn_disconnect(vpn: &VpnConnection) {
    unsafe {
        let _ = RasHangUpW(HRASCONN(vpn.handle as *mut core::ffi::c_void));
    }
}

/// Overall internet reachability, and whether the primary path is Wi-Fi. Needs a
/// multi-threaded COM apartment ([`crate::com::ComGuard::mta`]).
pub fn connectivity() -> (Connectivity, bool) {
    let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
        return (Connectivity::None, false);
    };
    let level = match profile.GetNetworkConnectivityLevel() {
        Ok(NetworkConnectivityLevel::InternetAccess) => Connectivity::Internet,
        Ok(NetworkConnectivityLevel::ConstrainedInternetAccess) => Connectivity::Limited,
        Ok(NetworkConnectivityLevel::LocalAccess) => Connectivity::LocalOnly,
        _ => Connectivity::None,
    };
    let is_wifi = profile.IsWlanConnectionProfile().unwrap_or(false);
    (level, is_wifi)
}

fn radios() -> Vec<Radio> {
    Radio::GetRadiosAsync().and_then(|op| op.join()).map(|v| v.into_iter().collect()).unwrap_or_default()
}

/// Is the Wi-Fi radio (if any) on? `None` when there's no Wi-Fi hardware.
pub fn wifi_radio_on() -> Option<bool> {
    radios().into_iter().find(|r| r.Kind().ok() == Some(RadioKind::WiFi)).and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
}

pub fn set_wifi_radio(on: bool) {
    for r in radios() {
        if r.Kind().ok() == Some(RadioKind::WiFi) {
            let _ = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|op| op.join());
        }
    }
}

/// Best-effort airplane mode: every radio off, or every radio that airplane mode turned off
/// back on. This mirrors what Windows' own quick-setting toggle does, without tracking each
/// radio's state from before airplane mode was turned on.
pub fn set_airplane_mode(on: bool) {
    for r in radios() {
        let _ = r.SetStateAsync(if on { RadioState::Off } else { RadioState::On }).and_then(|op| op.join());
    }
}

/// True while every known radio is off (the applet's best-effort read of airplane mode).
pub fn airplane_mode_on() -> bool {
    let all = radios();
    !all.is_empty() && all.iter().all(|r| r.State().ok() != Some(RadioState::On))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wstr_stops_at_the_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "hi".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wstr(&buf), "hi");
        assert_eq!(wstr(&[0u16; 4]), "");
    }

    /// Read-only: lists this machine's Wi-Fi interfaces, scans, and lists networks.
    #[test]
    #[ignore = "reads the live network state"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::mta();
        let wlan = Wlan::open().unwrap();
        let ifaces = wlan.interfaces();
        println!("interfaces: {ifaces:?}");
        for iface in &ifaces {
            wlan.scan(*iface);
            std::thread::sleep(std::time::Duration::from_secs(2));
            println!("networks on {iface:?}: {:#?}", wlan.available_networks(*iface));
        }
        println!("ethernet: {:?}", ethernet_status());
        println!("vpns: {:?}", vpn_connections());
        println!("connectivity: {:?}", connectivity());
        println!("wifi radio on: {:?}", wifi_radio_on());
        println!("airplane mode: {:?}", airplane_mode_on());
    }
}
