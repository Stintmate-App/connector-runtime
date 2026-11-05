//! Shared helpers of the link tests.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use stintmate_connector_link::link::{PitwallLink, Transport, key_source, peer_info};
use stintmate_connector_testing::{
    EXAMPLE_SPEC, FakePitwall, TempWorkspace, pitwall_pipe, unique_user,
};

/// A fake Pitwall, a workspace with its key and a started link to it.
pub struct Rig {
    pub fake: FakePitwall,
    pub ws: TempWorkspace,
    pub link: Arc<PitwallLink>,
}

impl Rig {
    pub fn new() -> Self {
        let ws = TempWorkspace::new();
        let pipe = pitwall_pipe(&unique_user());
        let fake = FakePitwall::start(&pipe);
        let link = Arc::new(
            PitwallLink::new(
                &pipe,
                peer_info(&EXAMPLE_SPEC, "recorder"),
                key_source(ws.path()).unwrap(),
            )
            .unwrap(),
        );
        link.start();
        assert!(link.wait_connected(Duration::from_secs(10)), "no hello");
        Self { fake, ws, link }
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        self.link.clone()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.link.close();
        self.fake.stop();
    }
}

/// Polls `f` every 20 ms for at most `secs` seconds.
pub fn wait_for(secs: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}
