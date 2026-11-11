//! The recorder's outbox: every write to Pitwall goes through the disk first (connector kit, contract.md section 5.4).
//!
//! [`Outbox::put`] stores a write as `<20-digit sequence>.msgpack` holding
//! `{method, v, ik, p, at}` (a temporary file, then a rename). The sender thread sends the files
//! in sequence order, one at a time, waiting for each answer (30 s) with the stored idempotency
//! key. `ok` deletes the file; a non-retryable error moves it to `rejected\`; anything else
//! keeps it and retries with the same key (after `retry_ms`, 1 s by default). The
//! [`OPTIONAL_METHODS`] are the exception: an older Pitwall may lack them, so when the linked
//! Pitwall does not declare one (or answers it with `unknown_method`) the write is deleted, not
//! counted as refused, and logged once per method per connection. Files of an
//! earlier run are loaded first and the sequence continues after the highest one on disk.

use crate::link::{CallError, Stop, Transport};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use stintmate_connector_platform::logging::local_iso;
use stintmate_ipc::codec::{self, Value};

/// File suffix.
pub const SUFFIX: &str = "msgpack";
/// Folder of refused writes.
pub const REJECTED: &str = "rejected";
/// The write methods' deadline (contract.md section 4.2).
pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// Errors that will not get better by sending again.
pub const NON_RETRYABLE: [&str; 6] = [
    "bad_request",
    "session_closed",
    "unknown_session",
    "too_large",
    "unknown_method",
    "version_unsupported",
];
/// Write methods an older Pitwall may not have. A write of one of these that the linked Pitwall
/// does not take is skipped (deleted, neither refused nor counted in `rejected`).
pub const OPTIONAL_METHODS: [&str; 3] = [
    "pitwall.write.sim_data_kinds",
    "pitwall.write.sim_data",
    "pitwall.write.lap_positions",
];
const RETRY_DEFAULT: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug)]
struct Item {
    seq: u64,
    path: PathBuf,
    method: String,
    ik: String,
    at: String,
}

#[derive(Default)]
struct State {
    items: VecDeque<Item>,
    rejected: u64,
    next: u64,
}

/// `connector.status.outbox`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Summary {
    /// Writes waiting.
    pub pending: usize,
    /// When the oldest was stored.
    pub oldest_at: Option<String>,
    /// Writes in `rejected\`.
    pub rejected: u64,
}

/// Called on the sender thread after an `ok` answer: `(method, params, result)`.
pub type OnResult = Box<dyn Fn(&str, &Value, &Value) + Send + Sync>;
/// Called on the sender thread after a non-retryable error: `(method, params, code)`.
pub type OnRejected = Box<dyn Fn(&str, &Value, &str) + Send + Sync>;
/// Called when the backlog changed.
pub type OnChange = Box<dyn Fn() + Send + Sync>;

struct Inner {
    folder: PathBuf,
    state: Mutex<State>,
    wake: Condvar,
    idle: Condvar,
    stop: Stop,
    link: Arc<dyn Transport>,
    on_result: Option<OnResult>,
    on_rejected: Option<OnRejected>,
    on_change: Option<OnChange>,
    /// Optional methods already logged as skipped on this connection.
    skipped: Mutex<HashSet<String>>,
}

/// The outbox (cheap to clone: one shared state).
#[derive(Clone)]
pub struct Outbox(Arc<Inner>);

fn file_name(seq: u64) -> String {
    format!("{seq:020}.{SUFFIX}")
}

