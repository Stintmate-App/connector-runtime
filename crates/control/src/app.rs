//! The control app's core: everything the tray and the window show and do, without a UI.
//!
//! [`ControlApp`] owns the ServiceManager, the options, managed mode and the control endpoint.
//! A UI drives it through [`crate::api`]: [`ControlApp::dispatch`] takes a
//! [`Command`], [`ControlApp::snapshot`] gives what to draw, [`ControlApp::take_events`] the
//! [`UiEvent`]s. The endpoint's `ui.command` / `ui.snapshot` carry the same types to a window
//! process.

use crate::api::{Command, Notice, OptionsChange, Snapshot, UiEvent};
use crate::autostart::Autostart;
use crate::config::{self, UiConfig};
use crate::endpoint::{ControlEndpoint, Handler, ServeConfig, StatusBoard, details};
use crate::managed::{PARENT_CHECK, ParentWatch};
use crate::resume_watch::ResumeProbe;
use crate::services::{Host, ManagerConfig, ServiceManager};
use crate::status::{self, Level, ServiceState};
use crate::texts::Texts;
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_ipc::error::{Error, RemoteError};

/// The answer goes out first; the exit starts after this.
const QUIT_DELAY: Duration = Duration::from_millis(50);
/// The supervisor's period.
pub const TICK: Duration = Duration::from_millis(1000);
/// The supervisor period while nothing runs, nothing is wanted and the app is standalone: the
/// only changes it still has to look for are an instance started by someone else (its mutex)
/// and the game starting (a window lookup); everything else arrives as an event (a command, a
/// service's `service.status`) and refreshes at once.
pub const IDLE_TICK: Duration = Duration::from_secs(5);
/// How long the cached autostart value (the `Run` key) is trusted; the app's own changes
/// refresh it at once.
pub const AUTOSTART_TTL: Duration = Duration::from_secs(30);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Everything the app needs from its surroundings.
pub struct Environment {
    /// The connector.
    pub spec: &'static ConnectorSpec,
    /// The connector's look at the game for a kept-open session.
    pub resume: Arc<dyn ResumeProbe>,
    /// The workspace folder.
    pub workspace: PathBuf,
    /// `<workspace>\connectors.<id>`.
    pub data_dir: PathBuf,
    /// One line on where the workspace came from.
    pub data_source: String,
    /// `--workspace ...` for the processes the app starts.
    pub workspace_args: Vec<String>,
    /// This exe (the Run value and the descriptor's launcher).
    pub ui_exe: PathBuf,
    /// The recorder's executable.
    pub recorder_exe: PathBuf,
    /// The recorder's mutex and stop event.
    pub recorder_mutex: String,
    /// The recorder's stop event.
    pub recorder_stop: String,
    /// Further arguments for the recorder (tests pass the pipes).
    pub recorder_extra_args: Vec<String>,
    /// Forced stop after this long.
    pub stop_timeout: Duration,
    /// The operating system's side.
    pub host: Box<dyn Host>,
    /// The "start with Windows" store.
    pub autostart: Autostart,
    /// The texts.
    pub texts: Texts,
    /// The managing app's watch.
    pub parent_watch: Box<dyn Fn(u32) -> ParentWatch + Send + Sync>,
    /// A tray icon can be shown.
    pub tray_available: bool,
    /// Replaces the clocks of the ServiceManager (tests).
    pub time: Option<crate::services::TimeSource>,
}

