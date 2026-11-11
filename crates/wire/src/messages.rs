//! The write API messages a connector sends (the write API's `write-api.md`), as typed structs.
//!
//! Each message serializes to the **params** of its IPC write method (`pitwall.write.<type>`):
//! the message without `type` and without `session_id` (the connector names its session by its
//! own `session_key`, which the sink adds), `None` fields left out. A lap's channels are
//! `{"encoding": "f32", "data": <bin: little-endian float32>}`; serializing them uses
//! `serialize_bytes`, so a msgpack serializer writes `bin` (the JSON form is only a debugging
//! aid). Encoding to the wire is the IPC layer's job.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// Little-endian float32 samples as raw bytes (`bin` on the IPC wire).
pub fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Bytes serialized as a byte string (`bin` in msgpack).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bin(pub Vec<u8>);

impl Serialize for Bin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

#[derive(Serialize)]
struct EncodedChannel {
    encoding: &'static str,
    data: Bin,
}

/// A lap's channels in catalogue order, serialized as `{name: {encoding: "f32", data: bin}}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LapChannels(pub Vec<(Cow<'static, str>, Vec<f32>)>);

impl Serialize for LapChannels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (name, col) in &self.0 {
            map.serialize_entry(
                name,
                &EncodedChannel {
                    encoding: "f32",
                    data: Bin(f32_bytes(col)),
                },
            )?;
        }
        map.end()
    }
}

/// `Some(v)` only for finite numbers (the wire never carries NaN or infinity).
pub fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// A car class of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarClass {
    /// Class id.
    pub class_id: String,
    /// Display name.
    pub name: String,
}

/// A car of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Car {
    /// Car id.
    pub car_id: String,
    /// Display name.
    pub name: String,
    /// Class id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<String>,
    /// Tank capacity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_capacity_l: Option<f64>,
}

/// A track of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
    /// Track id.
    pub track_id: String,
    /// Display name.
    pub name: String,
}

/// A layout of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    /// Track id.
    pub track_id: String,
    /// Layout id.
    pub layout_id: String,
    /// Display name.
    pub name: String,
    /// Length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_m: Option<f64>,
}

/// `pitwall.write.catalog`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Catalog {
    /// Classes.
    pub classes: Vec<CarClass>,
    /// Cars.
    pub cars: Vec<Car>,
    /// Tracks.
    pub tracks: Vec<Track>,
    /// Layouts.
    pub layouts: Vec<Layout>,
}

/// Weather at a session start or the end of a lap.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Conditions {
    /// Ambient temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient_c: Option<f64>,
    /// Track temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_c: Option<f64>,
    /// Rain density.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rain: Option<f64>,
    /// Wind speed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_speed_ms: Option<f64>,
    /// Cloud cover, 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<f64>,
}

/// `Some` unless every value is missing.
pub fn conditions(
    ambient: Option<f64>,
    track: Option<f64>,
    rain: Option<f64>,
    wind: Option<f64>,
) -> Option<Conditions> {
    let c = Conditions {
        ambient_c: finite(ambient),
        track_c: finite(track),
        rain: finite(rain),
        wind_speed_ms: finite(wind),
        cloud: None,
    };
    (c.ambient_c.is_some() || c.track_c.is_some() || c.rain.is_some() || c.wind_speed_ms.is_some())
        .then_some(c)
}

/// `pitwall.write.session_start`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionStart {
    /// ISO timestamp.
    pub started_at: String,
    /// `practice`, `qualifying`, `race` or `other`.
    pub session_type: String,
    /// Car id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub car_id: Option<String>,
    /// Track id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    /// Layout id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_id: Option<String>,
    /// Outside reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    /// Weather.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
}

/// One sector of a lap.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Sector {
    /// Time (absent when unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
}

/// `pitwall.write.lap`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lap {
    /// `formation`, `out`, `in` or `flying`.
    pub lap_type: String,
    /// The car was on the track (outside the pit lane) during the lap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_track: Option<bool>,
    /// Lap time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
    /// Valid for the game.
    pub valid: bool,
    /// `official` or `measured`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_source: Option<String>,
    /// The lap clock at the last sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_time_s: Option<f64>,
    /// ISO timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// The game's lap counter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_lap: Option<u32>,
    /// Fuel at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_start_l: Option<f64>,
    /// Fuel used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_used_l: Option<f64>,
    /// Top speed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speed_kmh: Option<f64>,
    /// Weather at the end of the lap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
    /// The three sectors.
    pub sectors: Vec<Sector>,
    /// The samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<LapChannels>,
}

