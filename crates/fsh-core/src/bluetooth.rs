//! Pure helpers for the Bluetooth service: device kinds, status text, and matching the same
//! device across Windows APIs (which spell container ids differently).

/// What sort of device this is, for the applet's icon and for whether we can connect it.
/// Classic devices report a "major class"; Bluetooth LE devices an appearance category.
pub fn device_kind(major_class: Option<i32>, le_category: Option<u16>) -> &'static str {
    if let Some(major) = major_class {
        return match major {
            1 => "computer",
            2 => "phone",
            4 => "audio",
            5 => "input",
            7 => "wearable",
            _ => "other",
        };
    }
    match le_category {
        Some(1) => "phone",
        Some(2) => "computer",
        Some(3) => "wearable", // watch
        Some(15) => "input",   // HID: keyboard, mouse, gamepad, ...
        // Earbuds/headsets/speakers, and hearing aids.
        Some(37 | 41) => "audio",
        _ => "other",
    }
}

/// `0xAABBCCDDEEFF` as `AA:BB:CC:DD:EE:FF`.
pub fn format_address(addr: u64) -> String {
    (0..6).rev().map(|i| format!("{:02X}", (addr >> (i * 8)) & 0xFF)).collect::<Vec<_>>().join(":")
}

/// The line under a device's name in the applet.
pub fn status_text(connected: bool, battery: Option<u8>) -> String {
    let state = if connected { "Connected" } else { "Paired" };
    match battery {
        Some(b) => format!("{state} · {}%", b.min(100)),
        None => state.to_owned(),
    }
}

/// Container ids come as `{8C7E...}` from one API and `8c7e...` from another; this is the
/// form to compare.
pub fn normalise_container(id: &str) -> String {
    id.trim().trim_start_matches('{').trim_end_matches('}').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_classic_major_class() {
        assert_eq!(device_kind(Some(4), None), "audio");
        assert_eq!(device_kind(Some(5), None), "input");
        assert_eq!(device_kind(Some(2), None), "phone");
        assert_eq!(device_kind(Some(9), None), "other");
        // The classic class wins if (oddly) both are known.
        assert_eq!(device_kind(Some(4), Some(15)), "audio");
    }

    #[test]
    fn kinds_from_le_appearance_category() {
        assert_eq!(device_kind(None, Some(15)), "input");
        assert_eq!(device_kind(None, Some(37)), "audio");
        assert_eq!(device_kind(None, Some(3)), "wearable");
        assert_eq!(device_kind(None, None), "other");
    }

    #[test]
    fn addresses() {
        assert_eq!(format_address(0x0011_2233_44AB), "00:11:22:33:44:AB", "leading zero bytes are kept");
        assert_eq!(format_address(0xA1B2_C3D4_E5F6), "A1:B2:C3:D4:E5:F6");
        assert_eq!(format_address(0), "00:00:00:00:00:00");
    }

    #[test]
    fn status_lines() {
        assert_eq!(status_text(true, Some(80)), "Connected · 80%");
        assert_eq!(status_text(false, None), "Paired");
        assert_eq!(status_text(true, Some(250)), "Connected · 100%", "clamped");
    }

    #[test]
    fn container_ids_compare_across_spellings() {
        assert_eq!(normalise_container("{8C7ED206-3F8A-4827-B3AB-AE9E1FAEFC6C}"), normalise_container("8c7ed206-3f8a-4827-b3ab-ae9e1faefc6c"));
        assert_eq!(normalise_container(" {AB} "), "ab");
    }
}
