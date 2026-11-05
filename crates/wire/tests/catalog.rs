//! The announcement catalog format: a small neutral catalog is valid, params are checked as
//! Pitwall checks them, and the canonical JSON and hashes match the vectors.

use serde_json::{Value, json};
use stintmate_connector_wire::catalog::*;

const EXAMPLE: &str = include_str!("fixtures/catalog.example.json");
const VECTORS: &str = include_str!("fixtures/source_hash.vectors.json");
const ID: &str = "connector.example";

fn cat() -> AnnounceCatalog {
    AnnounceCatalog::parse(EXAMPLE, ID).expect("the example catalog is valid")
}

fn doc() -> Value {
    serde_json::from_str(EXAMPLE).unwrap()
}

#[test]
fn the_example_catalog_is_valid() {
    assert_eq!(validate_catalog(&doc(), ID), Vec::<String>::new());
    let c = cat();
    assert_eq!(c.doc()["schema"], SCHEMA);
    assert_eq!(c.doc()["catalog"], ID);
    assert_eq!(c.codes().len(), 9);
}

#[test]
fn parse_reports_the_problems_and_the_wrong_id() {
    let err = AnnounceCatalog::parse(EXAMPLE, "connector.other").unwrap_err();
    assert_eq!(err, vec!["catalog must be connector.other".to_string()]);
    let err = AnnounceCatalog::parse("{not json", ID).unwrap_err();
    assert!(err[0].starts_with("not JSON"), "{err:?}");
    let err = AnnounceCatalog::parse(r#"{"schema":"stintmate-catalog/1","catalog":"connector.example","language":"en","messages":{}}"#, ID)
        .unwrap_err();
    assert_eq!(err, vec!["messages must be a non-empty map".to_string()]);
}

#[test]
fn codes_are_sorted_and_spotters_found() {
    let c = cat();
    let codes = c.codes();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    assert_eq!(codes, sorted);
    assert!(c.is_spotter("spotter_still_right"));
    assert!(!c.is_spotter("flag_red"));
    assert!(!c.is_spotter("nope"));
}

#[test]
fn variant_counts_include_the_text() {
    let c = cat();
    assert_eq!(c.variant_count("spotter_still_right"), 2);
    assert_eq!(c.variant_count("flag_blue"), 3);
    assert_eq!(c.variant_count("flag_red"), 1);
    assert_eq!(c.variant_count("nope"), 1);
}

#[test]
fn every_code_has_a_validation_status() {
    let c = cat();
    for code in c.codes() {
        assert!(c.validation(code, &[]).is_some(), "{code}");
    }
    let desk = ["radio_check", "engineer_test"];
    assert_eq!(c.validation("flag_yellow", &desk), Some(NEEDS_RACE));
    assert_eq!(c.validation("radio_check", &desk), Some(DESK_TESTED));
    assert_eq!(c.validation("engineer_test", &desk), Some(DESK_TESTED));
    assert_eq!(c.validation("nope", &desk), None);
    assert_eq!(c.validation("nope", &["nope"]), None);
}

#[test]
fn the_key_follows_the_prefix_and_the_catalog() {
    let c = cat();
    let ik = c.ik("example");
    let hash = sha12(&canonical(c.doc()));
    assert_eq!(ik, format!("example/textcatalog/{hash}"));
    assert_eq!(hash.len(), 12);
    let mut other = doc();
    other["messages"]["flag_red"]["text"] = json!("Red flag.");
    let other = AnnounceCatalog::parse(&other.to_string(), ID).unwrap();
    assert_ne!(other.ik("example"), ik);
    // The key does not depend on the order of the keys in the file.
    let reordered: Value =
        serde_json::from_str(&serde_json::to_string(&sorted_copy(&doc())).unwrap()).unwrap();
    let again = AnnounceCatalog::parse(&reordered.to_string(), ID).unwrap();
    assert_eq!(again.ik("example"), ik);
}

fn sorted_copy(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort_by(|a, b| b.cmp(a)); // reverse
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted_copy(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted_copy).collect()),
        o => o.clone(),
    }
}

