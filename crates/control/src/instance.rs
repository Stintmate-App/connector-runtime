//! Single instance of the control app, and the second launch's call.
//!
//! The lock is a named mutex (`Local\<prefix>.UI`). The first instance serves the
//! connector's endpoint; a second launch asks it to show its window (`control.show`), or,
//! launched managed, to be managed by its parent (`control.managed`), over that endpoint, and
//! exits.

use serde_json::Value as Json;
use std::time::{Duration, Instant};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_ipc::blocking::Client;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::connection::CallOptions;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::KeySource;

/// Converts JSON into an IPC value.
pub fn to_value(j: &Json) -> Value {
    let bytes = rmp_serde::to_vec_named(j).expect("JSON serializes");
    codec::decode(&bytes).expect("rmp-serde writes the subset")
}

/// Converts an IPC value into JSON (`bin` as an array of bytes, `ext` as null).
pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Nil | Value::Ext(..) => Json::Null,
        Value::Boolean(b) => Json::from(*b),
        Value::Integer(i) => i
            .as_i64()
            .map_or_else(|| Json::from(i.as_u64()), Json::from),
        Value::F32(f) => Json::from(f64::from(*f)),
        Value::F64(f) => Json::from(*f),
        Value::String(s) => Json::from(s.as_str().unwrap_or_default()),
        Value::Binary(b) => Json::from(b.clone()),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => Json::Object(
            m.iter()
                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), to_json(v)))
                .collect(),
        ),
    }
}

/// Why [`call_checked`] got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// The running app answered with an error (`code` is the IPC error code, for example
    /// `managed`); it is not retried.
    Remote {
        /// The IPC error code.
        code: String,
        /// The app's explanation.
        message: String,
    },
    /// Nobody answered within the timeout.
    Unreachable,
}

/// From another process (a second launch of the app): one call to the running app's endpoint
/// at `pipe`, retried while it is still starting. The result, or `None` when nobody answered
/// within `timeout`.
pub fn call(
    spec: &ConnectorSpec,
    pipe: &str,
    key: impl Fn() -> KeySource,
    method: &str,
    params: &Json,
    timeout: Duration,
) -> Option<Json> {
    call_checked(spec, pipe, key, method, params, timeout).ok()
}

/// [`call`] that tells an error answer of the running app (not retried) from silence.
pub fn call_checked(
    spec: &ConnectorSpec,
    pipe: &str,
    key: impl Fn() -> KeySource,
    method: &str,
    params: &Json,
    timeout: Duration,
) -> Result<Json, CallFailure> {
    let deadline = Instant::now() + timeout;
    let mut info = PeerInfo::new(spec.endpoint());
    info.name = spec.name.into();
    info.version = spec.version.into();
    info.process = Some("ui".into());
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(client) = Client::new(pipe, info.clone(), key()) {
            client.start();
            let connected = client.wait_connected(left.max(Duration::from_millis(500)));
            let result = connected.ok().map(|_| {
                client.call_with(
                    method,
                    to_value(params),
                    CallOptions {
                        timeout: Some(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .max(Duration::from_secs(1)),
                        ),
                        ..CallOptions::default()
                    },
                )
            });
            client.close();
            match result {
                Some(Ok(r)) => return Ok(to_json(&r)),
                Some(Err(stintmate_ipc::error::Error::Remote(r))) => {
                    return Err(CallFailure::Remote {
                        code: r.code,
                        message: r.message,
                    });
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(CallFailure::Unreachable);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