/// An extra setup value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtraEntry {
    /// Label.
    pub label: String,
    /// Value.
    pub value: ExtraValue,
    /// Group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// The value of an [`ExtraEntry`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ExtraValue {
    /// Text.
    Text(String),
    /// Number.
    Number(f64),
    /// Flag.
    Bool(bool),
}

/// `pitwall.write.setup` (source `telemetry`).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Setup {
    /// ISO timestamp.
    pub taken_at: String,
    /// `session_start` or `pit_exit`.
    pub reason: String,
    /// Always `telemetry`.
    pub source: String,
    /// Brake bias, % front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brake_bias_front_pct: Option<f64>,
    /// Front compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound_front: Option<String>,
    /// Rear compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound_rear: Option<String>,
    /// Cold pressure FL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_fl_kpa: Option<f64>,
    /// Cold pressure FR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_fr_kpa: Option<f64>,
    /// Cold pressure RL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_rl_kpa: Option<f64>,
    /// Cold pressure RR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_rr_kpa: Option<f64>,
    /// Fuel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_l: Option<f64>,
    /// Front wing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_front: Option<f64>,
    /// Rear wing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_rear: Option<f64>,
    /// ABS setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs: Option<f64>,
    /// TC setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc: Option<f64>,
    /// Extra entries.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, ExtraEntry>,
}

/// One row of `participant_laps`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticipantLap {
    /// Participant index.
    pub participant: u32,
    /// Lap (the game's count).
    pub lap: u32,
    /// Driver name.
    pub name: String,
    /// Car id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub car_id: Option<String>,
    /// Class id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<String>,
    /// The viewed car.
    pub is_player: bool,
    /// Lap time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
    /// `official` or `sectors`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_source: Option<String>,
    /// Sector times (S1, S2, S3) or empty.
    pub sectors: Vec<Option<f64>>,
    /// Not invalidated.
    pub valid: bool,
    /// A pit mode was seen during the lap.
    pub pit: bool,
    /// Pit stops so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pit_stops: Option<u32>,
    /// The lap started on a line crossing.
    pub from_line: bool,
    /// Race position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Seconds since the session began.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crossed_at_s: Option<f64>,
    /// The game's best lap of this car.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_best_lap_s: Option<f64>,
    /// Race state label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// `pitwall.write.pit_exit`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PitExit {
    /// ISO timestamp.
    pub at: String,
    /// Fuel at the exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_l: Option<f64>,
}

/// `pitwall.write.lap_positions`: better position channels for a lap already sent. Over IPC the
/// lap is named by `session_key` and `lap_seq` (the sink adds both); the write API message
/// `lap_positions` carries `session_id` and `lap` instead.
///
/// `channels` holds `x` and `z` (metres, plan view; both required) and optionally `y` (up), each
/// as many samples as the stored lap.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LapPositions {
    /// `x`, `z` and optionally `y`.
    pub channels: LapChannels,
}

/// `pitwall.write.sim_data`: data the simulator gave at a moment (a stop, a reset, the garage, a
/// penalty notice...) that Pitwall has no typed message for, in a structure the connector designs
/// and describes with [`SimDataKinds`]. Pitwall stores it as sent. Not for samples: those are
/// lap channels. Limits: [`crate::sim_data`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimData {
    /// ISO 8601 timestamp with UTC offset: when the simulator gave the data.
    pub taken_at: String,
    /// The connector's name for the structure of `data` (`^[a-z][a-z0-9_]{0,31}$`).
    pub kind: String,
    /// The structure, as a JSON object.
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// One field of a [`SimDataKind`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimDataField {
    /// What the field is (1 to 300 characters).
    pub meaning: String,
    /// Its unit, when it has one (1 to 24 characters).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// How a connector describes one kind of simulator data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimDataKind {
    /// The kind's name, as in [`SimData::kind`].
    pub kind: String,
    /// What a record is and when the connector sends one (1 to 500 characters).
    pub description: String,
    /// One of the guidance topics (`setup`, `capture`, `telemetry`, `session`, `car`, `track`,
    /// `tyres`, `fuel`, `strategy`, `controls`, `other`), when one fits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Field paths of `data` (dotted keys; a `*` segment stands for any key or array index)
    /// and their descriptions: 1 to 64.
    pub fields: BTreeMap<String, SimDataField>,
}

