//! The window process's life against a control app on a random pipe, a temporary workspace and
//! a unique mutex: one process at most, a second Open raises it instead of starting another,
//! it reports itself through the endpoint, and it ends when asked to close. The window it
//! opens is this test's own; nothing else is touched.

use stintmate_connector_control::api::{Command, UiEvent};
use stintmate_connector_control::app::{ControlApp, Environment};
use stintmate_connector_control::autostart::{Autostart, MemRunKey};
use stintmate_connector_control::endpoint::ServeConfig;
use stintmate_connector_control::managed::ParentWatch;
use stintmate_connector_control::services::{Host, SpawnError, Started};
use stintmate_connector_control::texts::Texts;
use stintmate_connector_control::window_host::{Outcome, WindowHost};
mod common;

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_testing::EXAMPLE_SPEC;
use stintmate_ipc::keyfile::{Key, KeySource, format_key};
use stintmate_ipc::naming;

const KEY: Key = [7u8; 32];

/// The example connector with its UI catalog.
static SPEC: ConnectorSpec = ConnectorSpec {
    ui_catalog: include_str!("fixtures/en.example.json"),
    ..EXAMPLE_SPEC
};

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

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn one_window_process_at_most_and_it_ends_when_asked_to_close() {
    // The window needs a desktop with OpenGL; CI runners (GitHub's, and our own, which runs as
    // a service) have none: the workflow sets STINTMATE_NO_DESKTOP. Runs on a developer's PC.
    if std::env::var_os("STINTMATE_NO_DESKTOP").is_some_and(|v| !v.is_empty()) {
        eprintln!("skipped: no desktop with OpenGL (STINTMATE_NO_DESKTOP)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    std::fs::create_dir_all(ws.join("ipc")).unwrap();
    let key_file = ws.join("ipc").join("key");
    std::fs::write(&key_file, format_key(&KEY)).unwrap();
    std::fs::write(ws.join("stintmate.json"), r#"{"schema": 1}"#).unwrap();
    let user = format!("w{}x{}", std::process::id(), std::process::id() % 97);
    let pipe = naming::pipe_path_for("connector-example", None, &user).unwrap();
    let exe = ws.join("example-connector-ui.exe");
    let app = ControlApp::new(Environment {
        spec: &SPEC,
        resume: std::sync::Arc::new(stintmate_connector_control::NoResume),
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("example"),
        data_source: "test".into(),
        workspace_args: vec![],
        ui_exe: exe.clone(),
        recorder_exe: ws.join("example-recorder.exe"),
        recorder_mutex: format!("Local\\Test.{user}.R"),
        recorder_stop: format!("Local\\Test.{user}.R.Stop"),
        recorder_extra_args: vec![],
        stop_timeout: Duration::from_secs(1),
        host: Box::new(NullHost),
        autostart: Autostart::new(SPEC.autostart_value, Box::new(MemRunKey::default()), &exe),
        texts: Texts::english(&SPEC),
        parent_watch: Box::new(ParentWatch::new),
        tray_available: true,
        time: None,
    });
    let events = app.take_events().unwrap();
    app.serve(ServeConfig {
        pipe: Some(pipe.clone()),
        registry: Some(ws.join("ipc").join("endpoints")),
        key: KeySource::Bytes(KEY),
    })
    .unwrap();

    let mutex = format!("Local\\ExampleConnector.Test.{user}.Window");
    let args: Vec<String> = [
        "--workspace",
        &ws.display().to_string(),
        "--pipe",
        &pipe,
        "--key-file",
        &key_file.display().to_string(),
        "--mutex",
        &mutex,
    ]
    .map(String::from)
    .to_vec();
    let host = WindowHost::new(common::window_exe(), args);

    // The first Open starts the process; it reports itself as shown through the endpoint.
    let Outcome::Started(pid) = host.ensure(None).unwrap() else {
        panic!("nothing was running");
    };
    wait_for("the window to report itself", 30, || {
        app.snapshot().0["window"] == "shown"
    });

    // A second Open starts nothing: same pid, and the request is published for the window.
    let before = app.snapshot().0["window_request"]["seq"].as_u64().unwrap();
    app.dispatch(Command::ShowLogs).unwrap();
    assert_eq!(host.ensure(Some("logs")).unwrap(), Outcome::Running(pid));
    assert_eq!(host.pid(), Some(pid));
    let after = app.snapshot().0["window_request"].clone();
    assert_eq!(after["seq"].as_u64().unwrap(), before + 1);
    assert_eq!(after["action"], "logs");
    assert!(
        events.try_iter().any(|e| matches!(e, UiEvent::ShowLogs)),
        "the tray was told"
    );

    // A second process of the window exits at once (single instance), the first stays.
    let second = std::process::Command::new(common::window_exe())
        .args(["--workspace", &ws.display().to_string(), "--mutex", &mutex])
        .args([
            "--pipe",
            &pipe,
            "--key-file",
            &key_file.display().to_string(),
        ])
        .status()
        .unwrap();
    assert!(second.success());
    assert_eq!(host.pid(), Some(pid));

    // Hide: the window closes itself, says so, and the host sees the process gone.
    let reply = stintmate_connector_control::instance::call(
        &SPEC,
        &pipe,
        || KeySource::Bytes(KEY),
        "control.hide",
        &json!({}),
        Duration::from_secs(3),
    );
    assert_eq!(reply, Some(json!({"window": "hidden"})));
    wait_for("the window process to end", 20, || host.poll_exit());
    assert_eq!(host.pid(), None);
    assert_eq!(app.snapshot().0["window"], "hidden");

    // And the next Open starts a new one.
    let Outcome::Started(next) = host.ensure(None).unwrap() else {
        panic!("a new process");
    };
    assert_ne!(next, pid);
    host.close();
    assert_eq!(host.pid(), None);
    app.unserve();
}
