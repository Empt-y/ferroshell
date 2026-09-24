//! Month grids for the calendar popup.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    /// Belongs to the month being shown (others are leading/trailing days).
    pub in_month: bool,
    pub today: bool,
    /// ISO 8601 week number.
    pub week: u32,
}

/// A 6×7 grid for `month` of `year` (month 1–12), starting on Monday or Sunday.
/// Out-of-range input is clamped rather than failing.
pub fn month_grid(year: i32, month: u32, monday_first: bool, today: NaiveDate) -> Vec<Day> {
    let month = month.clamp(1, 12);
    let year = year.clamp(1, 9999);
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else { return vec![] };
    let start_weekday = if monday_first { Weekday::Mon } else { Weekday::Sun };
    let offset = (7 + first.weekday().num_days_from_monday() as i64 - start_weekday.num_days_from_monday() as i64) % 7;
    let start = first - Duration::days(offset);
    (0..42)
        .map(|i| {
            let date = start + Duration::days(i);
            Day { date, in_month: date.month() == month, today: date == today, week: date.iso_week().week() }
        })
        .collect()
}

/// Step a (year, month) pair by `delta` months.
pub fn add_months(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + (month.clamp(1, 12) as i32 - 1) + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

pub fn month_title(year: i32, month: u32) -> String {
    NaiveDate::from_ymd_opt(year, month.clamp(1, 12), 1).map(|d| d.format("%B %Y").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn september_2026_grid() {
        // 1 Sep 2026 is a Tuesday.
        let g = month_grid(2026, 9, true, d(2026, 9, 24));
        assert_eq!(g.len(), 42);
        assert_eq!(g[0].date, d(2026, 8, 31), "Monday-first grid starts on Monday 31 Aug");
        assert!(!g[0].in_month && g[1].in_month);
        let today = g.iter().find(|x| x.today).unwrap();
        assert_eq!(today.date, d(2026, 9, 24));
        assert_eq!(today.week, 39);
        let sunday_first = month_grid(2026, 9, false, d(2026, 9, 24));
        assert_eq!(sunday_first[0].date, d(2026, 8, 30));
    }

    #[test]
    fn iso_weeks_at_year_edges() {
        // 1 Jan 2027 is a Friday, so it's in ISO week 53 of 2026.
        let g = month_grid(2027, 1, true, d(2000, 1, 1));
        let jan1 = g.iter().find(|x| x.date == d(2027, 1, 1)).unwrap();
        assert_eq!(jan1.week, 53);
        // A month starting on the first weekday has no leading days.
        let g = month_grid(2026, 6, true, d(2000, 1, 1)); // 1 June 2026 is a Monday
        assert_eq!(g[0].date, d(2026, 6, 1));
    }

    #[test]
    fn month_stepping_and_titles() {
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 5, -17), (2024, 12));
        assert_eq!(month_title(2026, 9), "September 2026");
        assert_eq!(month_grid(2026, 99, true, d(2026, 1, 1)).len(), 42, "clamped, not panicking");
    }
}
