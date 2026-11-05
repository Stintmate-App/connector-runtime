//! The links against a fake Pitwall: the hello, the states and the waits after a refusal.

mod common;

use common::{Rig, wait_for};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use stintmate_connector_link::link::{LinkState, PitwallLink, Transport, key_source, peer_info};
use stintmate_connector_testing::{
    EXAMPLE_SPEC, FakePitwall, TempWorkspace, pitwall_pipe, to_json, unique_user,
};
use stintmate_ipc::codec;

#[test]
fn the_hello_carries_the_spec_and_a_call_is_answered() {
    let rig = Rig::new();
    assert_eq!(rig.link.state().0, LinkState::Connected);
    assert_eq!(rig.link.generation(), 1);
    let params = codec::map([("catalog", codec::map::<[(&str, codec::Value); 0], &str>([]))]);
    rig.link
        .call(
            "pitwall.catalog.declare",
            params,
            Some("example/test/1"),
            Duration::from_secs(5),
        )
        .unwrap();
    let calls = rig.fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].ik.as_deref(), Some("example/test/1"));
    let peers = rig.fake.peers.lock().unwrap();
    let info = &peers[0];
    assert_eq!(info.app, "connector-example");
    assert_eq!(info.name, "Example connector");
    assert_eq!(info.process.as_deref(), Some("recorder"));
    let meta = to_json(info.meta.as_ref().unwrap());
    assert_eq!(meta["connector_id"], "example");
    assert_eq!(meta["product"], EXAMPLE_SPEC.product);
    assert_eq!(meta["service"], "recorder");
}

#[test]
fn the_link_waits_for_pitwall_and_connects_when_it_appears() {
    let ws = TempWorkspace::new();
    let pipe = pitwall_pipe(&unique_user());
    let link = Arc::new(
        PitwallLink::new(
            &pipe,
            peer_info(&EXAMPLE_SPEC, "recorder"),
            key_source(ws.path()).unwrap(),
        )
        .unwrap(),
    );
    let changes = Arc::new(AtomicU32::new(0));
    let c = changes.clone();
    link.on_state(move || {
        c.fetch_add(1, Ordering::Relaxed);
    });
    link.start();
    assert!(wait_for(5, || {
        link.state() == (LinkState::Waiting, Some("Pitwall is not running".into()))
    }));
    assert!(!link.connected());
    let fake = FakePitwall::start(&pipe);
    assert!(link.wait_connected(Duration::from_secs(15)));
    assert!(wait_for(5, || link.state().0 == LinkState::Connected));
    assert!(changes.load(Ordering::Relaxed) >= 1);
    // Pitwall goes away: the link says so and comes back with a new generation.
    let first = link.generation();
    fake.stop();
    assert!(wait_for(10, || link.state().0 == LinkState::Waiting));
    let again = FakePitwall::start(&pipe);
    assert!(wait_for(20, || link.generation() > first));
    again.stop();
    link.close();
}

#[test]
fn a_wrong_key_is_a_refusal_and_the_state_says_so() {
    let ws = TempWorkspace::with_key(&[9u8; 32]);
    let pipe = pitwall_pipe(&unique_user());
    let fake = FakePitwall::start(&pipe); // the key of the tests, not [9; 32]
    let link = PitwallLink::new(
        &pipe,
        peer_info(&EXAMPLE_SPEC, "recorder"),
        key_source(ws.path()).unwrap(),
    )
    .unwrap();
    link.start();
    assert!(wait_for(15, || link.state().0 == LinkState::Refused));
    assert!(!link.connected());
    link.close();
    fake.stop();
}
