//! The window's link to the control app: subscribes to `ui.snapshot` and calls `ui.command`.

use serde_json::Value as Json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stintmate_connector_control::api::{COMMAND_METHOD, Command, SNAPSHOT_CHANNEL, Snapshot};
use stintmate_connector_control::instance::{to_json, to_value};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_ipc::blocking::{Client, Delivery};
use stintmate_ipc::connection::CallOptions;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::KeySource;

/// What the link has heard.
#[derive(Debug, Default)]
pub struct Shared {
    /// The last snapshot.
    pub snapshot: Option<Snapshot>,
    /// The last command error, for a banner.
    pub error: Option<String>,
}

/// The endpoint client of the window.
pub struct Link {
    client: Arc<Client>,
    shared: Arc<Mutex<Shared>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    _subscription: stintmate_ipc::SubscriptionHandle,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Link {
    /// Connects to the endpoint of `spec` at `pipe` (reconnecting by itself) and subscribes; `wake` is
    /// called whenever something changed, from another thread.
    pub fn connect(
        spec: &ConnectorSpec,
        pipe: &str,
        key: KeySource,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, stintmate_ipc::error::Error> {
        let mut info = PeerInfo::new(spec.endpoint());
        info.name = spec.name.into();
        info.version = spec.version.into();
        info.process = Some("window".into());
        let client = Arc::new(Client::new(pipe, info, key)?);
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let (s, w) = (shared.clone(), wake.clone());
        let subscription = client.subscribe(SNAPSHOT_CHANNEL, Default::default(), move |d| {
            if let Delivery::Event(ev) = d {
                lock(&s).snapshot = Some(Snapshot::from_json(to_json(&ev.data)));
                w();
            }
        });
        client.start();
        Ok(Self {
            client,
            shared,
            wake,
            _subscription: subscription,
        })
    }

    /// The last snapshot, if any arrived.
    pub fn snapshot(&self) -> Option<Snapshot> {
        lock(&self.shared).snapshot.clone()
    }

    /// Whether the endpoint is connected.
    pub fn connected(&self) -> bool {
        self.client.is_connected()
    }

    /// The last command error, once.
    pub fn take_error(&self) -> Option<String> {
        lock(&self.shared).error.take()
    }

    /// Sends a command without blocking the UI.
    pub fn send(&self, command: Command) {
        let (client, shared, wake) = (self.client.clone(), self.shared.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let options = CallOptions {
                timeout: Some(Duration::from_secs(5)),
                ..CallOptions::default()
            };
            if let Err(e) = client.call_with(COMMAND_METHOD, to_value(&command.to_json()), options)
            {
                tracing::warn!("Command {command:?} failed: {e}");
                lock(&shared).error = Some(e.to_string());
                wake();
            }
        });
    }

    /// Sends a command and waits for the answer (the last words of the window).
    pub fn send_and_wait(&self, command: Command, timeout: Duration) -> Result<Json, String> {
        let options = CallOptions {
            timeout: Some(timeout),
            ..CallOptions::default()
        };
        self.client
            .call_with(COMMAND_METHOD, to_value(&command.to_json()), options)
            .map(|v| to_json(&v))
            .map_err(|e| e.to_string())
    }

    /// Stops reconnecting and closes.
    pub fn close(&self) {
        self.client.close();
    }
}
