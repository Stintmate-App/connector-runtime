//! Is the game running?
//!
//! The game's shared memory mapping outlives the game (any process holding a handle keeps it),
//! so its existence says nothing; the game's process does. [`game_running`] takes a process
//! snapshot (`CreateToolhelp32Snapshot`, several milliseconds, more on a busy desktop) and looks for one of the game's executables (`GameSpec::exes`).
//! [`GameWatch`] rate-limits the check for the loops that ask whenever the frames stop.
//!
//! The snapshot costs several milliseconds on a busy desktop, so [`GameProbe`] (the check the
//! long-lived processes use) avoids it: while the game runs it waits on the game's process
//! handle (zero-timeout wait, microseconds); while it does not, it looks for a cheap hint that
//! the game runs (its window, `FindWindowW` by class and by title, microseconds, and the spec's
//! `cheap_hint`) and takes the snapshot only when one appears, or every [`FULL_CHECK`] as a
//! backstop (a game still starting, without its window). A game with neither a known window
//! nor a hint has nothing cheap to look for: its snapshot is taken every [`CHECK_INTERVAL`]
//! instead, so it is never seen up to a minute late.

use crate::spec::GameSpec;
use crate::win::{OwnedHandle, Win32Error};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

/// Minimum time between real checks of a [`GameWatch`].
pub const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// The longest a [`GameProbe`] goes without a process snapshot while the game is not seen.
pub const FULL_CHECK: Duration = Duration::from_secs(60);

/// The executable names of every running process (lowercase).
pub fn process_names() -> Result<Vec<String>, Win32Error> {
    Ok(processes()?.into_iter().map(|(_, n)| n).collect())
}

/// The pid of the first process named like one of `exes` (lowercase).
pub fn find_process(exes: &[&str]) -> Result<Option<u32>, Win32Error> {
    Ok(processes()?
        .into_iter()
        .find(|(_, n)| exes.contains(&n.as_str()))
        .map(|(pid, _)| pid))
}

/// Every running process: pid and executable name (lowercase).
fn processes() -> Result<Vec<(u32, String)>, Win32Error> {
    // SAFETY: plain call; the result is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(Win32Error::last("CreateToolhelp32Snapshot"));
    }
    let snap =
        OwnedHandle::new(snap).ok_or_else(|| Win32Error::last("CreateToolhelp32Snapshot"))?;
    // SAFETY: PROCESSENTRY32W is plain data; all-zero is valid.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut names = Vec::new();
    // SAFETY: valid snapshot and entry with dwSize set.
    let mut ok = unsafe { Process32FirstW(snap.raw(), &mut entry) } != 0;
    while ok {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
        names.push((entry.th32ProcessID, name));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap.raw(), &mut entry) } != 0;
    }
    Ok(names)
}

/// True if a process named like one of `exes` (lowercase) exists. If the snapshot fails it
/// answers true: never drop a session because of our own failure.
pub fn any_running(exes: &[&str]) -> bool {
    match process_names() {
        Ok(names) => names.iter().any(|n| exes.contains(&n.as_str())),
        Err(_) => true,
    }
}

/// True if one of the game's executables runs.
pub fn game_running(game: &GameSpec) -> bool {
    any_running(game.exes)
}

/// The pid of the running game, if any (names the game process, e.g. to tell one run from the
/// next).
pub fn game_pid(game: &GameSpec) -> Option<u32> {
    find_process(game.exes).ok().flatten()
}

/// A cheap hint that the game may run, confirmed by a process snapshot: its window exists (by
/// class and by exact title, when the spec names them) or the spec's `cheap_hint` says so.
/// False when the spec has neither.
pub fn game_window_seen(game: &GameSpec) -> bool {
    if let (Some(title), Some(class)) = (game.window_title, game.window_class) {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (class, title) = (wide(class), wide(title));
        // SAFETY: NUL-terminated strings that live across the calls.
        let seen = unsafe {
            !FindWindowW(class.as_ptr(), std::ptr::null()).is_null()
                || !FindWindowW(std::ptr::null(), title.as_ptr()).is_null()
        };
        if seen {
            return true;
        }
    }
    game.cheap_hint.is_some_and(|hint| hint())
}

/// The cheap, exact game check of the long-lived processes (see the module docs).
pub struct GameProbe {
    spec: &'static GameSpec,
    /// The running game's process (SYNCHRONIZE access).
    game: Option<OwnedHandle>,
    last_full: Option<Instant>,
}

impl GameProbe {
    /// No game seen yet: the first [`running`](Self::running) takes a snapshot.
    pub const fn new(spec: &'static GameSpec) -> Self {
        Self {
            spec,
            game: None,
            last_full: None,
        }
    }

    /// The longest this probe goes without a process snapshot while the game is not seen:
    /// [`FULL_CHECK`] when the spec gives something cheap to look for, else [`CHECK_INTERVAL`].
    pub fn full_interval(&self) -> Duration {
        let g = self.spec;
        if (g.window_title.is_some() && g.window_class.is_some()) || g.cheap_hint.is_some() {
            FULL_CHECK
        } else {
            CHECK_INTERVAL
        }
    }

    /// The game's process exists.
    pub fn running(&mut self) -> bool {
        if let Some(h) = &self.game {
            // SAFETY: a valid process handle; a zero timeout never blocks.
            if unsafe { WaitForSingleObject(h.raw(), 0) } == WAIT_TIMEOUT {
                return true;
            }
            self.game = None;
        }
        let every = self.full_interval();
        let due = self.last_full.is_none_or(|t| t.elapsed() >= every);
        if !due && !game_window_seen(self.spec) {
            return false;
        }
        self.last_full = Some(Instant::now());
        match find_process(self.spec.exes) {
            Ok(Some(pid)) => {
                // SAFETY: plain call; a null handle (no access) is handled below.
                let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                self.game = OwnedHandle::new(h);
                true
            }
            Ok(None) => false,
            Err(_) => true, // as `any_running`: never drop a session because of our failure
        }
    }
}

/// The process check, at most every `interval`. The check and the clock are injectable.
pub struct GameWatch {
    check: Box<dyn FnMut() -> bool + Send>,
    clock: Box<dyn Fn() -> Instant + Send>,
    interval: Duration,
    at: Option<Instant>,
    last: bool,
}

impl GameWatch {
    /// The real check of `game` every [`CHECK_INTERVAL`].
    pub fn new(game: &'static GameSpec) -> Self {
        let mut probe = GameProbe::new(game);
        Self::with(move || probe.running(), Instant::now, CHECK_INTERVAL)
    }

    /// Custom check, clock and interval (tests).
    pub fn with(
        check: impl FnMut() -> bool + Send + 'static,
        clock: impl Fn() -> Instant + Send + 'static,
        interval: Duration,
    ) -> Self {
        Self {
            check: Box::new(check),
            clock: Box::new(clock),
            interval,
            at: None,
            last: false,
        }
    }

    /// The last answer, refreshed if `force` or older than the interval.
    pub fn running(&mut self, force: bool) -> bool {
        let now = (self.clock)();
        if force || self.due_at(now) {
            self.at = Some(now);
            self.last = (self.check)();
        }
        self.last
    }

    /// A fresh check is allowed now.
    pub fn due(&self) -> bool {
        self.due_at((self.clock)())
    }

    fn due_at(&self, now: Instant) -> bool {
        self.at
            .is_none_or(|t| now.duration_since(t) >= self.interval)
    }
}
