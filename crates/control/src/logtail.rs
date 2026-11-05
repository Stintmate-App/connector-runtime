//! Incremental tail of the connector's logs (`recorder.log`, `ui.log`) for the Logs pane.
//!
//! [`LogTail::open`] reads the last 64 KB and keeps the last 500 lines; [`LogTail::poll`] then
//! reads only the bytes appended since. A file that got smaller, or a different file at the
//! same path (replaced), is read again from its end. A line still being written (no newline
//! yet) is held back until it is complete.
//!
//! The log line is `YYYY-MM-DD HH:MM:SS,mmm LEVEL target: message`, so the level is the third
//! space-separated field; lines without one (traceback lines) belong to the entry above.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How much of the end of the file the first read takes.
pub const INITIAL_BYTES: u64 = 64 * 1024;
/// How many lines are kept.
pub const MAX_LINES: usize = 500;
/// The levels of a log line.
pub const LEVELS: [&str; 5] = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"];

/// The level column of a log line, or `None` for a continuation line.
pub fn line_level(line: &str) -> Option<&str> {
    let mut parts = line.splitn(4, ' ');
    let level = parts.nth(2)?;
    LEVELS.contains(&level).then_some(level)
}

fn is_alert(level: &str) -> bool {
    matches!(level, "WARNING" | "ERROR" | "CRITICAL")
}

/// Keeps warning and error entries with their continuation lines. Stateful, so it can be fed
/// the lines of a growing log in order.
#[derive(Debug, Default)]
pub struct LevelFilter {
    keep: bool,
}

impl LevelFilter {
    /// True if the line belongs to a kept entry.
    pub fn accept(&mut self, line: &str) -> bool {
        if let Some(level) = line_level(line) {
            self.keep = is_alert(level);
        }
        self.keep
    }

    /// The lines that pass.
    pub fn apply<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
        lines.into_iter().filter(|l| self.accept(l)).collect()
    }
}

/// What a poll found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    /// The whole view must be rebuilt from [`LogTail::lines`].
    pub reset: bool,
    /// New complete lines.
    pub new: Vec<String>,
}

type Ident = u64;

/// The tail of one log file.
#[derive(Debug)]
pub struct LogTail {
    path: PathBuf,
    max_lines: usize,
    initial_bytes: u64,
    /// The lines kept, oldest first.
    pub lines: VecDeque<String>,
    /// The file exists.
    pub exists: bool,
    pos: u64,
    ident: Option<Ident>,
    partial: Vec<u8>,
}

fn stat(path: &Path) -> Option<(u64, Ident)> {
    let md = std::fs::metadata(path).ok()?;
    #[cfg(windows)]
    let ident = std::os::windows::fs::MetadataExt::creation_time(&md);
    #[cfg(not(windows))]
    let ident = 0;
    Some((md.len(), ident))
}

