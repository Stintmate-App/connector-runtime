//! Start, stop, adopt, restart and watch the connector's service: the recorder.
//!
//! Every collaborator is injected, so the logic is tested with fakes:
//!
//! * [`Host`]: the operating system's side (the mutex probe, the stop event, forced
//!   termination, the kill-on-close job, spawning suspended). [`WinHost`] is the real one. The
//!   child is assigned to the job before it runs, so nothing it starts can escape the job;
//! * `statuses(service)`: the service's last `service.status` on the control endpoint, `None`
//!   while it has no connection to the app;
//! * the clocks (`clock`: monotonic seconds; `wall`: the time of day), `sleep`, `notify`
//!   (error notifications only: at most one per service per minute).
//!
//! Nothing starts at construction. The app calls [`ServiceManager::poll`] once at start (that
//! is when it detects and adopts instances started by hand) and then on its timer.
//!
//! **Restart with backoff.** A service the app started or adopted that ends without a stop
//! request is restarted after 2, 4, 8, 16, 32 s (five tries; a run of a minute resets the
//! count). A stop request or [`ServiceManager::start`] by the user resets it.

use crate::resume_watch::{NoResume, ResumeProbe};
use crate::status::{self, Level, Ownership, ServiceState};
use crate::texts::Texts;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use stintmate_connector_i18n::Arg;

/// A service that has not stopped by itself after this long is terminated.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// At most one notification per service per minute.
pub const NOTIFY_INTERVAL: f64 = 60.0;
/// The game's process is asked at most this often (Recorder off).
pub const GAME_CHECK: f64 = 5.0;
/// Waits before the restarts after a crash.
pub const RESTART_BACKOFF: [f64; 5] = [2.0, 4.0, 8.0, 16.0, 32.0];
/// A run this long resets the restart count.
pub const STABLE_RUN: f64 = 60.0;
const STOP_POLL: Duration = Duration::from_millis(100);
const FORCED_EXIT_WAIT: Duration = Duration::from_secs(3);

/// A process the manager started.
pub trait Child: Send {
    /// The process id.
    fn pid(&self) -> u32;
    /// The exit code, or `None` while it runs.
    fn try_wait(&self) -> Option<u32>;
    /// Waits up to `timeout` for the exit.
    fn wait(&self, timeout: Duration) -> Option<u32>;
    /// Terminates it.
    fn kill(&self);
}

/// A started service.
pub struct Started {
    /// The process.
    pub child: Box<dyn Child>,
    /// It is in the kill-on-close job.
    pub tied: bool,
}

/// Why a service could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// The executable is not there.
    NotFound,
    /// Anything else.
    Failed(String),
}

/// The operating system's side of the manager.
pub trait Host: Send + Sync {
    /// A process holds the named single-instance mutex.
    fn is_running(&self, mutex: &str) -> bool;
    /// Signals the named stop event; false if there is no such event (yet).
    fn signal_stop(&self, event: &str) -> bool;
    /// Terminates `pid` if it runs one of `images` (and started before `started_before`).
    fn terminate_pid(
        &self,
        pid: u32,
        images: &[PathBuf],
        started_before: Option<SystemTime>,
    ) -> bool;
    /// Puts a running process in the job (same checks); false if refused.
    fn adopt(&self, pid: u32, images: &[PathBuf], started_before: Option<SystemTime>) -> bool;
    /// Starts `argv` suspended, assigns it to the job and lets it run.
    fn start(&self, argv: &[String], cwd: &Path) -> Result<Started, SpawnError>;
    /// Closes the job: Windows kills anything still in it.
    fn close_job(&self);
    /// The game's process exists.
    fn game_running(&self) -> bool;
}

/// The real host.
#[cfg(windows)]
pub struct WinHost {
    job: std::sync::Mutex<stintmate_connector_platform::proc::KillOnCloseJob>,
    game: std::sync::Mutex<stintmate_connector_platform::game::GameProbe>,
}

#[cfg(windows)]
impl WinHost {
    /// A host that watches `game`'s processes.
    pub fn new(game: &'static stintmate_connector_platform::spec::GameSpec) -> Self {
        Self {
            job: std::sync::Mutex::new(stintmate_connector_platform::proc::KillOnCloseJob::create()),
            game: std::sync::Mutex::new(stintmate_connector_platform::game::GameProbe::new(game)),
        }
    }
}

