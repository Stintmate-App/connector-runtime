//! The connector's own endpoint `connector-<id>` (connector kit, contract.md sections 3 and 5.6).
//!
//! * **the control interface** (contract.md section 3): `control.show`, `control.hide`, `control.quit`,
//!   `control.managed`, `control.service.start`, `control.service.stop`,
//!   `control.autostart.set` (all `ordered`; the caller sets the deadline, 5 s by default), `control.catalog.get`,
//!   `control.activity.act`; the state channels `control.state` and `control.activity`, the
//!   event channels `control.notices` and `control.services.desired`. The methods only parse
//!   and check; the app's handler does the work and refuses with `RemoteError`. The caller is
//!   the connection's OS PID chain (`app.exe 12 < explorer.exe 34`), logged with every start,
//!   stop and quit;
//! * **the window's API** ([`crate::api`]): the state channel `ui.snapshot` and the method
//!   `ui.command`, used by the window process;
//! * **the services' status** (connector kit, contract.md section 5.6): each service process opens this endpoint when
//!   it runs and declares the state channel `service.status`; the endpoint subscribes on that
//!   connection and keeps the latest value per service on the [`StatusBoard`], with `pid`
//!   replaced by the connection's OS PID. A service without a connection has no status.

use crate::api::{COMMAND_METHOD, SNAPSHOT_CHANNEL};
use crate::instance::{to_json, to_value};
use crate::texts::Texts;
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use stintmate_ipc::blocking::{
    Delivery, Endpoint, EventChannel, MethodOpts, Peer, StateChannel, SubscribeOptions,
};
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::error::{Error, RemoteError};
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::KeySource;

/// The control methods.
pub const METHODS: [&str; 7] = [
    "control.show",
    "control.hide",
    "control.quit",
    "control.managed",
    "control.service.start",
    "control.service.stop",
    "control.autostart.set",
];
/// The services' status channel.
pub const SERVICE_STATUS: &str = "service.status";
/// A progress is published at most twice a second.
pub const ACTIVITY_PERIOD: Duration = Duration::from_millis(500);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The services' latest `service.status`, shared between the endpoint (writer) and the
/// ServiceManager (reader).
#[derive(Clone, Default)]
pub struct StatusBoard(Arc<Mutex<HashMap<String, (Peer, Json)>>>);

impl StatusBoard {
    /// The service's last status, `None` without a connection.
    pub fn get(&self, service: &str) -> Option<Json> {
        lock(&self.0).get(service).map(|(_, s)| s.clone())
    }
    fn set(&self, service: &str, peer: &Peer, status: Json) {
        lock(&self.0).insert(service.to_string(), (peer.clone(), status));
    }
    fn drop_peer(&self, peer: &Peer) -> Vec<String> {
        let mut m = lock(&self.0);
        let gone: Vec<String> = m
            .iter()
            .filter(|(_, (p, _))| p == peer)
            .map(|(k, _)| k.clone())
            .collect();
        for g in &gone {
            m.remove(g);
        }
        gone
    }
}

/// What the endpoint asks the app to do: `(method, params, caller text, caller pid)`.
pub type Handler =
    Arc<dyn Fn(&str, &Json, &str, Option<u32>) -> Result<Json, RemoteError> + Send + Sync>;

fn bad_request(msg: String, details: Option<Value>) -> RemoteError {
    let mut e = RemoteError::new("bad_request", msg);
    e.details = details;
    e
}

/// The params of a control method, checked (`bad_request` otherwise).
pub fn check_params(method: &str, params: &Json) -> Result<Json, RemoteError> {
    let params = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    if !params.is_object() {
        return Err(bad_request(format!("{method}: params must be a map"), None));
    }
    match method {
        "control.managed" => {
            let ok = params
                .get("parent_pid")
                .and_then(Json::as_i64)
                .is_some_and(|p| p > 0);
            if !ok {
                return Err(bad_request(
                    "control.managed: parent_pid must be a positive integer".into(),
                    None,
                ));
            }
        }
        "control.service.start" | "control.service.stop" => {
            let svc = params.get("service").and_then(Json::as_str);
            if !svc.is_some_and(|s| crate::SERVICES.contains(&s)) {
                return Err(bad_request(
                    format!(
                        "{method}: service must be one of {}",
                        crate::SERVICES.join(", ")
                    ),
                    Some(to_value(&json!({"services": crate::SERVICES}))),
                ));
            }
        }
        "control.autostart.set" if !params.get("enabled").is_some_and(Json::is_boolean) => {
            return Err(bad_request(
                "control.autostart.set: enabled must be a bool".into(),
                None,
            ));
        }
        _ => {}
    }
    Ok(params)
}

