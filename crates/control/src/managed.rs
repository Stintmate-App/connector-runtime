//! Managed mode's parent watch.
//!
//! `--managed --parent-pid <pid>` (or `control.managed`): a managing app owns the only tray
//! icon and runs this app hidden. [`ParentWatch`] tells whether that app is still alive: the
//! PID must be alive and still the same process (its creation time, read when the watch
//! starts), so a PID reused by Windows after the managing app died is not taken for it.

use std::time::{Duration, SystemTime};

/// How often a managed app checks its managing app.
pub const PARENT_CHECK: Duration = Duration::from_secs(2);

type StartTime = Box<dyn Fn(u32) -> Option<SystemTime> + Send + Sync>;
type Alive = Box<dyn Fn(u32, Option<SystemTime>) -> bool + Send + Sync>;

/// Whether the managing app (`pid`) is still running.
pub struct ParentWatch {
    /// The managing app's PID.
    pub pid: u32,
    started_at: Option<SystemTime>,
    alive_fn: Alive,
}

impl ParentWatch {
    /// Watches `pid` through the system.
    #[cfg(windows)]
    pub fn new(pid: u32) -> Self {
        Self::with(
            pid,
            Box::new(stintmate_connector_platform::procinfo::pid_start_time),
            Box::new(stintmate_connector_platform::procinfo::process_alive),
        )
    }

    /// Watches `pid` with injected process questions (tests).
    pub fn with(pid: u32, start_time: StartTime, alive_fn: Alive) -> Self {
        Self {
            pid,
            started_at: start_time(pid),
            alive_fn,
        }
    }

    /// The managing app still runs (false when it was already gone when the watch began).
    pub fn alive(&self) -> bool {
        self.started_at.is_some() && (self.alive_fn)(self.pid, self.started_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn already_gone_at_the_start_is_never_alive() {
        let w = ParentWatch::with(7, Box::new(|_| None), Box::new(|_, _| true));
        assert!(!w.alive());
    }

    #[test]
    fn alive_follows_the_process_and_checks_its_creation_time() {
        let up = Arc::new(AtomicBool::new(true));
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let flag = up.clone();
        let w = ParentWatch::with(
            7,
            Box::new(move |_| Some(t0)),
            Box::new(move |pid, started| {
                assert_eq!(
                    (pid, started),
                    (7, Some(t0)),
                    "the start time is passed along"
                );
                flag.load(Ordering::SeqCst)
            }),
        );
        assert!(w.alive());
        up.store(false, Ordering::SeqCst);
        assert!(!w.alive());
    }

    #[cfg(windows)]
    #[test]
    fn this_process_is_alive_and_a_bogus_pid_is_not() {
        assert!(ParentWatch::new(std::process::id()).alive());
        assert!(!ParentWatch::new(0xFFFF_FFF0).alive());
    }
}