struct Inner {
    manager: ServiceManager,
    cfg: UiConfig,
    managed: bool,
    parent: Option<ParentWatch>,
    window_visible: bool,
    /// The last request for the window process: a counter and `show`, `logs` or `hide`.
    window_request: (u64, &'static str),
    exiting: bool,
    tray_available: bool,
    last_parent_check: Instant,
}

/// The control app's core.
pub struct ControlApp {
    spec: &'static ConnectorSpec,
    inner: Mutex<Inner>,
    env_texts: Texts,
    data_dir: PathBuf,
    data_source: String,
    workspace: PathBuf,
    autostart: Autostart,
    parent_watch: Box<dyn Fn(u32) -> ParentWatch + Send + Sync>,
    notices_rx: Mutex<Receiver<(String, String, String)>>,
    events_tx: Mutex<Sender<UiEvent>>,
    latest: Mutex<Option<Snapshot>>,
    events_rx: Mutex<Option<Receiver<UiEvent>>>,
    endpoint: Mutex<Option<Arc<ControlEndpoint>>>,
    board: StatusBoard,
    stopping: AtomicBool,
    /// Wakes the ticker early (a service became busy, the app is exiting).
    wake: (Mutex<bool>, Condvar),
    /// The ticker is in its long idle wait.
    idle: AtomicBool,
    /// The autostart value and when it was read.
    autostart_cache: Mutex<Option<(Option<bool>, Instant)>>,
    me: Weak<ControlApp>,
}

fn remote(
    code: &str,
    msg: impl Into<String>,
    details: Option<stintmate_ipc::codec::Value>,
) -> RemoteError {
    let mut e = RemoteError::new(code, msg);
    e.details = details;
    e
}

impl ControlApp {
    /// Builds the app. Nothing starts and nothing is served yet.
    pub fn new(env: Environment) -> Arc<Self> {
        let board = StatusBoard::default();
        let (ntx, nrx) = channel::<(String, String, String)>();
        let ntx = Mutex::new(ntx);
        let reader = board.clone();
        let cfg = config::load(&env.data_dir.join(config::FILE_NAME), env.spec.recorder_hz);
        let mut manager = ServiceManager::new(
            ManagerConfig {
                recorder_exe: env.recorder_exe.clone(),
                cwd: env
                    .recorder_exe
                    .parent()
                    .map_or_else(|| PathBuf::from("."), PathBuf::from),
                mutex: env.recorder_mutex.clone(),
                stop_event: env.recorder_stop.clone(),
                workspace_args: env.workspace_args.clone(),
                extra_args: env.recorder_extra_args.clone(),
                stop_timeout: env.stop_timeout,
            },
            env.texts.clone(),
            env.host,
        )
        .with_resume(env.resume.clone())
        .with_statuses(Box::new(move |s| reader.get(s)))
        .with_notifier(Box::new(move |s, t, m| {
            let _ = lock(&ntx).send((s.into(), t.into(), m.into()));
        }));
        if let Some(t) = env.time {
            manager = manager.with_time(t.clock, t.wall, t.sleep);
        }
        let (etx, erx) = channel();
        Arc::new_cyclic(|me| Self {
            spec: env.spec,
            inner: Mutex::new(Inner {
                manager,
                cfg,
                managed: false,
                parent: None,
                window_visible: false,
                window_request: (0, "none"),
                exiting: false,
                tray_available: env.tray_available,
                last_parent_check: Instant::now(),
            }),
            env_texts: env.texts,
            data_dir: env.data_dir,
            data_source: env.data_source,
            workspace: env.workspace,
            autostart: env.autostart,
            parent_watch: env.parent_watch,
            notices_rx: Mutex::new(nrx),
            events_tx: Mutex::new(etx),
            latest: Mutex::new(None),
            events_rx: Mutex::new(Some(erx)),
            endpoint: Mutex::new(None),
            board,
            stopping: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            idle: AtomicBool::new(false),
            autostart_cache: Mutex::new(None),
            me: me.clone(),
        })
    }

    /// The UI's events (take it once).
    pub fn take_events(&self) -> Option<Receiver<UiEvent>> {
        lock(&self.events_rx).take()
    }

    fn emit(&self, ev: UiEvent) {
        let request = match &ev {
            UiEvent::ShowWindow => Some("show"),
            UiEvent::ShowLogs => Some("logs"),
            UiEvent::HideWindow => Some("hide"),
            _ => None,
        };
        if let Some(action) = request {
            let mut g = lock(&self.inner);
            g.window_request = (g.window_request.0 + 1, action);
        }
        let _ = lock(&self.events_tx).send(ev);
        if request.is_some() {
            self.publish();
        }
    }

