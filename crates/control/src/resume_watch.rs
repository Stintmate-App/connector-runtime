//! Watches a kept-open session while no recorder runs.
//!
//! A recorder stopped while the game runs leaves `session-resume.json` (see the recorder's
//! `sink` module) so the next one continues the same session. If the game goes to its menus or
//! another session begins before that, the file must go, otherwise a later session of the same
//! kind could join the old one. The recorder's own continuity check stands alone; this watcher
//! adds the one thing it cannot see afterwards, a stay in the menus between two sessions with
//! equal clocks. The connector supplies the look at the game as a [`ResumeProbe`]; the watcher
//! calls it about once a second, only while the file exists and no recorder runs.

use serde_json::Value;
use std::path::Path;

/// The file the recorder leaves, in the connector's data folder.
pub const FILE: &str = "session-resume.json";

/// What one look at the game found, given the clock the recorder stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeLook {
    /// The game is not running: the stored session is forgotten.
    Gone,
    /// No usable reading now (the game was writing, or exposes nothing yet): look again.
    NoReading,
    /// The game is still in the stored session, or in a menu between its parts: keep the file.
    Keep,
    /// The game left the stored session: the file goes.
    Drop,
}

/// The connector's look at the game for the watcher.
pub trait ResumeProbe: Send + Sync {
    /// One cheap look. `stored_clock` is the `clock` field of the resume file (whatever the
    /// recorder wrote there); `game_running` is the control app's cached process check.
    fn look(&self, stored_clock: &Value, game_running: bool) -> ResumeLook;
}

/// The probe of a connector that reads nothing of the game: [`ResumeLook::Gone`] when the game
/// is not running, [`ResumeLook::NoReading`] otherwise.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResume;

impl ResumeProbe for NoResume {
    fn look(&self, _stored_clock: &Value, game_running: bool) -> ResumeLook {
        if game_running {
            ResumeLook::NoReading
        } else {
            ResumeLook::Gone
        }
    }
}

/// The resume file exists.
pub fn pending(path: &Path) -> bool {
    path.is_file()
}

/// Asks `probe` about the file at `path` and deletes the file on [`ResumeLook::Gone`] or
/// [`ResumeLook::Drop`]; true when deleted. A file with no readable `clock` is left to the
/// recorder, which consumes it.
pub fn check(path: &Path, probe: &dyn ResumeProbe, game_running: bool) -> bool {
    let Some(stored) = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("clock").cloned())
    else {
        return false;
    };
    let look = probe.look(&stored, game_running);
    if matches!(look, ResumeLook::Gone | ResumeLook::Drop) {
        let removed = std::fs::remove_file(path).is_ok();
        if removed {
            tracing::info!("Kept-open session forgotten: the game left it ({look:?})");
        }
        return removed;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(ResumeLook);
    impl ResumeProbe for Fixed {
        fn look(&self, _: &Value, _: bool) -> ResumeLook {
            self.0
        }
    }

    fn file(dir: &Path) -> std::path::PathBuf {
        let p = dir.join(FILE);
        let v = serde_json::json!({ "key": "k", "clock": {"t": 1} });
        std::fs::write(&p, v.to_string()).unwrap();
        p
    }

    #[test]
    fn the_file_goes_only_on_gone_or_drop() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(dir.path());
        assert!(!check(&p, &Fixed(ResumeLook::Keep), true) && pending(&p));
        assert!(!check(&p, &Fixed(ResumeLook::NoReading), true) && pending(&p));
        assert!(check(&p, &Fixed(ResumeLook::Drop), true));
        assert!(!pending(&p));
        let p = file(dir.path());
        assert!(check(&p, &Fixed(ResumeLook::Gone), false));
        assert!(!pending(&p));
    }

    #[test]
    fn the_probe_gets_the_stored_clock_and_the_game_state() {
        struct Echo;
        impl ResumeProbe for Echo {
            fn look(&self, clock: &Value, running: bool) -> ResumeLook {
                if clock["t"] == 1 && running {
                    ResumeLook::Keep
                } else {
                    ResumeLook::Drop
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let p = file(dir.path());
        assert!(!check(&p, &Echo, true));
        assert!(check(&p, &Echo, false));
    }

    #[test]
    fn no_resume_forgets_the_session_only_when_the_game_is_gone() {
        let v = serde_json::json!({});
        assert_eq!(NoResume.look(&v, false), ResumeLook::Gone);
        assert_eq!(NoResume.look(&v, true), ResumeLook::NoReading);
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE);
        std::fs::write(&p, "{}").unwrap();
        assert!(!check(&p, &Fixed(ResumeLook::Drop), false) && pending(&p));
    }
}
