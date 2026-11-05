//! ServiceManager against a fake operating system.

/// The example connector with its UI catalog.
static SPEC: stintmate_connector_platform::spec::ConnectorSpec =
    stintmate_connector_testing::EXAMPLE_SPEC;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use stintmate_connector_control::services::{
    Child, Host, ManagerConfig, ServiceManager, SpawnError, Started,
};
use stintmate_connector_control::texts::Texts;

#[derive(Default)]
struct World {
    now: Mutex<f64>,
    mutex_held: AtomicBool,
    event_exists: AtomicBool,
    signals: AtomicU32,
    spawned: Mutex<Vec<Vec<String>>>,
    exited: Arc<AtomicBool>,
    child_alive: Arc<AtomicBool>,
    tie: AtomicBool,
    adopt_ok: AtomicBool,
    adopts: Mutex<Vec<u32>>,
    terminated: Mutex<Vec<u32>>,
    spawn_error: Mutex<Option<SpawnError>>,
    game: AtomicBool,
    status: Mutex<Option<Value>>,
    notices: Mutex<Vec<(String, String)>>,
    /// The service exits by itself when it gets the stop event.
    obeys_stop: AtomicBool,
    closed: AtomicBool,
}

struct FakeChild(Arc<AtomicBool>, u32);
impl Child for FakeChild {
    fn pid(&self) -> u32 {
        self.1
    }
    fn try_wait(&self) -> Option<u32> {
        (!self.0.load(Ordering::SeqCst)).then_some(0)
    }
    fn wait(&self, _: Duration) -> Option<u32> {
        self.try_wait()
    }
    fn kill(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct FakeHost(Arc<World>);
impl Host for FakeHost {
    fn is_running(&self, _: &str) -> bool {
        self.0.mutex_held.load(Ordering::SeqCst) || self.0.child_alive.load(Ordering::SeqCst)
    }
    fn signal_stop(&self, _: &str) -> bool {
        if !self.0.event_exists.load(Ordering::SeqCst) {
            return false;
        }
        self.0.signals.fetch_add(1, Ordering::SeqCst);
        if self.0.obeys_stop.load(Ordering::SeqCst) {
            self.0.child_alive.store(false, Ordering::SeqCst);
            self.0.mutex_held.store(false, Ordering::SeqCst);
        }
        true
    }
    fn terminate_pid(&self, pid: u32, _: &[PathBuf], _: Option<SystemTime>) -> bool {
        self.0.terminated.lock().unwrap().push(pid);
        self.0.mutex_held.store(false, Ordering::SeqCst);
        true
    }
    fn adopt(&self, pid: u32, _: &[PathBuf], _: Option<SystemTime>) -> bool {
        self.0.adopts.lock().unwrap().push(pid);
        self.0.adopt_ok.load(Ordering::SeqCst)
    }
    fn start(&self, argv: &[String], _: &Path) -> Result<Started, SpawnError> {
        if let Some(e) = self.0.spawn_error.lock().unwrap().clone() {
            return Err(e);
        }
        self.0.spawned.lock().unwrap().push(argv.to_vec());
        self.0.child_alive.store(true, Ordering::SeqCst);
        self.0.exited.store(false, Ordering::SeqCst);
        Ok(Started {
            child: Box::new(FakeChild(self.0.child_alive.clone(), 4242)),
            tied: self.0.tie.load(Ordering::SeqCst),
        })
    }
    fn close_job(&self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
    fn game_running(&self) -> bool {
        self.0.game.load(Ordering::SeqCst)
    }
}

struct Fx {
    w: Arc<World>,
    m: ServiceManager,
    _dir: tempfile::TempDir,
}

fn fixture(installed: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("example-recorder.exe");
    if installed {
        std::fs::write(&exe, b"").unwrap();
    }
    let w = Arc::new(World::default());
    w.tie.store(true, Ordering::SeqCst);
    w.adopt_ok.store(true, Ordering::SeqCst);
    w.event_exists.store(true, Ordering::SeqCst);
    let (c, s) = (w.clone(), w.clone());
    let n = w.clone();
    let st = w.clone();
    let m = ServiceManager::new(
        ManagerConfig {
            recorder_exe: exe,
            cwd: dir.path().to_path_buf(),
            mutex: "m".into(),
            stop_event: "e".into(),
            workspace_args: vec!["--workspace".into(), "W".into()],
            extra_args: vec![],
            stop_timeout: Duration::from_secs(10),
        },
        Texts::english(&SPEC),
        Box::new(FakeHost(w.clone())),
    )
    .with_time(
        Box::new(move || *c.now.lock().unwrap()),
        Box::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)),
        Box::new(move |d| *s.now.lock().unwrap() += d.as_secs_f64()),
    )
    .with_notifier(Box::new(move |_, t, m| {
        n.notices.lock().unwrap().push((t.into(), m.into()));
    }))
    .with_statuses(Box::new(move |_| st.status.lock().unwrap().clone()));
    Fx { w, m, _dir: dir }
}

fn advance(w: &World, secs: f64) {
    *w.now.lock().unwrap() += secs;
}

fn set_status(w: &World, v: Option<Value>) {
    *w.status.lock().unwrap() = v;
}

fn recording() -> Value {
    json!({"game_running": true, "mapped": true, "session_open": true, "pitwall": "connected",
           "pid": 4242, "session_id": 14, "lap": 3})
}

#[test]
fn nothing_starts_at_construction_or_poll() {
    let mut f = fixture(true);
    f.m.poll();
    assert!(f.w.spawned.lock().unwrap().is_empty());
    assert_eq!(f.m.state("recorder").state, "off");
}

#[test]
fn start_spawns_the_recorder_with_its_arguments_and_never_doubles() {
    let mut f = fixture(true);
    let s = f.m.start("recorder", Some(25.0));
    assert!(s.running);
    assert_eq!(s.state, "starting");
    let argv = f.w.spawned.lock().unwrap()[0].clone();
    assert_eq!(&argv[1..], ["--hz", "25", "--workspace", "W"]);
    f.m.start("recorder", Some(25.0));
    assert_eq!(f.w.spawned.lock().unwrap().len(), 1);
}

#[test]
fn an_external_instance_is_adopted_not_doubled_and_failure_is_external_once() {
    let mut f = fixture(true);
    f.w.mutex_held.store(true, Ordering::SeqCst);
    set_status(&f.w, Some(json!({"pid": 77})));
    let s = f.m.start("recorder", None);
    assert!(f.w.spawned.lock().unwrap().is_empty());
    assert_eq!(
        s.ownership,
        Some(stintmate_connector_control::status::Ownership::Adopted)
    );
    assert_eq!(*f.w.adopts.lock().unwrap(), [77]);
    assert!(f.m.wanted("recorder"));

    let mut g = fixture(true);
    g.w.adopt_ok.store(false, Ordering::SeqCst);
    g.w.mutex_held.store(true, Ordering::SeqCst);
    set_status(&g.w, Some(json!({"pid": 77})));
    g.m.poll();
    g.m.poll();
    let s = g.m.state("recorder");
    assert_eq!(
        s.ownership,
        Some(stintmate_connector_control::status::Ownership::External)
    );
    assert_eq!(
        g.w.adopts.lock().unwrap().len(),
        1,
        "not retried for the same pid"
    );
    assert!(
        s.full_label.contains("not tied") || s.full_label.contains(" · "),
        "{}",
        s.full_label
    );
}

#[test]
fn adoption_waits_for_a_status_with_a_pid() {
    let mut f = fixture(true);
    f.w.mutex_held.store(true, Ordering::SeqCst);
    f.m.poll();
    assert!(f.w.adopts.lock().unwrap().is_empty());
    set_status(&f.w, Some(json!({"pid": 5, "game_running": false})));
    f.m.poll();
    assert_eq!(*f.w.adopts.lock().unwrap(), [5]);
}

#[test]
fn an_unassignable_child_is_shown_untied() {
    let mut f = fixture(true);
    f.w.tie.store(false, Ordering::SeqCst);
    let s = f.m.start("recorder", None);
    assert_eq!(
        s.ownership,
        Some(stintmate_connector_control::status::Ownership::Untied)
    );
}

#[test]
fn stop_signals_the_event_and_waits_for_the_exit() {
    let mut f = fixture(true);
    f.w.obeys_stop.store(true, Ordering::SeqCst);
    f.m.start("recorder", None);
    let s = f.m.stop("recorder");
    assert!(!s.running);
    assert!(f.w.signals.load(Ordering::SeqCst) >= 1);
    assert!(f.w.terminated.lock().unwrap().is_empty());
    assert!(
        f.w.notices.lock().unwrap().is_empty(),
        "a clean stop is not a crash"
    );
    assert!(!f.m.wanted("recorder"));
}

#[test]
fn a_stop_that_is_ignored_is_forced_after_the_timeout_and_reported() {
    let mut f = fixture(true);
    f.m.start("recorder", None);
    set_status(&f.w, Some(recording()));
    let s = f.m.request_stop("recorder");
    assert_eq!(s.state, "stopping");
    advance(&f.w, 9.0);
    assert!(f.m.poll()[0].running, "still within the timeout");
    advance(&f.w, 2.0);
    let s = f.m.poll().remove(0);
    assert!(!s.running);
    assert_eq!(s.detail.get("forced_stop"), Some(&json!(true)));
    assert_eq!(*f.w.terminated.lock().unwrap(), [4242]);
    let notes = f.w.notices.lock().unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].0.contains("forced") || notes[0].0.contains("Recorder"),
        "{notes:?}"
    );
}

