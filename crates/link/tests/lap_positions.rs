//! `WriteSink::lap_with_address` and `lap_positions` over a fake Pitwall.

mod common;

use common::{Rig, wait_for};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stintmate_connector_link::sink::{ResumeClock, SessionOpen, WriteSink};
use stintmate_connector_wire::messages::{Lap, LapChannels, LapPositions, SessionStart};
use stintmate_ipc::codec::{self, Value};

#[derive(Clone)]
struct NoClock;

impl ResumeClock for NoClock {
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn from_json(_: &serde_json::Value) -> Option<Self> {
        Some(NoClock)
    }
    fn continues_as(&self, _: &Self) -> bool {
        true
    }
}

fn open() -> SessionOpen {
    SessionOpen {
        identity: "Example Car|Example Park|Short|RACE".into(),
        catalog: None,
        start: SessionStart {
            started_at: "2026-11-11T10:00:00.000+01:00".into(),
            session_type: "race".into(),
            car_id: None,
            track_id: None,
            layout_id: None,
            external_ref: None,
            conditions: None,
        },
    }
}

fn lap() -> Lap {
    Lap {
        lap_type: "flying".into(),
        on_track: Some(true),
        time_s: Some(90.0),
        valid: true,
        time_source: None,
        measured_time_s: None,
        started_at: None,
        sim_lap: None,
        fuel_start_l: None,
        fuel_used_l: None,
        max_speed_kmh: None,
        conditions: None,
        sectors: Vec::new(),
        channels: None,
    }
}

fn positions() -> LapPositions {
    LapPositions {
        channels: LapChannels(vec![
            (Cow::Borrowed("x"), vec![0.0, 1.5, 3.0]),
            (Cow::Borrowed("z"), vec![0.0, 0.5, 1.0]),
        ]),
    }
}

#[test]
fn positions_queued_after_session_end_are_sent_in_order_under_the_lap_address() {
    let rig = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let mut sink: WriteSink<NoClock> =
        WriteSink::new(&dir.path().join("outbox"), rig.transport(), None, "example").unwrap();
    let local = sink.start_session(open());
    let key = sink.session_key(Some(local)).unwrap();
    let address: Arc<Mutex<Vec<(String, u64)>>> = Arc::default();
    let slot = address.clone();
    sink.lap_with_address(
        local,
        Box::new(lap),
        Box::new(move |k, seq| slot.lock().unwrap().push((k.to_owned(), seq))),
    );
    sink.end_session(local, "2026-11-11T10:30:00.000+01:00");
    assert!(sink.queued(Duration::from_secs(5)));
    // The session is over and gone from the sink's state: the key and number name the lap.
    let (k, seq) = address.lock().unwrap()[0].clone();
    assert_eq!((k.as_str(), seq), (key.as_str(), 1));
    sink.lap_positions(&k, seq, Box::new(positions));
    assert!(sink.flush(Duration::from_secs(10)));
    assert!(wait_for(5, || rig
        .fake
        .count("pitwall.write.lap_positions")
        == 1));

    let calls = rig.fake.calls();
    let order: Vec<&str> = calls.iter().map(|c| c.method.as_str()).collect();
    assert_eq!(
        order,
        [
            "pitwall.write.session_start",
            "pitwall.write.lap",
            "pitwall.write.session_end",
            "pitwall.write.lap_positions"
        ]
    );
    assert_eq!(
        calls[1].ik.as_deref(),
        Some(format!("example/{key}/lap/1").as_str())
    );
    let p = &calls[3];
    assert_eq!(
        p.ik.as_deref(),
        Some(format!("example/{key}/lap_positions/1").as_str())
    );
    assert_eq!(
        codec::get(&p.params, "session_key").and_then(Value::as_str),
        Some(key.as_str())
    );
    assert_eq!(
        codec::get(&p.params, "lap_seq").and_then(Value::as_i64),
        Some(1)
    );
    assert!(codec::get(&p.params, "channels").is_some());
    assert!(codec::get(&p.params, "session_id").is_none());
    assert!(codec::get(&p.params, "lap").is_none());
    sink.close(Duration::ZERO);
}
