//! Sending announcements to Pitwall (connector kit, contract.md section 4.4) and the
//! per-connection declarations.
//!
//! At every new connection [`declare_connection`] sends, idempotently with their `ik`,
//! `pitwall.catalog.declare` (the connector's own catalog; nothing is announced before it was
//! answered), `pitwall.guidance.declare` and `pitwall.actions.declare` (5 s, a failure is only
//! logged). [`Announcer::announce`] sends `pitwall.announce {code, params, session_key?, data?}`
//! without waiting, after checking the params against the catalog and keeping the contract's
//! windows itself (10 per 60 s, and 120 per 60 s for the `spotter` level); while Pitwall is
//! away, or over the limit, it drops (never queues).

use crate::link::{CallError, Transport, to_value};
use serde_json::{Map, Value as Json, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use stintmate_connector_wire::actions::{DECLARE_TIMEOUT_S, actions_ik};
use stintmate_connector_wire::catalog::{AnnounceCatalog, canonical, check_params, sha12};
use stintmate_connector_wire::rate::{RATE_WINDOW_S, RateWindow, SPOTTER_RATE_LIMIT};
use stintmate_ipc::codec::Value;

/// Wait for the catalog declaration.
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for an announcement's accept.
pub const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes of `data` (JSON-encoded) before it is left out.
pub const MAX_DATA: usize = 2048;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One announcement to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    /// The catalog code.
    pub code: &'static str,
    /// Which wording: 0 is the catalog `text`, 1.. its `variants`.
    pub variant: u8,
    /// Its params, as the catalog declares them.
    pub params: Map<String, Json>,
    /// Facts for the engineer (`pitwall.announce` `data`).
    pub data: Option<Map<String, Json>>,
}

/// Counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceStats {
    /// Sent.
    pub sent: u64,
    /// Dropped (not connected, rate limit, bad params).
    pub dropped: u64,
    /// Refused by Pitwall.
    pub refused: u64,
}

/// Sends announcements. Shared between the loop and the action pool.
pub struct Announcer {
    link: Arc<dyn Transport>,
    catalog: &'static AnnounceCatalog,
    ik_prefix: &'static str,
    session_key: Box<dyn Fn() -> Option<String> + Send + Sync>,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    declared: AtomicBool,
    window: Mutex<RateWindow>,
    spotter_window: Mutex<RateWindow>,
    stats: Mutex<AnnounceStats>,
}

