//! `--stintmate-hold` and `--stintmate-release`: the connector's setup asks the
//! Stintmate hub to stop only this connector and keep it stopped while the files are replaced
//! (`hub.connector.hold`), and to start it again when the setup ends (`hub.connector.release`).
//!
//! A connector's setup tool names the hub's endpoint as a documented interface (contract
//! section 9.1); the connector itself never does. Exit statuses of the hold: 0 held and
//! nothing of the connector runs, 1 held but something still runs after the wait, 5 no
//! Stintmate to ask, 6 Stintmate runs but did not answer, 7 refused because a session is
//! being recorded. The release exits 0, 5 or 6.

use crate::instance::{to_json, to_value};
use serde_json::{Value as Json, json};
use std::time::{Duration, Instant};
use stintmate_connector_platform::single::{self, Names};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_ipc::blocking::Client;
use stintmate_ipc::connection::CallOptions;
use stintmate_ipc::error::Error;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::KeySource;

/// The hub's endpoint id.
pub const HUB_ENDPOINT: &str = "hub";
/// The mutex the running hub holds.
pub const HUB_MUTEX: &str = "Local\\Stintmate.Hub";
/// The hold's time to live, in seconds: the hub starts the connector again after it.
pub const HOLD_TTL_S: u32 = 300;
/// Default `--wait`, in seconds.
pub const DEFAULT_WAIT_S: u64 = 25;
/// Exit: no Stintmate to ask.
pub const EXIT_NO_HUB: i32 = 5;
/// Exit: Stintmate runs but did not answer.
pub const EXIT_NOT_ANSWERING: i32 = 6;
/// Exit: a driving session is being recorded.
pub const EXIT_RECORDING: i32 = 7;

/// Where and how long to ask the hub.
#[derive(Debug, Clone)]
pub struct HubTarget {
    /// The connector that asks.
    pub spec: &'static ConnectorSpec,
    /// The hub endpoint's pipe.
    pub pipe: String,
    /// The hub's mutex (a hub that holds it but has no pipe yet "runs but does not answer").
    pub mutex: String,
    /// How long to wait for the pipe to accept a connection.
    pub connect_wait: Duration,
    /// The deadline of one call.
    pub call_timeout: Duration,
}

impl HubTarget {
    /// The real hub of this user (or the dev-tagged one when `STINTMATE_DEV_INSTANCE` is
    /// set), with the spec's 3 s and 5 s.
    pub fn production(spec: &'static ConnectorSpec) -> Self {
        Self {
            spec,
            pipe: stintmate_connector_platform::dev::pipe_path(spec, HUB_ENDPOINT)
                .unwrap_or_default(),
            mutex: HUB_MUTEX.into(),
            connect_wait: Duration::from_secs(3),
            call_timeout: Duration::from_secs(5),
        }
    }
}

fn say(line: &str) {
    println!("{line}");
}

/// A failed call: the exit status and the line to print.
struct Failed(i32, String);

/// One connection to the hub, kept for the polls of a hold.
struct Hub<'a> {
    client: Client,
    target: &'a HubTarget,
}

fn connect<'a>(target: &'a HubTarget, key: &dyn Fn() -> KeySource) -> Result<Hub<'a>, Failed> {
    let mut info = PeerInfo::new(target.spec.endpoint());
    info.name = target.spec.name.into();
    info.version = target.spec.version.into();
    info.process = Some("ui".into());
    let deadline = Instant::now() + target.connect_wait;
    loop {
        if let Ok(client) = Client::new(&target.pipe, info.clone(), key()) {
            client.start();
            let left = deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(300));
            if client.wait_connected(left).is_ok() {
                return Ok(Hub { client, target });
            }
            client.close();
        }
        if Instant::now() >= deadline {
            return Err(if single::is_running(&target.mutex) {
                Failed(
                    EXIT_NOT_ANSWERING,
                    "Stintmate is running but its hub endpoint does not answer.".into(),
                )
            } else {
                Failed(EXIT_NO_HUB, "Stintmate is not running.".into())
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Hub<'_> {
    fn call(&self, method: &str, params: &Json) -> Result<Json, Failed> {
        let opts = CallOptions {
            timeout: Some(self.target.call_timeout),
            ..CallOptions::default()
        };
        match self.client.call_with(method, to_value(params), opts) {
            Ok(v) => Ok(to_json(&v)),
            Err(Error::Remote(r)) => {
                let details = r.details.as_ref().map(to_json).unwrap_or(Json::Null);
                Err(match r.code.as_str() {
                    "not_found" => Failed(
                        EXIT_NO_HUB,
                        format!("Stintmate does not know the connector: {}", r.message),
                    ),
                    "busy" if details.get("recording").and_then(Json::as_bool) == Some(true) => {
                        Failed(
                            EXIT_RECORDING,
                            "A driving session is being recorded.".into(),
                        )
                    }
                    code => Failed(
                        EXIT_NOT_ANSWERING,
                        format!("Stintmate refused {method} ({code}): {}", r.message),
                    ),
                })
            }
            Err(e) => Err(Failed(
                EXIT_NOT_ANSWERING,
                format!("Stintmate did not answer {method}: {e}"),
            )),
        }
    }
}

impl Drop for Hub<'_> {
    fn drop(&mut self) {
        self.client.close();
    }
}

fn all_free(names: &Names) -> bool {
    !single::is_running(&names.ui)
        && !single::is_running(&names.window)
        && !single::is_running(&names.recorder)
}

/// `--stintmate-hold`: holds the connector at the hub and waits (up to `wait`) until the hub
/// says `held` and this connector's own mutexes are free.
pub fn hold(
    names: &Names,
    target: &HubTarget,
    key: impl Fn() -> KeySource,
    purpose: &str,
    wait: Duration,
) -> i32 {
    let deadline = Instant::now() + wait;
    let hub = match connect(target, &key) {
        Ok(h) => h,
        Err(Failed(code, line)) => {
            say(&line);
            return code;
        }
    };
    let params = json!({"id": target.spec.id, "purpose": purpose, "ttl_s": HOLD_TTL_S});
    loop {
        let reply = match hub.call("hub.connector.hold", &params) {
            Ok(r) => r,
            Err(Failed(code, line)) => {
                say(&line);
                return code;
            }
        };
        if reply.get("state").and_then(Json::as_str) == Some("held") {
            break;
        }
        if Instant::now() >= deadline {
            say(&format!(
                "Stintmate is still stopping the {}.",
                target.spec.name
            ));
            return 1;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    while !all_free(names) {
        if Instant::now() >= deadline {
            say(&format!(
                "The {} is held but something of it still runs.",
                target.spec.name
            ));
            return 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    say(&format!(
        "The {} is held by Stintmate and stopped.",
        target.spec.name
    ));
    0
}

/// `--stintmate-release`: ends the hold; the hub starts the connector again unless `start` is
/// false. 0 released or nothing held, 5 no Stintmate, 6 not answering.
pub fn release(target: &HubTarget, key: impl Fn() -> KeySource, start: bool) -> i32 {
    let result = connect(target, &key).and_then(|hub| {
        hub.call(
            "hub.connector.release",
            &json!({"id": target.spec.id, "start": start}),
        )
    });
    match result {
        Ok(r) => {
            let released = r.get("released").and_then(Json::as_bool).unwrap_or(false);
            say(&if released {
                format!("The {} is released.", target.spec.name)
            } else {
                "Nothing was held.".to_string()
            });
            0
        }
        Err(Failed(code, line)) => {
            say(&line);
            // A hub that does not know the connector has nothing to start: the setup only logs.
            code
        }
    }
}