/// `pitwall.write.sim_data_kinds`: the descriptions of the kinds a connector sends, stored per
/// simulator and kind. A kind's fields never change meaning: a changed meaning is a new kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimDataKinds {
    /// 1 to 32 kinds, each named once.
    pub kinds: Vec<SimDataKind>,
}

/// `pitwall.write.participant_laps`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticipantLaps {
    /// The rows.
    pub laps: Vec<ParticipantLap>,
}

/// `pitwall.write.session_end`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionEnd {
    /// ISO timestamp.
    pub ended_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_bytes_are_little_endian() {
        assert_eq!(
            f32_bytes(&[1.0, -2.5]),
            [0, 0, 0x80, 0x3f, 0, 0, 0x20, 0xc0]
        );
        assert!(f32_bytes(&[]).is_empty());
    }

    #[test]
    fn params_leave_out_missing_fields() {
        let m = PitExit {
            at: "T".into(),
            fuel_l: None,
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"at": "T"})
        );
        let s = Setup {
            source: "telemetry".into(),
            ..Setup::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("extra").is_none() && v.get("abs").is_none());
        assert_eq!(conditions(None, None, None, None), None);
        assert_eq!(finite(Some(f64::NAN)), None);
        assert_eq!(finite(Some(f64::INFINITY)), None);
        assert_eq!(finite(Some(1.5)), Some(1.5));
    }

    #[test]
    fn sim_data_serializes_as_sent_and_leaves_out_missing_options() {
        let data =
            serde_json::json!({"occasion": "pit_stop", "wheels": {"fl": {"tread_mid": 0.98}}});
        let m = SimData {
            taken_at: "T".into(),
            kind: "stall_tyres".into(),
            data: data.as_object().unwrap().clone(),
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"taken_at": "T", "kind": "stall_tyres", "data": data})
        );
        let field = |unit: Option<&str>| SimDataField {
            meaning: "m".into(),
            unit: unit.map(str::to_owned),
        };
        let k = SimDataKinds {
            kinds: vec![SimDataKind {
                kind: "stall_tyres".into(),
                description: "d".into(),
                topic: None,
                fields: BTreeMap::from([
                    ("occasion".to_owned(), field(None)),
                    ("a.*.b".to_owned(), field(Some("C"))),
                ]),
            }],
        };
        assert_eq!(
            serde_json::to_value(&k).unwrap(),
            serde_json::json!({"kinds": [{"kind": "stall_tyres", "description": "d",
                "fields": {"a.*.b": {"meaning": "m", "unit": "C"}, "occasion": {"meaning": "m"}}}]})
        );
    }

    #[test]
    fn conditions_keep_only_finite_values() {
        let c = conditions(Some(21.0), Some(f64::NAN), None, Some(f64::INFINITY)).unwrap();
        assert_eq!(c.ambient_c, Some(21.0));
        assert_eq!(
            (c.track_c, c.rain, c.wind_speed_ms, c.cloud),
            (None, None, None, None)
        );
    }

    #[test]
    fn lap_channels_serialize_as_encoded_f32() {
        let ch = LapChannels(vec![("speed".into(), vec![100.0])]);
        let v = serde_json::to_value(&ch).unwrap();
        assert_eq!(v["speed"]["encoding"], "f32");
        assert_eq!(v["speed"]["data"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn lap_channels_accept_owned_names() {
        let ch = LapChannels(vec![
            ("zz".into(), vec![1.0]),
            (Cow::Owned(format!("ext_{}", "a")), vec![2.0, 3.0]),
        ]);
        let v = serde_json::to_value(&ch).unwrap();
        assert_eq!(v["ext_a"]["data"].as_array().unwrap().len(), 8);
    }
}
