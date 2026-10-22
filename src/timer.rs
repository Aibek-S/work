//! Session timer based on the monotonic clock (`Instant`).
//!
//! Elapsed time is accumulated from `Instant` differences, never from counting ticks,
//! so slow redraws, suspended terminals or a busy CPU cannot make the timer drift.
//! Every method takes `now` explicitly, which keeps the logic deterministic in tests.

use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Timer {
    total: Duration,
    /// Time accumulated before the current running stretch.
    banked: Duration,
    /// `Some(start)` while running, `None` while paused.
    running_since: Option<Instant>,
}

impl Timer {
    pub fn new(total: Duration, now: Instant) -> Timer {
        Timer {
            total,
            banked: Duration::ZERO,
            running_since: Some(now),
        }
    }

    pub fn total(&self) -> Duration {
        self.total
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        let running = self
            .running_since
            .map_or(Duration::ZERO, |s| now.saturating_duration_since(s));
        (self.banked + running).min(self.total)
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.total - self.elapsed(now)
    }

    pub fn progress(&self, now: Instant) -> f64 {
        if self.total.is_zero() {
            return 1.0;
        }
        self.elapsed(now).as_secs_f64() / self.total.as_secs_f64()
    }

    pub fn is_paused(&self) -> bool {
        self.running_since.is_none()
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        self.elapsed(now) >= self.total
    }

    pub fn pause(&mut self, now: Instant) {
        if let Some(start) = self.running_since.take() {
            self.banked += now.saturating_duration_since(start);
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if self.running_since.is_none() {
            self.running_since = Some(now);
        }
    }

    pub fn toggle(&mut self, now: Instant) {
        if self.is_paused() {
            self.resume(now)
        } else {
            self.pause(now)
        }
    }

    pub fn restart(&mut self, now: Instant) {
        self.banked = Duration::ZERO;
        self.running_since = Some(now);
    }

    /// How long until the displayed seconds value changes. Used to schedule redraws.
    pub fn until_next_second(&self, now: Instant) -> Duration {
        let rem = self.remaining(now);
        let frac = rem.subsec_nanos();
        if frac == 0 {
            Duration::from_secs(1)
        } else {
            Duration::from_nanos(frac as u64)
        }
    }
}

/// Formats a duration as `MM:SS`, or `H:MM:SS` for an hour or longer.
/// Rounds up so the display shows 00:00 only once the session is truly over.
pub fn format_remaining(d: Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn counts_down_by_wall_difference() {
        let t0 = Instant::now();
        let t = Timer::new(30 * MIN, t0);
        assert_eq!(t.remaining(t0), 30 * MIN);
        assert_eq!(t.remaining(t0 + 10 * MIN), 20 * MIN);
        assert!(!t.is_finished(t0 + 29 * MIN));
        assert!(t.is_finished(t0 + 30 * MIN));
        assert_eq!(t.remaining(t0 + 99 * MIN), Duration::ZERO);
        assert_eq!(t.progress(t0 + 15 * MIN), 0.5);
    }

    #[test]
    fn pause_freezes_time_and_resume_continues() {
        let t0 = Instant::now();
        let mut t = Timer::new(30 * MIN, t0);
        t.pause(t0 + 5 * MIN);
        assert!(t.is_paused());
        assert_eq!(t.remaining(t0 + 20 * MIN), 25 * MIN);
        t.resume(t0 + 20 * MIN);
        assert_eq!(t.remaining(t0 + 30 * MIN), 15 * MIN);
        // Repeated pause/resume calls are idempotent.
        t.resume(t0 + 31 * MIN);
        t.toggle(t0 + 30 * MIN);
        t.pause(t0 + 40 * MIN);
        assert_eq!(t.remaining(t0 + 50 * MIN), 15 * MIN);
    }

    #[test]
    fn restart_resets_and_runs() {
        let t0 = Instant::now();
        let mut t = Timer::new(10 * MIN, t0);
        t.pause(t0 + 4 * MIN);
        t.restart(t0 + 6 * MIN);
        assert!(!t.is_paused());
        assert_eq!(t.remaining(t0 + 7 * MIN), 9 * MIN);
    }

    #[test]
    fn next_second_scheduling() {
        let t0 = Instant::now();
        let t = Timer::new(MIN, t0);
        assert_eq!(t.until_next_second(t0), Duration::from_secs(1));
        let d = t.until_next_second(t0 + Duration::from_millis(300));
        assert_eq!(d, Duration::from_millis(700));
    }

    #[test]
    fn formatting() {
        assert_eq!(format_remaining(Duration::ZERO), "00:00");
        assert_eq!(format_remaining(Duration::from_millis(1)), "00:01");
        assert_eq!(format_remaining(Duration::from_millis(59_500)), "01:00");
        assert_eq!(format_remaining(25 * MIN), "25:00");
        assert_eq!(
            format_remaining(90 * MIN + Duration::from_secs(5)),
            "1:30:05"
        );
    }
}