impl Announcer {
    /// An announcer on `link` for `catalog`; its idempotency keys start with `ik_prefix` (the
    /// connector's id). `session_key()` names the session being recorded.
    pub fn new(
        link: Arc<dyn Transport>,
        catalog: &'static AnnounceCatalog,
        ik_prefix: &'static str,
        session_key: impl Fn() -> Option<String> + Send + Sync + 'static,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            link,
            catalog,
            ik_prefix,
            session_key: Box::new(session_key),
            clock: Box::new(clock),
            declared: AtomicBool::new(false),
            window: Mutex::new(RateWindow::default()),
            spotter_window: Mutex::new(RateWindow::new(SPOTTER_RATE_LIMIT, RATE_WINDOW_S)),
            stats: Mutex::default(),
        }
    }

    /// The prefix of the idempotency keys (the connector's id).
    pub fn ik_prefix(&self) -> &'static str {
        self.ik_prefix
    }

    /// The counters.
    pub fn stats(&self) -> AnnounceStats {
        *lock(&self.stats)
    }

    /// The catalog was declared on this connection.
    pub fn declared(&self) -> bool {
        self.declared.load(Ordering::Relaxed)
    }

    /// The connection ended: nothing is announced until the next declaration.
    pub fn lost(&self) {
        self.declared.store(false, Ordering::Relaxed);
    }

    /// `pitwall.catalog.declare` (idempotent), after each new connection.
    pub fn declare(&self) -> bool {
        self.declared.store(false, Ordering::Relaxed);
        let catalog = self.catalog.doc();
        let params = to_value(&json!({ "catalog": catalog }));
        match self.link.call(
            "pitwall.catalog.declare",
            params,
            Some(&self.catalog.ik(self.ik_prefix)),
            CATALOG_TIMEOUT,
        ) {
            Ok(_) => {
                self.declared.store(true, Ordering::Relaxed);
                tracing::info!(
                    "Announcement catalog {} declared",
                    catalog["catalog"].as_str().unwrap_or_default()
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    "Announcement catalog not declared: {e} (announcements are dropped until the next connection)"
                );
                false
            }
        }
    }

    fn drop_count(&self) {
        lock(&self.stats).dropped += 1;
    }

    /// The `pitwall.announce` params and the window slot, or why it is dropped.
    fn admit(&self, a: &Announcement) -> Result<(Value, f64, bool), String> {
        let params_json = Json::Object(a.params.clone());
        if let Some(problem) = check_params(a.code, &params_json, self.catalog.messages()) {
            tracing::error!("Announcement {} not sent: {problem}", a.code);
            self.drop_count();
            return Err(problem);
        }
        if !self.declared() || !self.link.connected() {
            tracing::info!("Announcement {} dropped: Pitwall is not connected", a.code);
            self.drop_count();
            return Err("Pitwall is not connected".into());
        }
        if a.variant >= self.catalog.variant_count(a.code) {
            tracing::error!(
                "Announcement {} not sent: variant {} is not in the catalog",
                a.code,
                a.variant
            );
            self.drop_count();
            return Err(format!("variant {} is not in the catalog", a.variant));
        }
        let now = (self.clock)();
        let spotter = self.catalog.is_spotter(a.code);
        let slot = {
            let mut w = lock(if spotter {
                &self.spotter_window
            } else {
                &self.window
            });
            match w.admit(now) {
                Some(s) => s,
                None => {
                    tracing::info!("Announcement {} dropped: rate limit", a.code);
                    let why = w.reason();
                    drop(w);
                    self.drop_count();
                    return Err(why);
                }
            }
        };
        let mut p = json!({"code": a.code, "params": params_json});
        if a.variant != 0 {
            p["variant"] = a.variant.into();
        }
        if let Some(key) = (self.session_key)() {
            p["session_key"] = key.into();
        }
        if let Some(data) = &a.data
            && !data.is_empty()
        {
            let d = Json::Object(data.clone());
            if d.to_string().len() <= MAX_DATA {
                p["data"] = d;
            } else {
                tracing::warn!(
                    "Announcement {}: data over {MAX_DATA} bytes left out",
                    a.code
                );
            }
        }
        Ok((to_value(&p), slot, spotter))
    }

    /// Sends one announcement now, or drops it. Never blocks.
    pub fn announce(self: &Arc<Self>, a: &Announcement) -> bool {
        let Ok((params, slot, spotter)) = self.admit(a) else {
            return false;
        };
        let (me, code) = (self.clone(), a.code);
        let sent = self.link.call_async(
            "pitwall.announce",
            params,
            ANNOUNCE_TIMEOUT,
            Box::new(move |r| {
                if let Err(e) = r {
                    lock(&me.stats).refused += 1;
                    tracing::warn!("Announcement {code} refused by Pitwall: {e}");
                }
            }),
        );
        match sent {
            Ok(()) => {
                lock(&self.stats).sent += 1;
                if spotter {
                    // a spotter call per side change: hundreds per race
                    tracing::debug!("Announcement {} sent", a.code);
                } else {
                    tracing::info!("Announcement {} sent", a.code);
                }
                true
            }
            Err(e) => {
                tracing::info!("Announcement {} dropped: {e}", a.code);
                self.drop_count();
                lock(if spotter {
                    &self.spotter_window
                } else {
                    &self.window
                })
                .release(slot);
                false
            }
        }
    }

    /// As [`announce`](Self::announce) but waits (at most `timeout`) for Pitwall's accept: for a
    /// thread that is not the loop (an action's handler).
    pub fn announce_wait(&self, a: &Announcement, timeout: Duration) -> Result<Value, String> {
        let (params, _, _) = self.admit(a)?;
        match self.link.call("pitwall.announce", params, None, timeout) {
            Ok(v) => {
                lock(&self.stats).sent += 1;
                Ok(v)
            }
            Err(e) => {
                lock(&self.stats).refused += 1;
                tracing::warn!("Announcement {} refused by Pitwall: {e}", a.code);
                Err(e.to_string())
            }
        }
    }
}

fn declare_optional(link: &dyn Transport, method: &str, params: Json, ik: &str) {
    let timeout = Duration::from_secs_f64(DECLARE_TIMEOUT_S);
    match link.call(method, to_value(&params), Some(ik), timeout) {
        Ok(_) => tracing::info!("{method}: declared"),
        Err(CallError::Remote { code, .. }) if code == "not_found" || code == "unknown_method" => {
            tracing::info!("{method}: this Pitwall does not support it ({code})");
        }
        Err(e) => tracing::warn!("{method}: not declared: {e}"),
    }
}

/// Everything declared at each new connection, in this order: catalog, guidance, actions.
pub fn declare_connection(
    link: &dyn Transport,
    announcer: &Announcer,
    guidance_items: &Json,
    actions: &Json,
) {
    announcer.declare();
    let prefix = announcer.ik_prefix;
    declare_optional(
        link,
        "pitwall.guidance.declare",
        json!({ "items": guidance_items }),
        &format!("{prefix}/guidance/{}", sha12(&canonical(guidance_items))),
    );
    declare_optional(
        link,
        "pitwall.actions.declare",
        json!({ "actions": actions }),
        &actions_ik(prefix, actions),
    );
}
