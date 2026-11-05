//! The recorder's writes: the connector's records, then write API messages, then the outbox,
//! then Pitwall.
//!
//! The connector hands ready messages (or builders of them) to [`WriteSink`]. It queues them and,
//! on its own writer thread, runs the builders, turns each message into the params of its IPC
//! method (`pitwall.write.<type>`, a lap's channels as `bin`) and stores it in the [`Outbox`].
//! The sampling loop never waits for any of it.
//!
//! Sessions are named by the connector's own **session key** (16 random hex digits chosen when
//! the connector opens the session). A recorder stopped while the game runs does not end its
//! session: it keeps the key and the write sequences in `session-resume.json` (next to the outbox
//! folder), and a recorder started later in the same game process, whose first session has the
//! same identity string and whose game clock continued ([`ResumeClock::continues_as`]), continues
//! it under the same key (Pitwall keeps the session open across restarts and replays the first
//! answer to `session_start`, so nothing is announced twice). Anything else gets a new key. Each
//! write has an idempotency key
//! `<prefix>/<session_key>/start|lap/<seq>|setup/<seq>|pit_exit/<seq>|participants/<seq>|end`,
//! `<prefix>/catalog/<sha256-12>` for a catalog, assigned here and stored with the write.

use crate::link::{Transport, to_value};
use crate::outbox::{OnChange, Outbox, Summary};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use stintmate_connector_wire::catalog::{canonical, sha12};
use stintmate_connector_wire::messages::{
    Catalog, Lap, ParticipantLap, ParticipantLaps, PitExit, SessionEnd, SessionStart, Setup,
};
use stintmate_ipc::codec::{self, Value};

/// What the outbox may still deliver at shutdown (under the 10 s forced stop).
pub const CLOSE_DRAIN: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// 16 random hex digits (contract 4.2).
pub fn new_session_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// `<prefix>/catalog/<sha256-12 of the params>`: the same catalog is one write, forever.
pub fn catalog_key(prefix: &str, params: &serde_json::Value) -> String {
    format!("{prefix}/catalog/{}", sha12(&canonical(params)))
}

/// The game's session clock, as far as resuming a session needs it.
pub trait ResumeClock: Clone + Send + 'static {
    /// The clock as stored in `session-resume.json`.
    fn to_json(&self) -> serde_json::Value;
    /// The clock back from the stored value; `None` when it is not one.
    fn from_json(v: &serde_json::Value) -> Option<Self>;
    /// Whether a session stored with this clock can be the one that now reads `now`.
    fn continues_as(&self, now: &Self) -> bool;
}

/// What opens a session.
pub struct SessionOpen {
    /// What must be equal to resume the session (car, track, layout and session type, joined by
    /// the connector).
    pub identity: String,
    /// The catalog entries the session needs, written before the start.
    pub catalog: Option<Catalog>,
    /// The `session_start` message (the sink adds the session key).
    pub start: SessionStart,
}

/// Builds a lap message; runs on the writer thread.
pub type LapBuild = Box<dyn FnOnce() -> Lap + Send>;
/// Builds a setup message; runs on the writer thread.
pub type SetupBuild = Box<dyn FnOnce() -> Setup + Send>;

#[derive(Default)]
struct Session {
    identity: String,
    key: String,
    seq_lap: u64,
    seq_setup: u64,
    seq_pit_exit: u64,
    seq_participants: u64,
    remote_id: Option<i64>,
    stint: Option<i64>,
}

#[derive(Default)]
struct Shared {
    next_local: u64,
    sessions: HashMap<u64, Session>,
    by_key: HashMap<String, u64>,
}

/// Counters for `service.status`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Laps stored in the outbox.
    pub laps_queued: u64,
    /// Laps Pitwall accepted.
    pub laps_sent: u64,
    /// Laps Pitwall refused.
    pub laps_rejected: u64,
}

