//! Dates and times: machine formats (ISO 8601) and the system's regional format for display.
//!
//! Everything the UI shows goes through [`short_datetime`] and [`log_line`] (the user's
//! Windows regional settings); machine formats (log files, JSON files, IPC payloads) stay ISO
//! and never pass through them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// An ISO 8601 text (`2026-10-23T12:00:00+02:00`, `...Z`, `...+0200`, a space instead of the
/// `T`, optional fraction) as a time. Without an offset the text is this PC's local time.
pub fn parse_iso(text: &str) -> Option<SystemTime> {
    parse_iso_with(
        text,
        stintmate_connector_platform::logging::local_offset_minutes(),
    )
}

/// As [`parse_iso`] with the local offset (minutes east of UTC) given.
pub fn parse_iso_with(text: &str, local_offset_min: i32) -> Option<SystemTime> {
    let b = text.trim().as_bytes();
    if b.len() < 19 || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let num = |from: usize, len: usize| -> Option<i64> {
        std::str::from_utf8(b.get(from..from + len)?)
            .ok()?
            .parse()
            .ok()
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, s) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') || b.get(i) == Some(&b',') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let digits = &text.trim()[start..i];
        let frac: String = digits.chars().take(9).collect();
        nanos = format!("{frac:0<9}").parse().ok()?;
    }
    let offset_min = match b.get(i) {
        None => i64::from(local_offset_min),
        Some(b'Z') | Some(b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let rest: String = text.trim()[i + 1..].chars().filter(|c| *c != ':').collect();
            if rest.len() != 4 && rest.len() != 2 {
                return None;
            }
            let hh: i64 = rest.get(0..2)?.parse().ok()?;
            let mm: i64 = if rest.len() == 4 {
                rest.get(2..4)?.parse().ok()?
            } else {
                0
            };
            let v = hh * 60 + mm;
            if sign == b'-' { -v } else { v }
        }
        Some(_) => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset_min * 60;
    let base = if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    };
    Some(base + Duration::from_nanos(u64::from(nanos)))
}

/// Now in UTC as `2026-10-23T12:00:00.123Z` (the `at` of `control.notices`).
pub fn iso_utc_now() -> String {
    iso_utc(SystemTime::now())
}

/// `t` in UTC as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn iso_utc(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(d.as_secs()).unwrap_or(0);
    let (y, m, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// A local wall-clock date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wall {
    /// Year.
    pub year: u16,
    /// Month 1-12.
    pub month: u16,
    /// Day 1-31.
    pub day: u16,
    /// Hour.
    pub hour: u16,
    /// Minute.
    pub minute: u16,
    /// Second.
    pub second: u16,
}

/// The local wall clock of `t`.
pub fn wall_of(t: SystemTime) -> Wall {
    let off = i64::from(stintmate_connector_platform::logging::local_offset_minutes());
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(0),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(0),
    } + off * 60;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    Wall {
        year: y as u16,
        month: m as u16,
        day: d as u16,
        hour: (rem / 3600) as u16,
        minute: (rem % 3600 / 60) as u16,
        second: (rem % 60) as u16,
    }
}

/// `wall` in the short date and the long time (with seconds) of `locale` (a Windows locale
/// name such as `de-DE`; `None` = the user's own regional settings). Falls back to ISO.
#[cfg(windows)]
pub fn short_datetime_in(locale: Option<&str>, wall: Wall) -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx};
    let name: Option<Vec<u16>> = locale.map(|l| l.encode_utf16().chain([0]).collect());
    let name_ptr = name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let st = SYSTEMTIME {
        wYear: wall.year,
        wMonth: wall.month,
        wDayOfWeek: 0,
        wDay: wall.day,
        wHour: wall.hour,
        wMinute: wall.minute,
        wSecond: wall.second,
        wMilliseconds: 0,
    };
    let mut date = [0u16; 80];
    let mut time = [0u16; 80];
    // SAFETY: the buffers are as long as the counts passed; the SYSTEMTIME is valid; the
    // locale name is NUL-terminated or null (the user's default).
    let (nd, nt) = unsafe {
        (
            GetDateFormatEx(
                name_ptr,
                DATE_SHORTDATE,
                &st,
                std::ptr::null(),
                date.as_mut_ptr(),
                date.len() as i32,
                std::ptr::null(),
            ),
            GetTimeFormatEx(
                name_ptr,
                0,
                &st,
                std::ptr::null(),
                time.as_mut_ptr(),
                time.len() as i32,
            ),
        )
    };
    if nd <= 1 || nt <= 1 {
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            wall.year, wall.month, wall.day, wall.hour, wall.minute, wall.second
        );
    }
    format!(
        "{} {}",
        String::from_utf16_lossy(&date[..nd as usize - 1]),
        String::from_utf16_lossy(&time[..nt as usize - 1])
    )
}

