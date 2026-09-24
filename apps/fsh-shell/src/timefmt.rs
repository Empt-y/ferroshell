//! strftime-style formatting for `Shell.format-time`, safe against bad user formats
//! (chrono panics when a `Display` with an invalid specifier is turned into a string).

use chrono::format::{Item, StrftimeItems};
use chrono::{Local, TimeZone};

pub fn format_time(unix_secs: i64, format: &str) -> String {
    let items: Vec<Item<'_>> = StrftimeItems::new(format).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return format!("bad format: {format}");
    }
    match Local.timestamp_opt(unix_secs, 0).single() {
        Some(t) => t.format_with_items(items.into_iter()).to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_survives_bad_input() {
        let s = format_time(0, "%Y");
        assert!(s == "1970" || s == "1969", "{s}");
        assert_eq!(format_time(0, "%Q"), "bad format: %Q");
        assert_eq!(format_time(0, ""), "");
        assert_eq!(format_time(i64::MAX, "%Y"), "");
    }
}
