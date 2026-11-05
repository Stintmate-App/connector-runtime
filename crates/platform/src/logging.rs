//! Logging: `tracing` to `<data dir>\<name>.log`, non-blocking, with a size cap.
//!
//! The line format is
//! (`%(asctime)s %(levelname)s %(name)s: %(message)s`):
//! `2026-10-23 14:05:09,123 INFO example_core::tracker: message key=value`.
//! The name is the event's target (the module path). Time is local, to the millisecond.
//!
//! The file is capped ([`DEFAULT_MAX_BYTES`]): when a
//! line would pass the cap, `<name>.log` becomes `<name>.log.1` (replacing the older one) and a
//! new file starts, so the logs take at most twice the cap.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing::{Event, Level, Subscriber};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// The size at which the log file rotates: 5 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// A local wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Year.
    pub year: u16,
    /// Month 1-12.
    pub month: u16,
    /// Day 1-31.
    pub day: u16,
    /// Hour 0-23.
    pub hour: u16,
    /// Minute.
    pub minute: u16,
    /// Second.
    pub second: u16,
    /// Millisecond.
    pub millis: u16,
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

/// The current local time.
#[cfg(windows)]
pub fn now_local() -> Stamp {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    // SAFETY: `t` is a valid out pointer.
    unsafe { GetLocalTime(&mut t) };
    Stamp {
        year: t.wYear,
        month: t.wMonth,
        day: t.wDay,
        hour: t.wHour,
        minute: t.wMinute,
        second: t.wSecond,
        millis: t.wMilliseconds,
    }
}

/// The current local time (non-Windows builds only exist for tooling: UTC-less zeros).
#[cfg(not(windows))]
pub fn now_local() -> Stamp {
    Stamp {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        millis: 0,
    }
}

/// The level names written in the log lines.
pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARNING",
        Level::INFO => "INFO",
        Level::DEBUG | Level::TRACE => "DEBUG",
    }
}

/// Formats an event as one line in the format above.
#[derive(Debug, Clone, Copy)]
pub struct LineFormat {
    /// The clock (a function so tests can fix it).
    pub now: fn() -> Stamp,
}

impl Default for LineFormat {
    fn default() -> Self {
        Self { now: now_local }
    }
}

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        write!(
            writer,
            "{} {} {}: ",
            (self.now)(),
            level_name(*meta.level()),
            meta.target()
        )?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// A log file that rotates to `<file>.1` when a write would pass `max_bytes`.
#[derive(Debug)]
pub struct CappedFile {
    path: PathBuf,
    max_bytes: u64,
    file: Option<File>,
    size: u64,
}

impl CappedFile {
    /// Open (append) `path`, creating its folder.
    pub fn open(path: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            max_bytes,
            file: Some(file),
            size,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None; // close before renaming
        let mut old = self.path.clone().into_os_string();
        old.push(".1");
        let renamed = fs::rename(&self.path, &old);
        let fresh = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path);
        // If the rename failed (a reader holds the file), keep appending: a log that
        // outgrows the cap is better than a lost line.
        let file = fresh?;
        self.size = if renamed.is_ok() {
            0
        } else {
            file.metadata()?.len()
        };
        self.file = Some(file);
        Ok(())
    }
}

impl Write for CappedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file closed"))?;
        let n = file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}

/// File and, optionally, stderr. Stderr errors are ignored: a
/// windowless process has no valid stderr.
struct Tee {
    file: Option<CappedFile>,
    stderr: bool,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.stderr {
            let _ = io::stderr().write_all(buf);
        }
        match &mut self.file {
            Some(f) => f.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}

/// Keeps the logger's worker alive and flushes it on drop: hold it until the process ends.
#[derive(Debug)]
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Logging options.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// The folder (the connector's data folder); created if missing.
    pub dir: PathBuf,
    /// File name without extension: `recorder` gives `recorder.log`.
    pub name: String,
    /// Size at which the file rotates.
    pub max_bytes: u64,
    /// Also write to stderr (the recorder in a console).
    pub stderr: bool,
    /// Maximum level.
    pub level: Level,
}

impl LogConfig {
    /// `<dir>\<name>.log`, INFO, capped at [`DEFAULT_MAX_BYTES`], no stderr.
    pub fn new(dir: impl Into<PathBuf>, name: &str) -> Self {
        Self {
            dir: dir.into(),
            name: name.into(),
            max_bytes: DEFAULT_MAX_BYTES,
            stderr: false,
            level: Level::INFO,
        }
    }

    /// The log file's path.
    pub fn file_path(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.name))
    }
}

/// The non-blocking writer for `config`, and its guard. If the file cannot be opened the
/// logger still runs (stderr only if asked, else nowhere): a missing log never stops the app.
pub fn writer(config: &LogConfig) -> (NonBlocking, WorkerGuard) {
    let file = CappedFile::open(config.file_path(), config.max_bytes).ok();
    tracing_appender::non_blocking(Tee {
        file,
        stderr: config.stderr,
    })
}

/// A subscriber writing lines in the format above to `make_writer`.
pub fn subscriber<W>(
    make_writer: W,
    level: Level,
    format: LineFormat,
) -> impl Subscriber + Send + Sync
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(make_writer)
        .event_format(format)
        .finish()
}

/// Install the process-wide logger and a panic hook that logs the panic. Errors only if a
/// global subscriber is already set.
pub fn init(config: &LogConfig) -> Result<LogGuard, tracing::subscriber::SetGlobalDefaultError> {
    let (non_blocking, worker) = writer(config);
    tracing::subscriber::set_global_default(subscriber(
        non_blocking,
        config.level,
        LineFormat::default(),
    ))?;
    install_panic_hook();
    Ok(LogGuard { _worker: worker })
}

/// Log panics as ERROR lines (then run the previous hook).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        previous(info);
    }));
}

/// The log file of `dir`/`name`, for callers that show or read it.
pub fn log_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.log"))
}

/// The local time as ISO 8601 with the UTC offset (`2026-10-23T14:05:09+02:00`), with
/// milliseconds when `millis` (`...09.123+02:00`): the format of the write API's timestamps.
pub fn local_iso(millis: bool) -> String {
    let t = now_local();
    let off = local_offset_minutes();
    let sign = if off < 0 { '-' } else { '+' };
    let (h, m) = (off.abs() / 60, off.abs() % 60);
    let ms = if millis {
        format!(".{:03}", t.millis)
    } else {
        String::new()
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{ms}{sign}{h:02}:{m:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// Minutes east of UTC of the local time zone now.
#[cfg(windows)]
pub fn local_offset_minutes() -> i32 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // SAFETY: all-zero is a valid TIME_ZONE_INFORMATION; the out pointer is valid.
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.Bias
        + match id {
            1 => tz.StandardBias,
            2 => tz.DaylightBias,
            _ => 0,
        };
    -bias
}

/// Minutes east of UTC (non-Windows builds only exist for tooling: UTC).
#[cfg(not(windows))]
pub fn local_offset_minutes() -> i32 {
    0
}
