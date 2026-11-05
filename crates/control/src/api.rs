//! The UI's API: commands in, snapshots and events out.
//!
//! The tray lives in the control app process and talks to [`crate::app::ControlApp`] directly;
//! the window is a separate, short-lived process (`<exe prefix>-connector-ui.exe --window`) that
//! reaches the same API over the control endpoint: it subscribes to the state channel
//! `ui.snapshot` ([`Snapshot::to_json`]) and calls the method `ui.command` with a
//! [`Command::to_json`]. Both ends therefore use exactly these types, and neither needs the
//! other's GUI libraries.

use serde_json::{Value, json};
use std::path::PathBuf;

/// The state channel carrying [`Snapshot::to_json`].
pub const SNAPSHOT_CHANNEL: &str = "ui.snapshot";
/// The method taking a [`Command`].
pub const COMMAND_METHOD: &str = "ui.command";

/// Changes of the options (`None` = leave as is).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptionsChange {
    /// Start with the window hidden.
    pub start_minimized: Option<bool>,
    /// The recorder's rate in Hz.
    pub recorder_hz: Option<f64>,
    /// The log the Logs pane opens on.
    pub logs_default: Option<String>,
}

/// What the tray and the window ask the control app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The user's own switch of a service (remembered as its start-with-the-app setting;
    /// published as `control.services.desired` while managed).
    Switch {
        /// Service id.
        service: String,
        /// On or off.
        on: bool,
    },
    /// Exit: stop the services cleanly and quit.
    Exit {
        /// Where the exit came from, for the log.
        origin: String,
    },
    /// Ask the UI to open the data folder (answered with [`UiEvent::OpenFolder`]).
    OpenDataFolder,
    /// Switch "start with Windows".
    SetAutostart(bool),
    /// Change the options.
    SetOptions(OptionsChange),
    /// The window opened or closed (reported by the window).
    WindowVisible(bool),
    /// Ask for the window (the tray's Open).
    ShowWindow,
    /// Ask for the window on the Logs pane (the tray's Logs).
    ShowLogs,
}

impl Command {
    /// The wire form (`{command, ...}`).
    pub fn to_json(&self) -> Value {
        match self {
            Command::Switch { service, on } => {
                json!({"command": "switch", "service": service, "on": on})
            }
            Command::Exit { origin } => json!({"command": "exit", "origin": origin}),
            Command::OpenDataFolder => json!({"command": "open_data_folder"}),
            Command::SetAutostart(v) => json!({"command": "set_autostart", "enabled": v}),
            Command::SetOptions(o) => json!({
                "command": "set_options",
                "start_minimized": o.start_minimized,
                "recorder_hz": o.recorder_hz,
                "logs_default": o.logs_default,
            }),
            Command::WindowVisible(v) => json!({"command": "window_visible", "visible": v}),
            Command::ShowWindow => json!({"command": "show_window"}),
            Command::ShowLogs => json!({"command": "show_logs"}),
        }
    }

    /// Reads the wire form; the error is a sentence for `bad_request`.
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let flag = |k: &str| {
            v.get(k)
                .and_then(Value::as_bool)
                .ok_or_else(|| format!("{k} must be a bool"))
        };
        match v.get("command").and_then(Value::as_str) {
            Some("switch") => Ok(Command::Switch {
                service: text("service").ok_or("service must be a string")?,
                on: flag("on")?,
            }),
            Some("exit") => Ok(Command::Exit {
                origin: text("origin").unwrap_or_else(|| "window".into()),
            }),
            Some("open_data_folder") => Ok(Command::OpenDataFolder),
            Some("set_autostart") => Ok(Command::SetAutostart(flag("enabled")?)),
            Some("set_options") => Ok(Command::SetOptions(OptionsChange {
                start_minimized: v.get("start_minimized").and_then(Value::as_bool),
                recorder_hz: v.get("recorder_hz").and_then(Value::as_f64),
                logs_default: text("logs_default"),
            })),
            Some("window_visible") => Ok(Command::WindowVisible(flag("visible")?)),
            Some("show_window") => Ok(Command::ShowWindow),
            Some("show_logs") => Ok(Command::ShowLogs),
            Some(other) => Err(format!("unknown command {other}")),
            None => Err("command must be a string".into()),
        }
    }
}

/// A message for the user (a tray balloon when standalone; always on `control.notices`).
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// The service it is about.
    pub service: String,
    /// Title, in the user's language.
    pub title: String,
    /// Message, in the user's language.
    pub message: String,
    /// `info`, `warn` or `error`.
    pub level: String,
}

/// What the control app tells its UI.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// Bring the window up (start the window process).
    ShowWindow,
    /// Close the window.
    HideWindow,
    /// Bring the window up on the Logs pane.
    ShowLogs,
    /// Open this folder in the file manager.
    OpenFolder(PathBuf),
    /// A balloon (standalone only).
    Notice(Notice),
    /// Managed mode changed: `true` = hide the tray icon, `false` = show it.
    ManagedChanged(bool),
    /// The snapshot changed: redraw.
    Changed,
    /// The control app is done: end the process.
    Quit,
}

/// Everything the tray and the window draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot(pub Value);

impl Snapshot {
    /// The wire form.
    pub fn to_json(&self) -> Value {
        self.0.clone()
    }
    /// Reads the wire form (a window process).
    pub fn from_json(v: Value) -> Self {
        Snapshot(v)
    }
    /// The tray colour word.
    pub fn level(&self) -> &str {
        self.0.get("level").and_then(Value::as_str).unwrap_or("off")
    }
    /// The tray tooltip.
    pub fn tooltip(&self) -> &str {
        self.0.get("tooltip").and_then(Value::as_str).unwrap_or("")
    }
    /// Managed by another app (no tray icon).
    pub fn managed(&self) -> bool {
        self.0
            .get("managed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_round_trips() {
        let all = [
            Command::Switch {
                service: "recorder".into(),
                on: true,
            },
            Command::Exit {
                origin: "tray menu".into(),
            },
            Command::OpenDataFolder,
            Command::SetAutostart(false),
            Command::SetOptions(OptionsChange {
                start_minimized: Some(false),
                recorder_hz: Some(25.0),
                logs_default: Some("ui".into()),
            }),
            Command::WindowVisible(true),
            Command::ShowWindow,
            Command::ShowLogs,
        ];
        for c in all {
            assert_eq!(Command::from_json(&c.to_json()), Ok(c.clone()), "{c:?}");
        }
        assert!(Command::from_json(&json!({"command": "nope"})).is_err());
        assert!(Command::from_json(&json!({})).is_err());
        assert!(Command::from_json(&json!({"command": "switch", "service": "r"})).is_err());
    }
}