impl LogTail {
    /// A tail of `path` with the default limits (nothing read yet).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_limits(path, MAX_LINES, INITIAL_BYTES)
    }

    /// A tail with explicit limits.
    pub fn with_limits(path: impl Into<PathBuf>, max_lines: usize, initial_bytes: u64) -> Self {
        Self {
            path: path.into(),
            max_lines,
            initial_bytes,
            lines: VecDeque::new(),
            exists: false,
            pos: 0,
            ident: None,
            partial: Vec::new(),
        }
    }

    /// (Re)reads the end of the file.
    pub fn open(&mut self) -> Update {
        self.lines.clear();
        self.partial.clear();
        self.pos = 0;
        let Some((size, ident)) = stat(&self.path) else {
            self.exists = false;
            self.ident = None;
            return Update {
                reset: true,
                new: Vec::new(),
            };
        };
        self.exists = true;
        self.ident = Some(ident);
        let start = size.saturating_sub(self.initial_bytes);
        let mut data = self.read(start);
        if start > 0 {
            // Drop the cut first line.
            data = match data.iter().position(|b| *b == b'\n') {
                Some(nl) => data[nl + 1..].to_vec(),
                None => Vec::new(),
            };
        }
        self.consume(data);
        Update {
            reset: true,
            new: Vec::new(),
        }
    }

    /// New complete lines since the last call (or a reset).
    pub fn poll(&mut self) -> Update {
        let Some((size, ident)) = stat(&self.path) else {
            if self.exists {
                return self.open(); // deleted
            }
            return Update::default();
        };
        if !self.exists || self.ident != Some(ident) || size < self.pos {
            return self.open(); // created, replaced or truncated
        }
        if size == self.pos {
            return Update::default();
        }
        let data = self.read(self.pos);
        Update {
            reset: false,
            new: self.consume(data),
        }
    }

    fn read(&mut self, start: u64) -> Vec<u8> {
        let mut data = Vec::new();
        let ok = File::open(&self.path)
            .and_then(|mut f| {
                f.seek(SeekFrom::Start(start))?;
                f.read_to_end(&mut data)
            })
            .is_ok();
        if !ok {
            return Vec::new();
        }
        self.pos = start + data.len() as u64;
        data
    }

    fn consume(&mut self, data: Vec<u8>) -> Vec<String> {
        let mut all = std::mem::take(&mut self.partial);
        all.extend(data);
        let mut parts: Vec<&[u8]> = all.split(|b| *b == b'\n').collect();
        let tail = parts.pop().unwrap_or_default().to_vec();
        let new: Vec<String> = parts
            .iter()
            .map(|raw| {
                let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
                String::from_utf8_lossy(raw).into_owned()
            })
            .collect();
        self.partial = tail;
        for line in &new {
            if self.lines.len() == self.max_lines {
                self.lines.pop_front();
            }
            self.lines.push_back(line.clone());
        }
        let skip = new.len().saturating_sub(self.max_lines);
        new.into_iter().skip(skip).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn levels_and_the_alert_filter_keep_continuations() {
        assert_eq!(
            line_level("2026-10-13 15:28:37,921 INFO x: y"),
            Some("INFO")
        );
        assert_eq!(line_level("  File \"a.py\", line 3"), None);
        assert_eq!(line_level("2026-10-13 15:28:37 ERROR x: y"), Some("ERROR"));
        let lines = [
            "2026-10-13 15:28:37 INFO a: one",
            "2026-10-13 15:28:38 ERROR a: two",
            "  trace line",
            "2026-10-13 15:28:39 INFO a: three",
            "2026-10-13 15:28:40 WARNING a: four",
        ];
        let kept = LevelFilter::default().apply(lines);
        assert_eq!(kept, vec![lines[1], lines[2], lines[4]]);
    }

    #[test]
    fn open_reads_the_end_and_poll_only_the_new_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        let mut tail = LogTail::with_limits(&path, 3, 1024);
        assert!(tail.open().reset);
        assert!(!tail.exists);
        append(&path, "a\nb\nc\nd\n");
        let up = tail.poll();
        assert!(up.reset, "created");
        assert_eq!(tail.lines, ["b", "c", "d"], "the last three");
        append(&path, "e\r\nhalf");
        assert_eq!(tail.poll().new, ["e"]);
        append(&path, " done\n");
        assert_eq!(tail.poll().new, ["half done"]);
        assert_eq!(tail.lines, ["d", "e", "half done"]);
        assert_eq!(tail.poll(), Update::default());
    }

    #[test]
    fn a_cut_first_line_is_dropped_and_a_truncated_file_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        append(&path, "aaaa\nbbbb\ncccc\n");
        let mut tail = LogTail::with_limits(&path, 10, 8);
        tail.open();
        assert_eq!(
            tail.lines,
            ["cccc"],
            "last 8 bytes = 'bbb\\ncccc\\n' minus the cut line"
        );
        std::fs::write(&path, "z\n").unwrap();
        let up = tail.poll();
        assert!(up.reset);
        assert_eq!(tail.lines, ["z"]);
        std::fs::remove_file(&path).unwrap();
        assert!(tail.poll().reset);
        assert!(!tail.exists);
    }
}
