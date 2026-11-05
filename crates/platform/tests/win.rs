//! Windows helpers: mutex, stop event, game check, processes, job object, timer.
//! Every object name is unique per test and
//! every child process is killed by a guard, so a test leaves nothing behind.
#![cfg(windows)]

mod common;

use common::SPEC;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use stintmate_connector_platform::game::{self, GameWatch};
use stintmate_connector_platform::proc::{self, Child, KillOnCloseJob, quote_arg, spawn_suspended};
use stintmate_connector_platform::single::{self, Names, SingleInstance, StopEvent};
use stintmate_connector_platform::timer::WaitableTimer;
use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

fn ping() -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
    root.join("System32").join("ping.exe")
}

fn other_image() -> PathBuf {
    ping().with_file_name("cmd.exe")
}

/// A child that runs about a minute, killed when the guard drops.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.kill();
        self.0.wait(Duration::from_secs(5));
    }
}

fn spawn_ping() -> Guard {
    let cwd = std::env::temp_dir();
    let argv = [
        ping().into_os_string(),
        "-n".into(),
        "60".into(),
        "127.0.0.1".into(),
    ];
    Guard(spawn_suspended(&argv, &cwd, 0).expect("spawn ping"))
}

fn inheritable(h: HANDLE) -> bool {
    let mut flags = 0u32;
    // SAFETY: valid handle and out pointer.
    assert_ne!(unsafe { GetHandleInformation(h, &mut flags) }, 0);
    flags & HANDLE_FLAG_INHERIT != 0
}

// ---------------------------------------------------------------- mutex and stop event

#[test]
fn default_names_are_the_production_ones() {
    let n = Names::production(&SPEC);
    assert_eq!(n.ui, "Local\\ExampleConnector.UI");
    assert_eq!(n.recorder, "Local\\ExampleConnector.Recorder");
    assert_eq!(n.recorder_stop, "Local\\ExampleConnector.Recorder.Stop");
    assert_ne!(Names::unique(&SPEC, "a"), Names::unique(&SPEC, "a"));
    assert!(
        Names::unique(&SPEC, "a")
            .recorder
            .starts_with("Local\\ExampleConnector.Test.")
    );
}

#[test]
fn single_instance_and_is_running() {
    let name = Names::unique(&SPEC, "mutex").recorder;
    assert!(!single::is_running(&name));
    let first = SingleInstance::acquire(&name).unwrap();
    assert!(!first.already_running());
    assert!(single::is_running(&name));
    // Another thread stands in for another process (a mutex is owned per thread).
    let n2 = name.clone();
    let refused =
        std::thread::spawn(move || SingleInstance::acquire(&n2).unwrap().already_running())
            .join()
            .unwrap();
    assert!(refused);
    assert!(
        single::is_running(&name),
        "probing and failing never releases the owner"
    );
    drop(first);
    assert!(!single::is_running(&name));
    // And it can be taken again.
    assert!(!SingleInstance::acquire(&name).unwrap().already_running());
}

#[test]
fn probe_does_not_block_a_starting_instance() {
    let name = Names::unique(&SPEC, "probe").ui;
    // Someone holds only a probing handle open (as `is_running` briefly does).
    let _owner = SingleInstance::acquire(&name).unwrap();
    let n2 = name.clone();
    let probe = std::thread::spawn(move || single::is_running(&n2))
        .join()
        .unwrap();
    assert!(probe);
}

#[test]
fn stop_event_signal_and_wait() {
    let name = Names::unique(&SPEC, "stop").recorder_stop;
    assert!(!single::signal_stop(&name), "no event, no service");
    let ev = StopEvent::create(&name).unwrap();
    assert!(!ev.is_set());
    let t = Instant::now();
    assert!(!ev.wait(Duration::from_millis(60)));
    assert!(t.elapsed() >= Duration::from_millis(50));
    assert!(single::signal_stop(&name));
    assert!(ev.is_set());
    assert!(ev.wait(Duration::from_secs(5)), "returns at once when set");
}

#[test]
fn stop_event_is_signalled_from_another_thread_while_waiting() {
    let name = Names::unique(&SPEC, "stopwait").recorder_stop;
    let ev = StopEvent::create(&name).unwrap();
    let n2 = name.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        single::signal_stop(&n2)
    });
    assert!(ev.wait(Duration::from_secs(5)));
    assert!(h.join().unwrap());
}

#[test]
fn stop_event_close_does_not_set_and_resets_stale() {
    let name = Names::unique(&SPEC, "stale").recorder_stop;
    let old = StopEvent::create(&name).unwrap();
    old.set();
    // A new instance while the old one is still exiting starts clean.
    let new = StopEvent::create(&name).unwrap();
    assert!(!new.is_set());
    drop(old);
    assert!(!new.is_set(), "closing never signals");
}

#[test]
fn handles_are_not_inheritable() {
    let ev = StopEvent::create(&Names::unique(&SPEC, "inh").recorder_stop).unwrap();
    assert!(!inheritable(ev.raw()));
    let job = KillOnCloseJob::create();
    assert!(job.ok() && !inheritable(job.raw().unwrap()));
    let timer = WaitableTimer::new().unwrap();
    drop(timer);
}

