//! The recorder's two links on Stintmate IPC's blocking façade.
//!
//! * [`PitwallLink`]: a client of `pitwall`. It owns the reconnection (the library's own is off)
//!   so that `connector_busy` / `unauthorized` wait 10 s and the state is `connected`, `waiting`
//!   or `refused`; the backoff is 0.25 / 0.5 / 1 / 2 s, then 2 s, all +-20 %.
//! * [`ControlLink`]: a client of the connector's own control app (`connector-<id>`), which
//!   reconnects by itself (a missing control app is normal).
//! * [`StatusPublisher`]: sets `connector.status` and `service.status`, only on change.

use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_ipc::blocking::{Client, PendingCall};
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::connection::{CallOptions, ClosedBy};
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{KeyFile, KeySource, default_key_path};
use stintmate_ipc::{Error, StateChannel};

/// Pitwall's endpoint id.
pub const PITWALL_ENDPOINT: &str = "pitwall";
/// The wait after a refusal (`connector_busy` / `unauthorized`).
pub const REFUSED_RETRY: Duration = Duration::from_secs(10);
/// The wait after `connector_id_taken`: the id belongs to another product, which only the user
/// can change, so the retry is slow. No shorter, and not jittered below this.
pub const TAKEN_RETRY: Duration = Duration::from_secs(60);
/// The process of this executable on the links.
pub const SERVICE: &str = "recorder";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Any serializable value as an IPC value (`bin` stays `bin`).
pub fn to_value<T: Serialize>(t: &T) -> Value {
    let bytes = rmp_serde::to_vec_named(t).expect("a message serializes");
    codec::decode(&bytes).expect("rmp-serde writes the subset")
}

/// Who this process is on both links (connector kit, contract.md section 4.1): `app` is the connector's endpoint, the meta
/// carries its id, name, simulator and product.
pub fn peer_info(spec: &ConnectorSpec, service: &str) -> PeerInfo {
    let meta = codec::map([
        ("connector_id", Value::from(spec.id)),
        ("connector_name", Value::from(spec.name)),
        ("sim", Value::from(spec.sim)),
        ("sim_name", Value::from(spec.sim_name)),
        ("service", Value::from(service)),
        ("product", Value::from(spec.product)),
    ]);
    PeerInfo {
        app: spec.endpoint(),
        name: spec.name.into(),
        version: spec.version.into(),
        process: Some(service.into()),
        meta: Some(meta),
    }
}

/// The workspace's key file, only read, and read again when it changes or appears (create off).
pub fn key_source(workspace: &Path) -> Result<KeySource, Error> {
    let path = default_key_path(Some(workspace))?;
    Ok(KeySource::File(KeyFile::open(path)))
}

/// Why a call failed (the part the outbox and the announcer care about).
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The peer answered with an error.
    Remote {
        /// Error code.
        code: String,
        /// Text.
        message: String,
        /// When to retry.
        retry_ms: Option<u64>,
    },
    /// Not connected (now).
    NotConnected,
    /// The connection ended while waiting.
    Lost,
    /// No answer in time.
    Timeout,
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote { code, message, .. } => write!(f, "{code}: {message}"),
            Self::NotConnected => f.write_str("not connected"),
            Self::Lost => f.write_str("connection lost"),
            Self::Timeout => f.write_str("no answer in time"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl From<Error> for CallError {
    fn from(e: Error) -> Self {
        match e {
            Error::Remote(r) => Self::Remote {
                code: r.code,
                message: r.message,
                retry_ms: r.retry_ms,
            },
            Error::NotConnected => Self::NotConnected,
            Error::ConnectionLost { .. } => Self::Lost,
            Error::CallTimeout => Self::Timeout,
            other => Self::Other(other.to_string()),
        }
    }
}

/// What the outbox and the announcer need from the link (a test double implements it too).
pub trait Transport: Send + Sync {
    /// The connection is up.
    fn connected(&self) -> bool;
    /// Waits until connected, at most `timeout`.
    fn wait_connected(&self, timeout: Duration) -> bool;
    /// A call, waiting for the answer.
    fn call(
        &self,
        method: &str,
        params: Value,
        ik: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CallError>;
    /// A call without waiting for the answer; `done` runs when it ends.
    fn call_async(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        done: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError>;
}

/// Link state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Connected.
    Connected,
    /// Pitwall is not running, or the connection dropped.
    Waiting,
    /// Pitwall refused (`connector_busy`, `unauthorized`, `connector_id_taken`).
    Refused,
}

impl LinkState {
    /// The word `service.status` carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Waiting => "waiting",
            Self::Refused => "refused",
        }
    }
}

