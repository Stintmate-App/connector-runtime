//! The outbox over a fake Pitwall: restart, rejected writes and retries.

mod common;

use common::{Rig, wait_for};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stintmate_connector_link::link::{CallError, Transport};
use stintmate_connector_link::outbox::{Outbox, REJECTED, SUFFIX};
use stintmate_ipc::codec::{self, Value};

const LAP: &str = "pitwall.write.lap";

/// A link that is never connected: writes pile up on disk.
struct Down;

impl Transport for Down {
    fn connected(&self) -> bool {
        false
    }
    fn wait_connected(&self, _: Duration) -> bool {
        false
    }
    fn call(&self, _: &str, _: Value, _: Option<&str>, _: Duration) -> Result<Value, CallError> {
        Err(CallError::NotConnected)
    }
    fn call_async(
        &self,
        _: &str,
        _: Value,
        _: Duration,
        _: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError> {
        Err(CallError::NotConnected)
    }
}

fn params(n: u32) -> Value {
    codec::map([("n", Value::from(n))])
}

fn files(folder: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some(SUFFIX))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn outbox_survives_restart_and_sends_leftovers_first() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("outbox");
    // Run one: no connection, two writes stay on disk.
    let (ob, h) = Outbox::start(&folder, Arc::new(Down), None, None, None).unwrap();
    ob.put(LAP, "example/lap/1", params(1)).unwrap();
    ob.put(LAP, "example/lap/2", params(2)).unwrap();
    let s = ob.summary();
    assert_eq!((s.pending, s.rejected), (2, 0));
    assert!(s.oldest_at.is_some());
    ob.close(Duration::ZERO);
    h.join().unwrap();
    assert_eq!(files(&folder).len(), 2);
    // A write cut short by a crash is dropped on the next start.
    std::fs::write(folder.join("00000000000000000099.tmp"), b"half").unwrap();

