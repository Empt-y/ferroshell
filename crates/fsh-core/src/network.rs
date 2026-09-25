//! Pure helpers for the network service: signal strength, security labels, and building a
//! Wi-Fi connection profile's XML (escaping is easy to get wrong and easy to test).
//!
//! `Security` itself lives in `fsh_win::network` (it's `fsh-win`'s translation of Windows'
//! `DOT11_AUTH_ALGORITHM`); this crate depends on `fsh-win`, not the other way round.

pub use fsh_win::network::Security;

/// `Security` is a foreign type here (it lives in `fsh-win`), so its convenience methods
/// are an extension trait rather than an inherent `impl`. Bring this into scope to call them.
pub trait SecurityExt {
    fn needs_password(self) -> bool;
    /// Whether [`wifi_profile_xml`] can build a profile for this (a personal/PSK network).
    fn supported(self) -> bool;
    fn label(self) -> &'static str;
}

impl SecurityExt for Security {
    fn needs_password(self) -> bool {
        !matches!(self, Security::Open)
    }

    fn supported(self) -> bool {
        !matches!(self, Security::Enterprise)
    }

    fn label(self) -> &'static str {
        match self {
            Security::Open => "Open",
            Security::Wep => "WEP",
            Security::Wpa => "WPA-Personal",
            Security::Wpa2 => "WPA2-Personal",
            Security::Wpa3 => "WPA3-Personal",
            Security::Enterprise => "Enterprise",
        }
    }
}

/// Signal quality (Windows' 0–100) as bars (0–4), matching Windows' own thresholds.
pub fn signal_bars(quality: u32) -> u8 {
    match quality.min(100) {
        0 => 0,
        1..=24 => 1,
        25..=49 => 2,
        50..=74 => 3,
        _ => 4,
    }
}

/// Escape text for a WLAN profile's XML (or any other XML this crate writes): the five
/// predefined entities. `quot`/`apos` matter because SSIDs and passwords can contain quotes.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// UTF-8 bytes as uppercase hex, the form `<SSID><hex>` wants.
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// A `WlanSetProfile`-ready profile for a personal (PSK) or open network. `ssid_hex` is the
/// SSID's raw bytes as hex (from a scan result, so it round-trips exactly even for
/// non-UTF-8 or punctuation-heavy names); `ssid_name` is only used for the profile's
/// display name and the `<name>` match. Returns `None` for [`Security::Enterprise`].
pub fn wifi_profile_xml(ssid_hex: &str, ssid_name: &str, security: Security, password: &str) -> Option<String> {
    let (auth, encryption) = match security {
        Security::Open => ("open", "none"),
        Security::Wep => ("shared", "WEP"),
        Security::Wpa => ("WPAPSK", "TKIP"),
        Security::Wpa2 => ("WPA2PSK", "AES"),
        Security::Wpa3 => ("WPA3SAE", "AES"),
        Security::Enterprise => return None,
    };
    let name = xml_escape(ssid_name);
    let shared_key = security.needs_password().then(|| {
        format!(
            "<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            xml_escape(password)
        )
    });
    Some(format!(
        concat!(
            "<?xml version=\"1.0\"?>",
            "<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">",
            "<name>{name}</name>",
            "<SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>",
            "<connectionType>ESS</connectionType>",
            "<connectionMode>auto</connectionMode>",
            "<MSM><security><authEncryption><authentication>{auth}</authentication>",
            "<encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>",
            "{shared_key}</security></MSM>",
            "</WLANProfile>"
        ),
        name = name,
        hex = ssid_hex,
        auth = auth,
        encryption = encryption,
        shared_key = shared_key.unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_match_windows_thresholds() {
        assert_eq!(signal_bars(0), 0);
        assert_eq!(signal_bars(24), 1);
        assert_eq!(signal_bars(25), 2);
        assert_eq!(signal_bars(49), 2);
        assert_eq!(signal_bars(50), 3);
        assert_eq!(signal_bars(74), 3);
        assert_eq!(signal_bars(75), 4);
        assert_eq!(signal_bars(100), 4);
        assert_eq!(signal_bars(255), 4, "clamped, not panicking");
    }

    #[test]
    fn escaping_handles_all_five_entities() {
        assert_eq!(xml_escape(r#"a&b<c>d"e'f"#), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        assert_eq!(xml_escape("Café's WiFi"), "Café&apos;s WiFi");
        assert_eq!(xml_escape(""), "");
    }

    #[test]
    fn hex_encoding() {
        assert_eq!(hex_encode(b"Home"), "486F6D65");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn open_network_profile_has_no_shared_key() {
        let xml = wifi_profile_xml("486F6D65", "Home", Security::Open, "").unwrap();
        assert!(xml.contains("<authentication>open</authentication>"));
        assert!(xml.contains("<encryption>none</encryption>"));
        assert!(!xml.contains("sharedKey"));
        assert!(xml.contains("<name>Home</name>"));
    }

    #[test]
    fn psk_profile_embeds_an_escaped_password() {
        let xml = wifi_profile_xml("486F6D65", "Home", Security::Wpa2, r#"p@ss"w'ord&"#).unwrap();
        assert!(xml.contains("<authentication>WPA2PSK</authentication>"));
        assert!(xml.contains("<keyMaterial>p@ss&quot;w&apos;ord&amp;</keyMaterial>"));
    }

    #[test]
    fn ssid_with_special_characters_is_escaped_in_the_name() {
        let xml = wifi_profile_xml("XXXX", r#"Bob's <Router> & "friends""#, Security::Wpa2, "hunter2").unwrap();
        assert!(xml.contains("<name>Bob&apos;s &lt;Router&gt; &amp; &quot;friends&quot;</name>"));
        // The XML is well-formed enough that no raw '<' or '>' sneaks in from user data.
        assert_eq!(xml.matches('<').count(), xml.matches('>').count());
    }

    #[test]
    fn enterprise_networks_have_no_profile() {
        assert!(wifi_profile_xml("XXXX", "Work", Security::Enterprise, "").is_none());
    }

    #[test]
    fn security_properties() {
        assert!(!Security::Open.needs_password());
        assert!(Security::Wpa2.needs_password());
        assert!(Security::Wpa2.supported());
        assert!(!Security::Enterprise.supported());
        assert_eq!(Security::Wpa2.label(), "WPA2-Personal");
    }
}