fn seq_of(path: &Path) -> Option<u64> {
    if path.extension()?.to_str()? != SUFFIX {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
}

fn read_doc(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    codec::decode(&bytes).map_err(|e| e.to_string())
}

fn move_rejected(folder: &Path, path: &Path) {
    let dir = folder.join(REJECTED);
    let _ = fs::create_dir_all(&dir);
    let Some(name) = path.file_name() else { return };
    for _ in 0..5 {
        match fs::rename(path, dir.join(name)) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

impl Outbox {
    /// Opens `folder` (created), loads what an earlier run left and starts the sender.
    pub fn start(
        folder: &Path,
        link: Arc<dyn Transport>,
        on_result: Option<OnResult>,
        on_rejected: Option<OnRejected>,
        on_change: Option<OnChange>,
    ) -> std::io::Result<(Self, std::thread::JoinHandle<()>)> {
        fs::create_dir_all(folder)?;
        let mut found = Vec::new();
        for entry in fs::read_dir(folder)?.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("tmp") {
                let _ = fs::remove_file(&p); // a write cut short by a crash
                continue;
            }
            let Some(seq) = seq_of(&p) else { continue };
            let item = read_doc(&p).and_then(|doc| {
                let get = |k: &str| {
                    codec::get(&doc, k)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                Ok(Item {
                    seq,
                    path: p.clone(),
                    method: get("method").ok_or("no method")?,
                    ik: get("ik").ok_or("no ik")?,
                    at: get("at").unwrap_or_default(),
                })
            });
            match item {
                Ok(i) => found.push(i),
                Err(e) => {
                    tracing::error!(
                        "Outbox file {} is unreadable ({e}): moved to {REJECTED}",
                        p.display()
                    );
                    move_rejected(folder, &p);
                }
            }
        }
        found.sort_by_key(|i| i.seq);
        let rejected_dir = folder.join(REJECTED);
        let rejected_seqs: Vec<u64> = fs::read_dir(&rejected_dir)
            .map(|d| d.flatten().filter_map(|e| seq_of(&e.path())).collect())
            .unwrap_or_default();
        let next = found
            .iter()
            .map(|i| i.seq)
            .chain(rejected_seqs.iter().copied())
            .max()
            .unwrap_or(0)
            + 1;
        if !found.is_empty() {
            tracing::info!(
                "Outbox: {} write(s) waiting from an earlier run",
                found.len()
            );
        }
        let inner = Arc::new(Inner {
            folder: folder.to_owned(),
            state: Mutex::new(State {
                items: found.into(),
                rejected: rejected_seqs.len() as u64,
                next,
            }),
            wake: Condvar::new(),
            idle: Condvar::new(),
            stop: Stop::default(),
            link,
            on_result,
            on_rejected,
            on_change,
            skipped: Mutex::default(),
        });
        let me = Self(inner);
        let runner = me.clone();
        let h = std::thread::Builder::new()
            .name("outbox".into())
            .spawn(move || runner.run())?;
        Ok((me, h))
    }

    /// Stores one write (atomic) and wakes the sender.
    pub fn put(&self, method: &str, ik: &str, params: Value) -> std::io::Result<()> {
        let at = local_iso(true);
        let doc = codec::map([
            ("method", Value::from(method)),
            ("v", Value::from(1u32)),
            ("ik", Value::from(ik)),
            ("p", params),
            ("at", Value::from(at.as_str())),
        ]);
        let bytes = codec::encode(&doc).map_err(|e| std::io::Error::other(e.to_string()))?;
        {
            let mut st = lock(&self.0.state);
            let seq = st.next;
            st.next += 1;
            let path = self.0.folder.join(file_name(seq));
            let tmp = path.with_extension("tmp");
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &path)?;
            st.items.push_back(Item {
                seq,
                path,
                method: method.to_owned(),
                ik: ik.to_owned(),
                at,
            });
        }
        self.0.wake.notify_all();
        self.changed();
        Ok(())
    }

    /// `connector.status.outbox`.
    pub fn summary(&self) -> Summary {
        let st = lock(&self.0.state);
        Summary {
            pending: st.items.len(),
            oldest_at: st.items.front().map(|i| i.at.clone()),
            rejected: st.rejected,
        }
    }

    /// [`summary`](Self::summary) lent to `f` as `(pending, oldest_at, rejected)`: no allocation.
    pub fn with_summary<R>(&self, f: impl FnOnce(usize, Option<&str>, u64) -> R) -> R {
        let st = lock(&self.0.state);
        f(
            st.items.len(),
            st.items.front().map(|i| i.at.as_str()),
            st.rejected,
        )
    }

    /// Waits until every stored write was delivered or rejected.
    pub fn flush(&self, timeout: Duration) -> bool {
        let st = lock(&self.0.state);
        let (st, _) = self
            .0
            .idle
            .wait_timeout_while(st, timeout, |s| !s.items.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        st.items.is_empty()
    }

    /// Delivers what it can within `drain` while connected, then stops the sender. The rest
    /// stays on disk for the next run.
    pub fn close(&self, drain: Duration) {
        if !drain.is_zero() && self.0.link.connected() {
            self.flush(drain);
        }
        self.0.stop.set();
        self.0.wake.notify_all();
    }

    fn changed(&self) {
        if let Some(f) = &self.0.on_change {
            f();
        }
    }

    fn run(&self) {
        let inner = &self.0;
        while !inner.stop.is_set() {
            let item = {
                let st = lock(&inner.state);
                match st.items.front().cloned() {
                    Some(i) => i,
                    None => {
                        inner.idle.notify_all();
                        let _ = inner
                            .wake
                            .wait_timeout(st, Duration::from_millis(200))
                            .unwrap_or_else(|p| p.into_inner());
                        continue;
                    }
                }
            };
            if !inner.link.connected() {
                lock(&inner.skipped).clear(); // the next connection may be another Pitwall
                inner.link.wait_connected(Duration::from_millis(100));
                if !inner.link.connected() {
                    inner.stop.wait(Duration::from_millis(50));
                }
                continue;
            }
            if OPTIONAL_METHODS.contains(&item.method.as_str())
                && inner.link.declares(&item.method) == Some(false)
            {
                self.skip(&item);
                continue;
            }
            self.send(&item);
        }
    }

    /// Drops an optional write the linked Pitwall does not take: not a refusal.
    fn skip(&self, item: &Item) {
        let first = lock(&self.0.skipped).insert(item.method.clone());
        if first {
            let version = self.0.link.peer_version().unwrap_or_else(|| "?".into());
            tracing::info!(
                "Pitwall {version} does not take {}; skipping those writes",
                item.method
            );
        }
        self.finish(item, false);
    }

    fn send(&self, item: &Item) {
        let inner = &self.0;
        let doc = match read_doc(&item.path) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(
                    "Outbox {} unreadable ({e}): moved to {REJECTED}",
                    item.path.display()
                );
                self.finish(item, true);
                return;
            }
        };
        let params = codec::get(&doc, "p").cloned().unwrap_or(Value::Nil);
        let t0 = Instant::now();
        match inner
            .link
            .call(&item.method, params.clone(), Some(&item.ik), SEND_TIMEOUT)
        {
            Ok(result) => {
                tracing::debug!("{} {} sent in {:?}", item.method, item.ik, t0.elapsed());
                self.finish(item, false);
                if let Some(f) = &inner.on_result {
                    f(&item.method, &params, &result);
                }
            }
            Err(CallError::Remote {
                code,
                message,
                retry_ms,
            }) => {
                if code == "unknown_method" && OPTIONAL_METHODS.contains(&item.method.as_str()) {
                    self.skip(item);
                } else if NON_RETRYABLE.contains(&code.as_str()) {
                    tracing::error!(
                        "{} {} refused by Pitwall: {code}: {message} (moved to outbox\\{REJECTED})",
                        item.method,
                        item.ik
                    );
                    self.finish(item, true);
                    if let Some(f) = &inner.on_rejected {
                        f(&item.method, &params, &code);
                    }
                } else {
                    let d = retry_ms.map_or(RETRY_DEFAULT, Duration::from_millis);
                    tracing::info!("{} {}: {code} (retrying in {:?})", item.method, item.ik, d);
                    inner
                        .stop
                        .wait(d.clamp(Duration::from_millis(50), RETRY_MAX));
                }
            }
            Err(CallError::NotConnected | CallError::Lost) => {
                inner.stop.wait(Duration::from_millis(100)); // kept: same key after the reconnect
            }
            Err(e) => {
                tracing::warn!(
                    "{} {}: {e} (retrying with the same key)",
                    item.method,
                    item.ik
                );
                inner.stop.wait(Duration::from_millis(100));
            }
        }
    }

    fn finish(&self, item: &Item, rejected: bool) {
        let inner = &self.0;
        if rejected {
            move_rejected(&inner.folder, &item.path);
        } else if let Err(e) = fs::remove_file(&item.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("Cannot delete {}: {e}", item.path.display());
        }
        {
            let mut st = lock(&inner.state);
            st.items.retain(|i| i.seq != item.seq);
            if rejected {
                st.rejected += 1;
            }
            if st.items.is_empty() {
                inner.idle.notify_all();
            }
        }
        self.changed();
    }
}
