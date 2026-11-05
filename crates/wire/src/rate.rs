//! The contract's announcement rate limit (4.4), kept by the connector itself: at most
//! [`RATE_LIMIT`] per [`RATE_WINDOW_S`] seconds, a sliding window, so Pitwall's `rate_limited`
//! is the exception. An announcement is worth something only now: over the limit it is dropped,
//! never queued. Time comes in from outside.

use std::collections::VecDeque;

/// Announcements per connector per window (connector kit, contract.md section 4.4).
pub const RATE_LIMIT: usize = 10;
/// The window, seconds.
pub const RATE_WINDOW_S: f64 = 60.0;
/// `spotter`-level announcements per connector per window (Pitwall's budget).
pub const SPOTTER_RATE_LIMIT: usize = 120;

/// A sliding window of send times.
#[derive(Debug, Clone)]
pub struct RateWindow {
    limit: usize,
    window_s: f64,
    sent: VecDeque<f64>,
}

impl Default for RateWindow {
    fn default() -> Self {
        Self::new(RATE_LIMIT, RATE_WINDOW_S)
    }
}

impl RateWindow {
    /// A window of `limit` sends per `window_s` seconds.
    pub fn new(limit: usize, window_s: f64) -> Self {
        Self {
            limit,
            window_s,
            sent: VecDeque::with_capacity(limit + 1),
        }
    }

    /// Takes a slot at `now`: the slot (its time, for [`release`](Self::release)) or `None`
    /// when `limit` were sent in the last `window_s` seconds.
    pub fn admit(&mut self, now: f64) -> Option<f64> {
        while self.sent.front().is_some_and(|&t| now - t >= self.window_s) {
            self.sent.pop_front();
        }
        if self.sent.len() >= self.limit {
            return None;
        }
        self.sent.push_back(now);
        Some(now)
    }

    /// Gives back the slot a failed send took (the one this call added, not the newest).
    pub fn release(&mut self, slot: f64) {
        if let Some(i) = self.sent.iter().position(|&t| t == slot) {
            self.sent.remove(i);
        }
    }

    /// Why a send was refused, for the log.
    pub fn reason(&self) -> String {
        format!(
            "rate limit: {} announcements per {:.0} s",
            self.limit, self.window_s
        )
    }

    /// Slots in use now.
    pub fn used(&self) -> usize {
        self.sent.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_per_minute_then_drops_until_the_oldest_ages_out() {
        let mut w = RateWindow::default();
        for i in 0..10 {
            assert!(w.admit(100.0 + f64::from(i)).is_some());
        }
        assert!(w.admit(110.0).is_none());
        assert!(w.admit(159.9).is_none());
        assert!(w.admit(160.0).is_some()); // the first (100.0) left the window
        assert!(w.admit(160.5).is_none());
        assert_eq!(w.reason(), "rate limit: 10 announcements per 60 s");
    }

    #[test]
    fn a_released_slot_is_free_again() {
        let mut w = RateWindow::new(2, 60.0);
        let a = w.admit(1.0).unwrap();
        w.admit(2.0).unwrap();
        assert!(w.admit(3.0).is_none());
        w.release(a);
        assert_eq!(w.used(), 1);
        assert!(w.admit(3.0).is_some());
        w.release(99.0); // already aged out or unknown: nothing happens
        assert_eq!(w.used(), 2);
    }
}