enum Job<C> {
    Start(u64, Box<(Option<Catalog>, SessionStart)>),
    /// `(session, ended_at, clock)`: a clock stores the resume file, not a `session_end`.
    End(u64, String, Option<C>),
    Lap(u64, LapBuild),
    Setup(u64, String, String, SetupBuild),
    PitExit(u64, String, Option<f64>),
    Participants(u64, Vec<ParticipantLap>),
    Split(String, Vec<Value>),
    Mark(Sender<()>),
    Quit,
}

struct Core<C: ResumeClock> {
    dir: PathBuf,
    prefix: &'static str,
    shared: Mutex<Shared>,
    stats: Mutex<Stats>,
    outbox: Mutex<Option<Outbox>>,
    queue: Mutex<Sender<Job<C>>>,
    /// Where an open session is kept for the next recorder of the same game process.
    resume_path: PathBuf,
    /// Identity of the running game process (its pid), when known.
    game_token: Mutex<Option<u64>>,
    /// The next `end_session` keeps the session open instead of closing it.
    /// Set when the recorder stops while the game runs: the clock of its last frame.
    stop_clock: Mutex<Option<C>>,
    /// The clock of the frame that opens a session (set by the recorder while none is open).
    start_clock: Mutex<Option<C>>,
}

/// What a stopped recorder leaves for the next one.
fn resume_json<C: ResumeClock>(token: u64, s: &Session, clock: &C) -> serde_json::Value {
    serde_json::json!({
        "game": token, "clock": clock.to_json(), "key": s.key, "identity": s.identity,
        "seq": [s.seq_lap, s.seq_setup, s.seq_pit_exit, s.seq_participants],
    })
}

/// The key and sequences stored by [`resume_json`] when they describe `identity` in game process
/// `token` and the stored session clock can continue as `now` ([`ResumeClock::continues_as`]).
/// The file is always consumed.
fn take_resume<C: ResumeClock>(
    path: &Path,
    token: Option<u64>,
    identity: &str,
    now: Option<&C>,
) -> Option<(String, [u64; 4])> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let text_of = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    let clock_ok = match (v.get("clock").and_then(C::from_json), now) {
        (Some(stored), Some(now)) => stored.continues_as(now),
        _ => false,
    };
    let same = token.is_some()
        && clock_ok
        && v.get("game").and_then(serde_json::Value::as_u64) == token
        && text_of("identity") == Some(identity);
    let key = text_of("key").filter(|k| same && !k.is_empty())?;
    let seq = v.get("seq")?.as_array()?;
    let mut n = [0u64; 4];
    for (slot, x) in n.iter_mut().zip(seq) {
        *slot = x.as_u64()?;
    }
    Some((key.to_owned(), n))
}

/// The connector's writes to Pitwall (see the module docs).
pub struct WriteSink<C: ResumeClock> {
    core: Arc<Core<C>>,
    writer: Option<std::thread::JoinHandle<()>>,
    sender: Option<std::thread::JoinHandle<()>>,
}

impl<C: ResumeClock> WriteSink<C> {
    /// Starts the writer thread and the outbox in `outbox_dir`. `ik_prefix` (the connector's id)
    /// starts every idempotency key.
    pub fn new(
        outbox_dir: &Path,
        link: Arc<dyn Transport>,
        on_change: Option<OnChange>,
        ik_prefix: &'static str,
    ) -> io::Result<Self> {
        let (tx, rx) = channel();
        let core = Arc::new(Core {
            dir: outbox_dir.to_owned(),
            prefix: ik_prefix,
            shared: Mutex::default(),
            stats: Mutex::default(),
            outbox: Mutex::new(None),
            queue: Mutex::new(tx),
            resume_path: outbox_dir.with_file_name("session-resume.json"),
            game_token: Mutex::new(None),
            stop_clock: Mutex::new(None),
            start_clock: Mutex::new(None),
        });
        let (c1, c2) = (core.clone(), core.clone());
        let on_result: crate::outbox::OnResult = Box::new(move |m, p, r| c1.delivered(m, p, r));
        let on_rejected: crate::outbox::OnRejected =
            Box::new(move |m, p, code| c2.rejected(m, p, code));
        let (outbox, sender) = Outbox::start(
            outbox_dir,
            link,
            Some(on_result),
            Some(on_rejected),
            on_change,
        )?;
        *lock(&core.outbox) = Some(outbox);
        let c3 = core.clone();
        let writer = std::thread::Builder::new()
            .name("pitwall-writer".into())
            .spawn(move || c3.run(rx))?;
        Ok(Self {
            core,
            writer: Some(writer),
            sender: Some(sender),
        })
    }