#[test]
fn canonical_json_sorts_keys_and_keeps_text() {
    let v = json!({"b": [1, {"y": 1, "x": "é"}], "a": null});
    assert_eq!(canonical(&v), r#"{"a":null,"b":[1,{"x":"é","y":1}]}"#);
}

#[test]
fn source_hashes_match_the_vectors() {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    for case in v["cases"].as_array().unwrap() {
        assert_eq!(
            source_hash(&case["message"]),
            case["hash"].as_str().unwrap(),
            "{}",
            case["message"]["text"]
        );
    }
}

#[test]
fn source_hashes_follow_the_text_and_not_the_connectors_fields() {
    let base = doc()["messages"]["flag_yellow"].clone();
    let h0 = source_hash(&base);
    assert!(
        h0.len() == 12
            && h0
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let mut changed = base.clone();
    changed["text"] = json!("Yellow, sector {sector}.");
    assert_ne!(source_hash(&changed), h0);
    let mut other = base.clone();
    other["max_wait_s"] = json!(99);
    other["note"] = json!("x");
    assert_eq!(source_hash(&other), h0);
    // Empty params hash as no params.
    let mut empty = doc()["messages"]["flag_red"].clone();
    let h = source_hash(&empty);
    empty["params"] = json!({});
    assert_eq!(source_hash(&empty), h);
}

#[test]
fn source_hash_matches_the_contract_example() {
    // catalogs.md section 8: canonical JSON of {"params", "text"}, sorted, no spaces.
    let msg = json!({"text": "Combustible para {laps}.", "params": {"laps": "laps"}});
    let raw = r#"{"params":{"laps":"laps"},"text":"Combustible para {laps}."}"#;
    assert_eq!(canonical(&msg), raw);
    assert_eq!(source_hash(&msg), sha12(raw));
}

#[test]
fn params_that_fit() {
    let c = cat();
    for (code, params) in [
        ("flag_yellow", json!({"sector": 2})),
        ("flag_yellow_cause", json!({"cause": "vehicle_crash"})),
        ("flag_blue", json!({"driver": "Paris"})),
        ("flag_red", json!({})),
        ("chequered", json!({"position": 3})),
        ("gap_ahead", json!({"gap": 1.5, "fuel": 12})),
        ("radio_check", json!({})),
        ("engineer_test", json!({"text": "hello"})),
    ] {
        assert_eq!(check_params(code, &params, c.messages()), None, "{code}");
    }
}

#[test]
fn params_refused_name_the_param() {
    let c = cat();
    for (code, params, word) in [
        ("nope", json!({}), "unknown code"),
        ("flag_yellow", json!({}), "missing"),
        ("flag_yellow", json!({"sector": 2, "x": 1}), "not declared"),
        ("flag_yellow", json!({"sector": "2"}), "integer"),
        ("flag_yellow", json!({"sector": true}), "integer"),
        ("flag_yellow", json!({"sector": 2.0}), "integer"),
        ("flag_blue", json!({"driver": 3}), "text"),
        ("chequered", json!({"position": 0}), ">= 1"),
        ("gap_ahead", json!({"gap": "x", "fuel": 1}), "number"),
        (
            "flag_yellow_cause",
            json!({"cause": "meteor"}),
            "not one of",
        ),
        ("radio_check", json!([]), "map"),
    ] {
        let reason = check_params(code, &params, c.messages());
        assert!(
            reason.as_deref().is_some_and(|r| r.contains(word)),
            "{code}: {reason:?}"
        );
    }
}

#[test]
fn a_refused_enum_value_is_not_echoed() {
    let c = cat();
    let reason = check_params(
        "flag_yellow_cause",
        &json!({"cause": "the user's own words"}),
        c.messages(),
    )
    .unwrap();
    assert!(reason.contains("not one of"), "{reason}");
    assert!(!reason.contains("own words"), "{reason}");
}

#[test]
fn name_value_pairs_are_typed_by_the_param_kinds() {
    let c = cat();
    let pairs = |p: &[&str]| p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let got = c
        .parse_params("gap_ahead", &pairs(&["gap=1.25", "fuel= 12 "]))
        .unwrap();
    assert_eq!(Value::Object(got), json!({"gap": 1.25, "fuel": 12.0}));
    let got = c
        .parse_params("chequered", &pairs(&["position=3"]))
        .unwrap();
    assert_eq!(Value::Object(got), json!({"position": 3}));
    let got = c
        .parse_params("flag_blue", &pairs(&["driver=A=B"]))
        .unwrap();
    assert_eq!(Value::Object(got), json!({"driver": "A=B"}));
    let err = c
        .parse_params("chequered", &pairs(&["position=x"]))
        .unwrap_err();
    assert!(err.contains("invalid literal for int()"), "{err}");
    let err = c.parse_params("gap_ahead", &pairs(&["gap=x"])).unwrap_err();
    assert!(err.contains("could not convert string to float"), "{err}");
    let err = c.parse_params("gap_ahead", &pairs(&["gap"])).unwrap_err();
    assert!(err.contains("NAME=VALUE"), "{err}");
    let err = c.parse_params("gap_ahead", &pairs(&["=1"])).unwrap_err();
    assert!(err.contains("NAME=VALUE"), "{err}");
}

#[test]
fn validate_catalog_finds_problems() {
    let mut bad = doc();
    let m = bad["messages"].as_object_mut().unwrap();
    let radio = m["radio_check"].clone();
    m.insert("Bad-Code".into(), radio);
    m["flag_yellow"]["text"] = json!("Yellow {sector} {lap}.");
    m["flag_red"]["text"] = json!("Clear\u{7}");
    m["chequered"]["priority"] = json!("urgent");
    m["gap_ahead"]["audience"] = json!("everyone");
    m["flag_blue"]["max_wait_s"] = json!(0);
    m["radio_check"]["ttl_s"] = json!(30);
    m["flag_blue"]["slot"] = json!("Blue-Flag");
    m["engineer_test"]["priority"] = json!("spotter");
    m["flag_yellow_cause"]["values"] = json!({"cause": {"solo_crash": "x"}});
    m["spotter_still_right"]["max_wait_s"] = json!(30);
    m["gap_ahead"]["group"] = json!("nowhere");
    m["gap_ahead"]["kind"] = json!("shout");
    m["gap_ahead"]["note"] = json!("bad\u{7}");
    m["gap_ahead"]["text"] = json!(format!("{} {{gap}} {{fuel}}", "x".repeat(200)));
    let problems = validate_catalog(&bad, ID).join("\n");
    for word in [
        "Bad-Code",
        "flag_yellow: placeholders",
        "flag_red: control",
        "chequered: priority",
        "gap_ahead: audience",
        "flag_blue: max_wait_s",
        "radio_check: ttl_s",
        "flag_blue: slot",
        "engineer_test: priority of an agent",
        "engineer_test: priority spotter",
        "flag_yellow_cause: enum",
        "spotter_still_right: max_wait_s",
        "gap_ahead: group",
        "gap_ahead: kind",
        "gap_ahead: note",
        "gap_ahead: longer",
    ] {
        assert!(problems.contains(word), "{word}\n{problems}");
    }
    let mut broken = doc();
    broken["messages"]["flag_red"]["priority"] = json!("spotter");
    let problems = validate_catalog(&broken, ID).join("\n");
    assert!(
        problems.contains("flag_red: priority spotter needs max_wait_s"),
        "{problems}"
    );
    broken["messages"]["flag_yellow"]["text"] = json!("Final {lap");
    let problems = validate_catalog(&broken, ID).join("\n");
    assert!(
        problems.contains("flag_yellow: bad placeholder"),
        "{problems}"
    );
}

#[test]
fn the_header_is_checked() {
    let mut bad = doc();
    bad["schema"] = json!("other/1");
    bad["language"] = json!("");
    let problems = validate_catalog(&bad, ID);
    assert!(problems.iter().any(|p| p.contains("schema must be")));
    assert!(problems.iter().any(|p| p.contains("language is required")));
}

#[test]
fn placeholders_parser() {
    assert_eq!(
        placeholders("a {x} b {{literal}} {y}"),
        Some(vec!["x".to_string(), "y".to_string()])
    );
    assert_eq!(placeholders("a } b"), None);
    assert_eq!(placeholders("{Bad}"), None);
    assert_eq!(placeholders("open {x"), None);
    assert_eq!(placeholders("no braces"), Some(vec![]));
}

#[test]
fn variants_are_validated() {
    let cases: [(&str, Value, &str); 9] = [
        ("flag_blue", json!("x"), "variants must be a list"),
        ("flag_blue", json!([]), "variants must be a list"),
        (
            "flag_blue",
            json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
            "variants must be a list",
        ),
        ("flag_blue", json!(["  "]), "variant 1 must be a non-empty"),
        (
            "flag_blue",
            json!(["Move\u{7} {driver}."]),
            "control character in variant 1",
        ),
        (
            "flag_blue",
            json!(["Move {nobody}."]),
            "variant 1 placeholders",
        ),
        ("flag_blue", json!(["x".repeat(201)]), "variant 1 longer"),
        (
            "flag_blue",
            json!(["Left {driver}.", "Left {driver}."]),
            "variant 2 repeats",
        ),
        (
            "spotter_still_right",
            json!(["Car right."]),
            "variant 1 repeats",
        ),
    ];
    for (code, variants, word) in cases {
        let mut bad = doc();
        bad["messages"][code]["variants"] = variants;
        let problems = validate_catalog(&bad, ID).join("\n");
        assert!(problems.contains(word), "{word}\n{problems}");
    }
}
