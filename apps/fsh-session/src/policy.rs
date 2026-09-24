//! When to restart the shell, when to fall back to safe mode, and when to give up.
//! Pure logic (time is passed in) so it can be tested without spawning anything.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Restart in the current mode after this delay.
    Restart(Duration),
    /// Too many crashes in normal mode: restart in safe mode.
    EnterSafeMode,
    /// Safe mode keeps crashing too: stop and hand the desktop back to Explorer.
    GiveUp,
}

#[derive(Debug)]
pub struct CrashPolicy {
    window: Duration,
    threshold: usize,
    crashes: VecDeque<Instant>,
    pub safe_mode: bool,
}

const BASE_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(8);

impl CrashPolicy {
    /// `threshold` crashes within `window` escalates (normal → safe mode → give up).
    pub fn new(threshold: usize, window: Duration) -> Self {
        Self { window, threshold, crashes: VecDeque::new(), safe_mode: false }
    }

    pub fn on_crash(&mut self, now: Instant) -> Decision {
        self.crashes.retain(|t| now.duration_since(*t) < self.window);
        self.crashes.push_back(now);
        let n = self.crashes.len();
        if n >= self.threshold {
            self.crashes.clear();
            if self.safe_mode {
                return Decision::GiveUp;
            }
            self.safe_mode = true;
            return Decision::EnterSafeMode;
        }
        let delay = BASE_DELAY.saturating_mul(1 << (n - 1).min(8)).min(MAX_DELAY);
        Decision::Restart(delay)
    }

    /// Forget crash history, e.g. after the user explicitly restarts.
    pub fn reset(&mut self, safe_mode: bool) {
        self.crashes.clear();
        self.safe_mode = safe_mode;
    }

    pub fn recent_crashes(&self) -> usize {
        self.crashes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn backs_off_then_escalates_to_safe_mode_then_gives_up() {
        let t = Instant::now();
        let mut p = CrashPolicy::new(3, MIN);
        assert_eq!(p.on_crash(t), Decision::Restart(Duration::from_millis(250)));
        assert_eq!(p.on_crash(t + Duration::from_secs(1)), Decision::Restart(Duration::from_millis(500)));
        assert_eq!(p.on_crash(t + Duration::from_secs(2)), Decision::EnterSafeMode);
        assert!(p.safe_mode);
        // Safe mode starts with a clean slate…
        assert_eq!(p.on_crash(t + Duration::from_secs(3)), Decision::Restart(Duration::from_millis(250)));
        assert_eq!(p.on_crash(t + Duration::from_secs(4)), Decision::Restart(Duration::from_millis(500)));
        // …and gives up if it keeps crashing.
        assert_eq!(p.on_crash(t + Duration::from_secs(5)), Decision::GiveUp);
    }

    #[test]
    fn crashes_outside_the_window_are_forgotten() {
        let t = Instant::now();
        let mut p = CrashPolicy::new(3, MIN);
        p.on_crash(t);
        p.on_crash(t + Duration::from_secs(30));
        // The first crash is now more than a minute old, so this is only the 2nd in the window.
        assert_eq!(p.on_crash(t + Duration::from_secs(70)), Decision::Restart(Duration::from_millis(500)));
        assert!(!p.safe_mode);
    }

    #[test]
    fn delay_is_capped() {
        let t = Instant::now();
        let mut p = CrashPolicy::new(100, MIN);
        let mut last = Decision::Restart(Duration::ZERO);
        for i in 0..20 {
            last = p.on_crash(t + Duration::from_millis(i));
        }
        assert_eq!(last, Decision::Restart(MAX_DELAY));
    }
}