// ---------------------------------------------------------------- game check

#[test]
fn process_names_lists_this_test_and_the_game_is_not_running() {
    let me = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    assert!(game::process_names().unwrap().contains(&me));
    assert!(game::any_running(&[me.as_str()]));
    assert!(!game::any_running(&["no-such-process-platform-test.exe"]));
}

#[test]
fn game_watch_rate_limits_the_check() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let calls = Arc::new(Mutex::new(0u32));
    let (c2, n2) = (calls.clone(), now.clone());
    let mut w = GameWatch::with(
        move || {
            *c2.lock().unwrap() += 1;
            true
        },
        move || *n2.lock().unwrap(),
        Duration::from_secs(5),
    );
    assert!(w.due());
    assert!(w.running(false));
    assert!(!w.due());
    assert!(w.running(false));
    assert_eq!(*calls.lock().unwrap(), 1);
    *now.lock().unwrap() += Duration::from_secs(6);
    assert!(w.due());
    w.running(false);
    assert_eq!(*calls.lock().unwrap(), 2);
    w.running(true);
    assert_eq!(*calls.lock().unwrap(), 3);
}

// ---------------------------------------------------------------- processes and the job

#[test]
fn quoting_follows_the_command_line_rules() {
    assert_eq!(quote_arg("plain"), "plain");
    assert_eq!(quote_arg(""), "\"\"");
    assert_eq!(
        quote_arg("C:\\Program Files\\x.exe"),
        "\"C:\\Program Files\\x.exe\""
    );
    assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    assert_eq!(quote_arg("dir with space\\"), "\"dir with space\\\\\"");
    assert_eq!(quote_arg("a\\b"), "a\\b");
}

#[test]
fn spawn_suspended_runs_nothing_until_resumed_and_reports_image_and_start() {
    let g = spawn_ping();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(g.0.try_wait(), None);
    assert!(
        game::process_names()
            .unwrap()
            .contains(&"ping.exe".to_string())
    );
    let image = proc::process_image(g.0.raw()).expect("image");
    assert!(image.to_string_lossy().to_lowercase().ends_with("ping.exe"));
    let started = proc::process_start_time(g.0.raw()).expect("start time");
    let age = SystemTime::now()
        .duration_since(started)
        .unwrap_or_default();
    assert!(age < Duration::from_secs(30), "{age:?}");
    assert_eq!(
        g.0.wait(Duration::from_millis(100)),
        None,
        "suspended, still running"
    );
    assert!(g.0.resume());
}

#[test]
fn spawn_of_a_missing_program_is_an_error() {
    let r = spawn_suspended(&["C:\\no\\such\\program.exe"], &std::env::temp_dir(), 0);
    assert!(r.is_err());
}

#[test]
fn job_kills_the_child_on_close() {
    let g = spawn_ping();
    let mut job = KillOnCloseJob::create();
    assert!(job.ok());
    assert!(job.assign_child(&g.0));
    assert!(g.0.resume());
    assert_eq!(
        g.0.wait(Duration::from_millis(300)),
        None,
        "running while the job is open"
    );
    job.close();
    assert!(
        g.0.wait(Duration::from_secs(10)).is_some(),
        "the job killed it"
    );
}

#[test]
fn adopt_a_running_child_then_close_kills_it() {
    let g = spawn_ping();
    assert!(g.0.resume());
    let mut job = KillOnCloseJob::create();
    let image = ping();
    assert!(job.adopt(g.0.pid(), &[image.as_path()], None));
    job.close();
    assert!(g.0.wait(Duration::from_secs(10)).is_some());
}

#[test]
fn adopt_refuses_other_images_and_reused_pids() {
    let g = spawn_ping();
    assert!(g.0.resume());
    let job = KillOnCloseJob::create();
    let other = other_image();
    let right = ping();
    assert!(
        !job.adopt(g.0.pid(), &[other.as_path()], None),
        "wrong image"
    );
    let long_ago = SystemTime::now() - Duration::from_secs(3600);
    assert!(
        !job.adopt(g.0.pid(), &[right.as_path()], Some(long_ago)),
        "started after the reported start"
    );
    assert!(!job.adopt(0, &[right.as_path()], None));
    assert_eq!(g.0.try_wait(), None, "refusals leave the process alone");
    assert!(job.adopt(g.0.pid(), &[right.as_path()], Some(SystemTime::now())));
}

#[test]
fn terminate_pid_checks_the_image() {
    let g = spawn_ping();
    assert!(g.0.resume());
    let (right, other) = (ping(), other_image());
    assert!(!proc::terminate_pid(g.0.pid(), &[other.as_path()], None, 9));
    assert_eq!(g.0.try_wait(), None);
    assert!(proc::terminate_pid(g.0.pid(), &[right.as_path()], None, 7));
    assert_eq!(g.0.wait(Duration::from_secs(5)), Some(7));
}