/// A flag with a timed, interruptible wait.
#[derive(Default)]
pub struct Stop(Mutex<bool>, Condvar);

impl Stop {
    /// Sets the flag and wakes the waiters.
    pub fn set(&self) {
        *lock(&self.0) = true;
        self.1.notify_all();
    }
    /// Whether it is set.
    pub fn is_set(&self) -> bool {
        *lock(&self.0)
    }
    /// Waits `d` or until set; true when set.
    pub fn wait(&self, d: Duration) -> bool {
        let g = lock(&self.0);
        let (g, _) = self
            .1
            .wait_timeout_while(g, d, |s| !*s)
            .unwrap_or_else(|p| p.into_inner());
        *g
    }
}

static JITTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// `d` with +-20 % jitter.
pub fn jitter(d: Duration) -> Duration {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.subsec_nanos() as u64);
    let x = JITTER
        .fetch_add(t | 1, Ordering::Relaxed)
        .wrapping_mul(0x2545_F491_4F6C_DD1D);
    let f = ((x >> 33) as f64) / f64::from(1u32 << 31);
    d.mul_f64(0.8 + 0.4 * f)
}

/// The delay before attempt number `attempt` (0-based failures in a row): 0.25, 0.5, 1, 2 s,
/// then 2 s; a refusal waits [`REFUSED_RETRY`]. This function adds no jitter; the caller does
/// (about +-20 %, [`jitter`]).
pub fn backoff(attempt: u32, refused: bool) -> Duration {
    if refused {
        return REFUSED_RETRY;
    }
    const STEPS: [u64; 4] = [250, 500, 1000, 2000];
    Duration::from_millis(STEPS[(attempt as usize).min(3)])
}

type Hook = Box<dyn Fn() + Send + Sync>;

struct LinkShared {
    state: Mutex<(LinkState, Option<String>)>,
    on_connected: Mutex<Vec<Hook>>,
    on_disconnected: Mutex<Vec<Hook>>,
    on_state: Mutex<Option<Hook>>,
    generation: AtomicU64,
    stop: Stop,
    last_error: Mutex<Option<(LinkState, String, Wait)>>,
    taken_retry: Mutex<Duration>,
    started: AtomicBool,
}

/// The service's connection to Pitwall's core.
#[derive(Clone)]
pub struct PitwallLink {
    /// The client: declare channels on it before [`start`](Self::start).
    pub client: Client,
    shared: Arc<LinkShared>,
    thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl PitwallLink {
    /// A link to the endpoint at `pipe` (full path or short name).
    pub fn new(pipe: &str, info: PeerInfo, key: KeySource) -> Result<Self, Error> {
        let client = Client::with(pipe, info, key, |b| b.reconnect(false))?;
        let shared = Arc::new(LinkShared {
            state: Mutex::new((LinkState::Waiting, Some("Pitwall is not running".into()))),
            on_connected: Mutex::default(),
            on_disconnected: Mutex::default(),
            on_state: Mutex::default(),
            generation: AtomicU64::new(0),
            stop: Stop::default(),
            last_error: Mutex::default(),
            taken_retry: Mutex::new(TAKEN_RETRY),
            started: AtomicBool::new(false),
        });
        let s = shared.clone();
        client.on_connect_error(move |e| {
            *lock(&s.last_error) = Some(classify(Some(e)));
        });
        Ok(Self {
            client,
            shared,
            thread: Arc::default(),
        })
    }

    /// Runs after every new connection, on the link's thread (it may call).
    pub fn on_connected(&self, f: impl Fn() + Send + Sync + 'static) {
        lock(&self.shared.on_connected).push(Box::new(f));
    }

    /// Runs when a connection ended.
    pub fn on_disconnected(&self, f: impl Fn() + Send + Sync + 'static) {
        lock(&self.shared.on_disconnected).push(Box::new(f));
    }

    /// Runs when the state or its message changed.
    pub fn on_state(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.on_state) = Some(Box::new(f));
    }

    /// The state and its message.
    pub fn state(&self) -> (LinkState, Option<String>) {
        lock(&self.shared.state).clone()
    }

    /// [`state`](Self::state) lent to `f`: no allocation.
    pub fn with_state<R>(&self, f: impl FnOnce(LinkState, Option<&str>) -> R) -> R {
        let g = lock(&self.shared.state);
        f(g.0, g.1.as_deref())
    }