    // Run two: connected, the leftovers go first, then the new write, the sequence goes on.
    let rig = Rig::new();
    let (ob, h) = Outbox::start(&folder, rig.transport(), None, None, None).unwrap();
    assert_eq!(ob.summary().pending, 2);
    ob.put(LAP, "example/lap/3", params(3)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    let iks: Vec<String> = rig
        .fake
        .calls()
        .iter()
        .filter(|c| c.method == LAP)
        .filter_map(|c| c.ik.clone())
        .collect();
    assert_eq!(iks, ["example/lap/1", "example/lap/2", "example/lap/3"]);
    assert!(files(&folder).is_empty());
    assert!(!folder.join("00000000000000000099.tmp").exists());
    ob.put(LAP, "example/lap/4", params(4)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    ob.close(Duration::ZERO);
    h.join().unwrap();
}

#[test]
fn the_sequence_continues_after_the_highest_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("outbox");
    let (ob, h) = Outbox::start(&folder, Arc::new(Down), None, None, None).unwrap();
    ob.put(LAP, "a", params(1)).unwrap();
    ob.close(Duration::ZERO);
    h.join().unwrap();
    let (ob, h) = Outbox::start(&folder, Arc::new(Down), None, None, None).unwrap();
    ob.put(LAP, "b", params(2)).unwrap();
    ob.close(Duration::ZERO);
    h.join().unwrap();
    assert_eq!(
        files(&folder),
        [
            "00000000000000000001.msgpack",
            "00000000000000000002.msgpack"
        ]
    );
}

#[test]
fn rejected_codes_go_to_rejected_dir() {
    let rig = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("outbox");
    let rejected: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let changes = Arc::new(AtomicU32::new(0));
    let (r, c) = (rejected.clone(), changes.clone());
    let (ob, h) = Outbox::start(
        &folder,
        rig.transport(),
        None,
        Some(Box::new(move |method, _, code| {
            r.lock().unwrap().push((method.into(), code.into()));
        })),
        Some(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        })),
    )
    .unwrap();
    rig.fake.rules.lock().unwrap().refuse_next_with = Some("bad_request".into());
    ob.put(LAP, "example/lap/1", params(1)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    assert!(wait_for(5, || ob.summary().rejected == 1));
    assert!(files(&folder).is_empty());
    assert_eq!(files(&folder.join(REJECTED)).len(), 1);
    assert_eq!(
        *rejected.lock().unwrap(),
        [(LAP.to_owned(), "bad_request".to_owned())]
    );
    assert!(changes.load(Ordering::Relaxed) >= 2);
    // The next write is not affected.
    ob.put(LAP, "example/lap/2", params(2)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    assert_eq!(rig.fake.count(LAP), 1);
    ob.close(Duration::ZERO);
    h.join().unwrap();
    // A later run counts the rejected file and numbers after it.
    let (ob, h) = Outbox::start(&folder, rig.transport(), None, None, None).unwrap();
    assert_eq!(ob.summary().rejected, 1);
    ob.put(LAP, "example/lap/3", params(3)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    ob.close(Duration::ZERO);
    h.join().unwrap();
    assert_eq!(rig.fake.count(LAP), 2);
}

#[test]
fn a_retryable_error_keeps_the_write_and_sends_it_again_with_the_same_key() {
    let rig = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("outbox");
    let results = Arc::new(AtomicU32::new(0));
    let r = results.clone();
    let (ob, h) = Outbox::start(
        &folder,
        rig.transport(),
        Some(Box::new(move |_, _, _| {
            r.fetch_add(1, Ordering::Relaxed);
        })),
        None,
        None,
    )
    .unwrap();
    rig.fake.rules.lock().unwrap().refuse_all_with = Some("busy".into());
    ob.put(LAP, "example/lap/1", params(1)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(ob.summary().pending, 1);
    assert_eq!(files(&folder).len(), 1);
    assert_eq!(rig.fake.count(LAP), 0);
    rig.fake.rules.lock().unwrap().refuse_all_with = None;
    assert!(ob.flush(Duration::from_secs(10)));
    assert_eq!(rig.fake.count(LAP), 1);
    assert_eq!(rig.fake.calls()[0].ik.as_deref(), Some("example/lap/1"));
    assert!(wait_for(5, || results.load(Ordering::Relaxed) == 1));
    ob.close(Duration::from_secs(1));
    h.join().unwrap();
}

const KINDS: &str = "pitwall.write.sim_data_kinds";

/// A scripted peer: declares what it is told to and answers `ok` or an error code per method.
struct Scripted {
    declared: Option<Vec<&'static str>>,
    unknown: Vec<&'static str>,
    sent: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(declared: Option<Vec<&'static str>>, unknown: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            declared,
            unknown,
            sent: Mutex::default(),
        })
    }
    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for Scripted {
    fn connected(&self) -> bool {
        true
    }
    fn wait_connected(&self, _: Duration) -> bool {
        true
    }
    fn call(&self, m: &str, _: Value, _: Option<&str>, _: Duration) -> Result<Value, CallError> {
        self.sent.lock().unwrap().push(m.to_owned());
        if self.unknown.contains(&m) {
            return Err(CallError::Remote {
                code: "unknown_method".into(),
                message: "no such method".into(),
                retry_ms: None,
            });
        }
        Ok(Value::Nil)
    }
    fn call_async(
        &self,
        _: &str,
        _: Value,
        _: Duration,
        _: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError> {
        Err(CallError::NotConnected)
    }
    fn declares(&self, m: &str) -> Option<bool> {
        self.declared.as_ref().map(|d| d.contains(&m))
    }
}

/// Puts `KINDS` then `LAP`, runs the outbox on `link` and returns it with the refused list.
fn run_kinds_then_lap(link: Arc<Scripted>) -> (usize, Vec<String>, u64) {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("outbox");
    let rejected: Arc<Mutex<Vec<String>>> = Arc::default();
    let r = rejected.clone();
    let (ob, h) = Outbox::start(
        &folder,
        link.clone(),
        None,
        Some(Box::new(move |m, _, _| r.lock().unwrap().push(m.into()))),
        None,
    )
    .unwrap();
    ob.put(KINDS, "example/key/sim_data_kinds/0123456789ab", params(1))
        .unwrap();
    ob.put(LAP, "example/key/lap/1", params(2)).unwrap();
    assert!(ob.flush(Duration::from_secs(10)));
    let n_rejected = ob.summary().rejected;
    let rej = folder.join(REJECTED);
    let leftover = files(&folder).len() + if rej.exists() { files(&rej).len() } else { 0 };
    ob.close(Duration::ZERO);
    h.join().unwrap();
    let refused = rejected.lock().unwrap().clone();
    assert_eq!(n_rejected as usize, files_len(&refused));
    (leftover, link.sent(), n_rejected)
}

fn files_len(v: &[String]) -> usize {
    v.len()
}

#[test]
fn an_optional_write_the_pitwall_does_not_declare_is_skipped_not_rejected() {
    let link = Scripted::new(Some(vec![LAP]), vec![]);
    let (leftover, sent, rejected) = run_kinds_then_lap(link);
    assert_eq!(rejected, 0);
    assert_eq!(leftover, 0, "the skipped file is deleted");
    assert_eq!(
        sent,
        [LAP],
        "the optional write is not sent, the next one is"
    );
}

#[test]
fn an_optional_write_the_pitwall_declares_is_sent() {
    let link = Scripted::new(Some(vec![KINDS, LAP]), vec![]);
    let (leftover, sent, rejected) = run_kinds_then_lap(link);
    assert_eq!((rejected, leftover), (0, 0));
    assert_eq!(sent, [KINDS, LAP]);
}

#[test]
fn unknown_declarations_send_as_before() {
    let link = Scripted::new(None, vec![]);
    let (_, sent, rejected) = run_kinds_then_lap(link);
    assert_eq!(rejected, 0);
    assert_eq!(sent, [KINDS, LAP]);
}

#[test]
fn unknown_method_on_an_optional_write_is_not_a_refusal() {
    // Declarations unknown, so the write is sent and the Pitwall answers `unknown_method`.
    let link = Scripted::new(None, vec![KINDS]);
    let (leftover, sent, rejected) = run_kinds_then_lap(link);
    assert_eq!((rejected, leftover), (0, 0));
    assert_eq!(sent, [KINDS, LAP], "no retry, and the next write is sent");
}

#[test]
fn unknown_method_on_a_required_write_is_still_rejected() {
    let link = Scripted::new(None, vec![LAP]);
    let (leftover, sent, rejected) = run_kinds_then_lap(link);
    assert_eq!(rejected, 1);
    assert_eq!(leftover, 1, "the refused file waits in rejected\\");
    assert_eq!(sent, [KINDS, LAP]);
}

#[test]
fn the_real_link_reports_what_the_pitwall_declares() {
    let rig = Rig::new();
    let t = rig.transport();
    assert_eq!(t.declares("pitwall.write.sim_data"), Some(true));
    assert_eq!(t.declares("pitwall.write.nothing"), Some(false));
    assert!(t.peer_version().is_some());
}