    /// The outbox (status, tests).
    pub fn outbox(&self) -> Outbox {
        self.core.outbox()
    }

    /// The outbox folder.
    pub fn outbox_dir(&self) -> PathBuf {
        self.core.dir.clone()
    }

    /// `connector.status.outbox`.
    pub fn outbox_summary(&self) -> Summary {
        self.outbox().summary()
    }

    /// [`outbox_summary`](Self::outbox_summary) lent to `f` as `(pending, oldest_at, rejected)`:
    /// no allocation.
    pub fn with_outbox_summary<R>(&self, f: impl FnOnce(usize, Option<&str>, u64) -> R) -> R {
        self.outbox().with_summary(f)
    }

    /// Counters.
    pub fn stats(&self) -> Stats {
        *lock(&self.core.stats)
    }

    /// Names the running game process (its pid); a resumed session needs it to match.
    pub fn set_game_token(&self, token: Option<u64>) {
        *lock(&self.core.game_token) = token;
    }

    /// The next `end_session` (the recorder is stopping while the game runs) keeps the session
    /// open for the next recorder instead of sending `session_end`; `clock` is the game's
    /// session clock at the last frame.
    pub fn keep_session_open(&self, clock: C) {
        *lock(&self.core.stop_clock) = Some(clock);
    }

    /// The game's session clock at the frame being processed; the recorder calls it only while
    /// no session is open, so a session opened by that frame knows where the clock stood.
    pub fn note_clock(&self, clock: C) {
        *lock(&self.core.start_clock) = Some(clock);
    }

    /// The session key of a session.
    pub fn session_key(&self, local: Option<u64>) -> Option<String> {
        let sh = lock(&self.core.shared);
        sh.sessions.get(&local?).map(|s| s.key.clone())
    }

    /// [`session_key`](Self::session_key) lent to `f`: no allocation.
    pub fn with_session_key<R>(&self, local: Option<u64>, f: impl FnOnce(Option<&str>) -> R) -> R {
        let sh = lock(&self.core.shared);
        f(local
            .and_then(|l| sh.sessions.get(&l))
            .map(|s| s.key.as_str()))
    }

    /// Pitwall's session id of a session, once Pitwall answered a write of it.
    pub fn remote_id(&self, local: Option<u64>) -> Option<i64> {
        lock(&self.core.shared).sessions.get(&local?)?.remote_id
    }

    /// Pitwall's stint of a session as last answered.
    pub fn stint(&self, local: Option<u64>) -> Option<i64> {
        lock(&self.core.shared).sessions.get(&local?)?.stint
    }

    /// Waits until every queued record is stored in the outbox (not yet delivered).
    pub fn queued(&self, timeout: Duration) -> bool {
        let (tx, rx) = channel();
        self.core.send(Job::Mark(tx));
        rx.recv_timeout(timeout).is_ok()
    }

    /// Waits until everything queued is stored and delivered (tests).
    pub fn flush(&self, timeout: Duration) -> bool {
        let t0 = std::time::Instant::now();
        self.queued(timeout) && self.outbox().flush(timeout.saturating_sub(t0.elapsed()))
    }