    /// +1 on every new connection.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    /// Starts the link's thread (once).
    pub fn start(&self) {
        if self.shared.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let h = std::thread::Builder::new()
            .name("pitwall-link".into())
            .spawn(move || me.run())
            .expect("a thread starts");
        *lock(&self.thread) = Some(h);
    }

    /// Replaces the wait after `connector_id_taken` (tests; the default is [`TAKEN_RETRY`]).
    pub fn set_taken_retry(&self, d: Duration) {
        *lock(&self.shared.taken_retry) = d;
    }

    /// Stops reconnecting and closes the connection.
    pub fn close(&self) {
        self.shared.stop.set();
        self.client.close();
        if let Some(h) = lock(&self.thread).take()
            && h.thread().id() != std::thread::current().id()
        {
            let _ = h.join();
        }
    }

    fn set(&self, state: LinkState, message: Option<String>) {
        {
            let mut g = lock(&self.shared.state);
            if *g == (state, message.clone()) {
                return;
            }
            *g = (state, message);
        }
        if let Some(f) = lock(&self.shared.on_state).as_ref() {
            f();
        }
    }

    /// The wait before the next attempt: the taken-retry wait exactly (never shorter, no jitter)
    /// after `connector_id_taken`, else the jittered backoff.
    fn delay(&self, attempt: u32, refused: bool, taken: bool) -> Duration {
        if taken {
            *lock(&self.shared.taken_retry)
        } else {
            jitter(backoff(attempt, refused))
        }
    }

    fn run(&self) {
        let sh = &self.shared;
        let mut attempt = 0u32;
        while !sh.stop.is_set() {
            *lock(&sh.last_error) = None;
            self.client.start();
            let mut refused = false;
            let mut taken = false;
            match self.client.wait_connected(Duration::from_secs(8)) {
                Ok(peer) => {
                    attempt = 0;
                    sh.generation.fetch_add(1, Ordering::Relaxed);
                    self.set(LinkState::Connected, None);
                    tracing::info!("Connected to Pitwall {}", peer.info().version);
                    for h in lock(&sh.on_connected).iter() {
                        h();
                    }
                    while peer.is_open() && !sh.stop.wait(Duration::from_millis(100)) {}
                    if sh.stop.is_set() {
                        break;
                    }
                    let bye = peer.inner().connection().close_info().and_then(|i| {
                        (i.by == ClosedBy::Peer)
                            .then(|| classify_bye(&i.code, &i.message))
                            .flatten()
                    });
                    if let Some((state, text, wait)) = bye {
                        refused = true;
                        taken = wait == Wait::Taken;
                        tracing::warn!("{text} (after the hello)");
                        self.set(state, Some(text));
                    } else {
                        tracing::warn!("Connection to Pitwall lost; reconnecting");
                        self.set(
                            LinkState::Waiting,
                            Some("connection to Pitwall lost".into()),
                        );
                    }
                    for h in lock(&sh.on_disconnected).iter() {
                        h();
                    }
                }
                Err(_) => {
                    let (state, text, wait) = lock(&sh.last_error)
                        .take()
                        .unwrap_or_else(|| classify(None));
                    refused = state == LinkState::Refused;
                    taken = wait == Wait::Taken;
                    if taken {
                        tracing::warn!(
                            "{text} (the id belongs to another product; retrying in {} s)",
                            lock(&sh.taken_retry).as_secs()
                        );
                    } else if refused || wait == Wait::Long {
                        tracing::warn!("{text} (retrying in 10 s)");
                    }
                    self.set(state, Some(text));
                    refused |= wait == Wait::Long;
                }
            }
            let d = self.delay(attempt, refused, taken);
            attempt += 1;
            if sh.stop.wait(d) {
                break;
            }
        }
    }
}

/// How long to wait before the next attempt, besides the short backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// The backoff steps.
    Short,
    /// [`REFUSED_RETRY`].
    Long,
    /// `connector_id_taken`: the link's taken-retry wait (60 s).
    Taken,
}

/// The `bye` codes of a refusal (connector protocol section 8): the link shows `refused`.
const REFUSAL_CODES: [&str; 5] = [
    "unauthorized",
    "bad_request",
    "busy",
    "connector_busy",
    "connector_id_taken",
];

/// State, message and wait for a `bye` that refuses the connection, whether it arrives during
/// the handshake or after the hello; `None` for any other code.
fn classify_bye(code: &str, message: &str) -> Option<(LinkState, String, Wait)> {
    if !REFUSAL_CODES.contains(&code) {
        return None;
    }
    let text = if message.is_empty() {
        format!("Pitwall refused the connection: {code}")
    } else {
        format!("Pitwall refused the connection: {code}: {message}")
    };
    let wait = if code == "connector_id_taken" {
        Wait::Taken
    } else {
        Wait::Long
    };
    Some((LinkState::Refused, text, wait))
}