#[test]
fn the_stop_signal_is_retried_until_the_event_exists() {
    let mut f = fixture(true);
    f.w.event_exists.store(false, Ordering::SeqCst);
    f.w.obeys_stop.store(true, Ordering::SeqCst);
    f.m.start("recorder", None);
    f.m.request_stop("recorder");
    assert_eq!(f.w.signals.load(Ordering::SeqCst), 0);
    f.w.event_exists.store(true, Ordering::SeqCst);
    f.m.poll();
    let s = f.m.poll().remove(0);
    assert!(f.w.signals.load(Ordering::SeqCst) >= 1);
    assert!(!s.running);
}

#[test]
fn an_unexpected_exit_is_a_crash_with_one_notification_and_a_restart_with_backoff() {
    let mut f = fixture(true);
    f.m.start("recorder", None);
    f.w.child_alive.store(false, Ordering::SeqCst);
    let s = f.m.poll().remove(0);
    assert_eq!(s.state, "crashed");
    assert_eq!(f.w.notices.lock().unwrap().len(), 1);
    f.m.poll();
    assert_eq!(f.w.notices.lock().unwrap().len(), 1, "one per minute");
    assert_eq!(
        f.w.spawned.lock().unwrap().len(),
        1,
        "not before the backoff"
    );
    advance(&f.w, 2.5);
    let s = f.m.poll().remove(0);
    assert!(s.running, "restarted after 2 s");
    assert_eq!(f.w.spawned.lock().unwrap().len(), 2);
    // Crashing at once again waits twice as long.
    f.w.child_alive.store(false, Ordering::SeqCst);
    f.m.poll();
    advance(&f.w, 3.0);
    assert!(!f.m.poll()[0].running, "4 s backoff not over");
    advance(&f.w, 1.5);
    assert!(f.m.poll()[0].running);
}