#[cfg(windows)]
struct WinChild(stintmate_connector_platform::proc::Child);

#[cfg(windows)]
impl Child for WinChild {
    fn pid(&self) -> u32 {
        self.0.pid()
    }
    fn try_wait(&self) -> Option<u32> {
        self.0.try_wait()
    }
    fn wait(&self, timeout: Duration) -> Option<u32> {
        self.0.wait(timeout)
    }
    fn kill(&self) {
        self.0.kill();
    }
}

#[cfg(windows)]
fn refs(images: &[PathBuf]) -> Vec<&Path> {
    images.iter().map(PathBuf::as_path).collect()
}

#[cfg(windows)]
impl Host for WinHost {
    fn is_running(&self, mutex: &str) -> bool {
        stintmate_connector_platform::single::is_running(mutex)
    }
    fn signal_stop(&self, event: &str) -> bool {
        stintmate_connector_platform::single::signal_stop(event)
    }
    fn terminate_pid(
        &self,
        pid: u32,
        images: &[PathBuf],
        started_before: Option<SystemTime>,
    ) -> bool {
        stintmate_connector_platform::proc::terminate_pid(pid, &refs(images), started_before, 1)
    }
    fn adopt(&self, pid: u32, images: &[PathBuf], started_before: Option<SystemTime>) -> bool {
        let job = self.job.lock().unwrap_or_else(|p| p.into_inner());
        job.adopt(pid, &refs(images), started_before)
    }
    fn start(&self, argv: &[String], cwd: &Path) -> Result<Started, SpawnError> {
        let child =
            stintmate_connector_platform::proc::spawn_suspended(argv, cwd, 0).map_err(|e| {
                if matches!(e.code, 2 | 3) {
                    SpawnError::NotFound
                } else {
                    SpawnError::Failed(e.to_string())
                }
            })?;
        let tied = self
            .job
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .assign_child(&child);
        child.resume();
        Ok(Started {
            child: Box::new(WinChild(child)),
            tied,
        })
    }
    fn close_job(&self) {
        self.job.lock().unwrap_or_else(|p| p.into_inner()).close();
    }
    fn game_running(&self) -> bool {
        self.game
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running()
    }
}

/// What the manager needs to know about the install.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// the recorder's executable.
    pub recorder_exe: PathBuf,
    /// The recorder's working folder.
    pub cwd: PathBuf,
    /// The recorder's single-instance mutex.
    pub mutex: String,
    /// The recorder's stop event.
    pub stop_event: String,
    /// `--workspace <dir>` (and `--allow-synced`) for every process the app starts.
    pub workspace_args: Vec<String>,
    /// Further arguments (tests pass the pipes).
    pub extra_args: Vec<String>,
    /// The forced stop after this long.
    pub stop_timeout: Duration,
}

type Clock = Box<dyn Fn() -> f64 + Send + Sync>;
type Wall = Box<dyn Fn() -> SystemTime + Send + Sync>;
type Sleep = Box<dyn Fn(Duration) + Send + Sync>;
type Notifier = Box<dyn Fn(&str, &str, &str) + Send + Sync>;
type Statuses = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

#[derive(Default)]
struct Track {
    child: Option<Box<dyn Child>>,
    ownership: Option<Ownership>,
    adopt_tried_pid: Option<u32>,
    started_wall: Option<SystemTime>,
    started_at: Option<f64>,
    stop_requested_at: Option<f64>,
    stop_signalled: bool,
    forced: bool,
    crashed: bool,
    not_installed: bool,
    was_running: bool,
    last_notify_at: Option<f64>,
    last_alert: Option<&'static str>,
    state: Option<ServiceState>,
    hz: Option<f64>,
    want_on: bool,
    restarts: usize,
    restart_at: Option<f64>,
}

/// Supervises the services.
pub struct ServiceManager {
    cfg: ManagerConfig,
    texts: Texts,
    host: Box<dyn Host>,
    clock: Clock,
    wall: Wall,
    sleep: Sleep,
    notify: Notifier,
    statuses: Statuses,
    tracks: BTreeMap<String, Track>,
    game_checked_at: Option<f64>,
    game_running: bool,
    resume: std::sync::Arc<dyn ResumeProbe>,
}

