//! The status the connector's loop reports, as a fixed struct, and the thread that publishes it.
//!
//! The loop fills a preallocated [`StatusInput`] in place (strings are cleared and refilled, so
//! after the first fill nothing allocates), compares it field by field with the last one handed
//! over and, only when it differs, copies it into the worker's slot and wakes the worker. The
//! worker thread builds the two JSON values (`connector.status`, `service.status`) and sets the
//! channels when they changed: all the allocation is off the loop.

use crate::link::StatusPublisher;
use serde_json::{Value as Json, json};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A string that may be absent, refilled in place.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OptStr {
    set: bool,
    text: String,
}

impl OptStr {
    /// Sets from `v`, reusing the buffer.
    pub fn assign(&mut self, v: Option<&str>) {
        self.set = v.is_some();
        self.text.clear();
        self.text.push_str(v.unwrap_or(""));
    }

    /// The value.
    pub fn get(&self) -> Option<&str> {
        self.set.then_some(self.text.as_str())
    }
}

/// The fields without text: copied by assignment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plain {
    /// `game_running`.
    pub game_running: bool,
    /// `phase`.
    pub phase: Option<&'static str>,
    /// `stint`.
    pub stint: Option<i64>,
    /// `lap`.
    pub lap: Option<u32>,
    /// `live` is an object (else null).
    pub live: bool,
    /// `live.session_type`.
    pub live_session_type: &'static str,
    /// `live.track_length_m` (null when not above zero).
    pub track_length_m: f64,
    /// `outbox.pending`.
    pub outbox_pending: usize,
    /// `outbox.rejected`.
    pub outbox_rejected: u64,
    /// `error_code`.
    pub error_code: Option<&'static str>,
    /// `message`.
    pub message: &'static str,
    /// `pitwall` (the link's state word).
    pub pitwall: &'static str,
    /// `mapped`.
    pub mapped: bool,
    /// `in_garage`.
    pub in_garage: bool,
    /// `session_open`.
    pub session_open: bool,
    /// `session_id` (Pitwall's).
    pub session_id: Option<i64>,
    /// `session_type`.
    pub session_type: Option<&'static str>,
    /// `laps_queued`.
    pub laps_queued: u64,
    /// `laps_sent`.
    pub laps_sent: u64,
    /// `laps_rejected`.
    pub laps_rejected: u64,
    /// `telemetry`.
    pub telemetry: bool,
    /// `uptime_s` (whole seconds).
    pub uptime_s: f64,
}

impl Default for Plain {
    fn default() -> Self {
        Self {
            game_running: false,
            phase: None,
            stint: None,
            lap: None,
            live: false,
            live_session_type: "other",
            track_length_m: 0.0,
            outbox_pending: 0,
            outbox_rejected: 0,
            error_code: None,
            message: "",
            pitwall: "waiting",
            mapped: false,
            in_garage: false,
            session_open: false,
            session_id: None,
            session_type: None,
            laps_queued: 0,
            laps_sent: 0,
            laps_rejected: 0,
            telemetry: false,
            uptime_s: 0.0,
        }
    }
}

/// Everything the two status channels carry (connector kit, contract.md sections 5.2 and 5.6), as plain data.
#[derive(Debug, PartialEq)]
pub struct StatusInput {
    /// The fields without text.
    pub plain: Plain,
    /// `session_key`.
    pub session_key: OptStr,
    /// `live.car_id`.
    pub car: String,
    /// `live.track_id`.
    pub track: String,
    /// `live.layout_id`.
    pub layout: String,
    /// `outbox.oldest_at`.
    pub oldest_at: OptStr,
    /// `pitwall_message`.
    pub pitwall_message: OptStr,
}

impl Default for StatusInput {
    /// Buffers sized for real ids and messages, so a fill never allocates.
    fn default() -> Self {
        let buf = || String::with_capacity(128);
        let opt = || OptStr {
            set: false,
            text: buf(),
        };
        Self {
            plain: Plain::default(),
            session_key: opt(),
            car: buf(),
            track: buf(),
            layout: buf(),
            oldest_at: opt(),
            pitwall_message: opt(),
        }
    }
}

fn refill(dst: &mut String, src: &str) {
    dst.clear();
    dst.push_str(src);
}

impl StatusInput {
    /// Copies `o` into `self`, reusing the buffers (no allocation once they are big enough).
    pub fn copy_from(&mut self, o: &Self) {
        self.plain = o.plain;
        self.session_key.assign(o.session_key.get());
        refill(&mut self.car, &o.car);
        refill(&mut self.track, &o.track);
        refill(&mut self.layout, &o.layout);
        self.oldest_at.assign(o.oldest_at.get());
        self.pitwall_message.assign(o.pitwall_message.get());
    }