#[test]
fn restarts_stop_after_five_tries_and_a_stop_request_cancels_them() {
    let mut f = fixture(true);
    f.m.start("recorder", None);
    for _ in 0..5 {
        f.w.child_alive.store(false, Ordering::SeqCst);
        f.m.poll();
        advance(&f.w, 40.0);
        f.m.poll();
    }
    f.w.child_alive.store(false, Ordering::SeqCst);
    f.m.poll();
    advance(&f.w, 100.0);
    assert!(!f.m.poll()[0].running, "gave up");
    assert_eq!(f.w.spawned.lock().unwrap().len(), 6);

    let mut g = fixture(true);
    g.w.obeys_stop.store(true, Ordering::SeqCst);
    g.m.start("recorder", None);
    g.m.stop("recorder");
    advance(&g.w, 100.0);
    assert!(!g.m.poll()[0].running);
    assert_eq!(g.w.spawned.lock().unwrap().len(), 1);
}

#[test]
fn a_status_without_the_process_shows_off_and_a_status_from_before_our_start_is_ignored() {
    let mut f = fixture(true);
    set_status(&f.w, Some(recording()));
    assert_eq!(f.m.state("recorder").state, "off");
    f.m.start("recorder", None);
    // started_at 100 s before the app's wall clock: a lingering connection of the old run.
    set_status(
        &f.w,
        Some(
            json!({"started_at": "1970-01-12T13:45:00+00:00", "pitwall": "connected",
                    "game_running": true, "mapped": true}),
        ),
    );
    assert_eq!(f.m.poll()[0].state, "starting");
}

#[test]
fn not_installed_and_spawn_failures_do_not_panic() {
    let mut f = fixture(false);
    let s = f.m.start("recorder", None);
    assert_eq!(s.state, "not_installed");
    assert!(!s.running);

    let mut g = fixture(true);
    *g.w.spawn_error.lock().unwrap() = Some(SpawnError::NotFound);
    assert_eq!(g.m.start("recorder", None).state, "not_installed");
    *g.w.spawn_error.lock().unwrap() = Some(SpawnError::Failed("access denied".into()));
    assert_eq!(g.m.start("recorder", None).state, "crashed");
    assert!(g.w.notices.lock().unwrap()[0].1.contains("access denied"));
}

#[test]
fn stop_all_stops_everything_running_and_nothing_otherwise() {
    let mut f = fixture(true);
    f.m.stop_all();
    assert_eq!(f.w.signals.load(Ordering::SeqCst), 0);
    f.w.obeys_stop.store(true, Ordering::SeqCst);
    f.m.start("recorder", None);
    let states = f.m.stop_all();
    assert!(states.iter().all(|s| !s.running));
    f.m.close();
    assert!(f.w.closed.load(Ordering::SeqCst));
}

#[test]
fn a_shm_error_notifies_once() {
    let mut f = fixture(true);
    f.m.start("recorder", None);
    set_status(
        &f.w,
        Some(json!({"error_code": "unsupported_sim_version", "pid": 4242})),
    );
    assert_eq!(f.m.poll()[0].state, "shm_error");
    advance(&f.w, 120.0);
    f.m.poll();
    assert_eq!(f.w.notices.lock().unwrap().len(), 1);
}

#[test]
fn recorder_off_while_the_game_runs_says_so_and_the_check_is_cached() {
    let mut f = fixture(true);
    f.w.game.store(true, Ordering::SeqCst);
    let s = f.m.poll().remove(0);
    assert_eq!(s.state, "off_game_running");
    assert_eq!(s.level, stintmate_connector_control::status::Level::Warn);
    f.w.game.store(false, Ordering::SeqCst);
    assert_eq!(f.m.poll()[0].state, "off_game_running", "cached for 5 s");
    advance(&f.w, 6.0);
    assert_eq!(f.m.poll()[0].state, "off");
}
