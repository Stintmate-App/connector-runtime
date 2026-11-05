//! A native message box, for what must be said before any window exists (the workspace error).

use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

/// Shows a Yes/No question under `title`; true for Yes. Not called in tests.
pub fn ask(title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (t, m) = (w(title), w(text));
    // SAFETY: NUL-terminated strings; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            m.as_ptr(),
            t.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

/// Shows `text` under `title` and waits for OK. Not called when the process was started by
/// another app that reads the exit status (`--managed`) or in tests.
pub fn error(title: &str, text: &str) {
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (t, m) = (w(title), w(text));
    // SAFETY: NUL-terminated strings; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            m.as_ptr(),
            t.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
