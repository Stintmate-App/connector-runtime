//! The announcer and the per-connection declarations over a fake Pitwall.

mod common;

use common::{Rig, wait_for};
use serde_json::{Value as Json, json};
use std::sync::Arc;
use std::time::Duration;
use stintmate_connector_link::announcer::{Announcement, Announcer, declare_connection};
use stintmate_connector_testing::{example_catalog, to_json};
use stintmate_connector_wire::actions::standard_actions;

fn announcer(rig: &Rig) -> Arc<Announcer> {
    Arc::new(Announcer::new(
        rig.transport(),
        example_catalog(),
        "example",
        || Some("session-1".to_owned()),
        || 0.0,
    ))
}

fn announcement(code: &'static str, variant: u8, params: Json) -> Announcement {
    let Json::Object(params) = params else {
        panic!("params are an object")
    };
    Announcement {
        code,
        variant,
        params,
        data: None,
    }
}

fn red() -> Announcement {
    announcement("flag_red", 0, json!({}))
}

fn declared(rig: &Rig, a: &Announcer) {
    let codes = example_catalog().codes();
    declare_connection(
        rig.link.as_ref(),
        a,
        &json!([{"id": "g1"}]),
        &standard_actions(&codes),
    );
    assert!(a.declared());
}

#[test]
fn the_declarations_use_keys_with_the_connector_id() {
    let rig = Rig::new();
    let a = announcer(&rig);
    assert!(!a.declared());
    declared(&rig, &a);
    let calls = rig.fake.calls();
    let order: Vec<&str> = calls.iter().map(|c| c.method.as_str()).collect();
    assert_eq!(
        order,
        [
            "pitwall.catalog.declare",
            "pitwall.guidance.declare",
            "pitwall.actions.declare"
        ]
    );
    let ik = |i: usize| calls[i].ik.clone().unwrap();
    assert!(ik(0).starts_with("example/textcatalog/"), "{}", ik(0));
    assert!(ik(1).starts_with("example/guidance/"), "{}", ik(1));
    assert!(ik(2).starts_with("example/actions/"), "{}", ik(2));
    assert_eq!(ik(0), example_catalog().ik("example"));
    let body = to_json(&calls[0].params);
    assert_eq!(body["catalog"]["catalog"], "connector.example");
    a.lost();
    assert!(!a.declared());
}

#[test]
fn an_announcement_carries_the_session_key_and_the_variant_only_when_set() {
    let rig = Rig::new();
    let a = announcer(&rig);
    // Nothing is announced before the catalog was declared.
    assert!(!a.announce(&red()));
    assert_eq!(a.stats().dropped, 1);
    declared(&rig, &a);
    assert!(a.announce(&announcement("flag_blue", 0, json!({"driver": "Ann"}))));
    assert!(a.announce(&announcement("flag_blue", 2, json!({"driver": "Ann"}))));
    assert!(rig.fake.wait_for(
        |c| c.iter().filter(|x| x.method == "pitwall.announce").count() == 2,
        10
    ));
    let sent: Vec<serde_json::Value> = rig
        .fake
        .calls()
        .iter()
        .filter(|c| c.method == "pitwall.announce")
        .map(|c| to_json(&c.params))
        .collect();
    let plain = sent.iter().find(|p| p.get("variant").is_none()).unwrap();
    assert_eq!(plain["code"], "flag_blue");
    assert_eq!(plain["session_key"], "session-1");
    assert_eq!(plain["params"]["driver"], "Ann");
    assert!(sent.iter().any(|p| p["variant"] == 2));
    assert_eq!(a.stats().sent, 2);
}

#[test]
fn a_variant_beyond_the_catalog_and_bad_params_are_not_sent() {
    let rig = Rig::new();
    let a = announcer(&rig);
    declared(&rig, &a);
    assert!(!a.announce(&announcement("flag_blue", 3, json!({"driver": "Ann"}))));
    assert!(!a.announce(&announcement("flag_blue", 0, json!({}))));
    assert_eq!(a.stats().dropped, 2);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.fake.count("pitwall.announce"), 0);
}

#[test]
fn ten_announcements_a_minute_then_they_are_dropped() {
    let rig = Rig::new();
    let a = announcer(&rig);
    declared(&rig, &a);
    let sent = (0..11).filter(|_| a.announce(&red())).count();
    assert_eq!(sent, 10);
    assert_eq!(a.stats().dropped, 1);
    assert!(rig.fake.wait_for(
        |c| c.iter().filter(|x| x.method == "pitwall.announce").count() == 10,
        10
    ));
}

#[test]
fn announce_wait_returns_the_accept_and_pitwalls_refusal_is_counted() {
    let rig = Rig::new();
    let a = announcer(&rig);
    declared(&rig, &a);
    let ok = a
        .announce_wait(&red(), Duration::from_secs(5))
        .expect("accepted");
    assert_eq!(to_json(&ok)["message_id"], "m1");
    rig.fake.rules.lock().unwrap().refuse_next_with = Some("rate_limited".into());
    let err = a.announce_wait(&red(), Duration::from_secs(5)).unwrap_err();
    assert!(err.contains("rate_limited"), "{err}");
    assert_eq!(a.stats().refused, 1);
    // A refused async announcement is counted too.
    rig.fake.rules.lock().unwrap().refuse_next_with = Some("rate_limited".into());
    assert!(a.announce(&red()));
    assert!(wait_for(5, || a.stats().refused == 2));
}
