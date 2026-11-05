//! Log line format, rotation and the non-blocking file writer.

use std::io::Write;
use std::sync::{Arc, Mutex};
use stintmate_connector_platform::logging::{self, CappedFile, LineFormat, LogConfig, Stamp};
use tracing::Level;

const FIXED: Stamp = Stamp {
    year: 2026,
    month: 10,
    day: 23,
    hour: 14,
    minute: 5,
    second: 9,
    millis: 7,
};

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fixed_clock() -> Stamp {
    FIXED
}

#[test]
fn the_line_format_is_the_documented_one() {
    let buf = Buf::default();
    let b2 = buf.clone();
    let sub = logging::subscriber(
        move || b2.clone(),
        Level::INFO,
        LineFormat { now: fixed_clock },
    );
    tracing::subscriber::with_default(sub, || {
        tracing::info!(target: "example_core::tracker", "lap {} done", 3);
        tracing::warn!("careful");
        tracing::error!(code = 7, "failed");
        tracing::debug!("not shown at INFO");
    });
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(
        lines[0],
        "2026-10-23 14:05:09,007 INFO example_core::tracker: lap 3 done"
    );
    assert!(
        lines[1].starts_with("2026-10-23 14:05:09,007 WARNING "),
        "{}",
        lines[1]
    );
    assert!(lines[1].ends_with(": careful"));
    assert!(
        lines[2].contains(" ERROR ") && lines[2].ends_with(": failed code=7"),
        "{}",
        lines[2]
    );
}

#[test]
fn the_real_clock_gives_a_plausible_local_time() {
    let s = logging::now_local();
    assert!(s.year >= 2026 && (1..=12).contains(&s.month) && s.hour < 24 && s.millis < 1000);
}

#[test]
fn the_file_rotates_at_the_cap_and_keeps_one_old_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("t.log");
    let mut f = CappedFile::open(&path, 100).unwrap();
    for i in 0..10 {
        // One write per line, as the formatter does (28 bytes each).
        f.write_all(
            format!(
                "line {i:02} {}
",
                "x".repeat(20)
            )
            .as_bytes(),
        )
        .unwrap();
    }
    f.flush().unwrap();
    let old = std::fs::read_to_string(dir.path().join("sub").join("t.log.1")).unwrap();
    let cur = std::fs::read_to_string(&path).unwrap();
    assert!(
        old.len() <= 100 && cur.len() <= 100,
        "{} {}",
        old.len(),
        cur.len()
    );
    assert!(cur.contains("line 09") && !cur.contains("line 00"));
    assert!(!dir.path().join("sub").join("t.log.2").exists());
    // Reopening appends and counts the existing size.
    drop(f);
    let mut f = CappedFile::open(&path, 100).unwrap();
    writeln!(f, "again").unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().ends_with("again\n"));
}

#[test]
fn non_blocking_writer_writes_the_file_and_flushes_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = LogConfig::new(dir.path().join("data"), "recorder");
    let (nb, guard) = logging::writer(&cfg);
    let sub = logging::subscriber(nb, Level::INFO, LineFormat::default());
    tracing::subscriber::with_default(sub, || {
        for i in 0..200 {
            tracing::info!("event {i}");
        }
    });
    drop(guard); // flushes and joins the worker
    let text = std::fs::read_to_string(cfg.file_path()).unwrap();
    assert_eq!(text.lines().count(), 200);
    assert!(text.lines().last().unwrap().ends_with("event 199"));
}

#[test]
fn an_unwritable_log_never_stops_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "x").unwrap();
    // The "folder" is a file: the log file cannot be created.
    let cfg = LogConfig::new(blocker.join("data"), "ui");
    let (nb, guard) = logging::writer(&cfg);
    let sub = logging::subscriber(nb, Level::INFO, LineFormat::default());
    tracing::subscriber::with_default(sub, || tracing::info!("nowhere to go"));
    drop(guard);
}
