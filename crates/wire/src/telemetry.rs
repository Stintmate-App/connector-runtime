//! `connector.telemetry` v1 (connector kit, contract.md section 5.5): the neutral 32-byte sample a brake cue scheduler
//! runs on.
//!
//! Layout (little endian, 32 bytes): `lap_distance_m f32`, `speed_ms f32`, `accel_long_ms2 f32`,
//! `track_length_m f32`, `lap_time_s f32`, `frame u32`, `sim_lap u16`, `session_type u8`,
//! `game_state u8`, `flags u16`, `reserved u16`.
//!
//! Mapping a game's values to a [`Sample`] belongs to each connector, and so does the sampling
//! rate: this module has the declared range a subscriber may ask for ([`RATE_MIN`],
//! [`RATE_MAX`]), the sample and its flags.

/// The channel's id.
pub const CHANNEL: &str = "connector.telemetry";
/// Subscription option `rate_hz` range.
pub const RATE_MIN: u32 = 10;
/// Subscription option `rate_hz` range.
pub const RATE_MAX: u32 = 200;
/// Bytes of one sample.
pub const SAMPLE_SIZE: usize = 32;

/// Flag: the car is on track.
pub const ON_TRACK: u16 = 1 << 0;
/// Flag: in the pit lane.
pub const IN_PIT_LANE: u16 = 1 << 1;
/// Flag: in the garage.
pub const IN_GARAGE: u16 = 1 << 2;
/// Flag: the lap clock runs.
pub const TIMED: u16 = 1 << 3;
/// Flag: a flying lap (timed, on track, not an out lap).
pub const FLYING: u16 = 1 << 4;
/// Flag: the game flagged the lap invalid.
pub const LAP_INVALID: u16 = 1 << 5;
/// Flag: the viewed car is the player's (not a replay, a valid index).
pub const PLAYER_VIEW: u16 = 1 << 6;
/// Flag: a new game frame (not a keep-alive repeat).
pub const FRESH: u16 = 1 << 7;

/// One 32-byte sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Lap distance, m (-1: unknown).
    pub lap_distance_m: f32,
    /// Speed, m/s.
    pub speed_ms: f32,
    /// Longitudinal acceleration, m/s2 (braking is negative).
    pub accel_long_ms2: f32,
    /// Track length, m (0: unknown).
    pub track_length_m: f32,
    /// Lap clock, s (-1: not timed).
    pub lap_time_s: f32,
    /// Frame counter.
    pub frame: u32,
    /// The game's lap counter.
    pub sim_lap: u16,
    /// 0 unknown, 1 practice, 2 test, 3 qualifying, 4 warmup, 5 race, 6 other.
    pub session_type: u8,
    /// 0 not running, 1 menus, 2 loading, 3 playing, 4 paused, 5 replay.
    pub game_state: u8,
    /// The `ON_TRACK` .. `FRESH` bits.
    pub flags: u16,
    /// Reserved, 0.
    pub reserved: u16,
}

impl Sample {
    /// The wire bytes (little endian).
    pub fn to_bytes(&self) -> [u8; SAMPLE_SIZE] {
        let mut b = [0u8; SAMPLE_SIZE];
        b[0..4].copy_from_slice(&self.lap_distance_m.to_le_bytes());
        b[4..8].copy_from_slice(&self.speed_ms.to_le_bytes());
        b[8..12].copy_from_slice(&self.accel_long_ms2.to_le_bytes());
        b[12..16].copy_from_slice(&self.track_length_m.to_le_bytes());
        b[16..20].copy_from_slice(&self.lap_time_s.to_le_bytes());
        b[20..24].copy_from_slice(&self.frame.to_le_bytes());
        b[24..26].copy_from_slice(&self.sim_lap.to_le_bytes());
        b[26] = self.session_type;
        b[27] = self.game_state;
        b[28..30].copy_from_slice(&self.flags.to_le_bytes());
        b[30..32].copy_from_slice(&self.reserved.to_le_bytes());
        b
    }

