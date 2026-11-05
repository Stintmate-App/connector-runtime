//! A service's `service.status` (plus its single-instance mutex) to the state shown for it.
//!
//! Pure functions over the JSON the service set (the `connector.status` fields plus
//! `{service, pid, started_at}`), or `None` while it has no connection to the app. The
//! connection is the liveness (the protocol's heartbeat closes a hung one).

use crate::texts::Texts;
use serde_json::{Map, Value};
use std::time::SystemTime;

/// The recorder's service id.
pub const RECORDER: &str = "recorder";

/// Levels drive the tray icon colour: off = grey, ok = green, warn = amber, error = red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    /// Not running.
    #[default]
    Off,
    /// Fine.
    Ok,
    /// Needs a look.
    Warn,
    /// Broken.
    Error,
}

impl Level {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// Who holds a running service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// Started by the app and tied to it (job object).
    App,
    /// Started outside the app (a `.bat`), tied to it afterwards.
    Adopted,
    /// Started outside the app; Windows refused to tie it.
    External,
    /// Started by the app, but Windows refused to tie it.
    Untied,
}

impl Ownership {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Ownership::App => "app",
            Ownership::Adopted => "adopted",
            Ownership::External => "external",
            Ownership::Untied => "untied",
        }
    }
    fn text_key(self) -> Option<&'static str> {
        match self {
            Ownership::App => None,
            Ownership::Adopted => Some("status.ownership.adopted"),
            Ownership::External => Some("status.ownership.external"),
            Ownership::Untied => Some("status.ownership.untied"),
        }
    }
}

/// The state shown for one service.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceState {
    /// The service id.
    pub service: String,
    /// Machine id, e.g. `recording`, `off`.
    pub state: String,
    /// One line for the UI, in the user's language.
    pub label: String,
    /// The tray level.
    pub level: Level,
    /// The process runs.
    pub running: bool,
    /// How a running service is held.
    pub ownership: Option<Ownership>,
    /// The service's PID (from the connection).
    pub pid: Option<u32>,
    /// The service's status fields, plus `forced_stop`.
    pub detail: Map<String, Value>,
    /// The service's display name.
    pub name: String,
    /// `label` plus how an instance not started by the app is held.
    pub full_label: String,
}

impl ServiceState {
    fn fill_full_label(&mut self, texts: &Texts) {
        let own = self
            .ownership
            .filter(|_| self.running)
            .and_then(Ownership::text_key)
            .map(|k| texts.s(k));
        self.full_label = match own {
            Some(o) => format!("{} · {o}", self.label),
            None => self.label.clone(),
        };
    }

    /// Sets the state fields together and refreshes the full label.
    pub fn set(&mut self, texts: &Texts, state: &str, label: String, level: Level) {
        self.state = state.into();
        self.label = label;
        self.level = level;
        self.fill_full_label(texts);
    }

    /// Records how the running service is held and refreshes the full label.
    pub fn set_ownership(&mut self, texts: &Texts, ownership: Option<Ownership>) {
        self.ownership = ownership;
        self.fill_full_label(texts);
    }
}

/// The display name of a service.
pub fn display_name(texts: &Texts, service: &str) -> String {
    match service {
        RECORDER => texts.s("service.recorder"),
        other => other.to_string(),
    }
}

/// `started_at` (ISO 8601 with offset) as a time, or `None` if unknown.
pub fn started_time(status: Option<&Value>) -> Option<SystemTime> {
    let text = status?.get("started_at")?.as_str()?;
    crate::timefmt::parse_iso(text)
}

fn pid_of(status: Option<&Value>) -> Option<u32> {
    status?
        .get("pid")?
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0)
}

fn pitwall_text(texts: &Texts, status: &Value) -> Option<String> {
    let key = match status.get("pitwall")?.as_str()? {
        "connected" => "status.pitwall.connected",
        "waiting" => "status.pitwall.absent",
        "refused" => "status.pitwall.refused",
        _ => return None,
    };
    Some(texts.s(key))
}

fn flag(status: &Value, key: &str) -> bool {
    status.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The recorder's state. `running`: its mutex is held (or the app's child is alive).