    /// Stores what is queued, lets the outbox deliver what it can within `drain`, stops both
    /// threads. The rest stays on disk.
    pub fn close(&mut self, drain: Duration) {
        self.core.send(Job::Quit);
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
        self.outbox().close(drain);
        if let Some(h) = self.sender.take() {
            let _ = h.join();
        }
    }

    /// Opens a session: resumes the stored one when it matches (see the module docs), else gives
    /// it a new key. Returns the local session number.
    pub fn start_session(&mut self, open: SessionOpen) -> u64 {
        let SessionOpen {
            identity,
            catalog,
            start,
        } = open;
        let local = {
            let mut sh = lock(&self.core.shared);
            sh.next_local += 1;
            let local = sh.next_local;
            let token = *lock(&self.core.game_token);
            let now = lock(&self.core.start_clock).clone();
            let (key, seq) = take_resume(&self.core.resume_path, token, &identity, now.as_ref())
                .unwrap_or_else(|| (new_session_key(), [0; 4]));
            sh.by_key.insert(key.clone(), local);
            sh.sessions.insert(
                local,
                Session {
                    identity,
                    key,
                    seq_lap: seq[0],
                    seq_setup: seq[1],
                    seq_pit_exit: seq[2],
                    seq_participants: seq[3],
                    ..Session::default()
                },
            );
            local
        };
        self.core
            .send(Job::Start(local, Box::new((catalog, start))));
        local
    }

    /// Ends a session, or keeps it open for the next recorder when
    /// [`keep_session_open`](Self::keep_session_open) was called.
    pub fn end_session(&mut self, local: u64, ended_at: &str) {
        let keep = lock(&self.core.stop_clock).clone();
        self.core.send(Job::End(local, ended_at.to_owned(), keep));
    }

    /// Queues a lap; `build` runs on the writer thread.
    pub fn lap(&mut self, local: u64, build: LapBuild) {
        self.core.send(Job::Lap(local, build));
    }

    /// Queues a setup reading taken at `at` for `origin`; `build` runs on the writer thread.
    pub fn setup(&mut self, local: u64, at: &str, origin: &str, build: SetupBuild) {
        self.core
            .send(Job::Setup(local, at.to_owned(), origin.to_owned(), build));
    }

    /// Queues the moment the car left the pits.
    pub fn pit_exit(&mut self, local: u64, at: &str, fuel_l: Option<f64>) {
        self.core.send(Job::PitExit(local, at.to_owned(), fuel_l));
    }

    /// Queues the other drivers' laps.
    pub fn participants(&mut self, local: u64, rows: Vec<ParticipantLap>) {
        self.core.send(Job::Participants(local, rows));
    }
}

impl<C: ResumeClock> Drop for WriteSink<C> {
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.close(Duration::ZERO);
        }
    }
}

fn with(mut params: Value, extra: Vec<(&str, Value)>) -> Value {
    if let Value::Map(m) = &mut params {
        for (k, v) in extra {
            m.push((Value::from(k), v));
        }
    }
    params
}

impl<C: ResumeClock> Core<C> {
    fn send(&self, job: Job<C>) {
        let _ = lock(&self.queue).send(job);
    }

    fn outbox(&self) -> Outbox {
        lock(&self.outbox).clone().expect("started")
    }

    fn run(&self, rx: Receiver<Job<C>>) {
        for job in rx {
            match job {
                Job::Mark(done) => {
                    let _ = done.send(());
                }
                Job::Quit => return,
                job => self.handle(job),
            }
        }
    }

    fn put(&self, kind: &str, ik: &str, params: Value) {
        if let Err(e) = self
            .outbox()
            .put(&format!("pitwall.write.{kind}"), ik, params)
        {
            tracing::error!("Cannot store {kind} {ik} in the outbox: {e} (dropped)");
        }
    }

    fn catalog(&self, cat: &Catalog) {
        let json = serde_json::to_value(cat).expect("a catalog serializes");
        self.put("catalog", &catalog_key(self.prefix, &json), to_value(cat));
    }

