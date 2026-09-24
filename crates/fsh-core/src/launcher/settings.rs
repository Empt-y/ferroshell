//! Windows Settings pages the launcher can open: (title, ms-settings URI, extra keywords).

pub const PAGES: &[(&str, &str, &str)] = &[
    ("Display", "ms-settings:display", "screen monitor resolution scale brightness night light hdr"),
    ("Night light", "ms-settings:nightlight", "blue light warm"),
    ("Sound", "ms-settings:sound", "audio volume speakers microphone output input"),
    ("Sound devices", "ms-settings:sound-devices", "audio devices speakers headphones"),
    ("Notifications", "ms-settings:notifications", "alerts focus do not disturb"),
    ("Focus", "ms-settings:quiethours", "focus assist do not disturb"),
    ("Power & battery", "ms-settings:powersleep", "power sleep battery screen timeout energy"),
    ("Storage", "ms-settings:storagesense", "disk space drive cleanup"),
    ("Multitasking", "ms-settings:multitasking", "snap windows alt tab virtual desktops"),
    ("Activation", "ms-settings:activation", "license product key windows"),
    ("Troubleshoot", "ms-settings:troubleshoot", "fix problems"),
    ("Recovery", "ms-settings:recovery", "reset restore advanced startup"),
    ("Clipboard", "ms-settings:clipboard", "copy paste history"),
    ("Remote Desktop", "ms-settings:remotedesktop", "rdp remote"),
    ("About", "ms-settings:about", "pc name specs system info rename"),
    ("Bluetooth & devices", "ms-settings:bluetooth", "bluetooth pair devices"),
    ("Printers & scanners", "ms-settings:printers", "printer scanner print"),
    ("Mouse", "ms-settings:mousetouchpad", "mouse cursor scroll pointer"),
    ("Touchpad", "ms-settings:devices-touchpad", "trackpad gestures"),
    ("Typing", "ms-settings:typing", "keyboard autocorrect spelling"),
    ("Pen & Windows Ink", "ms-settings:pen", "pen stylus ink"),
    ("AutoPlay", "ms-settings:autoplay", "usb removable media"),
    ("USB", "ms-settings:usb", "usb devices"),
    ("Network & internet", "ms-settings:network", "network internet connection"),
    ("Wi-Fi", "ms-settings:network-wifi", "wifi wireless wlan"),
    ("Ethernet", "ms-settings:network-ethernet", "wired lan cable"),
    ("VPN", "ms-settings:network-vpn", "vpn tunnel"),
    ("Mobile hotspot", "ms-settings:network-mobilehotspot", "hotspot tethering share"),
    ("Airplane mode", "ms-settings:network-airplanemode", "flight mode"),
    ("Proxy", "ms-settings:network-proxy", "proxy"),
    ("Personalization", "ms-settings:personalization", "personalise theme appearance"),
    ("Background", "ms-settings:personalization-background", "wallpaper desktop picture"),
    ("Colors", "ms-settings:personalization-colors", "colours accent dark mode light mode theme"),
    ("Themes", "ms-settings:themes", "theme"),
    ("Lock screen", "ms-settings:lockscreen", "lock screen picture"),
    ("Fonts", "ms-settings:fonts", "font typeface"),
    ("Start", "ms-settings:personalization-start", "start menu"),
    ("Taskbar", "ms-settings:taskbar", "taskbar"),
    ("Installed apps", "ms-settings:appsfeatures", "apps programs uninstall remove"),
    ("Default apps", "ms-settings:defaultapps", "default browser file associations open with"),
    ("Startup apps", "ms-settings:startupapps", "startup boot autostart"),
    ("Optional features", "ms-settings:optionalfeatures", "features"),
    ("Your info", "ms-settings:yourinfo", "account profile picture"),
    ("Sign-in options", "ms-settings:signinoptions", "password pin windows hello fingerprint face"),
    ("Email & accounts", "ms-settings:emailandaccounts", "email accounts microsoft"),
    ("Family", "ms-settings:family-group", "family users"),
    ("Other users", "ms-settings:otherusers", "users accounts add user"),
    ("Date & time", "ms-settings:dateandtime", "clock time zone date"),
    ("Language & region", "ms-settings:regionlanguage", "language region locale keyboard layout"),
    ("Speech", "ms-settings:speech", "voice speech recognition"),
    ("Gaming", "ms-settings:gaming-gamebar", "game bar xbox"),
    ("Game Mode", "ms-settings:gaming-gamemode", "game mode performance"),
    ("Captures", "ms-settings:gaming-gamedvr", "game dvr recording screenshots"),
    ("Accessibility", "ms-settings:easeofaccess", "ease of access"),
    ("Text size", "ms-settings:easeofaccess-display", "text size larger font"),
    ("Magnifier", "ms-settings:easeofaccess-magnifier", "zoom magnify"),
    ("Narrator", "ms-settings:easeofaccess-narrator", "screen reader"),
    ("Privacy & security", "ms-settings:privacy", "privacy permissions security"),
    ("Windows Security", "windowsdefender:", "antivirus defender virus firewall"),
    ("Location", "ms-settings:privacy-location", "gps location"),
    ("Camera privacy", "ms-settings:privacy-webcam", "camera webcam"),
    ("Microphone privacy", "ms-settings:privacy-microphone", "microphone mic"),
    ("Windows Update", "ms-settings:windowsupdate", "update updates patches"),
    ("Delivery Optimization", "ms-settings:delivery-optimization", "update bandwidth"),
    ("For developers", "ms-settings:developers", "developer mode"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_well_formed() {
        let mut uris = std::collections::HashSet::new();
        for (title, uri, _) in PAGES {
            assert!(!title.is_empty());
            assert!(uri.ends_with(':') || uri.starts_with("ms-settings:"), "{uri}");
            assert!(uris.insert(uri), "duplicate {uri}");
        }
        assert!(PAGES.len() >= 60);
    }
}