impl ServiceManager {
    /// A manager with the real clocks and no notifier; set the rest with the `with_*`
    /// methods.
    pub fn new(cfg: ManagerConfig, texts: Texts, host: Box<dyn Host>) -> Self {
        let t0 = std::time::Instant::now();
        Self {
            cfg,
            texts,
            host,
            clock: Box::new(move || t0.elapsed().as_secs_f64()),
            wall: Box::new(SystemTime::now),
            sleep: Box::new(std::thread::sleep),
            notify: Box::new(|_, _, _| {}),
            statuses: Box::new(|_| None),
            tracks: crate::SERVICES
                .iter()
                .map(|s| ((*s).to_string(), Track::default()))
                .collect(),
            game_checked_at: None,
            game_running: false,
            resume: std::sync::Arc::new(NoResume),
        }
    }

    /// Sets the connector's look at the game for the kept-open session (default: [`NoResume`]).
    pub fn with_resume(mut self, resume: std::sync::Arc<dyn ResumeProbe>) -> Self {
        self.resume = resume;
        self
    }

    /// Replaces the clocks and the sleep (tests).
    pub fn with_time(mut self, clock: Clock, wall: Wall, sleep: Sleep) -> Self {
        self.clock = clock;
        self.wall = wall;
        self.sleep = sleep;
        self
    }

    /// Sets the error notifier: `(service, title, message)`.
    pub fn with_notifier(mut self, notify: Notifier) -> Self {
        self.notify = notify;
        self
    }

    /// Sets where the services' last `service.status` comes from.
    pub fn with_statuses(mut self, statuses: Statuses) -> Self {
        self.statuses = statuses;
        self
    }

    /// The texts in use.
    pub fn texts(&self) -> &Texts {
        &self.texts
    }

    fn images(&self) -> Vec<PathBuf> {
        vec![self.cfg.recorder_exe.clone()]
    }

    /// The command line of a service, or `None` when the recorder is not installed.
    pub fn argv(&self, hz: Option<f64>) -> Option<Vec<String>> {
        if !self.cfg.recorder_exe.exists() {
            return None;
        }
        let mut argv = vec![self.cfg.recorder_exe.display().to_string()];
        if let Some(hz) = hz {
            argv.push("--hz".into());
            argv.push(format!("{hz}"));
        }
        argv.extend(self.cfg.workspace_args.iter().cloned());
        argv.extend(self.cfg.extra_args.iter().cloned());
        Some(argv)
    }

    // ---- queries ----------------------------------------------------------------------

    /// The state of a service (the last refresh).
    pub fn state(&mut self, name: &str) -> ServiceState {
        match self.tracks.get(name).and_then(|t| t.state.clone()) {
            Some(s) => s,
            None => self.refresh(name),
        }
    }

    /// Every service's state, in display order.
    pub fn states(&mut self) -> Vec<ServiceState> {
        crate::SERVICES.iter().map(|n| self.state(n)).collect()
    }

    /// Refreshes every service: detects exits, adopts external instances, enforces stop
    /// timeouts, restarts, raises error notifications. Cheap: a mutex probe and a status read
    /// per service.
    pub fn poll(&mut self) -> Vec<ServiceState> {
        crate::SERVICES.iter().map(|n| self.refresh(n)).collect()
    }

    /// The service was started or adopted by the app and has not been asked to stop.
    pub fn wanted(&self, name: &str) -> bool {
        self.tracks.get(name).is_some_and(|t| t.want_on)
    }

    // ---- actions ----------------------------------------------------------------------

    /// Starts a service unless it already runs (then it is adopted, never doubled).
    pub fn start(&mut self, name: &str, hz: Option<f64>) -> ServiceState {
        let state = self.refresh(name);
        if state.running {
            return state;
        }
        if let Some(t) = self.tracks.get_mut(name) {
            t.restarts = 0;
            t.restart_at = None;
            t.hz = hz;
        }
        self.spawn(name)
    }

