//! What the tray menu shows and does, as plain data (no GUI): built from a [`Snapshot`], so it
//! is tested without a tray. `tray` (the Win32 side) only turns it into menu items.

use crate::api::{Command, Snapshot};
use crate::status;
use crate::texts::Texts;

/// An item of the tray menu the user can click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayItem {
    /// Open the window.
    Open,
    /// Switch a service on or off.
    Service(String),
    /// Open the window on the Logs pane.
    Logs,
    /// Open the data folder.
    DataFolder,
    /// Exit.
    Exit,
}

impl TrayItem {
    /// A stable id string (the menu item's id).
    pub fn id(&self) -> String {
        match self {
            TrayItem::Open => "open".into(),
            TrayItem::Service(s) => format!("service:{s}"),
            TrayItem::Logs => "logs".into(),
            TrayItem::DataFolder => "data".into(),
            TrayItem::Exit => "exit".into(),
        }
    }

    /// The item of an id string.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "open" => Some(TrayItem::Open),
            "logs" => Some(TrayItem::Logs),
            "data" => Some(TrayItem::DataFolder),
            "exit" => Some(TrayItem::Exit),
            other => other
                .strip_prefix("service:")
                .map(|s| TrayItem::Service(s.to_string())),
        }
    }
}

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A separator.
    Separator,
    /// A clickable item.
    Item {
        /// Which.
        item: TrayItem,
        /// Its text.
        label: String,
        /// Whether it can be clicked.
        enabled: bool,
    },
}

fn exiting(snapshot: &Snapshot) -> bool {
    snapshot.0.get("exiting").and_then(|v| v.as_bool()) == Some(true)
}

fn service_of<'a>(snapshot: &'a Snapshot, id: &str) -> Option<&'a serde_json::Value> {
    snapshot
        .0
        .get("services")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|s| s["id"] == id))
}

/// The menu for `snapshot`, in the order of the tray menu: Open, the services' switches, Logs and
/// Open data folder, Exit. While exiting everything is disabled.
pub fn menu(snapshot: &Snapshot, texts: &Texts) -> Vec<Entry> {
    let exiting = exiting(snapshot);
    let item = |item: TrayItem, label: String| Entry::Item {
        item,
        label,
        enabled: !exiting,
    };
    let mut out = vec![item(TrayItem::Open, texts.s("tray.open")), Entry::Separator];
    for service in crate::SERVICES {
        let state = service_of(snapshot, service);
        let running = state.and_then(|s| s["running"].as_bool()).unwrap_or(false);
        let stopping = state.is_some_and(|s| s["state"] == "stopping");
        let key = if running {
            "tray.service.stop"
        } else {
            "tray.service.start"
        };
        let name = status::display_name(texts, service);
        out.push(Entry::Item {
            item: TrayItem::Service(service.into()),
            label: texts.t(key, &[("service", name.into())]),
            enabled: !exiting && !stopping,
        });
    }
    out.push(Entry::Separator);
    out.push(item(TrayItem::Logs, texts.s("tray.logs")));
    out.push(item(
        TrayItem::DataFolder,
        texts.s("services.open_data_folder"),
    ));
    out.push(Entry::Separator);
    out.push(item(TrayItem::Exit, texts.s("tray.exit")));
    out
}

/// The tray tooltip: the services' lines, or "stopping" while exiting.
pub fn tooltip(snapshot: &Snapshot, texts: &Texts) -> String {
    if exiting(snapshot) {
        texts.s("tray.stopping")
    } else if snapshot.tooltip().is_empty() {
        texts.s("app.name")
    } else {
        snapshot.tooltip().to_string()
    }
}

/// The command a click on `item` sends to the control app.
pub fn command(item: &TrayItem, snapshot: &Snapshot) -> Command {
    match item {
        TrayItem::Open => Command::ShowWindow,
        TrayItem::Logs => Command::ShowLogs,
        TrayItem::DataFolder => Command::OpenDataFolder,
        TrayItem::Exit => Command::Exit {
            origin: "tray menu".into(),
        },
        TrayItem::Service(service) => {
            let running = service_of(snapshot, service)
                .and_then(|s| s["running"].as_bool())
                .unwrap_or(false);
            Command::Switch {
                service: service.clone(),
                on: !running,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap(running: bool, exiting: bool) -> Snapshot {
        Snapshot(json!({
            "services": [{"id": "recorder", "running": running,
                          "state": if running {"recording"} else {"off"}}],
            "level": "ok", "tooltip": "Recorder: Off", "exiting": exiting,
        }))
    }

    fn labels(m: &[Entry]) -> Vec<String> {
        m.iter()
            .map(|e| match e {
                Entry::Separator => "-".into(),
                Entry::Item { label, .. } => label.clone(),
            })
            .collect()
    }

    #[test]
    fn the_menu_has_the_documented_order_and_the_switch_follows_the_state() {
        let t = Texts::english(&crate::testspec::SPEC);
        assert_eq!(
            labels(&menu(&snap(false, false), &t)),
            [
                "Open Example connector",
                "-",
                "Recorder: Start",
                "-",
                "Open logs",
                "Open data folder",
                "-",
                "Exit Example connector"
            ]
        );
        assert!(labels(&menu(&snap(true, false), &t)).contains(&"Recorder: Stop".to_string()));
    }

    #[test]
    fn exiting_disables_every_item_and_a_stopping_service_its_switch() {
        let t = Texts::english(&crate::testspec::SPEC);
        let m = menu(&snap(true, true), &t);
        assert!(m.iter().all(|e| match e {
            Entry::Item { enabled, .. } => !enabled,
            Entry::Separator => true,
        }));
        assert_eq!(tooltip(&snap(true, true), &t), t.s("tray.stopping"));
        let stopping = Snapshot(json!({"services": [{"id": "recorder", "running": true,
                                                       "state": "stopping"}]}));
        assert!(menu(&stopping, &t).iter().any(|e| matches!(
            e,
            Entry::Item {
                item: TrayItem::Service(_),
                enabled: false,
                ..
            }
        )));
    }

    #[test]
    fn clicks_map_to_commands() {
        let (off, on) = (snap(false, false), snap(true, false));
        assert_eq!(command(&TrayItem::Open, &off), Command::ShowWindow);
        assert_eq!(command(&TrayItem::Logs, &off), Command::ShowLogs);
        assert_eq!(
            command(&TrayItem::DataFolder, &off),
            Command::OpenDataFolder
        );
        assert_eq!(
            command(&TrayItem::Exit, &off),
            Command::Exit {
                origin: "tray menu".into()
            }
        );
        let switch = |on| Command::Switch {
            service: "recorder".into(),
            on,
        };
        let svc = TrayItem::Service("recorder".into());
        assert_eq!(command(&svc, &off), switch(true));
        assert_eq!(command(&svc, &on), switch(false));
    }

    #[test]
    fn item_ids_round_trip() {
        for i in [
            TrayItem::Open,
            TrayItem::Logs,
            TrayItem::DataFolder,
            TrayItem::Exit,
            TrayItem::Service("recorder".into()),
        ] {
            assert_eq!(TrayItem::from_id(&i.id()), Some(i));
        }
        assert_eq!(TrayItem::from_id("zzz"), None);
    }
}
