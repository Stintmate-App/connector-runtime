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

/// One wheel of a [`TyreReadings`]: whatever the simulator gave, the rest `None` (never invented).
///
/// `in` is the edge towards the car's centreline, `out` the edge away from it.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct TyreWheel {
    /// Carcass temperature, inner edge, in degrees Celsius (-50..500).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carcass_in_c: Option<f64>,
    /// Carcass temperature, middle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carcass_mid_c: Option<f64>,
    /// Carcass temperature, outer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carcass_out_c: Option<f64>,
    /// Tread surface temperature, inner edge, in degrees Celsius (-50..500).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_in_c: Option<f64>,
    /// Tread surface temperature, middle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_mid_c: Option<f64>,
    /// Tread surface temperature, outer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_out_c: Option<f64>,
    /// Tread remaining, inner edge, 0 to 1 where **1 is new**.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tread_in: Option<f64>,
    /// Tread remaining, middle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tread_mid: Option<f64>,
    /// Tread remaining, outer edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tread_out: Option<f64>,
    /// Pressure at that moment (hot), in kPa (greater than 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_hot_kpa: Option<f64>,
}

impl TyreWheel {
    /// Whether no reading is set (Pitwall refuses an empty wheel).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The wheel with every value that is not a finite number removed (the wire never carries
    /// NaN or infinity).
    #[must_use]
    pub fn finite(self) -> Self {
        Self {
            carcass_in_c: finite(self.carcass_in_c),
            carcass_mid_c: finite(self.carcass_mid_c),
            carcass_out_c: finite(self.carcass_out_c),
            surface_in_c: finite(self.surface_in_c),
            surface_mid_c: finite(self.surface_mid_c),
            surface_out_c: finite(self.surface_out_c),
            tread_in: finite(self.tread_in),
            tread_mid: finite(self.tread_mid),
            tread_out: finite(self.tread_out),
            pressure_hot_kpa: finite(self.pressure_hot_kpa),
        }
    }
}

/// The wheels of a [`TyreReadings`], serialized as an object keyed `fl`, `fr`, `rl`, `rr`;
/// a wheel that is `None` is left out. At least one must be present.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct TyreWheels {
    /// Front left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fl: Option<TyreWheel>,
    /// Front right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fr: Option<TyreWheel>,
    /// Rear left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rl: Option<TyreWheel>,
    /// Rear right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rr: Option<TyreWheel>,
}

impl TyreWheels {
    /// Whether no wheel has any reading: such a message is not worth sending.
    pub fn is_empty(&self) -> bool {
        [&self.fl, &self.fr, &self.rl, &self.rr]
            .iter()
            .all(|w| w.as_ref().is_none_or(TyreWheel::is_empty))
    }
}

/// `pitwall.write.tyre_readings`: tyre readings the simulator gave at a moment (a stop, a reset,
/// the driver getting into the car, the garage), not live telemetry.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TyreReadings {
    /// ISO 8601 timestamp with UTC offset: when the simulator gave the readings.
    pub taken_at: String,
    /// `pit_stop`, `reset`, `car_entry`, `garage` or `other`.
    pub occasion: String,
    /// The readings per wheel.
    pub wheels: TyreWheels,
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
    fn tyre_readings_serialize_as_an_object_per_present_wheel() {
        let m = TyreReadings {
            taken_at: "T".into(),
            occasion: "pit_stop".into(),
            wheels: TyreWheels {
                fl: Some(TyreWheel {
                    carcass_mid_c: Some(75.5),
                    tread_in: Some(0.98),
                    ..TyreWheel::default()
                }),
                ..TyreWheels::default()
            },
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({
                "taken_at": "T", "occasion": "pit_stop",
                "wheels": {"fl": {"carcass_mid_c": 75.5, "tread_in": 0.98}}
            })
        );
        assert!(!m.wheels.is_empty());
        assert!(TyreWheels::default().is_empty());
        let empty_wheel = TyreWheels {
            rr: Some(TyreWheel::default()),
            ..TyreWheels::default()
        };
        assert!(empty_wheel.is_empty());
    }

    #[test]
    fn tyre_wheel_finite_drops_nan_and_infinity() {
        let w = TyreWheel {
            carcass_in_c: Some(f64::NAN),
            tread_out: Some(f64::INFINITY),
            pressure_hot_kpa: Some(180.0),
            ..TyreWheel::default()
        }
        .finite();
        assert_eq!(
            w,
            TyreWheel {
                pressure_hot_kpa: Some(180.0),
                ..TyreWheel::default()
            }
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