    fn spawn(&mut self, name: &str) -> ServiceState {
        let hz = self.tracks.get(name).and_then(|t| t.hz);
        let argv = self.argv(hz);
        if let Some(t) = self.tracks.get_mut(name) {
            t.not_installed = argv.is_none();
            t.crashed = false;
            t.forced = false;
            t.stop_requested_at = None;
            t.last_alert = None;
            t.restart_at = None;
        }
        let Some(argv) = argv else {
            return self.refresh(name);
        };
        match self.host.start(&argv, &self.cfg.cwd) {
            Err(SpawnError::NotFound) => {
                if let Some(t) = self.tracks.get_mut(name) {
                    t.not_installed = true;
                }
            }
            Err(SpawnError::Failed(why)) => {
                if let Some(t) = self.tracks.get_mut(name) {
                    t.crashed = true;
                }
                let title = self.texts.t(
                    "notice.start_failed",
                    &[("service", status::display_name(&self.texts, name).into())],
                );
                self.notify_limited(name, &title, &why);
            }
            Ok(started) => {
                let now = (self.clock)();
                let wall = (self.wall)();
                if let Some(t) = self.tracks.get_mut(name) {
                    t.started_wall = Some(wall);
                    t.started_at = Some(now);
                    t.ownership = Some(if started.tied {
                        Ownership::App
                    } else {
                        Ownership::Untied
                    });
                    t.child = Some(started.child);
                    t.was_running = true;
                    t.want_on = true;
                }
            }
        }
        self.refresh(name)
    }

    /// Asks a service to stop cleanly (its stop event) without waiting; [`poll`](Self::poll)
    /// forces it after the timeout.
    pub fn request_stop(&mut self, name: &str) -> ServiceState {
        let state = self.refresh(name);
        if !state.running {
            return state;
        }
        let now = (self.clock)();
        if let Some(t) = self.tracks.get_mut(name) {
            t.want_on = false;
            t.restart_at = None;
            if t.stop_requested_at.is_none() {
                t.stop_requested_at = Some(now);
                t.stop_signalled = false;
            }
        }
        self.signal(name);
        self.refresh(name)
    }

    /// Clean stop, waiting up to the stop timeout; then forced (with a notification).
    pub fn stop(&mut self, name: &str) -> ServiceState {
        self.request_stop(name);
        self.wait_stopped(&[name.to_string()]).remove(0)
    }

    /// For Exit: stops every running service cleanly, forcing after the timeout. Blocks.
    pub fn stop_all(&mut self) -> Vec<ServiceState> {
        let names: Vec<String> = crate::SERVICES
            .iter()
            .filter(|n| self.refresh(n).running)
            .map(|n| (*n).to_string())
            .collect();
        for n in &names {
            self.request_stop(n);
        }
        self.wait_stopped(&names);
        self.states()
    }

    /// Releases the job (Windows kills anything still in it). Call after [`stop_all`](Self::stop_all).
    pub fn close(&mut self) {
        self.host.close_job();
    }

    // ---- internals --------------------------------------------------------------------

    fn wait_stopped(&mut self, names: &[String]) -> Vec<ServiceState> {
        let deadline =
            (self.clock)() + (self.cfg.stop_timeout + FORCED_EXIT_WAIT).as_secs_f64() + 1.0;
        loop {
            let states: Vec<ServiceState> = names.iter().map(|n| self.refresh(n)).collect();
            if !states.iter().any(|s| s.running) || (self.clock)() >= deadline {
                return states;
            }
            (self.sleep)(STOP_POLL);
        }
    }

    fn signal(&mut self, name: &str) {
        let event = self.cfg.stop_event.clone();
        if let Some(t) = self.tracks.get_mut(name)
            && !t.stop_signalled
        {
            t.stop_signalled = self.host.signal_stop(&event);
        }
    }

    fn alive(&self, name: &str) -> bool {
        let child_alive = self
            .tracks
            .get(name)
            .and_then(|t| t.child.as_ref())
            .is_some_and(|c| c.try_wait().is_none());
        child_alive || self.host.is_running(&self.cfg.mutex)
    }

    fn notify_limited(&mut self, name: &str, title: &str, message: &str) {
        let now = (self.clock)();
        let Some(t) = self.tracks.get_mut(name) else {
            return;
        };
        if t.last_notify_at
            .is_some_and(|at| now - at < NOTIFY_INTERVAL)
        {
            return;
        }
        t.last_notify_at = Some(now);
        (self.notify)(name, title, message);
    }

    fn display(&self, name: &str) -> Arg {
        status::display_name(&self.texts, name).into()
    }

