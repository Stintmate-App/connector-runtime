//! The control app against a fake hub on a random pipe: the control interface, the services'
//! status, managed mode, the second launch, the window's API and the conformance check.
//! Nothing here touches a real user's pipes, key, descriptor, registry or Run key.

/// The example connector with its UI catalog.
static SPEC: stintmate_connector_platform::spec::ConnectorSpec =
    stintmate_connector_testing::EXAMPLE_SPEC;

use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use stintmate_connector_control::api::{Command, OptionsChange, UiEvent};
use stintmate_connector_control::app::{ControlApp, Environment};
use stintmate_connector_control::autostart::{Autostart, MemRunKey};
use stintmate_connector_control::endpoint::ServeConfig;
use stintmate_connector_control::instance::{to_json, to_value};
use stintmate_connector_control::managed::ParentWatch;
use stintmate_connector_control::services::{Host, SpawnError, Started};
use stintmate_connector_control::texts::Texts;
use stintmate_ipc::blocking::Client;
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::Error;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{Key, KeySource, format_key};
use stintmate_ipc::naming;

const KEY: Key = [9u8; 32];

struct NullHost;
impl Host for NullHost {
    fn is_running(&self, _: &str) -> bool {
        false
    }
    fn signal_stop(&self, _: &str) -> bool {
        false
    }
    fn terminate_pid(&self, _: u32, _: &[PathBuf], _: Option<SystemTime>) -> bool {
        false
    }
    fn adopt(&self, _: u32, _: &[PathBuf], _: Option<SystemTime>) -> bool {
        false
    }
    fn start(&self, _: &[String], _: &Path) -> Result<Started, SpawnError> {
        Err(SpawnError::NotFound)
    }
    fn close_job(&self) {}
    fn game_running(&self) -> bool {
        false
    }
}

struct Rig {
    app: Arc<ControlApp>,
    events: Receiver<UiEvent>,
    pipe: String,
    registry: PathBuf,
    dir: tempfile::TempDir,
}

fn rig(parent_watch: Box<dyn Fn(u32) -> ParentWatch + Send + Sync>) -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let data = ws.join("connectors").join("example");
    std::fs::create_dir_all(ws.join("ipc")).unwrap();
    std::fs::write(ws.join("ipc").join("key"), format_key(&KEY)).unwrap();
    let user = format!(
        "t{}x{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let pipe = naming::pipe_path_for("connector-example", None, &user).unwrap();
    let registry = ws.join("ipc").join("endpoints");
    let exe = ws.join("example-connector-ui.exe");
    let app = ControlApp::new(Environment {
        spec: &SPEC,
        resume: Arc::new(stintmate_connector_control::NoResume),
        workspace: ws.clone(),
        data_dir: data,
        data_source: "test".into(),
        workspace_args: vec![],
        ui_exe: exe.clone(),
        recorder_exe: ws.join("example-recorder.exe"), // not installed
        recorder_mutex: format!("Local\\Test.{user}.R"),
        recorder_stop: format!("Local\\Test.{user}.R.Stop"),
        recorder_extra_args: vec![],
        stop_timeout: Duration::from_secs(1),
        host: Box::new(NullHost),
        autostart: Autostart::new(SPEC.autostart_value, Box::new(MemRunKey::default()), &exe),
        texts: Texts::english(&SPEC),
        parent_watch,
        tray_available: true,
        time: None,
    });
    let events = app.take_events().unwrap();
    app.serve(ServeConfig {
        pipe: Some(pipe.clone()),
        registry: Some(registry.clone()),
        key: KeySource::Bytes(KEY),
    })
    .unwrap();
    Rig {
        app,
        events,
        pipe,
        registry,
        dir,
    }
}

fn real_watch() -> Box<dyn Fn(u32) -> ParentWatch + Send + Sync> {
    Box::new(ParentWatch::new)
}

fn client(pipe: &str, info: PeerInfo) -> Client {
    let c = Client::new(pipe, info, KeySource::Bytes(KEY)).unwrap();
    c.start();
    c.wait_connected(Duration::from_secs(5)).unwrap();
    c
}

