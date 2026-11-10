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

fn tyre_wheel_full() -> TyreWheel {
    TyreWheel {
        carcass_in_c: Some(74.1),
        carcass_mid_c: Some(75.56),
        carcass_out_c: Some(73.9),
        surface_in_c: Some(60.0),
        surface_mid_c: Some(61.0),
        surface_out_c: Some(59.5),
        tread_in: Some(0.9811),
        tread_mid: Some(0.9817),
        tread_out: Some(0.9822),
        pressure_hot_kpa: Some(185.5),
    }
}

#[test]
fn tyre_readings_validate() {
    for occasion in ["pit_stop", "reset", "car_entry", "garage", "other"] {
        ok(
            "tyre_readings",
            true,
            &TyreReadings {
                taken_at: "2026-11-07T11:52:41.512+01:00".into(),
                occasion: occasion.into(),
                wheels: TyreWheels {
                    fl: Some(tyre_wheel_full()),
                    fr: Some(tyre_wheel_full()),
                    rl: Some(tyre_wheel_full()),
                    rr: Some(tyre_wheel_full()),
                },
            },
        );
    }
    // the smallest message: one wheel, one reading
    ok(
        "tyre_readings",
        true,
        &TyreReadings {
            taken_at: "2026-11-07T11:52:41Z".into(),
            occasion: "other".into(),
            wheels: TyreWheels {
                rr: Some(TyreWheel {
                    tread_mid: Some(1.0),
                    ..TyreWheel::default()
                }),
                ..TyreWheels::default()
            },
        },
    );
}

#[test]
fn the_schema_refuses_bad_tyre_readings() {
    let good = TyreReadings {
        taken_at: "2026-11-07T11:52:41Z".into(),
        occasion: "pit_stop".into(),
        wheels: TyreWheels {
            fl: Some(tyre_wheel_full()),
            ..TyreWheels::default()
        },
    };
    let bad = |edit: &dyn Fn(&mut Value)| {
        let mut m = message("tyre_readings", true, &good);
        edit(&mut m);
        refused("tyre_readings", &m);
    };
    bad(&|m| m["occasion"] = json!("tyre_change"));
    bad(&|m| m["wheels"] = json!({}));
    bad(&|m| m["wheels"] = json!({"fl": {}}));
    bad(&|m| m["wheels"]["fl"]["tread_in"] = json!(1.5));
    bad(&|m| m["wheels"]["fl"]["carcass_mid_c"] = json!(600));
    bad(&|m| m["wheels"]["fl"]["pressure_hot_kpa"] = json!(0));
    bad(&|m| m["wheels"]["xx"] = json!({"tread_in": 0.5}));
    bad(&|m| {
        m.as_object_mut().unwrap().remove("session_id");
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