    fn force(&mut self, name: &str, st: Option<&Value>) {
        let images = self.images();
        let pid = st.and_then(|s| s.get("pid")).and_then(Value::as_u64);
        let before = status::started_time(st);
        if let Some(t) = self.tracks.get_mut(name) {
            t.forced = true;
        }
        if let Some(pid) = pid.and_then(|p| u32::try_from(p).ok()) {
            self.host.terminate_pid(pid, &images, before);
        }
        if let Some(c) = self.tracks.get(name).and_then(|t| t.child.as_ref()) {
            c.kill();
            c.wait(FORCED_EXIT_WAIT);
        }
        let title = self
            .texts
            .t("notice.forced.title", &[("service", self.display(name))]);
        let message = self.texts.t(
            "notice.forced.message",
            &[(
                "seconds",
                format!("{:.0}", self.cfg.stop_timeout.as_secs_f64()).into(),
            )],
        );
        self.notify_limited(name, &title, &message);
    }

    /// The service process is gone: clean stop, forced stop or crash.
    fn ended(&mut self, name: &str) {
        let now = (self.clock)();
        let Some(t) = self.tracks.get_mut(name) else {
            return;
        };
        let requested = t.stop_requested_at.is_some();
        let ran_for = t.started_at.map(|s| now - s);
        let crashed = !requested;
        let restart = crashed && t.want_on;
        if crashed {
            t.crashed = true;
        }
        if restart {
            if ran_for.is_some_and(|r| r >= STABLE_RUN) {
                t.restarts = 0;
            }
            if let Some(wait) = RESTART_BACKOFF.get(t.restarts) {
                t.restart_at = Some(now + wait);
                t.restarts += 1;
            }
        }
        t.child = None;
        t.ownership = None;
        t.adopt_tried_pid = None;
        t.started_wall = None;
        t.started_at = None;
        t.stop_requested_at = None;
        t.stop_signalled = false;
        t.was_running = false;
        if crashed {
            let title = self
                .texts
                .t("notice.crashed.title", &[("service", self.display(name))]);
            let message = self.texts.s("notice.see_logs");
            self.notify_limited(name, &title, &message);
        }
    }