fn hub(pipe: &str) -> Client {
    client(pipe, PeerInfo::new("hub"))
}

fn call(c: &Client, method: &str, params: Json) -> Result<Json, (String, Json)> {
    match c.call(method, to_value(&params)) {
        Ok(v) => Ok(to_json(&v)),
        Err(Error::Remote(r)) => Err((r.code, r.details.map_or(Json::Null, |d| to_json(&d)))),
        Err(e) => panic!("{method}: {e}"),
    }
}

fn watch_channel(c: &Client, channel: &str) -> Arc<Mutex<Option<Json>>> {
    let last: Arc<Mutex<Option<Json>>> = Arc::default();
    let l = last.clone();
    let handle = c.subscribe(channel, Default::default(), move |d| {
        if let stintmate_ipc::blocking::Delivery::Event(ev) = d {
            *l.lock().unwrap() = Some(to_json(&ev.data));
        }
    });
    std::mem::forget(handle); // dropping it would end the subscription
    last
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drain(events: &Receiver<UiEvent>) -> Vec<UiEvent> {
    events
        .try_iter()
        .filter(|e| *e != UiEvent::Changed)
        .collect()
}

#[test]
fn the_endpoint_passes_the_conformance_check_and_registers_itself() {
    let r = rig(real_watch());
    let key_file = r.dir.path().join("ipc").join("key");
    let argv: Vec<String> = [
        "server",
        &r.pipe,
        "--fast",
        "--key-file",
        key_file.to_str().unwrap(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let code = std::thread::spawn(move || stintmate_ipc_check::cli::main(&argv))
        .join()
        .unwrap();
    assert_eq!(code, 0, "stintmate-ipc-check server --fast");
    let entries: Vec<_> = std::fs::read_dir(&r.registry).unwrap().flatten().collect();
    assert!(
        entries.iter().any(|e| e
            .file_name()
            .to_string_lossy()
            .contains("connector-example")),
        "{entries:?}"
    );
    r.app.unserve();
}

#[test]
fn control_methods_answer_check_their_params_and_the_state_has_every_field() {
    let r = rig(real_watch());
    let h = hub(&r.pipe);
    let state = watch_channel(&h, "control.state");
    wait_for("control.state", || state.lock().unwrap().is_some());
    let s = state.lock().unwrap().clone().unwrap();
    for k in [
        "app",
        "pid",
        "version",
        "managed",
        "parent_pid",
        "window",
        "exiting",
        "autostart",
        "level",
        "services",
        "connector",
        "game_running",
        "pitwall",
        "outbox",
        "start_with_app",
    ] {
        assert!(s.get(k).is_some(), "{k} in {s}");
    }
    assert_eq!(s["app"], "example-connector-ui");
    assert_eq!(s["connector"]["id"], "example");
    let svc = &s["services"][0];
    assert_eq!(svc["id"], "recorder");
    assert_eq!(svc["delegable"], true);
    assert_eq!(svc["default"], "on");
    assert_eq!(svc["forced_stop"], false);
    assert_eq!(s["start_with_app"], json!(["recorder"]));

    assert_eq!(
        call(&h, "control.show", json!({})).unwrap(),
        json!({"window": "shown"})
    );
    assert!(drain(&r.events).contains(&UiEvent::ShowWindow));
    assert_eq!(
        call(&h, "control.hide", json!({})).unwrap(),
        json!({"window": "hidden"})
    );
    assert_eq!(
        call(&h, "control.autostart.set", json!({"enabled": true})).unwrap(),
        json!({"autostart": true})
    );
    wait_for("autostart in the state", || {
        state
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s["autostart"] == true)
    });
    assert_eq!(
        call(&h, "control.autostart.set", json!({"enabled": "yes"}))
            .unwrap_err()
            .0,
        "bad_request"
    );
    let (code, details) =
        call(&h, "control.service.start", json!({"service": "nope"})).unwrap_err();
    assert_eq!(
        (code.as_str(), &details["services"]),
        ("bad_request", &json!(["recorder"]))
    );
    assert_eq!(
        call(&h, "control.managed", json!({"parent_pid": -3}))
            .unwrap_err()
            .0,
        "bad_request"
    );
    assert_eq!(
        call(
            &h,
            "control.activity.act",
            json!({"id": "x", "action": "y"})
        )
        .unwrap_err()
        .0,
        "not_found"
    );
    // The recorder is not installed here: the start is answered, with that state.
    let r2 = call(&h, "control.service.start", json!({"service": "recorder"})).unwrap();
    assert_eq!(r2, json!({"service": "recorder", "state": "not_installed"}));

    let cat = call(&h, "control.catalog.get", json!({})).unwrap();
    assert_eq!(cat["catalog"]["catalog"], "example-connector");
    assert_eq!(cat["rev"].as_str().unwrap().len(), 12);
    assert_eq!(
        cat,
        call(&h, "control.catalog.get", json!({})).unwrap(),
        "a stable rev"
    );
    r.app.unserve();
}

#[test]
fn a_service_declares_its_status_and_the_state_follows_with_the_os_pid() {
    let r = rig(real_watch());
    let h = hub(&r.pipe);
    let state = watch_channel(&h, "control.state");
    let mut info = PeerInfo::new("connector-example");
    info.process = Some("recorder".into());
    let rec = Client::new(&r.pipe, info, KeySource::Bytes(KEY)).unwrap();
    let ch = rec
        .state_channel("service.status", Value::Nil, &[1])
        .unwrap();
    rec.start();
    rec.wait_connected(Duration::from_secs(5)).unwrap();
    ch.set(&to_value(&json!({
        "service": "recorder", "pid": 1, "started_at": "2026-10-23T10:00:00+00:00",
        "game_running": true, "mapped": true, "session_open": true, "pitwall": "connected",
        "session_id": 14, "lap": 3,
        "outbox": {"pending": 4, "oldest_at": "2026-10-23T10:01:00+00:00"},
    })))
    .unwrap();
    // Nothing runs (the host says the mutex is free), so the service is "off" even with a
    // status; the status is still on the board with the connection's pid.
    wait_for("the status", || {
        r.app.snapshot().0["services"][0]["detail"]["session_id"] == 14
    });
    assert_eq!(
        r.app.snapshot().0["services"][0]["detail"]["pid"],
        json!(std::process::id()),
        "the OS's word, not the service's"
    );
    rec.close();
    wait_for("the disconnect", || {
        r.app.snapshot().0["services"][0]["detail"]
            .get("session_id")
            .is_none()
    });
    let _ = state;
    r.app.unserve();
}

#[test]
fn managed_mode_refuses_strangers_quits_for_the_boss_and_falls_back() {
    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let a = alive.clone();
    let r = rig(Box::new(move |pid| {
        let a = a.clone();
        ParentWatch::with(
            pid,
            Box::new(|_| Some(SystemTime::UNIX_EPOCH)),
            Box::new(move |_, _| a.load(std::sync::atomic::Ordering::SeqCst)),
        )
    }));
    let h = hub(&r.pipe);
    assert_eq!(
        call(&h, "control.managed", json!({"parent_pid": 1})).unwrap(),
        json!({"managed": true, "parent_pid": 1})
    );
    assert!(drain(&r.events).contains(&UiEvent::ManagedChanged(true)));
    let (code, details) = call(&h, "control.quit", json!({})).unwrap_err();
    assert_eq!(code, "managed");
    assert_eq!(details["managed_by"], 1);
    // Services start only on request while managed, and a hub command is not remembered.
    r.app.start(None);
    assert!(!r.app.config().start_with_app.is_empty());
    // The managing app goes away: standalone again.
    alive.store(false, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(2100));
    r.app.tick();
    assert!(drain(&r.events).contains(&UiEvent::ManagedChanged(false)));
    assert_eq!(r.app.snapshot().0["managed"], false);
    r.app.unserve();
}

#[test]
fn a_bogus_parent_pid_is_refused() {
    let r = rig(real_watch());
    let h = hub(&r.pipe);
    assert_eq!(
        call(
            &h,
            "control.managed",
            json!({"parent_pid": 4_000_000_000_u64})
        )
        .unwrap_err()
        .0,
        "bad_request"
    );
    assert_eq!(
        call(
            &h,
            "control.managed",
            json!({"parent_pid": std::process::id()})
        )
        .unwrap(),
        json!({"managed": true, "parent_pid": std::process::id()})
    );
    // The boss (this process) may quit it; the answer comes first, then the exit.
    assert_eq!(
        call(&h, "control.quit", json!({})).unwrap(),
        json!({"exiting": true})
    );
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        match r.events.recv_timeout(Duration::from_millis(200)) {
            Ok(UiEvent::Quit) => break,
            _ => assert!(Instant::now() < end, "no Quit event"),
        }
    }
}

#[test]
fn a_second_launch_calls_the_running_instance() {
    let r = rig(real_watch());
    let reply = stintmate_connector_control::instance::call(
        &SPEC,
        &r.pipe,
        || KeySource::Bytes(KEY),
        "control.show",
        &json!({}),
        Duration::from_secs(3),
    );
    assert_eq!(reply, Some(json!({"window": "shown"})));
    assert!(drain(&r.events).contains(&UiEvent::ShowWindow));
    r.app.unserve();
    let gone = stintmate_connector_control::instance::call(
        &SPEC,
        &r.pipe,
        || KeySource::Bytes(KEY),
        "control.show",
        &json!({}),
        Duration::from_millis(600),
    );
    assert_eq!(gone, None, "nobody answered");
}

#[test]
fn the_window_process_reaches_the_api_over_the_endpoint() {
    let r = rig(real_watch());
    let w = hub(&r.pipe);
    let snap = watch_channel(&w, stintmate_connector_control::api::SNAPSHOT_CHANNEL);
    wait_for("the snapshot", || snap.lock().unwrap().is_some());
    let s = snap.lock().unwrap().clone().unwrap();
    assert_eq!(s["config"]["recorder_hz"], SPEC.recorder_hz);
    assert_eq!(s["data_source"], "test");

    let cmd = |c: Command| {
        call(
            &w,
            stintmate_connector_control::api::COMMAND_METHOD,
            c.to_json(),
        )
    };
    cmd(Command::WindowVisible(true)).unwrap();
    wait_for("window shown", || {
        snap.lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s["window"] == "shown")
    });
    cmd(Command::SetOptions(OptionsChange {
        recorder_hz: Some(25.0),
        start_minimized: Some(false),
        logs_default: Some("ui".into()),
    }))
    .unwrap();
    wait_for("the options", || {
        snap.lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s["config"]["recorder_hz"] == 25.0)
    });
    let saved =
        std::fs::read_to_string(r.dir.path().join("connectors/example/ui_config.json")).unwrap();
    assert!(saved.contains("\"recorder_hz\": 25.0"), "{saved}");
    assert_eq!(
        cmd(Command::SetOptions(OptionsChange {
            recorder_hz: Some(900.0),
            ..Default::default()
        }))
        .unwrap_err()
        .0,
        "bad_request"
    );
    // A hand switch starts it (here: not installed).
    cmd(Command::Switch {
        service: "recorder".into(),
        on: true,
    })
    .unwrap();
    wait_for("not installed", || {
        snap.lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s["services"][0]["state"] == "not_installed")
    });
    cmd(Command::OpenDataFolder).unwrap();
    assert!(
        drain(&r.events)
            .iter()
            .any(|e| matches!(e, UiEvent::OpenFolder(p) if p.ends_with("example")))
    );
    assert_eq!(
        call(
            &w,
            stintmate_connector_control::api::COMMAND_METHOD,
            json!({"command": "?"})
        )
        .unwrap_err()
        .0,
        "bad_request"
    );
    r.app.unserve();
}