    /// Takes the next sequence number of `kind` for a session and returns it with the key.
    fn next(&self, local: u64, kind: &str) -> Option<(String, u64)> {
        let mut sh = lock(&self.shared);
        let s = sh.sessions.get_mut(&local)?;
        let seq = match kind {
            "lap" => &mut s.seq_lap,
            "setup" => &mut s.seq_setup,
            "pit_exit" => &mut s.seq_pit_exit,
            _ => &mut s.seq_participants,
        };
        *seq += 1;
        Some((s.key.clone(), *seq))
    }

    /// Stores a message of a session under its key and sequence; `segment` is the idempotency
    /// key's word for the kind.
    fn put_seq(&self, kind: &str, segment: &str, key: &str, seq: u64, params: Value) {
        let params = with(
            params,
            vec![("session_key", Value::from(key)), ("seq", Value::from(seq))],
        );
        let ik = format!("{}/{key}/{segment}/{seq}", self.prefix);
        self.put(kind, &ik, params);
    }

    fn handle(&self, job: Job<C>) {
        match job {
            Job::Start(local, open) => {
                let (cat, start) = *open;
                let Some(key) = lock(&self.shared)
                    .sessions
                    .get(&local)
                    .map(|s| s.key.clone())
                else {
                    return;
                };
                if let Some(cat) = cat {
                    self.catalog(&cat);
                }
                let params = with(
                    to_value(&start),
                    vec![("session_key", Value::from(key.as_str()))],
                );
                self.put(
                    "session_start",
                    &format!("{}/{key}/start", self.prefix),
                    params,
                );
                tracing::info!("Session {key} queued for Pitwall (recorder session {local})");
            }
            Job::End(local, ended_at, keep_clock) => {
                let (key, resume) = {
                    let sh = lock(&self.shared);
                    let Some(s) = sh.sessions.get(&local) else {
                        return;
                    };
                    let token = *lock(&self.game_token);
                    (
                        s.key.clone(),
                        token.zip(keep_clock).map(|(t, c)| resume_json(t, s, &c)),
                    )
                };
                if let Some(r) = resume {
                    match std::fs::write(&self.resume_path, r.to_string()) {
                        Ok(()) => {
                            tracing::info!("Session {key} kept open for the next recorder");
                            return;
                        }
                        Err(e) => tracing::warn!("Cannot keep session {key} open: {e}"),
                    }
                }
                let params = with(
                    to_value(&SessionEnd { ended_at }),
                    vec![("session_key", Value::from(key.as_str()))],
                );
                self.put("session_end", &format!("{}/{key}/end", self.prefix), params);
            }
            Job::Lap(local, build) => {
                let Some((key, seq)) = self.next(local, "lap") else {
                    return;
                };
                let msg = build();
                self.put_seq("lap", "lap", &key, seq, to_value(&msg));
                lock(&self.stats).laps_queued += 1;
                tracing::info!(
                    "Lap ({}, {} channels) stored in the outbox ({key}/lap/{seq})",
                    msg.lap_type,
                    msg.channels.as_ref().map_or(0, |c| c.0.len())
                );
            }
            Job::Setup(local, taken_at, reason, build) => {
                let Some((key, seq)) = self.next(local, "setup") else {
                    return;
                };
                let mut msg = build();
                msg.taken_at = taken_at;
                msg.reason = reason;
                self.put_seq("setup", "setup", &key, seq, to_value(&msg));
            }
            Job::PitExit(local, at, fuel_l) => {
                let Some((key, seq)) = self.next(local, "pit_exit") else {
                    return;
                };
                let msg = PitExit { at, fuel_l };
                self.put_seq("pit_exit", "pit_exit", &key, seq, to_value(&msg));
            }
            Job::Participants(local, rows) => {
                if rows.is_empty() || !lock(&self.shared).sessions.contains_key(&local) {
                    return;
                }
                let batch = to_value(&ParticipantLaps { laps: rows });
                let laps = codec::get(&batch, "laps")
                    .and_then(Value::as_array)
                    .cloned();
                self.participants(local, laps.unwrap_or_default());
            }
            Job::Split(key, laps) => {
                let Some(local) = lock(&self.shared).by_key.get(&key).copied() else {
                    return;
                };
                for row in laps {
                    self.participants(local, vec![row]);
                }
            }
            Job::Mark(_) | Job::Quit => {}
        }
    }

