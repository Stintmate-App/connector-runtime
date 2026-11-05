//! The time source of a recorder, injected so that tests and replays control it.

use std::time::Instant;
use stintmate_connector_platform::logging::local_iso;

/// Time source (injected so tests and replays control it).
pub trait Clock: Send {
    /// Monotonic seconds.
    fn now(&self) -> f64;
    /// The wall clock as an ISO 8601 timestamp with offset, whole seconds.
    fn iso(&self) -> String;
}

/// The real clock: seconds since it was created, and the local wall clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// A clock whose zero is now.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn iso(&self) -> String {
        local_iso(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_counts_up_and_writes_an_offset() {
        let c = SystemClock::new();
        let a = c.now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(c.now() > a);
        let iso = c.iso();
        assert!(iso.len() >= 20 && iso.contains('T'), "{iso}");
    }
}