#[test]
fn activity_is_published_at_most_twice_a_second_and_notices_reach_the_channel() {
    let r = rig(real_watch());
    let h = hub(&r.pipe);
    let act = watch_channel(&h, "control.activity");
    wait_for("the initial activity", || act.lock().unwrap().is_some());
    assert_eq!(act.lock().unwrap().clone().unwrap(), json!({"items": []}));
    let ep = r.app_endpoint();
    ep.set_activity(vec![
        json!({"id": "a", "kind": "info", "title": "A", "lifetime": "held"}),
    ]);
    ep.set_activity(vec![
        json!({"id": "b", "kind": "info", "title": "B", "lifetime": "held"}),
    ]);
    wait_for("the coalesced item", || {
        act.lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a["items"][0]["id"] == "b")
    });
    r.app.unserve();
}

impl Rig {
    fn app_endpoint(&self) -> Arc<stintmate_connector_control::endpoint::ControlEndpoint> {
        self.app.endpoint_for_tests()
    }
}

// --- `--quit` (cli::quit) against unique names and a random pipe ---

fn hold(
    names: &stintmate_connector_platform::single::Names,
) -> stintmate_connector_platform::single::SingleInstance {
    let i = stintmate_connector_platform::single::SingleInstance::acquire(&names.ui).unwrap();
    assert!(!i.already_running());
    i
}

