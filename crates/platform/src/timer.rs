//! A high-resolution waitable timer for the 50 Hz recorder loop.
//!
//! `CreateWaitableTimerExW` with `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` (Windows 10 1803+)
//! wakes much closer to the due time than `Sleep` at the default 15.6 ms tick, without raising
//! the system-wide timer resolution. Windows older than that refuse the flag; the timer then
//! falls back to a normal one ([`WaitableTimer::is_high_res`] says which).

use crate::win::{OwnedHandle, Win32Error};
use std::time::Duration;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW, INFINITE,
    SetWaitableTimer, WaitForSingleObject,
};

const TIMER_ALL_ACCESS: u32 = 0x001F_0003;

/// A waitable timer, periodic or one-shot. Cancelled and closed on drop.
#[derive(Debug)]
pub struct WaitableTimer {
    handle: OwnedHandle,
    high_res: bool,
}

impl WaitableTimer {
    /// Create an auto-reset timer, high resolution when the system supports it.
    pub fn new() -> Result<Self, Win32Error> {
        for (flags, high_res) in [(CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, true), (0, false)] {
            // SAFETY: null attributes and name; the result is checked.
            let raw = unsafe {
                CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), flags, TIMER_ALL_ACCESS)
            };
            if let Some(handle) = OwnedHandle::new(raw) {
                return Ok(Self { handle, high_res });
            }
        }
        Err(Win32Error::last("CreateWaitableTimerExW"))
    }

    /// True if the timer has the high-resolution flag (otherwise it is a normal timer).
    pub fn is_high_res(&self) -> bool {
        self.high_res
    }

    fn set(&self, first: Duration, period_ms: i32) -> Result<(), Win32Error> {
        // Negative = relative, in 100 ns units.
        let due = -i64::try_from(first.as_nanos() / 100)
            .unwrap_or(i64::MAX)
            .max(1);
        // SAFETY: valid handle and due-time pointer; no completion routine.
        let ok = unsafe {
            SetWaitableTimer(
                self.handle.raw(),
                &due,
                period_ms,
                None,
                std::ptr::null(),
                0,
            )
        };
        if ok == 0 {
            Err(Win32Error::last("SetWaitableTimer"))
        } else {
            Ok(())
        }
    }

    /// Fire every `period` (whole milliseconds, at least 1), first one `period` from now.
    pub fn start_periodic(&self, period: Duration) -> Result<(), Win32Error> {
        let ms = i32::try_from(period.as_millis().max(1)).unwrap_or(i32::MAX);
        self.set(period, ms)
    }

    /// Fire once, `after` from now.
    pub fn start_once(&self, after: Duration) -> Result<(), Win32Error> {
        self.set(after, 0)
    }

    /// Stop firing.
    pub fn cancel(&self) {
        // SAFETY: valid handle.
        unsafe { CancelWaitableTimer(self.handle.raw()) };
    }

    /// Block until the timer fires.
    pub fn wait(&self) {
        // SAFETY: valid handle.
        unsafe { WaitForSingleObject(self.handle.raw(), INFINITE) };
    }

    /// Block until the timer fires or `timeout` passes; true if it fired.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid handle.
        unsafe { WaitForSingleObject(self.handle.raw(), ms) == WAIT_OBJECT_0 }
    }

    /// Sleep for `d` using the timer (a one-shot).
    pub fn sleep(&self, d: Duration) -> Result<(), Win32Error> {
        self.start_once(d)?;
        self.wait();
        Ok(())
    }
}

impl Drop for WaitableTimer {
    fn drop(&mut self) {
        self.cancel();
    }
}
