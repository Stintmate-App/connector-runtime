//! `--stintmate-hold` and `--stintmate-release` (`hold::hold` / `hold::release`) against a fake
//! hub endpoint on a random pipe: the replies the hub's endpoint defines (contract.md
//! section 7 of the connector kit) and the exit statuses `hold` assigns. Unique mutex names, a temporary registry and a test key:
//! nothing here touches a real hub, pipes, key or workspace.

/// The example connector with its UI catalog.
static SPEC: stintmate_connector_platform::spec::ConnectorSpec =
    stintmate_connector_testing::EXAMPLE_SPEC;

use serde_json::{Value as Json, json};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stintmate_connector_control::hold::{self, HubTarget};
use stintmate_connector_control::instance::{to_json, to_value};
use stintmate_connector_platform::single::{Names, SingleInstance};
use stintmate_ipc::blocking::{Endpoint, MethodOpts};
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::RemoteError;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{Key, KeySource};
use stintmate_ipc::naming;

const KEY: Key = [7u8; 32];

type Replies = Arc<Mutex<Vec<Result<Json, (String, Json)>>>>;
type Seen = Arc<Mutex<Vec<(String, Json)>>>;

/// A fake hub: `hub.connector.hold` / `hub.connector.release` answer from the queue (the last
/// reply repeats), and every call is recorded.
struct FakeHub {
    ep: Endpoint,
    pipe: String,
    seen: Seen,
    _dir: tempfile::TempDir,
}

fn unique_pipe() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let user = format!(
        "h{}x{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    naming::pipe_path_for("hub", None, &user).unwrap()
}

fn fake_hub(hold: Vec<Result<Json, (String, Json)>>, delay: Duration) -> FakeHub {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe();
    let ep = Endpoint::builder(PeerInfo::new("hub"), KeySource::Bytes(KEY))
        .pipe(pipe.clone())
        .registry(dir.path().join("endpoints"))
        .build()
        .unwrap();
    let seen: Seen = Seen::default();
    let queue: Replies = Arc::new(Mutex::new(hold));
    for method in ["hub.connector.hold", "hub.connector.release"] {
        let (seen, queue) = (seen.clone(), queue.clone());
        ep.method(method, MethodOpts::default().ordered(true), move |_, p| {
            seen.lock().unwrap().push((method.into(), to_json(&p)));
            std::thread::sleep(delay);
            let mut q = queue.lock().unwrap();
            let next = if q.len() > 1 {
                q.remove(0)
            } else {
                q[0].clone()
            };
            match next {
                Ok(j) => Ok(to_value(&j)),
                Err((code, details)) => {
                    let mut e = RemoteError::new(&code, "test");
                    if !details.is_null() {
                        e.details = Some(to_value(&details));
                    }
                    Err(e)
                }
            }
        })
        .unwrap();
    }
    ep.start().unwrap();
    FakeHub {
        ep,
        pipe,
        seen,
        _dir: dir,
    }
}

impl Drop for FakeHub {
    fn drop(&mut self) {
        self.ep.stop();
    }
}

fn target(pipe: &str, mutex: &str) -> HubTarget {
    HubTarget {
        spec: &SPEC,
        pipe: pipe.into(),
        mutex: mutex.into(),
        connect_wait: Duration::from_secs(1),
        call_timeout: Duration::from_secs(2),
    }
}

fn held() -> Result<Json, (String, Json)> {
    Ok(json!({"id": "example", "state": "held", "restart": true, "expires_in_s": 300}))
}

fn stopping() -> Result<Json, (String, Json)> {
    Ok(json!({"id": "example", "state": "stopping", "restart": true, "expires_in_s": 300}))
}

fn key() -> KeySource {
    KeySource::Bytes(KEY)
}

fn run_hold(h: &FakeHub, names: &Names, wait: Duration) -> i32 {
    hold::hold(
        names,
        &target(&h.pipe, "Local\\Test.none"),
        key,
        "update",
        wait,
    )
}

#[test]
fn held_at_once_is_0_and_sends_the_spec_params() {
    let h = fake_hub(vec![held()], Duration::ZERO);
    let names = Names::unique(&SPEC, "hold-ok");
    assert_eq!(run_hold(&h, &names, Duration::from_secs(5)), 0);
    let seen = h.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "hub.connector.hold");
    assert_eq!(
        seen[0].1,
        json!({"id": "example", "purpose": "update", "ttl_s": 300})
    );
}

#[test]
fn stopping_is_polled_until_held() {
    let h = fake_hub(vec![stopping(), stopping(), held()], Duration::ZERO);
    let names = Names::unique(&SPEC, "hold-poll");
    let t = Instant::now();
    assert_eq!(run_hold(&h, &names, Duration::from_secs(10)), 0);
    assert_eq!(h.seen.lock().unwrap().len(), 3);
    assert!(
        t.elapsed() >= Duration::from_millis(900),
        "polls every 0.5 s"
    );
}

