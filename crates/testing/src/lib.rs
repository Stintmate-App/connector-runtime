//! Test support for connectors: a fake Pitwall on a unique pipe, temporary workspaces with a
//! test key and an example [`ConnectorSpec`] that names no real simulator.
//!
//! Use it as a dev-dependency only. Every pipe user from [`unique_user`] is unique to the test
//! process and the call, so tests can run in parallel.

#![deny(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use stintmate_connector_platform::spec::{ConnectorSpec, GameSpec};
use stintmate_connector_resources::Glyph;
use stintmate_connector_wire::catalog::AnnounceCatalog;
use stintmate_ipc::blocking::Endpoint;
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::RemoteError;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{Key, KeySource, format_key};
use stintmate_ipc::{MethodOpts, naming};

/// The key of the tests (never a real user's).
pub const KEY: Key = [7u8; 32];

/// The example connector: every value is made up.
pub const EXAMPLE_SPEC: ConnectorSpec = ConnectorSpec {
    id: "example",
    name: "Example connector",
    sim: "example",
    sim_name: "Example Simulator",
    product: "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43",
    version: "0.1.0",
    exe_prefix: "example",
    mutex_prefix: "ExampleConnector",
    autostart_value: "Stintmate Example connector",
    game: GameSpec {
        exes: &["example-sim.exe"],
        window_title: None,
        window_class: None,
        cheap_hint: None,
    },
    recorder_hz: 60.0,
    features: &["live_frame", "lap_status"],
    ui_catalog: include_str!("../fixtures/ui.example.json"),
    glyph: Glyph::I,
    dev_env_aliases: &[],
};

/// The announcement catalog of the example connector (its `catalog` field is
/// `connector.example`).
pub const EXAMPLE_ANNOUNCE_CATALOG: &str = include_str!("../fixtures/announce.example.json");

/// [`EXAMPLE_ANNOUNCE_CATALOG`], parsed once.
pub fn example_catalog() -> &'static AnnounceCatalog {
    static CATALOG: OnceLock<AnnounceCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        AnnounceCatalog::parse(EXAMPLE_ANNOUNCE_CATALOG, "connector.example")
            .expect("the example catalog is valid")
    })
}

/// One call the fake received.
#[derive(Clone, Debug)]
pub struct Call {
    /// The method name.
    pub method: String,
    /// The idempotency key, when the caller sent one.
    pub ik: Option<String>,
    /// The call's params.
    pub params: Value,
}

/// What the fake does besides answering `ok`.
#[derive(Default)]
pub struct Rules {
    /// Answer `bad_request` to the next `participant_laps` write with more than one row.
    pub refuse_next_batch: bool,
    /// Answer the next call (any method) with this error code, once.
    pub refuse_next_with: Option<String>,
    /// Answer every call with this error code until it is cleared (a retryable code keeps the
    /// write in the sender's outbox).
    pub refuse_all_with: Option<String>,
}

/// A Pitwall that accepts the write methods and records every call.
pub struct FakePitwall {
    endpoint: Endpoint,
    /// The pipe it listens on.
    pub pipe: String,
    /// Every call received, in order.
    pub calls: Arc<Mutex<Vec<Call>>>,
    /// The hello of the peer of every call received.
    pub peers: Arc<Mutex<Vec<PeerInfo>>>,
    /// What it does besides answering `ok`.
    pub rules: Arc<Mutex<Rules>>,
}

static N: AtomicU64 = AtomicU64::new(0);