#[test]
fn a_failed_job_never_panics() {
    let job = KillOnCloseJob::default();
    assert!(!job.ok());
    let p: &Path = &ping();
    assert!(!job.assign(std::ptr::null_mut()));
    assert!(!job.adopt(std::process::id(), &[p], None));
}

// ---------------------------------------------------------------- timer

#[test]
fn timer_period_jitter_at_50_hz() {
    let timer = WaitableTimer::new().unwrap();
    let period = Duration::from_millis(20);
    timer.start_periodic(period).unwrap();
    timer.wait(); // align to the first tick
    let mut last = Instant::now();
    let mut errors_ms = Vec::new();
    for _ in 0..150 {
        timer.wait();
        let now = Instant::now();
        errors_ms.push((now - last).as_secs_f64() * 1000.0 - 20.0);
        last = now;
    }
    timer.cancel();
    let mean = errors_ms.iter().sum::<f64>() / errors_ms.len() as f64;
    let mut abs: Vec<f64> = errors_ms.iter().map(|e| e.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let p95 = abs[abs.len() * 95 / 100];
    eprintln!(
        "high_res={} mean error {mean:.3} ms, p95 |error| {p95:.3} ms, max {:.3} ms",
        timer.is_high_res(),
        abs.last().unwrap()
    );
    // The period does not drift: 150 ticks add up to 3 s within a few ms.
    assert!(mean.abs() < 0.5, "mean period error {mean} ms");
    if timer.is_high_res() {
        assert!(p95 < 4.0, "p95 jitter {p95} ms");
    } else {
        assert!(p95 < 20.0, "p95 jitter {p95} ms (normal timer)");
    }
}

#[test]
fn timer_one_shot_sleep_and_timeout() {
    let timer = WaitableTimer::new().unwrap();
    let t = Instant::now();
    timer.sleep(Duration::from_millis(30)).unwrap();
    let e = t.elapsed();
    assert!(
        e >= Duration::from_millis(25) && e < Duration::from_millis(200),
        "{e:?}"
    );
    // Nothing armed: the wait times out.
    assert!(!timer.wait_timeout(Duration::from_millis(20)));
    timer.start_once(Duration::from_millis(10)).unwrap();
    assert!(timer.wait_timeout(Duration::from_secs(2)));
}

#[test]
fn procinfo_sees_this_process_and_its_chain() {
    use stintmate_connector_platform::procinfo::*;
    let me = std::process::id();
    let started = pid_start_time(me).expect("own start time");
    assert!(process_alive(me, Some(started)));
    assert!(!process_alive(me, Some(started + Duration::from_secs(60))));
    assert!(!process_alive(0, None));
    let chain = process_chain(me, 4);
    assert_eq!(chain.first().map(|c| c.0), Some(me));
    assert!(describe_process(Some(me), 4).contains(&me.to_string()));
    assert_eq!(describe_process(None, 4), "unknown");
}

#[test]
fn tagged_names_are_separate_from_the_production_ones() {
    let p = Names::production(&SPEC);
    let t = Names::tagged(&SPEC, "desk1");
    assert_eq!(t.ui, r"Local\ExampleConnector.UI.desk1");
    assert_eq!(t.window, r"Local\ExampleConnector.Window.desk1");
    assert_eq!(t.recorder, r"Local\ExampleConnector.Recorder.desk1");
    assert_eq!(
        t.recorder_stop,
        r"Local\ExampleConnector.Recorder.Stop.desk1"
    );
    for (a, b) in [
        (&p.ui, &t.ui),
        (&p.window, &t.window),
        (&p.recorder, &t.recorder),
        (&p.recorder_stop, &t.recorder_stop),
    ] {
        assert_ne!(a, b);
    }
}

#[test]
fn game_watch_without_class_checks_every_interval() {
    use stintmate_connector_platform::game::{CHECK_INTERVAL, FULL_CHECK, GameProbe};
    use stintmate_connector_platform::spec::GameSpec;
    static BARE: GameSpec = GameSpec {
        exes: &["example-sim.exe"],
        window_title: None,
        window_class: None,
        cheap_hint: None,
    };
    static HINTED: GameSpec = GameSpec {
        exes: &["example-sim.exe"],
        window_title: None,
        window_class: None,
        cheap_hint: Some(|| false),
    };
    // Nothing cheap to look for: the snapshot runs at the watch interval, not a minute later.
    assert_eq!(GameProbe::new(&BARE).full_interval(), CHECK_INTERVAL);
    // A known window, or a hint, makes the long backstop safe.
    assert_eq!(GameProbe::new(&SPEC.game).full_interval(), FULL_CHECK);
    assert_eq!(GameProbe::new(&HINTED).full_interval(), FULL_CHECK);
    // The game is not running here, with or without a hint.
    assert!(!GameProbe::new(&BARE).running());
    assert!(!GameProbe::new(&HINTED).running());
    assert!(!game::game_running(&BARE));
    assert!(game::game_pid(&BARE).is_none());
    assert!(!game::game_window_seen(&BARE));
    assert!(!game::game_window_seen(&HINTED));
    let _ = GameWatch::new(&BARE);
}