    fn endpoint(&self) -> Option<Arc<ControlEndpoint>> {
        lock(&self.endpoint).clone()
    }

    /// Serves `connector-<id>`. Fails with `EndpointInUse` when another process serves it.
    pub fn serve(&self, cfg: ServeConfig) -> Result<(), Error> {
        let weak = self.me.clone();
        let handler: Handler = Arc::new(move |method, params, caller, pid| match weak.upgrade() {
            Some(app) => app.handle(method, params, caller, pid),
            None => Err(remote("busy", "the app is exiting", None)),
        });
        let weak = self.me.clone();
        let on_status: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(app) = weak.upgrade() {
                app.refresh();
            }
        });
        let ep =
            ControlEndpoint::new(&self.env_texts, cfg, handler, self.board.clone(), on_status)?;
        ep.start()?;
        *lock(&self.endpoint) = Some(ep);
        self.publish();
        Ok(())
    }

    /// Stops serving.
    pub fn unserve(&self) {
        if let Some(ep) = lock(&self.endpoint).take() {
            ep.stop();
        }
    }

    // ---- lifecycle --------------------------------------------------------------------

    /// Polls once (adopts instances started by hand), enters managed mode when `parent_pid`
    /// is given and alive, and starts the services set to start with the app (standalone).
    pub fn start(&self, parent_pid: Option<u32>) {
        if let Some(pid) = parent_pid
            && let Err(e) = self.set_managed(pid)
        {
            tracing::warn!("Not managed: {}; running standalone", e.message);
        }
        self.refresh();
        if !lock(&self.inner).managed {
            self.start_remembered();
        }
    }

    /// Starts every service whose "start with the app" setting is on and that is not already
    /// running (an adopted instance counts). Returns the names started.
    pub fn start_remembered(&self) -> Vec<String> {
        let mut started = Vec::new();
        {
            let mut g = lock(&self.inner);
            for name in crate::SERVICES {
                if !g.cfg.starts_with_app(name) || g.manager.state(name).running {
                    continue;
                }
                tracing::info!("Start {name} (start with the app)");
                let hz = g.cfg.recorder_hz;
                g.manager.start(name, Some(hz));
                started.push(name.to_string());
            }
        }
        if !started.is_empty() {
            self.refresh();
        }
        started
    }

    /// One supervisor period: refresh, and watch the managing app every 2 s.
    pub fn tick(&self) {
        let fall_back = {
            let mut g = lock(&self.inner);
            if g.exiting {
                return;
            }
            if g.last_parent_check.elapsed() >= PARENT_CHECK {
                g.last_parent_check = Instant::now();
                g.parent.as_ref().is_some_and(|p| !p.alive())
            } else {
                false
            }
        };
        if fall_back {
            self.fall_back();
        }
        self.refresh();
    }

    /// Runs [`tick`](Self::tick) on a thread until [`stop_ticker`](Self::stop_ticker): every
    /// [`TICK`] while [`period`](Self::period) says so, else every [`IDLE_TICK`] (woken early
    /// when a service becomes busy). Between ticks the thread sleeps on a condition variable.
    pub fn spawn_ticker(self: &Arc<Self>) -> std::thread::JoinHandle<()> {
        let app = self.clone();
        std::thread::Builder::new()
            .name("ui-ticker".into())
            .spawn(move || {
                while !app.stopping.load(Ordering::SeqCst) {
                    app.tick();
                    let period = app.period();
                    app.idle.store(period > TICK, Ordering::SeqCst);
                    let (m, cv) = &app.wake;
                    let woken = lock(m);
                    let (mut woken, _) = cv
                        .wait_timeout_while(woken, period, |w| {
                            !*w && !app.stopping.load(Ordering::SeqCst)
                        })
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *woken = false;
                }
            })
            .expect("a thread starts")
    }

    /// The next supervisor period: [`TICK`] while a service is busy (see
    /// [`ServiceManager::busy`]), [`PARENT_CHECK`] in managed mode, else [`IDLE_TICK`].
    pub fn period(&self) -> Duration {
        let g = lock(&self.inner);
        if g.manager.busy()
            || g.manager
                .watching(&self.data_dir.join(crate::resume_watch::FILE))
        {
            TICK
        } else if g.parent.is_some() {
            PARENT_CHECK
        } else {
            IDLE_TICK
        }
    }

    fn wake_ticker(&self) {
        let (m, cv) = &self.wake;
        *lock(m) = true;
        cv.notify_all();
    }

    /// Ends the ticker thread.
    pub fn stop_ticker(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.wake_ticker();
    }

    /// The autostart value, read from the `Run` key at most every [`AUTOSTART_TTL`].
    fn autostart_enabled(&self) -> Option<bool> {
        let mut c = lock(&self.autostart_cache);
        match *c {
            Some((v, at)) if at.elapsed() < AUTOSTART_TTL => v,
            _ => {
                let v = self.autostart.is_enabled().ok();
                *c = Some((v, Instant::now()));
                v
            }
        }
    }

    /// Forgets the cached autostart value (the app changed it).
    fn autostart_changed(&self) {
        *lock(&self.autostart_cache) = None;
    }

    /// Exit: stops every service cleanly (forced after the stop timeout), closes the job and
    /// the endpoint, then sends [`UiEvent::Quit`]. Runs on its own thread.
    pub fn exit(&self, origin: &str) {
        {
            let mut g = lock(&self.inner);
            if g.exiting {
                return;
            }
            g.exiting = true;
        }
        tracing::info!("Exit requested ({origin})");
        self.publish();
        let me = self.me.clone();
        std::thread::Builder::new()
            .name("ui-exit".into())
            .spawn(move || {
                let Some(app) = me.upgrade() else { return };
                {
                    let mut g = lock(&app.inner);
                    let running: Vec<String> = crate::SERVICES
                        .iter()
                        .filter(|n| g.manager.state(n).running)
                        .map(|n| (*n).to_string())
                        .collect();
                    for n in &running {
                        g.manager.request_stop(n);
                    }
                }
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let any = {
                        let mut g = lock(&app.inner);
                        g.manager.poll().iter().any(|s| s.running)
                    };
                    if !any || Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                lock(&app.inner).manager.close();
                app.stop_ticker();
                app.unserve();
                tracing::info!("{} app exiting", app.spec.name);
                app.emit(UiEvent::Quit);
            })
            .expect("a thread starts");
    }

    // ---- managed mode -----------------------------------------------------------------

    /// Runs for a managing app: no tray icon, notices on the channel only, watching `pid`.
    pub fn set_managed(&self, pid: u32) -> Result<Json, RemoteError> {
        let watch = (self.parent_watch)(pid);
        if !watch.alive() {
            return Err(remote(
                "bad_request",
                format!("process {pid} is not running"),
                None,
            ));
        }
        let was = {
            let mut g = lock(&self.inner);
            let was = g.managed;
            g.parent = Some(watch);
            g.managed = true;
            was
        };
        if was {
            tracing::info!("Managed mode: now watching parent pid {pid}");
        } else {
            tracing::info!("Managed mode (parent pid {pid}): tray icon hidden");
            self.emit(UiEvent::ManagedChanged(true));
        }
        self.publish();
        Ok(json!({"managed": true, "parent_pid": pid}))
    }

    fn fall_back(&self) {
        let (pid, states) = {
            let mut g = lock(&self.inner);
            let pid = g.parent.as_ref().map(|p| p.pid);
            g.parent = None;
            g.managed = false;
            let states: Vec<String> = g
                .manager
                .states()
                .iter()
                .map(|s| format!("{} {}", s.service, s.state))
                .collect();
            (pid, states)
        };
        tracing::warn!(
            "The managing app (pid {pid:?}) is gone: standalone mode, tray icon back; the services are left as they are ({})",
            states.join(", ")
        );
        self.emit(UiEvent::ManagedChanged(false));
        self.publish();
    }

    // ---- control methods --------------------------------------------------------------

    /// One control call (params already checked). `ui.command` goes to [`dispatch`](Self::dispatch).
    pub fn handle(
        &self,
        method: &str,
        params: &Json,
        caller: &str,
        caller_pid: Option<u32>,
    ) -> Result<Json, RemoteError> {
        if lock(&self.inner).exiting {
            return Err(remote(
                "busy",
                "the app is exiting",
                Some(details(&[("state", json!("exiting"))])),
            ));
        }
        match method {
            "control.show" => {
                self.emit(UiEvent::ShowWindow);
                Ok(json!({"window": "shown"}))
            }
            "control.hide" => {
                {
                    let g = lock(&self.inner);
                    if !g.managed && !g.tray_available {
                        return Err(remote(
                            "bad_request",
                            "no tray icon to show the window again",
                            None,
                        ));
                    }
                }
                self.emit(UiEvent::HideWindow);
                lock(&self.inner).window_visible = false;
                self.publish();
                Ok(json!({"window": "hidden"}))
            }
            "control.quit" => {
                self.check_quit_allowed(caller, caller_pid)?;
                let origin = format!("control.quit, caller {caller}");
                let me = self.me.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(QUIT_DELAY);
                    if let Some(app) = me.upgrade() {
                        app.exit(&origin);
                    }
                });
                Ok(json!({"exiting": true}))
            }
            "control.managed" => {
                let pid = params["parent_pid"].as_u64().unwrap_or(0);
                self.set_managed(u32::try_from(pid).unwrap_or(0))
            }
            "control.service.start" | "control.service.stop" => {
                let name = params["service"].as_str().unwrap_or_default().to_string();
                let state = lock(&self.inner).manager.state(&name);
                if state.state == "stopping" {
                    return Err(remote(
                        "busy",
                        format!("the {name} is busy ({})", state.state),
                        Some(details(&[("state", json!(state.state))])),
                    ));
                }
                tracing::info!("{method} {name}, caller {caller}");
                self.switch(&name, method == "control.service.start", true);
                let st = lock(&self.inner).manager.state(&name).state;
                Ok(json!({"service": name, "state": st}))
            }
            "control.autostart.set" => {
                let enabled = params["enabled"].as_bool().unwrap_or(false);
                let r = if enabled {
                    self.autostart.enable()
                } else {
                    self.autostart.disable()
                }
                .and_then(|()| self.autostart.is_enabled());
                self.autostart_changed();
                let now = r.map_err(|e| {
                    remote(
                        "internal",
                        format!("cannot change the Windows autostart entry: {e}"),
                        None,
                    )
                })?;
                tracing::info!(
                    "Autostart {} (control.autostart.set, caller {caller})",
                    if enabled { "on" } else { "off" }
                );
                self.publish();
                Ok(json!({"autostart": now}))
            }
            crate::api::COMMAND_METHOD => {
                let cmd = Command::from_json(params).map_err(|m| remote("bad_request", m, None))?;
                self.dispatch(cmd)
                    .map_err(|m| remote("bad_request", m, None))?;
                Ok(json!({"ok": true}))
            }
            other => Err(remote(
                "bad_request",
                format!("unknown method {other}"),
                None,
            )),
        }
    }

    fn check_quit_allowed(&self, caller: &str, caller_pid: Option<u32>) -> Result<(), RemoteError> {
        let boss = {
            let g = lock(&self.inner);
            if !g.managed {
                return Ok(());
            }
            let Some(p) = g.parent.as_ref() else {
                return Ok(());
            };
            p.pid
        };
        let related = caller_pid == Some(boss)
            || caller_pid.is_some_and(|c| {
                stintmate_connector_platform::procinfo::process_chain(c, 6)
                    .iter()
                    .any(|(p, _)| *p == boss)
            });
        if related {
            return Ok(());
        }
        let name = stintmate_connector_platform::procinfo::process_chain(boss, 1)
            .first()
            .map_or_else(|| "the managing app".to_string(), |c| c.1.clone());
        tracing::warn!("control.quit refused: managed by pid {boss}, caller {caller}");
        Err(remote(
            "managed",
            format!("managed by {name} (pid {boss}): quit it from there"),
            Some(details(&[("managed_by", json!(boss))])),
        ))
    }

    /// Start (`on`) a service that is off, or ask a running one to stop. A hand switch is
    /// remembered as its start-with-the-app setting and, managed, published as
    /// `control.services.desired`; a managing app's command (`by_hub`) is neither.
    fn switch(&self, name: &str, on: bool, by_hub: bool) {
        let (running, managed) = {
            let mut g = lock(&self.inner);
            (g.manager.state(name).running, g.managed)
        };
        if on == running {
            self.refresh();
            return;
        }
        tracing::info!("{} {name} requested", if on { "Start" } else { "Stop" });
        if !by_hub {
            self.remember(name, on);
            if managed && let Some(ep) = self.endpoint() {
                ep.desired(name, on, &crate::timefmt::iso_utc_now());
            }
        }
        {
            let mut g = lock(&self.inner);
            if on {
                let hz = g.cfg.recorder_hz;
                g.manager.start(name, Some(hz));
            } else {
                g.manager.request_stop(name);
            }
        }
        self.refresh();
    }

    fn remember(&self, name: &str, on: bool) {
        let mut g = lock(&self.inner);
        if g.cfg.starts_with_app(name) == on {
            return;
        }
        g.cfg.start_with_app.insert(name.to_string(), on);
        if let Err(e) = config::save(&g.cfg, &self.data_dir.join(config::FILE_NAME)) {
            tracing::warn!("Cannot save the start-with-the-app setting: {e}");
        }
    }

    // ---- the UI's API -----------------------------------------------------------------

    /// What the tray and the window ask for.
    pub fn dispatch(&self, cmd: Command) -> Result<(), String> {
        match cmd {
            Command::Switch { service, on } => {
                if !crate::SERVICES.contains(&service.as_str()) {
                    return Err(format!("unknown service {service}"));
                }
                if lock(&self.inner).manager.state(&service).state == "stopping" {
                    return Ok(());
                }
                self.switch(&service, on, false);
            }
            Command::Exit { origin } => self.exit(&origin),
            Command::OpenDataFolder => {
                let _ = std::fs::create_dir_all(&self.data_dir);
                self.emit(UiEvent::OpenFolder(self.data_dir.clone()));
            }
            Command::SetAutostart(on) => {
                if lock(&self.inner).managed {
                    return Err("the autostart entry is the managing app's".into());
                }
                if on {
                    self.autostart.enable()
                } else {
                    self.autostart.disable()
                }
                .map_err(|e| e.to_string())?;
                self.autostart_changed();
                self.publish();
            }
            Command::SetOptions(change) => self.set_options(&change)?,
            Command::WindowVisible(v) => {
                lock(&self.inner).window_visible = v;
                self.publish();
            }
            Command::ShowWindow => self.emit(UiEvent::ShowWindow),
            Command::ShowLogs => self.emit(UiEvent::ShowLogs),
        }
        Ok(())
    }

    fn set_options(&self, change: &OptionsChange) -> Result<(), String> {
        {
            let mut g = lock(&self.inner);
            let mut next = g.cfg.clone();
            if let Some(v) = change.start_minimized {
                next.start_minimized = v;
            }
            if let Some(v) = change.recorder_hz {
                next.recorder_hz = v;
            }
            if let Some(v) = &change.logs_default {
                next.logs_default.clone_from(v);
            }
            config::save(&next, &self.data_dir.join(config::FILE_NAME))
                .map_err(|e| e.to_string())?;
            g.cfg = next;
        }
        self.publish();
        Ok(())
    }

    /// The options now.
    pub fn config(&self) -> UiConfig {
        lock(&self.inner).cfg.clone()
    }

    /// The snapshot last published (cheap: no recomputation), or a fresh one before the
    /// first publication. What the tray redraws from every second.
    pub fn latest_snapshot(&self) -> Snapshot {
        let latest = lock(&self.latest).clone();
        latest.unwrap_or_else(|| self.snapshot())
    }

    /// Everything to draw.
    pub fn snapshot(&self) -> Snapshot {
        let mut g = lock(&self.inner);
        let states = g.manager.states();
        self.snapshot_of(&mut g, &states)
    }

    fn snapshot_of(&self, g: &mut Inner, states: &[ServiceState]) -> Snapshot {
        let texts = &self.env_texts;
        let level = status::overall_level(states);
        let outbox = outbox_of(states);
        let services: Vec<Json> = states
            .iter()
            .map(|s| {
                let mut v = service_summary(s);
                v["full_label"] = json!(s.full_label);
                v["detail"] = Json::Object(s.detail.clone());
                v
            })
            .collect();
        Snapshot(json!({
            "services": services,
            "level": level.as_str(),
            "tooltip": status::tooltip(texts, states),
            "managed": g.managed,
            "parent_pid": g.parent.as_ref().map(|p| p.pid),
            "window": if g.window_visible { "shown" } else { "hidden" },
            "window_request": {"seq": g.window_request.0, "action": g.window_request.1},
            "exiting": g.exiting,
            "autostart": self.autostart_enabled(),
            "config": g.cfg.to_json(),
            "data_dir": self.data_dir.display().to_string(),
            "data_source": self.data_source,
            "workspace": self.workspace.display().to_string(),
            "game_running": game_running(g, states),
            "pitwall": pitwall_link(states),
            "outbox": outbox,
        }))
    }

    /// `control.state` (contract 3.2: the common fields and the connector's).
    fn state_value(&self, g: &mut Inner, states: &[ServiceState]) -> Json {
        let outbox = outbox_of(states);
        json!({
            "app": self.spec.ui_app_id(), "pid": std::process::id(), "version": self.spec.version,
            "managed": g.managed, "parent_pid": g.parent.as_ref().map(|p| p.pid),
            "window": if g.window_visible { "shown" } else { "hidden" },
            "exiting": g.exiting, "autostart": self.autostart_enabled(),
            "level": status::overall_level(states).as_str(),
            "services": states.iter().map(service_summary).collect::<Vec<_>>(),
            "connector": {"id": self.spec.id, "name": self.spec.name,
                          "version": self.spec.version, "sim": self.spec.sim, "sim_name": self.spec.sim_name},
            "game_running": game_running(g, states),
            "pitwall": pitwall_link(states),
            "outbox": {"pending": outbox["pending"], "oldest_at": outbox["oldest_at"]},
            "start_with_app": crate::SERVICES.iter()
                .filter(|n| g.cfg.starts_with_app(n)).collect::<Vec<_>>(),
        })
    }

    /// Polls the services, drains their notices and publishes the state.
    pub fn refresh(&self) {
        let notices: Vec<(String, String, String)> = {
            let g = lock(&self.inner);
            if g.exiting {
                return;
            }
            drop(g);
            let rx = lock(&self.notices_rx);
            rx.try_iter().collect()
        };
        let busy = {
            let mut g = lock(&self.inner);
            g.manager.poll();
            g.manager
                .watch_resume(&self.data_dir.join(crate::resume_watch::FILE));
            g.manager.busy()
        };
        if busy && self.idle.load(Ordering::SeqCst) {
            self.idle.store(false, Ordering::SeqCst);
            self.wake_ticker();
        }
        for (service, title, message) in notices {
            self.notify(&service, &title, &message);
        }
        self.publish();
    }

    /// Publishes `control.state` and `ui.snapshot` (the endpoint pushes only what changed) and
    /// tells the UI when the snapshot changed; true then.
    fn publish(&self) -> bool {
        let Some(ep) = self.endpoint() else {
            return false;
        };
        let mut g = lock(&self.inner);
        let states = g.manager.states();
        let state = self.state_value(&mut g, &states);
        let snap = self.snapshot_of(&mut g, &states);
        drop(g);
        ep.set_state(&state);
        ep.set_snapshot(&snap.0);
        let mut latest = lock(&self.latest);
        if latest.as_ref() == Some(&snap) {
            return false;
        }
        *latest = Some(snap);
        drop(latest);
        self.emit(UiEvent::Changed);
        true
    }

    /// An error notice: always on `control.notices`; a tray balloon only when standalone.
    fn notify(&self, service: &str, title: &str, message: &str) {
        if let Some(ep) = self.endpoint() {
            ep.notice(title, message, "error", &crate::timefmt::iso_utc_now());
        }
        if !lock(&self.inner).managed {
            self.emit(UiEvent::Notice(Notice {
                service: service.into(),
                title: title.into(),
                message: message.into(),
                level: "error".into(),
            }));
        }
    }

    /// The tray menu's switch for a service: `Recorder: Stop` / `Recorder: Start`.
    pub fn tray_action(&self, service: &str, running: bool) -> String {
        let name = status::display_name(&self.env_texts, service);
        self.env_texts.t(
            if running {
                "tray.service.stop"
            } else {
                "tray.service.start"
            },
            &[("service", name.into())],
        )
    }
}

