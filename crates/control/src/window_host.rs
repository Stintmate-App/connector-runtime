//! The window process's life, seen from the control app: at most one
//! window executable at a time, started on demand and gone when its window closes.
//!
//! A second "Open" while it runs starts nothing: the control app publishes the request in
//! `ui.snapshot` (`window_request`), which the window process watches, and this host only lets
//! the window take the foreground (`AllowSetForegroundWindow`).

use std::io;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;

/// What [`WindowHost::ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new window process (pid).
    Started(u32),
    /// One was already running (pid): nothing started.
    Running(u32),
}

/// Starts and watches the window process.
pub struct WindowHost {
    exe: PathBuf,
    args: Vec<String>,
    child: Mutex<Option<Child>>,
}

impl WindowHost {
    /// A host that runs `exe` with `args` (the workspace arguments and test overrides).
    pub fn new(exe: PathBuf, args: Vec<String>) -> Self {
        Self {
            exe,
            args,
            child: Mutex::new(None),
        }
    }

    /// The window executable next to the control app's.
    pub fn sibling_exe(
        spec: &stintmate_connector_platform::spec::ConnectorSpec,
        ui_exe: &std::path::Path,
    ) -> PathBuf {
        ui_exe.with_file_name(spec.window_exe())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The pid of the running window process, if one runs.
    pub fn pid(&self) -> Option<u32> {
        let mut g = self.lock();
        match g.as_mut()?.try_wait() {
            Ok(None) => g.as_ref().map(Child::id),
            _ => None,
        }
    }

    /// Makes sure a window process runs, on `page` (`logs`) when it starts one.
    pub fn ensure(&self, page: Option<&str>) -> io::Result<Outcome> {
        if let Some(pid) = self.pid() {
            allow_foreground(pid);
            return Ok(Outcome::Running(pid));
        }
        let mut cmd = Command::new(&self.exe);
        cmd.args(&self.args);
        if let Some(p) = page {
            cmd.args(["--page", p]);
        }
        let child = cmd.spawn()?;
        let pid = child.id();
        allow_foreground(pid);
        *self.lock() = Some(child);
        Ok(Outcome::Started(pid))
    }

    /// True once after the window process has ended (closed by the user or gone).
    pub fn poll_exit(&self) -> bool {
        let mut g = self.lock();
        match g.as_mut().map(Child::try_wait) {
            Some(Ok(Some(_))) | Some(Err(_)) => {
                *g = None;
                true
            }
            _ => false,
        }
    }

    /// Ends the window process (the control app is quitting).
    pub fn close(&self) {
        if let Some(mut c) = self.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for WindowHost {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(windows)]
fn allow_foreground(pid: u32) {
    // SAFETY: no pointers; a pid that is gone just fails.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
}

#[cfg(not(windows))]
fn allow_foreground(_pid: u32) {}