#[test]
fn quit_with_nothing_running_is_0() {
    let names = stintmate_connector_platform::single::Names::unique(&SPEC, "quit-none");
    let code = stintmate_connector_control::cli::quit(
        &SPEC,
        &names,
        "unused",
        || KeySource::Bytes(KEY),
        Duration::from_secs(1),
    );
    assert_eq!(code, 0);
}

#[test]
fn quit_asks_an_unmanaged_instance_to_exit_and_waits_for_it() {
    let names = stintmate_connector_platform::single::Names::unique(&SPEC, "quit-ok");
    let r = rig(real_watch());
    let instance = hold(&names);
    let pipe = r.pipe.clone();
    let n = names.clone();
    let t = std::thread::spawn(move || {
        stintmate_connector_control::cli::quit(
            &SPEC,
            &n,
            &pipe,
            || KeySource::Bytes(KEY),
            Duration::from_secs(10),
        )
    });
    // The app answers, then exits: on its Quit event the "process" (the mutex) goes away.
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        match r.events.recv_timeout(Duration::from_millis(200)) {
            Ok(UiEvent::Quit) => break,
            _ => assert!(Instant::now() < end, "no Quit event"),
        }
    }
    drop(instance);
    assert_eq!(t.join().unwrap(), 0);
}

#[test]
fn quit_refuses_a_managed_instance_with_4_and_leaves_it_running() {
    let names = stintmate_connector_platform::single::Names::unique(&SPEC, "quit-managed");
    let r = rig(Box::new(|pid| {
        ParentWatch::with(
            pid,
            Box::new(|_| Some(SystemTime::UNIX_EPOCH)),
            Box::new(|_, _| true),
        )
    }));
    let _instance = hold(&names);
    let h = hub(&r.pipe);
    call(&h, "control.managed", json!({"parent_pid": 1})).unwrap();
    let code = stintmate_connector_control::cli::quit(
        &SPEC,
        &names,
        &r.pipe,
        || KeySource::Bytes(KEY),
        Duration::from_secs(2),
    );
    assert_eq!(code, 4);
    std::thread::sleep(Duration::from_millis(800)); // QUIT_DELAY would have passed
    assert!(
        !drain(&r.events).contains(&UiEvent::Quit),
        "the managed app was quit"
    );
    assert!(stintmate_connector_platform::single::is_running(&names.ui));
    r.app.unserve();
}

#[test]
fn quit_is_1_when_the_instance_does_not_answer_or_does_not_go() {
    let names = stintmate_connector_platform::single::Names::unique(&SPEC, "quit-stuck");
    let _instance = hold(&names);
    // Nobody serves this pipe.
    let code = stintmate_connector_control::cli::quit(
        &SPEC,
        &names,
        &naming::pipe_path_for("connector-example", None, "nobodyhome").unwrap(),
        || KeySource::Bytes(KEY),
        Duration::from_secs(1),
    );
    assert_eq!(code, 1);
    // It answers but its process stays (the mutex is never released): still running after the wait.
    let r = rig(real_watch());
    let code = stintmate_connector_control::cli::quit(
        &SPEC,
        &names,
        &r.pipe,
        || KeySource::Bytes(KEY),
        Duration::from_secs(1),
    );
    assert_eq!(code, 1);
}
