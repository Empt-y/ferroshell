//! Pure helpers for the notifications service: grouping, ordering, time labels and which
//! notifications are new.

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDateTime};
use fsh_win::notifications::Toast;

/// "now", "5 min ago", "14:32" (today), "Yesterday", "Mon" (this week), "25 Sep".
/// Both times are local.
pub fn time_text(created: NaiveDateTime, now: NaiveDateTime) -> String {
    let age = now - created;
    if age < Duration::minutes(1) {
        return "now".to_owned();
    }
    if age < Duration::hours(1) {
        return format!("{} min ago", age.num_minutes());
    }
    let (day, today) = (created.date(), now.date());
    if day == today {
        created.format("%H:%M").to_string()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".to_owned()
    } else if age < Duration::days(7) {
        created.format("%a").to_string()
    } else if day.year() == today.year() {
        created.format("%-d %b").to_string()
    } else {
        created.format("%-d %b %Y").to_string()
    }
}

/// Notifications grouped by app for display: apps ordered by their newest notification,
/// newest first within each app. Each entry says whether it starts its app's group.
pub fn grouped(toasts: &[Toast]) -> Vec<(bool, &Toast)> {
    let mut apps: Vec<(&str, i64)> = Vec::new();
    for t in toasts {
        match apps.iter_mut().find(|(a, _)| *a == t.app_id) {
            Some((_, newest)) => *newest = (*newest).max(t.created),
            None => apps.push((&t.app_id, t.created)),
        }
    }
    apps.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut out = Vec::with_capacity(toasts.len());
    for (app, _) in apps {
        let mut mine: Vec<&Toast> = toasts.iter().filter(|t| t.app_id == app).collect();
        mine.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        for (i, t) in mine.into_iter().enumerate() {
            out.push((i == 0, t));
        }
    }
    out
}

/// Ids in `current` not in `known`: notifications that arrived since the last look, newest
/// first. `known == None` (the first look) never counts as new, so existing notifications
/// don't all pop up as banners at startup.
pub fn arrivals<'a>(known: Option<&HashSet<u32>>, current: &'a [Toast]) -> Vec<&'a Toast> {
    let Some(known) = known else { return vec![] };
    let mut new: Vec<&Toast> = current.iter().filter(|t| !known.contains(&t.id)).collect();
    new.sort_by_key(|t| std::cmp::Reverse(t.created));
    new
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(d: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn toast(id: u32, app: &str, created: i64) -> Toast {
        Toast { id, app_id: app.into(), app_name: app.into(), created, title: format!("t{id}"), body: String::new() }
    }

    #[test]
    fn time_labels() {
        let now = at(25, 16, 30); // a Friday
        assert_eq!(time_text(at(25, 16, 30), now), "now");
        assert_eq!(time_text(at(25, 16, 5), now), "25 min ago");
        assert_eq!(time_text(at(25, 9, 7), now), "09:07");
        assert_eq!(time_text(at(24, 23, 59), now), "Yesterday");
        assert_eq!(time_text(at(21, 12, 0), now), "Mon");
        assert_eq!(time_text(at(2, 12, 0), now), "2 Sep");
        let last_year = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap().and_hms_opt(8, 0, 0).unwrap();
        assert_eq!(time_text(last_year, now), "31 Dec 2025");
        // A clock that went backwards (created "in the future") still says "now".
        assert_eq!(time_text(at(25, 16, 40), now), "now");
    }

    #[test]
    fn grouping_orders_apps_by_newest_and_items_newest_first() {
        let toasts = [toast(1, "mail", 100), toast(2, "chat", 300), toast(3, "mail", 200), toast(4, "chat", 50)];
        let g: Vec<(bool, u32)> = grouped(&toasts).into_iter().map(|(first, t)| (first, t.id)).collect();
        // chat's newest (300) beats mail's (200).
        assert_eq!(g, vec![(true, 2), (false, 4), (true, 3), (false, 1)]);
        assert!(grouped(&[]).is_empty());
    }

    #[test]
    fn only_later_arrivals_are_new() {
        let toasts = [toast(1, "a", 10), toast(2, "a", 30), toast(3, "b", 20)];
        assert!(arrivals(None, &toasts).is_empty(), "first look: nothing is new");
        let known: HashSet<u32> = [1].into();
        let new: Vec<u32> = arrivals(Some(&known), &toasts).into_iter().map(|t| t.id).collect();
        assert_eq!(new, vec![2, 3], "newest first");
    }
}
