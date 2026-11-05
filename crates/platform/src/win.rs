//! Small Win32 plumbing shared by the modules: owned handles, errors, wide strings.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};

/// A failed Win32 call: what was called and `GetLastError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{call} failed with Win32 error {code}")]
pub struct Win32Error {
    /// The API that failed.
    pub call: &'static str,
    /// The `GetLastError` code.
    pub code: u32,
}

impl Win32Error {
    /// The error of the last Win32 call on this thread.
    pub fn last(call: &'static str) -> Self {
        // SAFETY: no preconditions.
        Self {
            call,
            code: unsafe { GetLastError() },
        }
    }
}

/// A kernel handle closed on drop. Handles are not inheritable unless said otherwise.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

// SAFETY: kernel handles may be used and closed from any thread (thread-affine objects such
// as mutex ownership are documented where they apply).
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// Takes ownership; `None` for a null handle.
    pub fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }

    /// The raw handle (still owned by `self`).
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A NUL-terminated UTF-16 copy of `s`.
pub(crate) fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}