    /// Reads a sample from its wire bytes.
    pub fn from_bytes(b: &[u8; SAMPLE_SIZE]) -> Self {
        let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Self {
            lap_distance_m: f(0),
            speed_ms: f(4),
            accel_long_ms2: f(8),
            track_length_m: f(12),
            lap_time_s: f(16),
            frame: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            sim_lap: u16::from_le_bytes([b[24], b[25]]),
            session_type: b[26],
            game_state: b[27],
            flags: u16::from_le_bytes([b[28], b[29]]),
            reserved: u16::from_le_bytes([b[30], b[31]]),
        }
    }
}
/// The keep-alive sample while the game is not running (`game_state` 0).
pub fn idle_values(frame: u32) -> Sample {
    Sample {
        lap_distance_m: -1.0,
        speed_ms: 0.0,
        accel_long_ms2: 0.0,
        track_length_m: 0.0,
        lap_time_s: -1.0,
        frame,
        sim_lap: 0,
        session_type: 0,
        game_state: 0,
        flags: 0,
        reserved: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> Sample {
        Sample {
            lap_distance_m: 1.0,
            speed_ms: 2.0,
            accel_long_ms2: 3.0,
            track_length_m: 4.0,
            lap_time_s: 5.0,
            frame: 6,
            sim_lap: 7,
            session_type: 8,
            game_state: 9,
            flags: 10,
            reserved: 0,
        }
    }

    #[test]
    fn layout_is_the_contracts() {
        assert_eq!(SAMPLE_SIZE, 32);
        let s = sample();
        let b = s.to_bytes();
        // Layout: `<5fIH2B2H`, little-endian
        assert_eq!(&b[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&b[4..8], &2.0f32.to_le_bytes());
        assert_eq!(&b[8..12], &3.0f32.to_le_bytes());
        assert_eq!(&b[12..16], &4.0f32.to_le_bytes());
        assert_eq!(&b[16..20], &5.0f32.to_le_bytes());
        assert_eq!(&b[20..24], &6u32.to_le_bytes());
        assert_eq!(&b[24..26], &7u16.to_le_bytes());
        assert_eq!((b[26], b[27]), (8, 9));
        assert_eq!(&b[28..30], &10u16.to_le_bytes());
        assert_eq!(&b[30..32], &[0, 0]);
        assert_eq!(Sample::from_bytes(&b), s);
    }

    #[test]
    fn flags_are_distinct_single_bits() {
        let all = [
            ON_TRACK,
            IN_PIT_LANE,
            IN_GARAGE,
            TIMED,
            FLYING,
            LAP_INVALID,
            PLAYER_VIEW,
            FRESH,
        ];
        let mut seen = 0u16;
        for f in all {
            assert_eq!(f.count_ones(), 1);
            assert_eq!(seen & f, 0);
            seen |= f;
        }
        assert_eq!(seen, 0xFF);
        assert_eq!(CHANNEL, "connector.telemetry");
    }

    #[test]
    fn idle_sample_is_unknown_everywhere() {
        let idle = idle_values(5);
        assert_eq!(idle.frame, 5);
        assert_eq!(
            (
                idle.lap_distance_m,
                idle.lap_time_s,
                idle.game_state,
                idle.flags
            ),
            (-1.0, -1.0, 0, 0)
        );
        assert_eq!(
            (
                idle.speed_ms,
                idle.track_length_m,
                idle.sim_lap,
                idle.session_type
            ),
            (0.0, 0.0, 0, 0)
        );
    }

    #[test]
    fn the_frame_counter_keeps_all_its_bits() {
        let s = idle_values(u32::MAX);
        assert_eq!(Sample::from_bytes(&s.to_bytes()).frame, u32::MAX);
    }

    proptest! {
        #[test]
        fn any_bytes_round_trip(bytes in proptest::array::uniform32(any::<u8>())) {
            prop_assert_eq!(Sample::from_bytes(&bytes).to_bytes(), bytes);
        }
    }
}
