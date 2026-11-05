//! The wire formats of a Stintmate connector, free of any simulator, Windows or IPC code.
//!
//! * [`messages`]: the write API messages a connector sends, as typed structs that serialize to
//!   the params of their IPC write methods.
//! * [`telemetry`]: the 32-byte `connector.telemetry` v1 sample and its flags.
//! * [`catalog`]: the announcement text catalog format: validation, canonical JSON, hashes and
//!   the typed parameters of a message ([`catalog::AnnounceCatalog`]).
//! * [`rate`]: the sliding window of the announcement rate limit.
//! * [`actions`]: the two standard actions every connector declares.
//! * [`schema_check`]: a small JSON Schema checker for tests, and the two schemas the formats
//!   above are checked against.
//!
//! Nothing here reads a clock or a file: time and data come in from the caller.

#![deny(missing_docs)]

pub mod actions;
pub mod catalog;
pub mod messages;
pub mod rate;
pub mod schema_check;
pub mod telemetry;

/// Python-compatible `round(x, ndigits)`: correctly rounded, ties to even on the exact value.
pub fn round_to(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

/// A NUL-terminated `char[]` field as text (invalid UTF-8 replaced); borrows when it can.
pub fn decode_str(raw: &[u8]) -> std::borrow::Cow<'_, str> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_python() {
        assert_eq!(round_to(2.675, 2), 2.67); // the exact value is below the tie
        assert_eq!(round_to(0.5, 0), 0.0);
        assert_eq!(round_to(1.5, 0), 2.0);
        assert_eq!(round_to(25.5, 2), 25.5);
        assert!(round_to(f64::NAN, 2).is_nan());
        assert_eq!(round_to(f64::INFINITY, 1), f64::INFINITY);
    }

    #[test]
    fn decode_stops_at_nul() {
        assert_eq!(decode_str(b"abc\0def"), "abc");
        assert_eq!(decode_str(b"abc"), "abc");
        assert_eq!(decode_str(b"\0"), "");
        assert_eq!(decode_str(b"a\xffb\0"), "a\u{fffd}b");
    }
}