    fn participants(&self, local: u64, laps: Vec<Value>) {
        let Some((key, seq)) = self.next(local, "participants") else {
            return;
        };
        let params = codec::map([("laps", Value::Array(laps))]);
        self.put_seq("participant_laps", "participants", &key, seq, params);
    }

    fn delivered(&self, method: &str, params: &Value, result: &Value) {
        let key = codec::get(params, "session_key").and_then(Value::as_str);
        let mut sh = lock(&self.shared);
        let local = key.and_then(|k| sh.by_key.get(k).copied());
        if let Some(s) = local.and_then(|l| sh.sessions.get_mut(&l)) {
            if let Some(id) = codec::get(result, "session_id").and_then(Value::as_i64) {
                s.remote_id = Some(id);
            }
            if let Some(st) = codec::get(result, "stint").and_then(Value::as_i64) {
                s.stint = Some(st);
            }
        }
        drop(sh);
        if method == "pitwall.write.lap" {
            lock(&self.stats).laps_sent += 1;
        }
    }

    fn rejected(&self, method: &str, params: &Value, code: &str) {
        if method == "pitwall.write.lap" {
            lock(&self.stats).laps_rejected += 1;
        }
        if method != "pitwall.write.participant_laps" || code != "bad_request" {
            return;
        }
        let laps = codec::get(params, "laps")
            .and_then(Value::as_array)
            .cloned();
        let key = codec::get(params, "session_key").and_then(Value::as_str);
        if let (Some(laps), Some(key)) = (laps, key)
            && laps.len() > 1
        {
            // One odd row (a name with control characters) must not lose the batch.
            tracing::warn!(
                "Rival batch refused; queueing its {} rows one by one",
                laps.len()
            );
            self.send(Job::Split(key.to_owned(), laps));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::CallError;
    use std::sync::Mutex as StdMutex;

    /// A test clock with AMS2's rule: laps not fewer, remaining time not higher than stored + 2 s.
    #[derive(Clone, Copy)]
    struct TestClock {
        left: f32,
        laps: u32,
    }

    impl ResumeClock for TestClock {
        fn to_json(&self) -> serde_json::Value {
            serde_json::json!({ "left": self.left, "laps": self.laps })
        }
        fn from_json(v: &serde_json::Value) -> Option<Self> {
            Some(Self {
                left: v.get("left")?.as_f64()? as f32,
                laps: u32::try_from(v.get("laps")?.as_u64()?).ok()?,
            })
        }
        fn continues_as(&self, now: &Self) -> bool {
            self.left >= 0.0
                && now.left >= 0.0
                && now.laps >= self.laps
                && now.left <= self.left + 2.0
        }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// A Pitwall that is never there: the writes stay in the outbox.
    struct Down;

    impl Transport for Down {
        fn connected(&self) -> bool {
            false
        }
        fn wait_connected(&self, _: Duration) -> bool {
            false
        }
        fn call(
            &self,
            _: &str,
            _: Value,
            _: Option<&str>,
            _: Duration,
        ) -> Result<Value, CallError> {
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

    fn session(state: &str) -> SessionOpen {
        SessionOpen {
            identity: format!("Example Car|Example Park|Short|{state}"),
            catalog: None,
            start: SessionStart {
                started_at: "2026-10-25T10:00:00.000+02:00".into(),
                session_type: state.to_lowercase(),
                car_id: None,
                track_id: None,
                layout_id: None,
                external_ref: None,
                conditions: None,
            },
        }
    }

    fn lap_message() -> Lap {
        Lap {
            lap_type: "flying".into(),
            on_track: Some(true),
            time_s: Some(90.0),
            valid: true,
            time_source: None,
            measured_time_s: None,
            started_at: None,
            sim_lap: None,
            fuel_start_l: None,
            fuel_used_l: None,
            max_speed_kmh: None,
            conditions: None,
            sectors: Vec::new(),
            channels: None,
        }
    }

    /// A recorder of game process `token`.
    fn recorder(dir: &Path, token: u64, prefix: &'static str) -> WriteSink<TestClock> {
        let sink = WriteSink::new(&dir.join("outbox"), Arc::new(Down), None, prefix).unwrap();
        sink.set_game_token(Some(token));
        sink
    }

    /// The game's clock in a 30 min practice: `left` s to go, `laps` completed.
    fn clock(left: f32, laps: u32) -> TestClock {
        TestClock { left, laps }
    }

    fn open(sink: &mut WriteSink<TestClock>, state: &str) -> String {
        open_at(sink, state, clock(1500.0, 3))
    }

    fn open_at(sink: &mut WriteSink<TestClock>, state: &str, at: TestClock) -> String {
        sink.note_clock(at);
        let id = sink.start_session(session(state));
        sink.session_key(Some(id)).unwrap()
    }

    /// Stops a recorder; `game_runs`: the game goes on (the session is kept open).
    fn stop(mut sink: WriteSink<TestClock>, id: u64, game_runs: bool) -> usize {
        if game_runs {
            sink.keep_session_open(clock(1500.0, 3));
        }
        sink.end_session(id, "2026-10-25T10:05:00.000+02:00");
        assert!(sink.queued(Duration::from_secs(5)));
        let pending = sink.outbox_summary().pending;
        sink.close(Duration::ZERO);
        pending
    }

    #[test]
    fn a_restarted_recorder_in_the_same_game_session_keeps_the_key_and_sends_no_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        let key = open(&mut a, "PRACTICE");
        a.pit_exit(1, "t", None);
        assert!(a.queued(Duration::from_secs(5)));
        let before = a.outbox_summary().pending;
        assert_eq!(
            stop(a, 1, true),
            before,
            "no session_end while the game runs"
        );

        let mut b = recorder(dir.path(), 4242, "example");
        assert_eq!(open(&mut b, "PRACTICE"), key, "same key after the restart");
        b.pit_exit(1, "t", None);
        assert!(b.queued(Duration::from_secs(5)));
        // the pit exit continues the sequence (a repeat of seq 1 would be replayed by Pitwall)
        let pending = b.outbox_summary().pending;
        assert_eq!(pending, before + 2); // the start again (Pitwall replays it), the exit
        let ik = std::fs::read_dir(dir.path().join("outbox"))
            .unwrap()
            .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
            .filter(|t| contains(t, format!("{key}/pit_exit/2").as_bytes()))
            .count();
        assert_eq!(ik, 1, "the second pit exit is seq 2");
        b.close(Duration::ZERO);
        assert!(!dir.path().join("session-resume.json").exists());
    }

    #[test]
    fn a_new_game_session_gets_a_new_key() {
        for (token, state) in [(7, "PRACTICE"), (4242, "QUALIFY"), (4242, "RACE")] {
            let dir = tempfile::tempdir().unwrap();
            let mut a = recorder(dir.path(), 4242, "example");
            let key = open(&mut a, "PRACTICE");
            stop(a, 1, true);
            let mut b = recorder(dir.path(), token, "example");
            let other = open(&mut b, state);
            assert_ne!(other, key, "{token} {state}");
            b.close(Duration::ZERO);
        }
    }

    #[test]
    fn a_session_ended_for_good_is_not_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        let key = open(&mut a, "PRACTICE");
        let before = a.outbox_summary().pending;
        assert!(
            stop(a, 1, false) > before,
            "the game left: session_end is queued"
        );
        assert!(!dir.path().join("session-resume.json").exists());
        let mut b = recorder(dir.path(), 4242, "example");
        assert_ne!(open(&mut b, "PRACTICE"), key);
        b.close(Duration::ZERO);
    }

    #[test]
    fn an_unknown_game_process_never_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        let mut b = WriteSink::<TestClock>::new(
            &dir.path().join("outbox"),
            Arc::new(Down),
            None,
            "example",
        )
        .unwrap();
        assert_ne!(open(&mut b, "PRACTICE"), key);
        b.close(Duration::ZERO);
    }

    #[test]
    fn the_same_session_after_a_long_stop_keeps_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        // however long the recorder was off, the game's clock went on
        let mut b = recorder(dir.path(), 4242, "example");
        let again = open_at(&mut b, "PRACTICE", clock(700.0, 9));
        assert_eq!(again, key, "the clock continued: the same session");
        b.close(Duration::ZERO);
    }

    #[test]
    fn a_new_session_of_the_same_kind_gets_a_new_key() {
        // clock back at the total, laps back to 0, or the time unset: another session
        for (left, laps) in [(1800.0, 0), (1500.0, 0), (1800.0, 3), (-1.0, 3)] {
            let dir = tempfile::tempdir().unwrap();
            let mut a = recorder(dir.path(), 4242, "example");
            let key = open(&mut a, "PRACTICE");
            stop(a, 1, true);
            let mut b = recorder(dir.path(), 4242, "example");
            let other = open_at(&mut b, "PRACTICE", clock(left, laps));
            assert_ne!(other, key, "{left} s left, {laps} laps");
            b.close(Duration::ZERO);
        }
    }

    #[test]
    fn a_session_opened_without_a_clock_is_never_a_resume() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        let key = open(&mut a, "PRACTICE");
        stop(a, 1, true);
        let mut b = recorder(dir.path(), 4242, "example");
        let id = b.start_session(session("PRACTICE"));
        assert_ne!(b.session_key(Some(id)).unwrap(), key);
        b.close(Duration::ZERO);
    }