/// `t` in the user's regional format (short date, time with seconds).
#[cfg(windows)]
pub fn short_datetime(t: SystemTime) -> String {
    short_datetime_in(None, wall_of(t))
}

/// The start of a log line: `2026-10-13 15:28:37` or `2026-10-13 15:28:37,921` (local time).
fn log_stamp(line: &str) -> Option<(Wall, usize)> {
    let b = line.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |from: usize| -> Option<u16> { line.get(from..from + 2)?.parse().ok() };
    let wall = Wall {
        year: line.get(0..4)?.parse().ok()?,
        month: n(5)?,
        day: n(8)?,
        hour: n(11)?,
        minute: n(14)?,
        second: n(17)?,
    };
    if !(1..=12).contains(&wall.month) || !(1..=31).contains(&wall.day) || wall.hour > 23 {
        return None;
    }
    let mut end = 19;
    if b.get(end) == Some(&b',') {
        end += 1;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    Some((wall, end))
}

/// A log line for display in the regional format of `locale` (`None` = the user's own): its
/// leading stamp replaced; other lines (tracebacks) unchanged. The files and "Copy" keep the
/// original text.
#[cfg(windows)]
pub fn log_line_in(locale: Option<&str>, line: &str) -> String {
    match log_stamp(line) {
        Some((wall, end)) => format!("{}{}", short_datetime_in(locale, wall), &line[end..]),
        None => line.to_string(),
    }
}

/// [`log_line_in`] for the user's own regional settings.
#[cfg(windows)]
pub fn log_line(line: &str) -> String {
    log_line_in(None, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_with_offsets_and_fractions() {
        let t = |s: &str| parse_iso_with(s, 120).unwrap();
        let utc = t("2026-10-23T10:00:00Z");
        assert_eq!(t("2026-10-23T12:00:00+02:00"), utc);
        assert_eq!(t("2026-10-23T12:00:00+0200"), utc);
        assert_eq!(t("2026-10-23 12:00:00"), utc, "naive = local (+2 here)");
        assert_eq!(
            t("2026-10-23T10:00:00.250Z"),
            utc + Duration::from_millis(250)
        );
        assert_eq!(t("2026-10-23T05:00:00-05:00"), utc);
        assert!(parse_iso_with("nope", 0).is_none());
        assert!(parse_iso_with("2026-13-01T00:00:00Z", 0).is_none());
        assert_eq!(
            parse_iso_with("1970-01-01T00:00:00Z", 0),
            Some(UNIX_EPOCH),
            "the epoch"
        );
    }

    #[test]
    fn utc_text_round_trips() {
        let t = parse_iso_with("2026-02-28T23:59:59.5Z", 0).unwrap();
        assert_eq!(iso_utc(t), "2026-02-28T23:59:59.500Z");
        assert_eq!(parse_iso_with(&iso_utc(t), 0), Some(t));
        let leap = parse_iso_with("2028-02-29T00:00:00Z", 0).unwrap();
        assert_eq!(iso_utc(leap), "2028-02-29T00:00:00.000Z");
    }

    #[test]
    fn log_stamps_are_found_and_other_lines_are_left() {
        let (w, end) = log_stamp("2026-10-13 15:28:37,921 INFO x: y").unwrap();
        assert_eq!(
            (w.year, w.month, w.day, w.hour, w.minute, w.second),
            (2026, 10, 13, 15, 28, 37)
        );
        assert_eq!(end, 23);
        assert!(log_stamp("Traceback (most recent call last):").is_none());
        assert!(log_stamp("2026-13-13 15:28:37 INFO").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn the_regional_format_follows_the_locale() {
        let w = Wall {
            year: 2026,
            month: 10,
            day: 13,
            hour: 15,
            minute: 28,
            second: 37,
        };
        let us = short_datetime_in(Some("en-US"), w);
        assert!(us.starts_with("10/13/2026"), "{us}");
        assert!(us.contains("3:28:37"), "{us}");
        let de = short_datetime_in(Some("de-DE"), w);
        assert!(de.starts_with("13.10.2026"), "{de}");
        assert!(de.contains("15:28:37"), "{de}");
        let line = log_line_in(Some("de-DE"), "2026-10-13 15:28:37,921 INFO example: hello");
        assert!(
            line.starts_with("13.10.2026 15:28:37 INFO example: hello"),
            "{line}"
        );
        assert_eq!(
            log_line_in(Some("de-DE"), "  at file.rs:1"),
            "  at file.rs:1"
        );
    }
}