#[derive(Default)]
struct Activity {
    items: Vec<Json>,
    last_flush: Option<Instant>,
    timer_pending: bool,
}

/// Where and how the endpoint is served.
pub struct ServeConfig {
    /// The pipe path (`None` = the user's `connector-<id>` pipe).
    pub pipe: Option<String>,
    /// The registry folder (`None` = the workspace's `ipc\endpoints`).
    pub registry: Option<PathBuf>,
    /// The workspace's key.
    pub key: KeySource,
}

/// The served endpoint.
pub struct ControlEndpoint {
    ep: Endpoint,
    state: StateChannel,
    snapshot: StateChannel,
    activity_ch: StateChannel,
    notices: EventChannel,
    desired_ch: EventChannel,
    /// The services' statuses.
    pub board: StatusBoard,
    last_state: Mutex<Option<Json>>,
    last_snapshot: Mutex<Option<Json>>,
    activity: Mutex<Activity>,
    subscribed: Mutex<Vec<Peer>>,
    on_status: Arc<dyn Fn() + Send + Sync>,
    endpoint_id: String,
}

impl ControlEndpoint {
    /// Builds the endpoint (not serving yet). `on_status` is called (on a hook thread) when a
    /// service's status changed or it (dis)connected.
    pub fn new(
        texts: &Texts,
        cfg: ServeConfig,
        handler: Handler,
        board: StatusBoard,
        on_status: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Arc<Self>, Error> {
        let spec = texts.spec();
        let endpoint_id = spec.endpoint();
        let mut info = PeerInfo::new(&endpoint_id);
        info.name = spec.name.into();
        info.version = spec.version.into();
        let mut b = Endpoint::builder(info, cfg.key);
        if let Some(p) = cfg
            .pipe
            .or_else(|| stintmate_connector_platform::dev::pipe_path_if_dev(spec, &endpoint_id))
        {
            b = b.pipe(p);
        }
        if let Some(r) = cfg.registry {
            b = b.registry(r);
        }
        let ep = b.build()?;
        for name in METHODS {
            let h = handler.clone();
            ep.method(
                name,
                MethodOpts::default().ordered(true),
                move |ctx, params| {
                    let checked = check_params(name, &to_json(&params))?;
                    let pid = ctx.peer().pid();
                    let caller = stintmate_connector_platform::procinfo::describe_process(pid, 4);
                    h(name, &checked, &caller, pid).map(|r| to_value(&r))
                },
            )?;
        }
        ep.method(COMMAND_METHOD, MethodOpts::default().ordered(true), {
            let h = handler.clone();
            move |ctx, params| {
                let pid = ctx.peer().pid();
                let caller = stintmate_connector_platform::procinfo::describe_process(pid, 4);
                h(COMMAND_METHOD, &to_json(&params), &caller, pid).map(|r| to_value(&r))
            }
        })?;
        let catalog_opts = MethodOpts {
            idempotent: true,
            idempotency_cache: false,
            ..MethodOpts::default()
        };
        let (catalog_doc, catalog_rev) = {
            let (doc, rev) = texts.document();
            (doc.clone(), rev.to_string())
        };
        ep.method("control.catalog.get", catalog_opts, move |_, _| {
            Ok(to_value(
                &json!({"catalog": catalog_doc, "rev": catalog_rev}),
            ))
        })?;
        ep.method("control.activity.act", MethodOpts::default(), |_, _| {
            Err(RemoteError::new(
                "not_found",
                "the control app's activity items have no actions",
            ))
        })?;
        let state = ep.state_channel("control.state", Value::Nil, &[1])?;
        let snapshot = ep.state_channel(SNAPSHOT_CHANNEL, Value::Nil, &[1])?;
        let activity_ch =
            ep.state_channel("control.activity", to_value(&json!({"items": []})), &[1])?;
        let notices = ep.event_channel("control.notices", &[1])?;
        let desired_ch = ep.event_channel("control.services.desired", &[1])?;
        let this = Arc::new(Self {
            ep,
            state,
            snapshot,
            activity_ch,
            notices,
            desired_ch,
            board,
            last_state: Mutex::default(),
            last_snapshot: Mutex::default(),
            activity: Mutex::default(),
            subscribed: Mutex::default(),
            on_status,
            endpoint_id,
        });
        let weak = Arc::downgrade(&this);
        let w = weak.clone();
        this.ep.on_connect(move |p| {
            if let Some(t) = w.upgrade() {
                t.on_connect(&p);
            }
        });
        let w = weak.clone();
        this.ep.on_redeclare(move |p| {
            if let Some(t) = w.upgrade() {
                t.on_connect(&p);
            }
        });
        this.ep.on_disconnect(move |p| {
            if let Some(t) = weak.upgrade() {
                t.on_disconnect(&p);
            }
        });
        Ok(this)
    }

    /// Serves. Fails with `EndpointInUse` when another process serves the name.
    pub fn start(&self) -> Result<(), Error> {
        self.ep.start()?;
        tracing::info!("Serving {}", self.ep.path());
        Ok(())
    }

    /// Stops serving.
    pub fn stop(&self) {
        self.ep.stop();
    }

    /// The pipe path.
    pub fn path(&self) -> &str {
        self.ep.path()
    }

    /// `control.state`, pushed only when it changed.
    pub fn set_state(&self, value: &Json) -> bool {
        Self::set_if_changed(&self.state, &self.last_state, value)
    }

    /// `ui.snapshot`, pushed only when it changed.
    pub fn set_snapshot(&self, value: &Json) -> bool {
        Self::set_if_changed(&self.snapshot, &self.last_snapshot, value)
    }

    fn set_if_changed(ch: &StateChannel, last: &Mutex<Option<Json>>, value: &Json) -> bool {
        {
            let mut l = lock(last);
            if l.as_ref() == Some(value) {
                return false;
            }
            *l = Some(value.clone());
        }
        let _ = ch.set(&to_value(value));
        true
    }

    /// Replaces the `control.activity` items; published at most twice a second.
    pub fn set_activity(self: &Arc<Self>, items: Vec<Json>) {
        let wait = {
            let mut a = lock(&self.activity);
            a.items = items;
            let wait = a.last_flush.map_or(Duration::ZERO, |t| {
                (t + ACTIVITY_PERIOD).saturating_duration_since(Instant::now())
            });
            if wait.is_zero() {
                None
            } else if a.timer_pending {
                return;
            } else {
                a.timer_pending = true;
                Some(wait)
            }
        };
        match wait {
            None => self.flush_activity(),
            Some(w) => {
                let me = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(w);
                    me.flush_activity();
                });
            }
        }
    }

    fn flush_activity(&self) {
        let items = {
            let mut a = lock(&self.activity);
            a.timer_pending = false;
            a.last_flush = Some(Instant::now());
            a.items.clone()
        };
        let _ = self.activity_ch.set(&to_value(&json!({"items": items})));
    }

    /// Publishes a notice on `control.notices`.
    pub fn notice(&self, title: &str, message: &str, level: &str, at: &str) {
        let _ = self.notices.publish(&to_value(
            &json!({"title": title, "message": message, "level": level, "at": at}),
        ));
    }

    /// A switch the user made in the app's own window or tray while managed.
    pub fn desired(&self, service: &str, on: bool, at: &str) {
        let _ = self
            .desired_ch
            .publish(&to_value(&json!({"service": service, "on": on, "at": at})));
    }

    fn changed(&self) {
        (self.on_status)();
    }

    fn on_connect(self: &Arc<Self>, peer: &Peer) {
        if peer.info().app != self.endpoint_id || !peer.decl().channels.contains_key(SERVICE_STATUS)
        {
            return;
        }
        {
            let mut s = lock(&self.subscribed);
            if s.contains(peer) {
                return;
            }
            s.push(peer.clone());
        }
        let me = self.clone();
        let peer = peer.clone();
        std::thread::spawn(move || me.subscribe(&peer));
    }

    fn subscribe(self: &Arc<Self>, peer: &Peer) {
        let service = peer
            .info()
            .process
            .clone()
            .unwrap_or_else(|| "?".to_string());
        let (me, p, svc) = (self.clone(), peer.clone(), service.clone());
        let result = peer.subscribe(SERVICE_STATUS, SubscribeOptions::default(), move |d| {
            let Delivery::Event(ev) = d else { return };
            let Json::Object(mut status) = to_json(&ev.data) else {
                return;
            };
            // The OS's word, not the service's.
            status.insert("pid".into(), json!(p.pid()));
            me.board.set(&svc, &p, Json::Object(status));
            me.changed();
        });
        match result {
            Ok(_) => tracing::info!("The {service} (pid {:?}) is connected", peer.pid()),
            Err(e) => {
                tracing::warn!("Cannot subscribe to the {service}'s status: {e}");
                lock(&self.subscribed).retain(|p| p != peer);
            }
        }
    }

    fn on_disconnect(&self, peer: &Peer) {
        lock(&self.subscribed).retain(|p| p != peer);
        let gone = self.board.drop_peer(peer);
        if !gone.is_empty() {
            tracing::info!("The {} disconnected", gone.join(", "));
            self.changed();
        }
    }
}

/// A map value from pairs, for the details of an error.
pub fn details(pairs: &[(&str, Json)]) -> Value {
    codec::map(pairs.iter().map(|(k, v)| (*k, to_value(v))))
}