    #[test]
    fn lap_builder_runs_off_the_caller_thread() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "example");
        open(&mut a, "RACE");
        let seen = Arc::new(StdMutex::new(None));
        let slot = seen.clone();
        a.lap(
            1,
            Box::new(move || {
                *slot.lock().unwrap() = Some(std::thread::current().id());
                lap_message()
            }),
        );
        assert!(a.queued(Duration::from_secs(5)));
        let built_on = seen.lock().unwrap().expect("the builder ran");
        assert_ne!(built_on, std::thread::current().id());
        assert_eq!(a.stats().laps_queued, 1);
        a.close(Duration::ZERO);
    }

    #[test]
    fn ik_prefix_is_the_connector_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = recorder(dir.path(), 4242, "sample-sim");
        let key = open(&mut a, "RACE");
        a.lap(1, Box::new(lap_message));
        a.pit_exit(1, "t", Some(10.0));
        a.participants(1, Vec::new());
        a.end_session(1, "2026-10-25T10:05:00.000+02:00");
        assert!(a.queued(Duration::from_secs(5)));
        let texts: Vec<Vec<u8>> = std::fs::read_dir(dir.path().join("outbox"))
            .unwrap()
            .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
            .collect();
        for tail in ["start", "lap/1", "pit_exit/1", "end"] {
            let ik = format!("sample-sim/{key}/{tail}");
            assert_eq!(
                texts.iter().filter(|t| contains(t, ik.as_bytes())).count(),
                1,
                "{ik}"
            );
        }
        assert!(
            catalog_key("sample-sim", &serde_json::json!({ "cars": [] }))
                .starts_with("sample-sim/catalog/")
        );
        a.close(Duration::ZERO);
    }
}