/// State, message and the wait for the error of a failed attempt.
fn classify(e: Option<&Error>) -> (LinkState, String, Wait) {
    match e {
        Some(Error::PeerBye { code, message }) => {
            classify_bye(code, message).unwrap_or_else(|| {
                (
                    LinkState::Waiting,
                    format!("cannot reach Pitwall: bye {code}"),
                    Wait::Short,
                )
            })
        }
        Some(Error::KeyFile(k)) => (LinkState::Waiting, k.to_string(), Wait::Long),
        Some(Error::PipeNotFound(_)) | None => (
            LinkState::Waiting,
            "Pitwall is not running".into(),
            Wait::Short,
        ),
        Some(Error::Handshake(f)) if matches!(f.code.as_str(), "unauthorized") => (
            LinkState::Refused,
            format!(
                "Pitwall refused the connection: unauthorized: {}",
                f.message
            ),
            Wait::Long,
        ),
        Some(other) => (
            LinkState::Waiting,
            format!("cannot reach Pitwall: {other}"),
            Wait::Short,
        ),
    }
}

impl Transport for PitwallLink {
    fn connected(&self) -> bool {
        self.client.is_connected()
    }

    fn wait_connected(&self, timeout: Duration) -> bool {
        self.client.is_connected() || {
            // The link thread connects in the background; poll politely.
            let end = std::time::Instant::now() + timeout;
            while std::time::Instant::now() < end {
                if self.client.is_connected() {
                    return true;
                }
                if self.shared.stop.wait(Duration::from_millis(20)) {
                    break;
                }
            }
            self.client.is_connected()
        }
    }

    fn call(
        &self,
        method: &str,
        params: Value,
        ik: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        let opts = CallOptions {
            timeout: Some(timeout),
            idempotency_key: ik.map(str::to_owned),
            ..CallOptions::default()
        };
        self.client
            .call_with(method, params, opts)
            .map_err(CallError::from)
    }

    fn call_async(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        done: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError> {
        let opts = CallOptions {
            timeout: Some(timeout),
            ..CallOptions::default()
        };
        let pending: PendingCall = self.client.call_async(method, params, opts)?;
        std::thread::spawn(move || done(pending.wait().map_err(CallError::from)));
        Ok(())
    }
}

/// The service's connection to its control app: serves nothing, declares `service.status`.
pub struct ControlLink {
    client: Client,
    /// The state channel `service.status`.
    pub channel: StateChannel,
}

impl ControlLink {
    /// A link to the control endpoint at `pipe`.
    pub fn new(pipe: &str, info: PeerInfo, key: KeySource) -> Result<Self, Error> {
        let client = Client::new(pipe, info, key)?;
        let channel = client.state_channel("service.status", Value::Nil, &[1])?;
        Ok(Self { client, channel })
    }
    /// Connects in the background and keeps retrying.
    pub fn start(&self) {
        self.client.start();
    }
    /// Whether the control app is connected.
    pub fn connected(&self) -> bool {
        self.client.is_connected()
    }
    /// Closes.
    pub fn close(&self) {
        self.client.close();
    }
}

/// The `connector.status` v1 fields (contract.md section 5.2).
pub const STATUS_FIELDS: [&str; 9] = [
    "game_running",
    "phase",
    "session_key",
    "stint",
    "lap",
    "live",
    "outbox",
    "error_code",
    "message",
];

/// Sets `connector.status` and `service.status` from one JSON object, only when it changed.
pub struct StatusPublisher {
    status: StateChannel,
    control: Option<Arc<ControlLink>>,
    pid: u32,
    started_at: String,
    last: Mutex<(Option<Value>, Option<Value>)>,
}

impl StatusPublisher {
    /// Declares `connector.status` on the Pitwall link's client.
    pub fn new(
        pitwall: &PitwallLink,
        control: Option<Arc<ControlLink>>,
        started_at: String,
    ) -> Result<Self, Error> {
        let status =
            pitwall
                .client
                .state_channel("connector.status", to_value(&blank_status()), &[1])?;
        Ok(Self {
            status,
            control,
            pid: std::process::id(),
            started_at,
            last: Mutex::default(),
        })
    }