    /// `connector.status` v1 (contract.md section 5.2).
    pub fn protocol_json(&self) -> Json {
        let p = &self.plain;
        let live = if p.live {
            json!({"session_type": p.live_session_type, "car_id": self.car,
                   "track_id": self.track, "layout_id": self.layout,
                   "track_length_m": if p.track_length_m > 0.0 { json!(p.track_length_m) } else { Json::Null }})
        } else {
            Json::Null
        };
        json!({
            "game_running": p.game_running,
            "phase": p.phase,
            "session_key": self.session_key.get(),
            "stint": p.stint,
            "lap": p.lap,
            "live": live,
            "outbox": {"pending": p.outbox_pending, "oldest_at": self.oldest_at.get(),
                       "rejected": p.outbox_rejected},
            "error_code": p.error_code,
            "message": p.message,
        })
    }

    /// What only the control app shows (`service.status`, contract.md section 5.6).
    pub fn extra_json(&self) -> Json {
        let p = &self.plain;
        json!({
            "pitwall": p.pitwall, "pitwall_message": self.pitwall_message.get(),
            "mapped": p.mapped, "in_garage": p.in_garage,
            "session_open": p.session_open,
            "session_id": p.session_id,
            "session_type": p.session_type,
            "laps_queued": p.laps_queued, "laps_sent": p.laps_sent,
            "laps_rejected": p.laps_rejected,
            "telemetry": p.telemetry,
            "uptime_s": p.uptime_s,
        })
    }
}

#[derive(Default)]
struct Slot {
    input: StatusInput,
    pending: bool,
    stop: bool,
}

struct Shared {
    publisher: StatusPublisher,
    slot: Mutex<Slot>,
    wake: Condvar,
    /// Held while a status is built and sent, so an older one never follows a newer one.
    publishing: Mutex<()>,
}

impl Shared {
    fn publish(&self, input: &StatusInput) {
        self.publisher
            .publish(&input.protocol_json(), &input.extra_json());
    }

    fn run(&self) {
        let mut mine = StatusInput::default();
        loop {
            {
                let mut s = lock(&self.slot);
                while !s.pending && !s.stop {
                    s = self.wake.wait(s).unwrap_or_else(|p| p.into_inner());
                }
            }
            let _turn = lock(&self.publishing);
            let stop = {
                let mut s = lock(&self.slot);
                if s.pending {
                    s.pending = false;
                    mine.copy_from(&s.input);
                    drop(s);
                    self.publish(&mine);
                    continue;
                }
                s.stop
            };
            if stop {
                return;
            }
        }
    }
}

/// Publishes statuses on its own thread; the loop only copies a struct into its slot.
pub struct StatusWorker {
    shared: std::sync::Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// The last input handed over (loop side).
    last: Mutex<(bool, StatusInput)>,
}

impl StatusWorker {
    /// Starts the thread.
    pub fn new(publisher: StatusPublisher) -> Self {
        let shared = std::sync::Arc::new(Shared {
            publisher,
            slot: Mutex::default(),
            wake: Condvar::new(),
            publishing: Mutex::new(()),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("status".into())
            .spawn(move || s.run())
            .ok();
        Self {
            shared,
            thread: Mutex::new(thread),
            last: Mutex::new((false, StatusInput::default())),
        }
    }

    /// Hands `input` over when it differs from the last one handed over (no allocation once the
    /// buffers are sized). `now`: publish on this thread before returning (rare moments: a
    /// connection, a session end).
    pub fn submit(&self, input: &StatusInput, now: bool) {
        let mut last = lock(&self.last);
        let (valid, l) = &mut *last;
        if !now && *valid && *l == *input {
            return;
        }
        *valid = true;
        l.copy_from(input);
        if now {
            let _turn = lock(&self.shared.publishing);
            lock(&self.shared.slot).pending = false;
            self.shared.publish(input);
            return;
        }
        {
            let mut s = lock(&self.shared.slot);
            s.input.copy_from(input);
            s.pending = true;
        }
        self.shared.wake.notify_one();
    }

    /// Publishes what is pending and stops the thread.
    pub fn close(&self) {
        lock(&self.shared.slot).stop = true;
        self.shared.wake.notify_one();
        if let Some(h) = lock(&self.thread).take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_reuses_buffers_and_equals() {
        let mut a = StatusInput::default();
        a.car.push_str("Formula");
        a.session_key.assign(Some("k"));
        a.plain.lap = Some(3);
        let mut b = StatusInput::default();
        b.copy_from(&a);
        assert_eq!(a, b);
        assert_eq!(b.protocol_json()["lap"], 3);
        a.session_key.assign(None);
        assert_ne!(a, b);
    }
}