fn service_summary(st: &ServiceState) -> Json {
    let mut out = json!({
        "id": st.service, "name": st.name, "state": st.state, "label": st.full_label,
        "level": st.level.as_str(), "running": st.running,
        "ownership": if st.running { st.ownership.map(|o| o.as_str()) } else { None },
        "forced_stop": st.detail.get("forced_stop").and_then(Json::as_bool).unwrap_or(false),
        "delegable": true, "default": "on",
    });
    if let (true, Some(pid)) = (st.running, st.pid) {
        out["pid"] = json!(pid);
    }
    out
}

fn outbox_of(states: &[ServiceState]) -> Json {
    let ob = states
        .iter()
        .find(|s| s.service == status::RECORDER && s.running)
        .and_then(|s| s.detail.get("outbox"))
        .cloned()
        .unwrap_or(Json::Null);
    json!({
        "pending": ob.get("pending").and_then(Json::as_u64).unwrap_or(0),
        "oldest_at": ob.get("oldest_at").cloned().unwrap_or(Json::Null),
        "rejected": ob.get("rejected").and_then(Json::as_u64).unwrap_or(0),
    })
}

/// What a running recorder says about the game, else the app's own check of the process.
fn game_running(g: &mut Inner, states: &[ServiceState]) -> bool {
    let said: Vec<bool> = states
        .iter()
        .filter(|s| s.running && s.service == status::RECORDER)
        .filter_map(|s| s.detail.get("game_running").and_then(Json::as_bool))
        .collect();
    if said.is_empty() {
        g.manager.game_running()
    } else {
        said.into_iter().any(|b| b)
    }
}

/// `control.state.pitwall`: `connected` when any running service is connected, `refused` when
/// one was refused, else `waiting`.
pub fn pitwall_link(states: &[ServiceState]) -> &'static str {
    let links: Vec<&str> = states
        .iter()
        .filter(|s| s.running)
        .filter_map(|s| s.detail.get("pitwall").and_then(Json::as_str))
        .collect();
    ["connected", "refused", "waiting"]
        .into_iter()
        .find(|x| links.contains(x))
        .unwrap_or("waiting")
}

/// The tray colour word of a level, for the UI.
pub fn level_word(level: Level) -> &'static str {
    level.as_str()
}

impl ControlApp {
    /// The served endpoint (tests; panics when not serving).
    #[doc(hidden)]
    pub fn endpoint_for_tests(&self) -> Arc<ControlEndpoint> {
        self.endpoint().expect("the endpoint is served")
    }
}