    /// Sets both channels; true when something was sent.
    pub fn publish(&self, status: &serde_json::Value, extra: &serde_json::Value) -> bool {
        let mut core = serde_json::Map::new();
        for k in STATUS_FIELDS {
            core.insert(k.into(), status.get(k).cloned().unwrap_or_default());
        }
        let mut full = core.clone();
        if let Some(o) = extra.as_object() {
            full.extend(o.clone());
        }
        full.insert("service".into(), SERVICE.into());
        full.insert("pid".into(), self.pid.into());
        full.insert("started_at".into(), self.started_at.clone().into());
        let (core, full) = (to_value(&core), to_value(&full));
        let mut sent = false;
        let mut last = lock(&self.last);
        if last.0.as_ref() != Some(&core) {
            if self.status.set(&core).is_ok() {
                sent = true;
            }
            last.0 = Some(core);
        }
        if last.1.as_ref() != Some(&full) {
            if let Some(c) = &self.control {
                let _ = c.channel.set(&full);
                sent = true;
            }
            last.1 = Some(full);
        }
        sent
    }
}

/// The status before the first publish.
pub fn blank_status() -> serde_json::Value {
    serde_json::json!({"game_running": false, "phase": null, "session_key": null, "stint": null,
        "lap": null, "live": null, "outbox": null, "error_code": null, "message": "starting"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use stintmate_connector_testing::{EXAMPLE_SPEC, to_json};

    #[test]
    fn backoff_steps_and_refusal() {
        let ms: Vec<u128> = (0..6).map(|a| backoff(a, false).as_millis()).collect();
        assert_eq!(ms, [250, 500, 1000, 2000, 2000, 2000]);
        assert_eq!(backoff(0, true), Duration::from_secs(10));
        for _ in 0..50 {
            let j = jitter(Duration::from_secs(1)).as_secs_f64();
            assert!((0.8..=1.2).contains(&j), "{j}");
        }
    }

    #[test]
    fn connector_id_taken_waits_a_minute_and_the_wait_is_injectable() {
        assert!(TAKEN_RETRY >= Duration::from_secs(60));
        let info = peer_info(&EXAMPLE_SPEC, "recorder");
        let link =
            PitwallLink::new("pitwall-test-unused", info, KeySource::Bytes([1u8; 32])).unwrap();
        for _ in 0..20 {
            assert_eq!(link.delay(0, true, true), TAKEN_RETRY);
            assert!(link.delay(0, true, false) <= Duration::from_secs(12));
        }
        link.set_taken_retry(Duration::from_millis(30));
        assert_eq!(link.delay(3, true, true), Duration::from_millis(30));
    }

    #[test]
    fn peer_info_carries_product_and_endpoint() {
        let info = peer_info(&EXAMPLE_SPEC, "recorder");
        assert_eq!(info.app, "connector-example");
        assert_eq!(info.process.as_deref(), Some("recorder"));
        let meta = to_json(info.meta.as_ref().unwrap());
        assert_eq!(meta["product"], EXAMPLE_SPEC.product);
        assert_eq!(meta["connector_id"], "example");
        assert_eq!(meta["connector_name"], "Example connector");
        assert_eq!(meta["sim"], "example");
        assert_eq!(meta["sim_name"], "Example Simulator");
        assert_eq!(meta["service"], "recorder");
    }

    #[test]
    fn refusals_are_classified() {
        let busy = Error::PeerBye {
            code: "connector_busy".into(),
            message: "other".into(),
        };
        let (s, text, long) = classify(Some(&busy));
        assert_eq!(s, LinkState::Refused);
        assert!(long == Wait::Long && text.contains("connector_busy: other"));
        assert_eq!(classify(None).0, LinkState::Waiting);
        let taken = Error::PeerBye {
            code: "connector_id_taken".into(),
            message: String::new(),
        };
        let (s, text, wait) = classify(Some(&taken));
        assert_eq!((s, wait), (LinkState::Refused, Wait::Taken));
        assert!(text.ends_with("connector_id_taken"));
    }

    #[test]
    fn a_bye_is_classified_by_its_code() {
        for code in REFUSAL_CODES {
            let (state, text, wait) = classify_bye(code, "why").unwrap();
            assert_eq!(state, LinkState::Refused);
            assert!(text.ends_with(&format!("{code}: why")));
            let want = if code == "connector_id_taken" {
                Wait::Taken
            } else {
                Wait::Long
            };
            assert_eq!(wait, want, "{code}");
        }
        assert!(classify_bye("normal", "").is_none());
        assert!(classify_bye("timeout", "").is_none());
    }
}