pub fn recorder_state(texts: &Texts, status: Option<&Value>, running: bool) -> ServiceState {
    let mut st = ServiceState {
        service: RECORDER.into(),
        state: String::new(),
        label: String::new(),
        level: Level::Off,
        running,
        ownership: None,
        pid: pid_of(status),
        detail: status
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        name: display_name(texts, RECORDER),
        full_label: String::new(),
    };
    let (state, label, level) = recorder_label(texts, status, running);
    st.set(texts, state, label, level);
    st
}

fn recorder_label(
    texts: &Texts,
    status: Option<&Value>,
    running: bool,
) -> (&'static str, String, Level) {
    if !running {
        return ("off", texts.s("status.off"), Level::Off);
    }
    let Some(status) = status else {
        return ("starting", texts.s("status.starting"), Level::Ok);
    };
    if status.get("error_code").and_then(Value::as_str) == Some("unsupported_sim_version") {
        return ("shm_error", texts.s("status.shm_error"), Level::Warn);
    }
    let link = pitwall_text(texts, status);
    let linked = status.get("pitwall").and_then(Value::as_str) == Some("connected");
    let level = if linked { Level::Ok } else { Level::Warn };
    let with_link = |text: String| match &link {
        Some(l) => format!("{text} · {l}"),
        None => text,
    };
    if !flag(status, "game_running") || !flag(status, "mapped") {
        return ("waiting", with_link(texts.s("status.game_waiting")), level);
    }
    let garage =
        flag(status, "in_garage") || status.get("phase").and_then(Value::as_str) == Some("garage");
    if !flag(status, "session_open") {
        // A recorder started with the car already in the garage has no session yet.
        if garage {
            return ("garage", with_link(texts.s("status.garage")), level);
        }
        return ("connected", with_link(texts.s("status.menus")), level);
    }
    if !linked {
        // Recording into the outbox: the laps are delivered when Pitwall runs.
        let key = if garage {
            "status.recorder.garage_offline"
        } else {
            "status.recorder.driving_offline"
        };
        return ("not_linked", texts.s(key), Level::Warn);
    }
    let mut parts = vec![texts.s(if garage {
        "status.garage"
    } else {
        "status.recorder.recording"
    })];
    let num = |k: &str| status.get(k).filter(|v| !v.is_null()).map(num_text);
    if let Some(n) = num("session_id") {
        parts.push(texts.t("status.recorder.session", &[("session", n.into())]));
    }
    if let Some(n) = num("stint") {
        parts.push(texts.t("status.recorder.stint", &[("stint", n.into())]));
    }
    if let Some(n) = num("lap") {
        parts.push(texts.t("status.recorder.lap", &[("lap", n.into())]));
    }
    let l = link.unwrap_or_default();
    (
        if garage { "garage" } else { "recording" },
        format!("{} · {l}", parts.join(" · ")),
        Level::Ok,
    )
}

fn num_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The Recorder is off (not crashed, not stopping) while the game runs: say so loudly (amber);
/// laps driven now are lost.
pub fn recorder_off_while_game_runs(texts: &Texts, st: &mut ServiceState) {
    if st.service == RECORDER && st.state == "off" && !st.running {
        st.set(
            texts,
            "off_game_running",
            texts.s("status.recorder.off_game_running"),
            Level::Warn,
        );
    }
}

/// The tray colour for a set of services: the worst level among them.
pub fn overall_level(states: &[ServiceState]) -> Level {
    states.iter().map(|s| s.level).max().unwrap_or(Level::Off)
}

/// One line per service, e.g. `Recorder: Recording · session 14`.
pub fn tooltip(texts: &Texts, states: &[ServiceState]) -> String {
    states
        .iter()
        .map(|s| {
            texts.t(
                "status.tooltip_line",
                &[
                    ("service", s.name.as_str().into()),
                    ("state", s.full_label.as_str().into()),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text for a recorder that is not installed.
pub fn not_installed_label(texts: &Texts) -> String {
    texts.s("status.not_installed")
}