#[test]
fn still_stopping_after_the_wait_is_1() {
    let h = fake_hub(vec![stopping()], Duration::ZERO);
    let names = Names::unique(&SPEC, "hold-slow");
    assert_eq!(run_hold(&h, &names, Duration::from_millis(700)), 1);
}

#[test]
fn held_but_a_mutex_of_the_connector_still_exists_is_1() {
    let h = fake_hub(vec![held()], Duration::ZERO);
    let names = Names::unique(&SPEC, "hold-running");
    let _ui = SingleInstance::acquire(&names.ui).unwrap();
    assert_eq!(run_hold(&h, &names, Duration::from_millis(700)), 1);
}

#[test]
fn not_found_is_5() {
    let h = fake_hub(vec![Err(("not_found".into(), Json::Null))], Duration::ZERO);
    let names = Names::unique(&SPEC, "hold-nf");
    assert_eq!(run_hold(&h, &names, Duration::from_secs(2)), 5);
}

#[test]
fn recording_is_7_and_other_busy_is_6() {
    let names = Names::unique(&SPEC, "hold-busy");
    let h = fake_hub(
        vec![Err(("busy".into(), json!({"recording": true})))],
        Duration::ZERO,
    );
    assert_eq!(run_hold(&h, &names, Duration::from_secs(2)), 7);
    let h = fake_hub(
        vec![Err(("busy".into(), json!({"state": "starting"})))],
        Duration::ZERO,
    );
    assert_eq!(run_hold(&h, &names, Duration::from_secs(2)), 6);
}

#[test]
fn an_older_hub_without_the_method_is_6() {
    let names = Names::unique(&SPEC, "hold-old");
    let h = fake_hub(
        vec![Err(("unknown_method".into(), Json::Null))],
        Duration::ZERO,
    );
    assert_eq!(run_hold(&h, &names, Duration::from_secs(2)), 6);
    let h = fake_hub(vec![Err(("internal".into(), Json::Null))], Duration::ZERO);
    assert_eq!(run_hold(&h, &names, Duration::from_secs(2)), 6);
}

#[test]
fn a_call_that_times_out_is_6() {
    let names = Names::unique(&SPEC, "hold-timeout");
    let h = fake_hub(vec![held()], Duration::from_secs(4));
    let mut t = target(&h.pipe, "Local\\Test.none");
    t.call_timeout = Duration::from_secs(1);
    let start = Instant::now();
    assert_eq!(
        hold::hold(&names, &t, key, "update", Duration::from_secs(5)),
        6
    );
    assert!(start.elapsed() < Duration::from_secs(4));
}

#[test]
fn no_pipe_and_no_mutex_is_5_and_a_held_mutex_without_a_pipe_is_6() {
    let names = Names::unique(&SPEC, "hold-nopipe");
    let pipe = unique_pipe();
    let mutex = Names::unique(&SPEC, "hub-mutex").ui;
    let t = target(&pipe, &mutex);
    assert_eq!(
        hold::hold(&names, &t, key, "update", Duration::from_secs(2)),
        5
    );
    let _hub_alive = SingleInstance::acquire(&mutex).unwrap();
    let start = Instant::now();
    assert_eq!(
        hold::hold(&names, &t, key, "update", Duration::from_secs(2)),
        6
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(hold::release(&t, key, true), 6);
}

#[test]
fn the_production_waits_are_the_specs() {
    let t = HubTarget::production(&SPEC);
    assert_eq!(t.connect_wait, Duration::from_secs(3));
    assert_eq!(t.call_timeout, Duration::from_secs(5));
    assert_eq!(t.mutex, "Local\\Stintmate.Hub");
}

#[test]
fn release_sends_start_and_is_0_even_when_nothing_was_held() {
    let h = fake_hub(
        vec![Ok(
            json!({"id": "example", "released": true, "starting": true}),
        )],
        Duration::ZERO,
    );
    let t = target(&h.pipe, "Local\\Test.none");
    assert_eq!(hold::release(&t, key, true), 0);
    assert_eq!(hold::release(&t, key, false), 0);
    {
        let seen = h.seen.lock().unwrap();
        assert_eq!(seen[0].0, "hub.connector.release");
        assert_eq!(seen[0].1, json!({"id": "example", "start": true}));
        assert_eq!(seen[1].1, json!({"id": "example", "start": false}));
    }
    let h = fake_hub(
        vec![Ok(
            json!({"id": "example", "released": false, "starting": false}),
        )],
        Duration::ZERO,
    );
    assert_eq!(
        hold::release(&target(&h.pipe, "Local\\Test.none"), key, true),
        0
    );
}

#[test]
fn release_without_a_hub_is_5_and_with_a_refusing_hub_is_6() {
    let t = target(&unique_pipe(), "Local\\Test.Hub.nothing-holds-this");
    assert_eq!(hold::release(&t, key, true), 5);
    let h = fake_hub(vec![Err(("busy".into(), Json::Null))], Duration::ZERO);
    assert_eq!(
        hold::release(&target(&h.pipe, "Local\\Test.none"), key, true),
        6
    );
    let _ = Value::Nil;
}