/// A pipe user no other test uses.
pub fn unique_user() -> String {
    format!(
        "t{}x{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// Fills `dir` as a workspace holding `ipc\key` for `key`.
pub fn workspace(dir: &Path, key: &Key) -> PathBuf {
    let ipc = dir.join("ipc");
    std::fs::create_dir_all(&ipc).expect("the ipc folder is created");
    std::fs::write(ipc.join("key"), format_key(key)).expect("the key is written");
    dir.to_path_buf()
}

/// A temporary workspace folder with the test key, deleted when dropped.
pub struct TempWorkspace {
    dir: tempfile::TempDir,
}

impl TempWorkspace {
    /// A new workspace with [`KEY`].
    pub fn new() -> Self {
        Self::with_key(&KEY)
    }

    /// A new workspace with `key`.
    pub fn with_key(key: &Key) -> Self {
        let dir = tempfile::tempdir().expect("a temp folder");
        workspace(dir.path(), key);
        Self { dir }
    }

    /// The workspace folder.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Default for TempWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// The pipe of Pitwall for pipe user `user`.
pub fn pitwall_pipe(user: &str) -> String {
    naming::pipe_path_for("pitwall", None, user).expect("a valid pipe name")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakePitwall {
    /// Listens on `pipe` with [`KEY`].
    pub fn start(pipe: &str) -> Self {
        Self::start_with_key(pipe, KEY)
    }

    /// Listens on `pipe` with `key`.
    pub fn start_with_key(pipe: &str, key: Key) -> Self {
        let endpoint = Endpoint::builder(PeerInfo::new("pitwall"), KeySource::Bytes(key))
            .pipe(pipe)
            .build()
            .expect("the endpoint is built");
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let peers: Arc<Mutex<Vec<PeerInfo>>> = Arc::default();
        let rules: Arc<Mutex<Rules>> = Arc::default();
        let lap_n = Arc::new(AtomicU64::new(0));
        let methods = [
            "pitwall.write.catalog",
            "pitwall.write.session_start",
            "pitwall.write.lap",
            "pitwall.write.setup",
            "pitwall.write.pit_exit",
            "pitwall.write.participant_laps",
            "pitwall.write.session_end",
            "pitwall.catalog.declare",
            "pitwall.guidance.declare",
            "pitwall.actions.declare",
            "pitwall.announce",
        ];
        for m in methods {
            let (c, p, r, n) = (calls.clone(), peers.clone(), rules.clone(), lap_n.clone());
            let name = m.to_owned();
            endpoint
                .method(
                    m,
                    MethodOpts::default().ordered(true).idempotent(true),
                    move |ctx, params| {
                        {
                            let mut rules = lock(&r);
                            if let Some(code) = rules.refuse_all_with.clone() {
                                return Err(RemoteError::new(code, "refused by the fake"));
                            }
                            if let Some(code) = rules.refuse_next_with.take() {
                                return Err(RemoteError::new(code, "refused by the fake"));
                            }
                            if name == "pitwall.write.participant_laps" {
                                let rows = stintmate_ipc::codec::get(&params, "laps")
                                    .and_then(Value::as_array)
                                    .map_or(0, Vec::len);
                                if rules.refuse_next_batch && rows > 1 {
                                    rules.refuse_next_batch = false;
                                    return Err(RemoteError::new("bad_request", "an odd row"));
                                }
                            }
                        }
                        lock(&p).push(ctx.peer().info().clone());
                        lock(&c).push(Call {
                            method: name.clone(),
                            ik: ctx.idempotency_key().map(str::to_owned),
                            params,
                        });
                        Ok(match name.as_str() {
                            "pitwall.write.session_start" => {
                                stintmate_ipc::codec::map([("session_id", Value::from(1u32))])
                            }
                            "pitwall.write.lap" => stintmate_ipc::codec::map([
                                ("lap", Value::from(n.fetch_add(1, Ordering::Relaxed) + 1)),
                                ("stint", Value::from(1u32)),
                            ]),
                            "pitwall.announce" => stintmate_ipc::codec::map([
                                ("message_id", Value::from("m1")),
                                ("at", Value::from("T")),
                            ]),
                            _ => stintmate_ipc::codec::map::<[(&str, Value); 0], &str>([]),
                        })
                    },
                )
                .expect("the method is registered");
        }
        endpoint.start().expect("the endpoint starts");
        Self {
            endpoint,
            pipe: pipe.to_owned(),
            calls,
            peers,
            rules,
        }
    }

    /// Every call received so far.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    /// How many calls of `method` were received.
    pub fn count(&self, method: &str) -> usize {
        self.calls().iter().filter(|c| c.method == method).count()
    }

    /// Polls (20 ms) until `what` holds for the calls received, at most `secs` seconds.
    pub fn wait_for(&self, what: impl Fn(&[Call]) -> bool, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if what(&lock(&self.calls)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The connector's connection, once it is up.
    pub fn peer(&self) -> Option<stintmate_ipc::blocking::Peer> {
        self.endpoint.peers().into_iter().next()
    }

    /// Stops listening.
    pub fn stop(&self) {
        self.endpoint.stop();
    }
}

/// A msgpack value as JSON (`bin` as an array of bytes).
pub fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::{Map, json};
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => i.as_i64().map_or_else(|| json!(i.as_u64()), |x| json!(x)),
        Value::F32(f) => json!(f64::from(*f)),
        Value::F64(f) => json!(f),
        Value::String(s) => json!(s.as_str()),
        Value::Binary(b) => json!(b),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => serde_json::Value::Object(
            m.iter()
                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), to_json(v)))
                .collect::<Map<_, _>>(),
        ),
        Value::Ext(..) => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_spec_is_valid_and_its_catalog_parses() {
        EXAMPLE_SPEC.validate().unwrap();
        assert!(example_catalog().codes().contains(&"flag_blue"));
    }

    #[test]
    fn unique_users_differ_and_the_workspace_holds_the_key() {
        assert_ne!(unique_user(), unique_user());
        let ws = TempWorkspace::new();
        assert!(ws.path().join("ipc").join("key").is_file());
    }
}
