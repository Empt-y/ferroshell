//! Pure helpers for the power service: battery time and health text, and the icon level.

/// "2 h 15 min", "45 min", "under a minute".
pub fn duration_text(seconds: u32) -> String {
    let minutes = seconds / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Seconds until full at the current charge rate, if it's charging and the numbers make
/// sense.
pub fn time_to_full(remaining_mwh: i32, full_mwh: i32, rate_mw: i32) -> Option<u32> {
    if rate_mw <= 0 || full_mwh <= 0 || remaining_mwh >= full_mwh {
        return None;
    }
    let hours = f64::from(full_mwh - remaining_mwh) / f64::from(rate_mw);
    // Past a day the estimate is noise (e.g. a trickle charge right after plugging in).
    (hours < 24.0).then(|| (hours * 3600.0).round() as u32)
}

/// Battery wear: today's full capacity against the design capacity, 0–100.
pub fn health_percent(full_mwh: i32, design_mwh: i32) -> Option<u8> {
    (full_mwh > 0 && design_mwh > 0).then(|| ((f64::from(full_mwh) / f64::from(design_mwh) * 100.0).round() as i64).clamp(0, 100) as u8)
}

/// The line under the percentage in the applet.
/// `seconds_left` is Windows' discharge estimate; `seconds_to_full` from [`time_to_full`].
pub fn status_line(percent: u8, on_ac: bool, charging: bool, seconds_left: Option<u32>, seconds_to_full: Option<u32>) -> String {
    if on_ac {
        if percent >= 100 || !charging {
            return if percent >= 100 { "Fully charged".to_owned() } else { "Plugged in, not charging".to_owned() };
        }
        return match seconds_to_full {
            Some(s) => format!("Charging · {} until full", duration_text(s)),
            None => "Charging".to_owned(),
        };
    }
    match seconds_left {
        Some(s) => format!("{} left", duration_text(s)),
        None => "On battery".to_owned(),
    }
}

/// How many of the battery icon's five bars to fill.
pub fn icon_level(percent: u8) -> u8 {
    match percent.min(100) {
        0..=9 => 0,
        p => (p / 20 + u8::from(p % 20 >= 10)).min(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration_text(30), "under a minute");
        assert_eq!(duration_text(45 * 60), "45 min");
        assert_eq!(duration_text(2 * 3600), "2 h");
        assert_eq!(duration_text(2 * 3600 + 15 * 60 + 59), "2 h 15 min");
    }

    #[test]
    fn time_to_full_estimates() {
        // 30 Wh to go at 30 W: one hour.
        assert_eq!(time_to_full(20_000, 50_000, 30_000), Some(3600));
        assert_eq!(time_to_full(50_000, 50_000, 30_000), None, "already full");
        assert_eq!(time_to_full(20_000, 50_000, 0), None, "not charging");
        assert_eq!(time_to_full(20_000, 50_000, -5_000), None, "discharging");
        assert_eq!(time_to_full(0, 50_000, 1_000), None, "50 h is noise");
    }

    #[test]
    fn health() {
        assert_eq!(health_percent(45_000, 50_000), Some(90));
        assert_eq!(health_percent(52_000, 50_000), Some(100), "new batteries can exceed design; clamp");
        assert_eq!(health_percent(0, 50_000), None);
        assert_eq!(health_percent(45_000, 0), None);
    }

    #[test]
    fn status_lines() {
        assert_eq!(status_line(100, true, false, None, None), "Fully charged");
        assert_eq!(status_line(80, true, false, None, None), "Plugged in, not charging");
        assert_eq!(status_line(40, true, true, None, Some(3600)), "Charging · 1 h until full");
        assert_eq!(status_line(40, true, true, None, None), "Charging");
        assert_eq!(status_line(40, false, false, Some(5400), None), "1 h 30 min left");
        assert_eq!(status_line(40, false, false, None, None), "On battery");
    }

    #[test]
    fn icon_levels() {
        assert_eq!(icon_level(0), 0);
        assert_eq!(icon_level(9), 0);
        assert_eq!(icon_level(10), 1);
        assert_eq!(icon_level(50), 3);
        assert_eq!(icon_level(95), 5);
        assert_eq!(icon_level(100), 5);
        assert_eq!(icon_level(255), 5);
    }
}