    fn refresh(&mut self, name: &str) -> ServiceState {
        let mut st = (self.statuses)(name);
        let started_wall = self.tracks.get(name).and_then(|t| t.started_wall);
        if let (Some(s), Some(wall)) = (st.as_ref(), started_wall)
            && let Some(ts) = status::started_time(Some(s))
            && ts + Duration::from_secs(1) < wall
        {
            st = None; // an instance from before our start (its connection lingers)
        }

        let child_alive = match self.tracks.get(name).and_then(|t| t.child.as_ref()) {
            Some(c) if c.try_wait().is_none() => true,
            Some(_) => {
                self.ended(name);
                false
            }
            None => false,
        };
        let held = self.host.is_running(&self.cfg.mutex);
        let mut running = child_alive || held;

        let has_child = self.tracks.get(name).is_some_and(|t| t.child.is_some());
        if running && !has_child {
            // An instance the app did not start (or whose child we lost): adopt it.
            let (own, tried) = self
                .tracks
                .get(name)
                .map_or((None, None), |t| (t.ownership, t.adopt_tried_pid));
            if let Some(t) = self.tracks.get_mut(name) {
                t.was_running = true;
            }
            if matches!(own, None | Some(Ownership::External)) {
                let pid = st
                    .as_ref()
                    .and_then(|s| s.get("pid"))
                    .and_then(Value::as_u64)
                    .and_then(|p| u32::try_from(p).ok());
                match pid {
                    Some(pid) if Some(pid) != tried => {
                        let images = self.images();
                        let adopted =
                            self.host
                                .adopt(pid, &images, status::started_time(st.as_ref()));
                        if let Some(t) = self.tracks.get_mut(name) {
                            t.adopt_tried_pid = Some(pid);
                            t.ownership = Some(if adopted {
                                t.want_on = true;
                                Ownership::Adopted
                            } else {
                                Ownership::External
                            });
                        }
                    }
                    _ => {
                        if let Some(t) = self.tracks.get_mut(name)
                            && t.ownership.is_none()
                        {
                            t.ownership = Some(Ownership::External);
                        }
                    }
                }
            }
        } else if !running && self.tracks.get(name).is_some_and(|t| t.was_running) {
            self.ended(name);
        }

        // A stop was asked: repeat the signal (a just-started service may not have its event
        // yet) and force after the timeout.
        let stop_at = self.tracks.get(name).and_then(|t| t.stop_requested_at);
        if running && let Some(at) = stop_at {
            self.signal(name);
            let forced = self.tracks.get(name).is_some_and(|t| t.forced);
            if !forced && (self.clock)() - at >= self.cfg.stop_timeout.as_secs_f64() {
                self.force(name, st.as_ref());
                running = self.alive(name);
                if !running {
                    self.ended(name);
                }
            }
        }

        // Restart with backoff.
        let due = self.tracks.get(name).is_some_and(|t| {
            !running && t.want_on && t.restart_at.is_some_and(|at| (self.clock)() >= at)
        });
        if due {
            tracing::warn!("Restarting the {name} after it ended");
            return self.spawn(name);
        }

        let mut state = status::recorder_state(&self.texts, st.as_ref(), running);
        let (stopping, ownership, forced, crashed, not_installed, alert) = {
            let t = &self.tracks[name];
            (
                t.stop_requested_at.is_some(),
                t.ownership,
                t.forced,
                t.crashed,
                t.not_installed,
                t.last_alert,
            )
        };
        let mut new_alert = None;
        if running {
            state.set_ownership(&self.texts, ownership);
            if stopping {
                let label = self.texts.s("status.stopping");
                state.set(&self.texts, "stopping", label, Level::Ok);
            }
            if state.state == "shm_error" {
                new_alert = Some("shm_error");
                if alert != new_alert {
                    let title = self.texts.s("notice.shm_error.title");
                    let message = self.texts.s("notice.shm_error.message");
                    self.notify_limited(name, &title, &message);
                }
            }
        } else {
            if not_installed {
                let label = status::not_installed_label(&self.texts);
                state.set(&self.texts, "not_installed", label, Level::Error);
            } else if crashed {
                let label = self.texts.s("status.crashed");
                state.set(&self.texts, "crashed", label, Level::Error);
            }
            if forced {
                state.detail.insert("forced_stop".into(), Value::Bool(true));
            }
            if name == status::RECORDER && state.state == "off" && self.game_runs() {
                status::recorder_off_while_game_runs(&self.texts, &mut state);
            }
        }
        if let Some(t) = self.tracks.get_mut(name) {
            t.last_alert = new_alert;
            t.state = Some(state.clone());
        }
        state
    }

    /// Something may change without an event: a service runs (its process can end), is wanted
    /// (a restart is due), is stopping, or an instance the app did not start was seen. The
    /// supervisor ticks every second then, and only rarely when this is false.
    pub fn busy(&self) -> bool {
        self.tracks.values().any(|t| {
            t.child.is_some()
                || t.was_running
                || t.stop_requested_at.is_some()
                || (t.want_on && t.restart_at.is_some())
        })
    }

    /// A kept-open session waits at `resume` and no recorder runs: the app should look at the
    /// game every second (see [`crate::resume_watch`]).
    pub fn watching(&self, resume: &Path) -> bool {
        crate::resume_watch::pending(resume) && !self.alive("recorder")
    }

    /// One look at the game for the kept-open session at `resume`; the file is deleted when the
    /// game left the session. True when deleted.
    pub fn watch_resume(&mut self, resume: &Path) -> bool {
        if !self.watching(resume) {
            return false;
        }
        let running = self.game_runs();
        crate::resume_watch::check(resume, self.resume.as_ref(), running)
    }

    /// The game's process exists, from the same cached check (`control.state` when no service
    /// says it).
    pub fn game_running(&mut self) -> bool {
        self.game_runs()
    }

    fn game_runs(&mut self) -> bool {
        let now = (self.clock)();
        if self.game_checked_at.is_none_or(|at| now - at >= GAME_CHECK) {
            self.game_checked_at = Some(now);
            self.game_running = self.host.game_running();
        }
        self.game_running
    }
}

/// Replacement clocks for the manager (tests).
pub struct TimeSource {
    /// Monotonic seconds.
    pub clock: Box<dyn Fn() -> f64 + Send + Sync>,
    /// The time of day.
    pub wall: Box<dyn Fn() -> SystemTime + Send + Sync>,
    /// Sleeping.
    pub sleep: Box<dyn Fn(Duration) + Send + Sync>,
}
