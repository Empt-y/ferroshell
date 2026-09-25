//! Battery, the Windows 11 "power mode" (the Balanced plan's overlay: best power
//! efficiency / balanced / best performance), energy saver status, and notifications when
//! any of those (or the built-in screen's brightness) change.
//!
//! WinRT calls need a multi-threaded COM apartment ([`crate::com::ComGuard::mta`]).

use windows::Devices::Power::Battery as WinRtBattery;
use windows::System::Power::{EnergySaverStatus, PowerManager};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, HPOWERNOTIFY, POWERBROADCAST_SETTING, PowerGetActiveScheme, RegisterPowerSettingNotification,
    SYSTEM_POWER_STATUS, UnregisterPowerSettingNotification,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_POWER_SAVING_STATUS, GUID_POWERSCHEME_PERSONALITY,
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_WINDOW_HANDLE;
use windows::core::GUID;

use crate::Hwnd;

pub use windows::Win32::UI::WindowsAndMessaging::{PBT_POWERSETTINGCHANGE, WM_POWERBROADCAST};

/// The built-in screen's brightness changed (e.g. Fn keys); the value is 0–100.
pub const BRIGHTNESS_CHANGED: GUID = GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// 0–100
    pub percent: u8,
    pub on_ac: bool,
    pub charging: bool,
    /// Windows' estimate while discharging.
    pub seconds_left: Option<u32>,
    /// Positive while charging, negative while discharging.
    pub charge_rate_mw: Option<i32>,
    pub remaining_mwh: Option<i32>,
    pub full_mwh: Option<i32>,
    pub design_mwh: Option<i32>,
}

/// `None` on machines without a battery.
pub fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    // 128: no system battery; 255: unknown.
    if s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 {
        return None;
    }
    let report = WinRtBattery::AggregateBattery().and_then(|b| b.GetReport()).ok();
    let mwh = |f: &dyn Fn(&windows::Devices::Power::BatteryReport) -> windows::core::Result<windows::Foundation::IReference<i32>>| {
        report.as_ref().and_then(|r| f(r).ok()).and_then(|v| v.Value().ok())
    };
    Some(Battery {
        percent: s.BatteryLifePercent.min(100),
        on_ac: s.ACLineStatus == 1,
        charging: s.BatteryFlag & 8 != 0,
        seconds_left: (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
        charge_rate_mw: mwh(&|r| r.ChargeRateInMilliwatts()),
        remaining_mwh: mwh(&|r| r.RemainingCapacityInMilliwattHours()),
        full_mwh: mwh(&|r| r.FullChargeCapacityInMilliwattHours()),
        design_mwh: mwh(&|r| r.DesignCapacityInMilliwattHours()),
    })
}

/// The power-mode slider's three positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMode {
    Efficiency,
    Balanced,
    Performance,
}

const OVERLAY_EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
const OVERLAY_BALANCED: GUID = GUID::zeroed();
const OVERLAY_PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);
/// The built-in Balanced plan; the power-mode overlay only applies on top of it.
const SCHEME_BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);

impl PowerMode {
    pub fn from_overlay(g: GUID) -> Option<Self> {
        match g {
            OVERLAY_EFFICIENCY => Some(Self::Efficiency),
            OVERLAY_BALANCED => Some(Self::Balanced),
            OVERLAY_PERFORMANCE => Some(Self::Performance),
            _ => None,
        }
    }

    pub fn overlay(self) -> GUID {
        match self {
            Self::Efficiency => OVERLAY_EFFICIENCY,
            Self::Balanced => OVERLAY_BALANCED,
            Self::Performance => OVERLAY_PERFORMANCE,
        }
    }
}

// Undocumented but stable since Windows 10 1709 and what Settings' slider calls (not bound
// by windows-rs).
windows_core::link!("powrprof.dll" "system" fn PowerGetEffectiveOverlayScheme(effectiveoverlayguid: *mut GUID) -> u32);
windows_core::link!("powrprof.dll" "system" fn PowerSetActiveOverlayScheme(overlayschemeguid: *const GUID) -> u32);

/// The current power mode; `None` for an overlay we don't know (e.g. an OEM's).
pub fn power_mode() -> Option<PowerMode> {
    let mut g = GUID::zeroed();
    (unsafe { PowerGetEffectiveOverlayScheme(&mut g) } == 0).then_some(())?;
    PowerMode::from_overlay(g)
}

