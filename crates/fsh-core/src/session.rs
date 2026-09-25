//! Pure decisions for Ferroshell as the login shell.

/// When the supervisor dies while Ferroshell is the login shell, the shell relaunches it —
/// unless that has already happened `max` times within `window_secs` (a session that keeps
/// dying), in which case Explorer is started instead. `history` is the Unix times of earlier
/// relaunches; returns whether to relaunch now.
pub fn relaunch_allowed(history: &[i64], now: i64, window_secs: i64, max: usize) -> bool {
    history.iter().filter(|&&t| now - t < window_secs && t <= now).count() < max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunches_until_it_keeps_dying() {
        assert!(relaunch_allowed(&[], 1000, 60, 3));
        assert!(relaunch_allowed(&[990, 995], 1000, 60, 3));
        assert!(!relaunch_allowed(&[970, 990, 995], 1000, 60, 3), "a 4th within a minute");
        assert!(relaunch_allowed(&[900, 910, 995], 1000, 60, 3), "old ones don't count");
        // A clock that jumped backwards doesn't count future entries.
        assert!(relaunch_allowed(&[2000, 2001, 2002], 1000, 60, 3));
    }
}
