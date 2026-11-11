//! Every message of the wire crate validates against the write API schema, filled and minimal,
//! and the bad ones are refused.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use stintmate_connector_wire::messages::*;
use stintmate_connector_wire::schema_check::{PROTOCOL_SCHEMA, WRITE_API_SCHEMA, definition};

fn docs() -> Vec<Value> {
    vec![
        serde_json::from_str(WRITE_API_SCHEMA).expect("the write API schema is JSON"),
        serde_json::from_str(PROTOCOL_SCHEMA).expect("the protocol schema is JSON"),
    ]
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
    }
    out
}

/// The wire message of a struct: its params plus `type` (and `session_id` when it has one).
/// In JSON the bytes of a channel are an array of numbers; the schema's form is base64 text.
fn message(kind: &str, session: bool, m: &impl Serialize) -> Value {
    let mut v = serde_json::to_value(m).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("type".into(), json!(kind));
    if session {
        o.insert("session_id".into(), json!(1));
    }
    if let Some(Value::Object(chs)) = o.get_mut("channels") {
        for ch in chs.values_mut() {
            let bytes: Vec<u8> = ch["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
                .collect();
            ch["data"] = json!(base64(&bytes));
        }
    }
    v
}

fn ok(kind: &str, session: bool, m: &impl Serialize) {
    let v = message(kind, session, m);
    definition(&docs(), kind, &v).unwrap_or_else(|e| panic!("{kind}: {e}\n{v}"));
}

fn refused(kind: &str, v: &Value) {
    assert!(definition(&docs(), kind, v).is_err(), "{kind} accepted {v}");
}

fn conditions_full() -> Conditions {
    Conditions {
        ambient_c: Some(21.5),
        track_c: Some(33.0),
        rain: Some(0.25),
        wind_speed_ms: Some(3.0),
        cloud: Some(0.5),
    }
}

#[test]
fn catalog_validates() {
    ok("catalog", false, &Catalog::default());
    ok(
        "catalog",
        false,
        &Catalog {
            classes: vec![CarClass {
                class_id: "gt3".into(),
                name: "GT3".into(),
            }],
            cars: vec![
                Car {
                    car_id: "c1".into(),
                    name: "Car one".into(),
                    class_id: Some("gt3".into()),
                    fuel_capacity_l: Some(100.0),
                },
                Car {
                    car_id: "c2".into(),
                    name: "Car two".into(),
                    class_id: None,
                    fuel_capacity_l: None,
                },
            ],
            tracks: vec![Track {
                track_id: "t".into(),
                name: "Track".into(),
            }],
            layouts: vec![
                Layout {
                    track_id: "t".into(),
                    layout_id: "gp".into(),
                    name: "Grand Prix".into(),
                    length_m: Some(4300.0),
                },
                Layout {
                    track_id: "t".into(),
                    layout_id: "short".into(),
                    name: "Short".into(),
                    length_m: None,
                },
            ],
        },
    );
}

#[test]
fn session_start_validates() {
    ok(
        "session_start",
        false,
        &SessionStart {
            started_at: "2026-01-02T03:04:05Z".into(),
            session_type: "other".into(),
            car_id: None,
            track_id: None,
            layout_id: None,
            external_ref: None,
            conditions: None,
        },
    );
    ok(
        "session_start",
        false,
        &SessionStart {
            started_at: "2026-01-02T03:04:05Z".into(),
            session_type: "race".into(),
            car_id: Some("c1".into()),
            track_id: Some("t".into()),
            layout_id: Some("gp".into()),
            external_ref: Some("ref-1".into()),
            conditions: Some(conditions_full()),
        },
    );
}

fn lap_min() -> Lap {
    Lap {
        lap_type: "in".into(),
        on_track: None,
        time_s: None,
        valid: true,
        time_source: None,
        measured_time_s: None,
        started_at: None,
        sim_lap: None,
        fuel_start_l: None,
        fuel_used_l: None,
        max_speed_kmh: None,
        conditions: None,
        sectors: vec![Sector::default(); 3],
        channels: None,
    }
}

#[test]
fn lap_validates() {
    ok("lap", true, &lap_min());
    ok(
        "lap",
        true,
        &Lap {
            lap_type: "flying".into(),
            on_track: Some(true),
            time_s: Some(92.345),
            valid: false,
            time_source: Some("official".into()),
            measured_time_s: Some(92.4),
            started_at: Some("2026-01-02T03:04:05Z".into()),
            sim_lap: Some(4),
            fuel_start_l: Some(40.0),
            fuel_used_l: Some(2.5),
            max_speed_kmh: Some(250.0),
            conditions: Some(conditions_full()),
            sectors: vec![
                Sector { time_s: Some(30.1) },
                Sector { time_s: None },
                Sector { time_s: Some(31.2) },
            ],
            channels: Some(LapChannels(vec![
                ("speed".into(), vec![100.0, 101.5]),
                ("ext_boost".into(), vec![1.0]),
            ])),
        },
    );
}

#[test]
fn lap_channels_serialize_as_bin() {
    let ch = LapChannels(vec![("speed".into(), vec![1.0, -2.5])]);
    let v = serde_json::to_value(&ch).unwrap();
    assert_eq!(v["speed"]["encoding"], "f32");
    // In JSON the bin is an array of the little-endian bytes.
    assert_eq!(
        v["speed"]["data"],
        json!([0, 0, 0x80, 0x3f, 0, 0, 0x20, 0xc0])
    );
    assert_eq!(
        f32_bytes(&[1.0, -2.5]),
        [0, 0, 0x80, 0x3f, 0, 0, 0x20, 0xc0]
    );
    // Through a serializer that has a byte type, the data is a byte string.
    assert_eq!(
        serde_json::to_value(Bin(vec![1, 2])).unwrap(),
        json!([1, 2])
    );
}

#[test]
fn setup_validates() {
    ok(
        "setup",
        true,
        &Setup {
            taken_at: "2026-01-02T03:04:05Z".into(),
            reason: "session_start".into(),
            source: "telemetry".into(),
            ..Setup::default()
        },
    );
    let mut extra = BTreeMap::new();
    extra.insert(
        "diff".to_owned(),
        ExtraEntry {
            label: "Diff".into(),
            value: ExtraValue::Number(50.0),
            group: Some("Drivetrain".into()),
            unit: Some("%".into()),
        },
    );
    extra.insert(
        "mode".to_owned(),
        ExtraEntry {
            label: "Mode".into(),
            value: ExtraValue::Text("wet".into()),
            group: None,
            unit: None,
        },
    );
    extra.insert(
        "aid".to_owned(),
        ExtraEntry {
            label: "Aid".into(),
            value: ExtraValue::Bool(true),
            group: None,
            unit: None,
        },
    );
    ok(
        "setup",
        true,
        &Setup {
            taken_at: "2026-01-02T03:04:05Z".into(),
            reason: "pit_exit".into(),
            source: "telemetry".into(),
            brake_bias_front_pct: Some(56.0),
            compound_front: Some("soft".into()),
            compound_rear: Some("soft".into()),
            pressure_fl_kpa: Some(180.0),
            pressure_fr_kpa: Some(181.0),
            pressure_rl_kpa: Some(170.0),
            pressure_rr_kpa: Some(171.0),
            fuel_l: Some(30.0),
            wing_front: Some(3.0),
            wing_rear: Some(5.0),
            abs: Some(2.0),
            tc: Some(4.0),
            extra,
        },
    );
}

#[test]
fn pit_exit_validates() {
    ok(
        "pit_exit",
        true,
        &PitExit {
            at: "2026-01-02T03:04:05Z".into(),
            fuel_l: None,
        },
    );
    ok(
        "pit_exit",
        true,
        &PitExit {
            at: "2026-01-02T03:04:05Z".into(),
            fuel_l: Some(12.5),
        },
    );
}

fn sim_data(data: Value) -> SimData {
    SimData {
        taken_at: "2026-11-07T11:52:41.512+01:00".into(),
        kind: "stall_tyres".into(),
        data: data.as_object().unwrap().clone(),
    }
}

fn kinds_message() -> SimDataKinds {
    let field = |meaning: &str, unit: Option<&str>| SimDataField {
        meaning: meaning.into(),
        unit: unit.map(str::to_owned),
    };
    SimDataKinds {
        kinds: vec![
            SimDataKind {
                kind: "stall_tyres".into(),
                description: "Tyre temperatures and tread given only in the pits.".into(),
                topic: Some("tyres".into()),
                fields: BTreeMap::from([
                    (
                        "occasion".to_owned(),
                        field("Why the car was in the pits.", None),
                    ),
                    (
                        "wheels.*.carcass_in_c".to_owned(),
                        field("Carcass temperature, inner edge.", Some("C")),
                    ),
                ]),
            },
            SimDataKind {
                kind: "damage".into(),
                description: "A damage report.".into(),
                topic: None,
                fields: BTreeMap::from([("parts.*".to_owned(), field("Damage of a part.", None))]),
            },
        ],
    }
}

#[test]
fn sim_data_validates() {
    ok(
        "sim_data",
        true,
        &sim_data(json!({
            "occasion": "pit_stop",
            "wheels": {"fl": {"carcass_in_c": 74.1, "tread_mid": 0.9817}, "rr": {"tread_mid": 1.0}},
            "flags": [true, null, "x"]
        })),
    );
    // the smallest message: an empty structure
    ok("sim_data", true, &sim_data(json!({})));
}

#[test]
// The schema checker used here does not evaluate `pattern`: the name patterns are covered by
// `stintmate_connector_wire::sim_data` tests.
fn the_schema_refuses_bad_sim_data() {
    let good = sim_data(json!({"occasion": "pit_stop"}));
    let bad = |edit: &dyn Fn(&mut Value)| {
        let mut m = message("sim_data", true, &good);
        edit(&mut m);
        refused("sim_data", &m);
    };
    bad(&|m| m["kind"] = json!(""));
    bad(&|m| m["data"] = json!([1, 2]));
    bad(&|m| m["data"] = json!("text"));
    bad(&|m| m["extra"] = json!(1));
    bad(&|m| {
        m.as_object_mut().unwrap().remove("session_id");
    });
    bad(&|m| {
        m.as_object_mut().unwrap().remove("kind");
    });
}

#[test]
fn sim_data_kinds_validate() {
    ok("sim_data_kinds", false, &kinds_message());
}

#[test]
fn the_schema_refuses_bad_sim_data_kinds() {
    let bad = |edit: &dyn Fn(&mut Value)| {
        let mut m = message("sim_data_kinds", false, &kinds_message());
        edit(&mut m);
        refused("sim_data_kinds", &m);
    };
    bad(&|m| m["kinds"] = json!([]));
    bad(&|m| m["kinds"][0]["description"] = json!(""));
    bad(&|m| m["kinds"][0]["topic"] = json!("wheels"));
    bad(&|m| m["kinds"][0]["fields"] = json!({}));
    bad(&|m| m["kinds"][0]["fields"]["occasion"] = json!({"unit": "C"}));
    bad(&|m| m["kinds"][0]["fields"]["occasion"]["meaning"] = json!(""));
    bad(&|m| m["kinds"][0]["fields"]["occasion"]["extra"] = json!(1));
    bad(&|m| m["kinds"][0]["unknown"] = json!(1));
    bad(&|m| {
        m.as_object_mut().unwrap().remove("kinds");
    });
}

fn row_min() -> ParticipantLap {
    ParticipantLap {
        participant: 0,
        lap: 1,
        name: "Driver".into(),
        car_id: None,
        class_id: None,
        is_player: false,
        time_s: None,
        time_source: None,
        sectors: vec![],
        valid: true,
        pit: false,
        pit_stops: None,
        from_line: true,
        position: None,
        crossed_at_s: None,
        sim_best_lap_s: None,
        state: None,
    }
}

#[test]
fn participant_laps_validate() {
    ok("participant_laps", true, &ParticipantLaps { laps: vec![] });
    ok(
        "participant_laps",
        true,
        &ParticipantLaps {
            laps: vec![row_min()],
        },
    );
    ok(
        "participant_laps",
        true,
        &ParticipantLaps {
            laps: vec![ParticipantLap {
                participant: 3,
                lap: 7,
                name: "Another".into(),
                car_id: Some("c1".into()),
                class_id: Some("gt3".into()),
                is_player: true,
                time_s: Some(91.2),
                time_source: Some("sectors".into()),
                sectors: vec![Some(30.0), None, Some(31.0)],
                valid: false,
                pit: true,
                pit_stops: Some(2),
                from_line: false,
                position: Some(4),
                crossed_at_s: Some(640.5),
                sim_best_lap_s: Some(90.9),
                state: Some("racing".into()),
            }],
        },
    );
}

#[test]
fn session_end_validates() {
    ok(
        "session_end",
        true,
        &SessionEnd {
            ended_at: "2026-01-02T04:00:00Z".into(),
        },
    );
}

#[test]
fn the_schema_refuses_what_the_structs_never_write() {
    let docs = docs();
    let mut lap = message("lap", true, &lap_min());
    lap["lap_type"] = json!("sprint");
    refused("lap", &lap);
    let mut lap = message("lap", true, &lap_min());
    lap["surprise"] = json!(1);
    refused("lap", &lap);
    let mut lap = message("lap", true, &lap_min());
    lap.as_object_mut().unwrap().remove("session_id");
    refused("lap", &lap);
    let mut lap = message("lap", true, &lap_min());
    lap["channels"] = json!({"speed": {"encoding": "f64", "data": "AAAA"}});
    refused("lap", &lap);
    let mut setup = message(
        "setup",
        true,
        &Setup {
            taken_at: "T".into(),
            reason: "session_start".into(),
            source: "telemetry".into(),
            ..Setup::default()
        },
    );
    setup["extra"] = json!({"x": {"label": "L"}});
    refused("setup", &setup);
    // The checker really reads the write API document, not an empty schema.
    assert!(docs[0]["$defs"]["lap"].is_object());
}

fn positions(channels: Vec<(&'static str, Vec<f32>)>) -> LapPositions {
    LapPositions {
        channels: LapChannels(channels.into_iter().map(|(n, c)| (n.into(), c)).collect()),
    }
}

/// The write API message: the sink's params carry `session_key` and `lap_seq` instead of
/// `session_id` and `lap`.
fn positions_message(m: &LapPositions) -> Value {
    let mut v = message("lap_positions", true, m);
    v["lap"] = json!(5);
    v
}

#[test]
fn lap_positions_validates() {
    let xz = positions(vec![("x", vec![0.0, 1.5]), ("z", vec![2.0, 3.5])]);
    let v = positions_message(&xz);
    definition(&docs(), "lap_positions", &v).unwrap_or_else(|e| panic!("{e}\n{v}"));
    let xyz = positions(vec![("x", vec![0.0]), ("z", vec![1.0]), ("y", vec![2.0])]);
    let v = positions_message(&xyz);
    definition(&docs(), "lap_positions", &v).unwrap_or_else(|e| panic!("{e}\n{v}"));
}

#[test]
fn the_schema_refuses_bad_lap_positions() {
    let good = positions_message(&positions(vec![("x", vec![0.0]), ("z", vec![1.0])]));
    // x and z are both required
    let mut v = good.clone();
    v["channels"].as_object_mut().unwrap().remove("z");
    refused("lap_positions", &v);
    // only x, y and z
    let mut v = good.clone();
    v["channels"]["speed"] = v["channels"]["x"].clone();
    refused("lap_positions", &v);
    let mut v = good.clone();
    v.as_object_mut().unwrap().remove("lap");
    refused("lap_positions", &v);
    let mut v = good;
    v["extra"] = json!(1);
    refused("lap_positions", &v);
}