pub fn set_power_mode(mode: PowerMode) -> anyhow::Result<()> {
    let rc = unsafe { PowerSetActiveOverlayScheme(&mode.overlay()) };
    anyhow::ensure!(rc == 0, "PowerSetActiveOverlayScheme failed ({rc})");
    Ok(())
}

/// Power mode only applies while the active plan is Balanced (as in Settings).
pub fn power_mode_available() -> bool {
    let mut p: *mut GUID = std::ptr::null_mut();
    if unsafe { PowerGetActiveScheme(None, &mut p) } != ERROR_SUCCESS || p.is_null() {
        return false;
    }
    let active = unsafe { *p };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(p.cast())));
    }
    active == SCHEME_BALANCED
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergySaver {
    /// Not available right now (typically: plugged in).
    Unavailable,
    Off,
    On,
}

pub fn energy_saver() -> EnergySaver {
    match PowerManager::EnergySaverStatus() {
        Ok(EnergySaverStatus::On) => EnergySaver::On,
        Ok(EnergySaverStatus::Off) => EnergySaver::Off,
        _ => EnergySaver::Unavailable,
    }
}

/// Registered power-setting notifications: `WM_POWERBROADCAST`/`PBT_POWERSETTINGCHANGE`
/// arrive at the window for brightness, AC/battery, percentage, energy saver and power
/// plan changes. Unregistered on drop.
pub struct PowerNotifications {
    handles: Vec<HPOWERNOTIFY>,
}

impl PowerNotifications {
    pub fn register(window: Hwnd) -> Self {
        let handles = [
            GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
            GUID_ACDC_POWER_SOURCE,
            GUID_BATTERY_PERCENTAGE_REMAINING,
            GUID_POWER_SAVING_STATUS,
            GUID_POWERSCHEME_PERSONALITY,
        ]
        .iter()
        .filter_map(|g| unsafe { RegisterPowerSettingNotification(HANDLE(window.0 as _), g, DEVICE_NOTIFY_WINDOW_HANDLE) }.ok())
        .collect();
        Self { handles }
    }
}

impl Drop for PowerNotifications {
    fn drop(&mut self) {
        for h in self.handles.drain(..) {
            unsafe {
                let _ = UnregisterPowerSettingNotification(h);
            }
        }
    }
}

/// What a power-setting notification was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSetting {
    /// The built-in screen's brightness is now this (0–100).
    Brightness(u8),
    /// Battery, AC, energy saver or power plan: re-read them.
    Other,
}

/// Read a `PBT_POWERSETTINGCHANGE` message's `lparam`. Only valid for the `lparam` of that
/// exact message, while it's being handled.
pub fn parse_power_setting(lparam: isize) -> Option<PowerSetting> {
    if lparam == 0 {
        return None;
    }
    let s = unsafe { &*(lparam as *const POWERBROADCAST_SETTING) };
    if s.PowerSetting == BRIGHTNESS_CHANGED && s.DataLength >= 4 {
        let level = unsafe { std::ptr::read_unaligned(s.Data.as_ptr().cast::<u32>()) };
        return Some(PowerSetting::Brightness(level.min(100) as u8));
    }
    Some(PowerSetting::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_modes_round_trip() {
        for m in [PowerMode::Efficiency, PowerMode::Balanced, PowerMode::Performance] {
            assert_eq!(PowerMode::from_overlay(m.overlay()), Some(m));
        }
        assert_eq!(PowerMode::from_overlay(GUID::from_u128(1)), None);
    }

    #[test]
    fn parses_a_power_setting_message() {
        // POWERBROADCAST_SETTING with a 4-byte payload: brightness 60.
        #[repr(C)]
        struct Msg {
            guid: GUID,
            len: u32,
            data: [u8; 4],
        }
        let m = Msg { guid: BRIGHTNESS_CHANGED, len: 4, data: 60u32.to_le_bytes() };
        assert_eq!(parse_power_setting(&m as *const Msg as isize), Some(PowerSetting::Brightness(60)));
        let m = Msg { guid: GUID_ACDC_POWER_SOURCE, len: 4, data: 1u32.to_le_bytes() };
        assert_eq!(parse_power_setting(&m as *const Msg as isize), Some(PowerSetting::Other));
        assert!(parse_power_setting(0).is_none());
    }

    #[test]
    #[ignore = "reads the live power state"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::mta();
        println!("battery: {:#?}", battery());
        println!("power mode: {:?} (available: {})", power_mode(), power_mode_available());
        println!("energy saver: {:?}", energy_saver());
    }
}
