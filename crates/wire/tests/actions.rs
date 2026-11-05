//! The standard actions: their codes follow the connector's catalog, and their own schemas hold.

use serde_json::{Value, json};
use stintmate_connector_wire::actions::*;
use stintmate_connector_wire::schema_check::check_schema;

fn by_code<'a>(actions: &'a Value, code: &str) -> &'a Value {
    actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["code"] == code)
        .unwrap()
}

#[test]
fn standard_actions_enum_follows_codes() {
    let a = standard_actions(&["alpha", "beta"]);
    let enumeration = &by_code(&a, TEST_ANNOUNCE)["params"]["properties"]["code"]["enum"];
    assert_eq!(enumeration, &json!(["alpha", "beta"]));
    let other = standard_actions(&["gamma"]);
    let enumeration = &by_code(&other, TEST_ANNOUNCE)["params"]["properties"]["code"]["enum"];
    assert_eq!(enumeration, &json!(["gamma"]));
    assert_eq!(a.as_array().unwrap().len(), 2);
    assert_eq!(by_code(&a, TEST_PING)["code"], "test.ping");
}

#[test]
fn the_key_follows_the_prefix_and_the_actions() {
    let a = standard_actions(&["alpha"]);
    let ik = actions_ik("example", &a);
    assert!(ik.starts_with("example/actions/"));
    assert_eq!(ik.len(), "example/actions/".len() + 12);
    assert_eq!(ik, actions_ik("example", &standard_actions(&["alpha"])));
    assert_ne!(
        ik,
        actions_ik("example", &standard_actions(&["alpha", "beta"]))
    );
    assert_ne!(ik, actions_ik("other", &a));
}

#[test]
fn every_example_fits_its_params_schema() {
    let a = standard_actions(&["engineer_test"]);
    for action in a.as_array().unwrap() {
        let schema = &action["params"];
        let r = check_schema(&[], schema, schema, &action["example"]);
        assert_eq!(r, Ok(()), "{}", action["code"]);
        assert!(action["timeout_s"].as_f64().unwrap() > 0.0);
    }
    let schema = &by_code(&a, TEST_ANNOUNCE)["params"];
    assert!(check_schema(&[], schema, schema, &json!({"code": "nope"})).is_err());
    assert!(check_schema(&[], schema, schema, &json!({})).is_err());
}
