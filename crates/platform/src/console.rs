//! Output of a GUI-subsystem exe run from a console.
//!
//! A connector's control app is a Windows-subsystem program (no console window of its own), so
//! its `println!` goes nowhere by default. [`attach_parent_output`] makes the command-line
//! options (`--version`, `--register`, `--unregister`, `--quit`, usage errors) print into the
//! console that started it.

use crate::win::{OwnedHandle, wide};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

fn unset(which: STD_HANDLE) -> bool {
    // SAFETY: no preconditions.
    let h = unsafe { GetStdHandle(which) };
    h.is_null() || h == INVALID_HANDLE_VALUE
}

/// Attaches to the parent process's console and points standard output and error at it, but
/// only for a stream that is not already redirected (to a file or a pipe, as a test or a
/// script does): that one keeps working as it is. Without a parent console (started from
/// Explorer or as a service) nothing changes. Returns whether output now reaches a console.
///
/// A shell does not wait for a GUI-subsystem program: from `cmd` use `start /wait`, from
/// PowerShell `Start-Process -Wait` or pipe the output (`| Write-Output`), and the exit status
/// is in `%ERRORLEVEL%` / `$LASTEXITCODE` only then.
pub fn attach_parent_output() -> bool {
    let need_out = unset(STD_OUTPUT_HANDLE);
    let need_err = unset(STD_ERROR_HANDLE);
    if !(need_out || need_err) {
        return true;
    }
    // SAFETY: no preconditions; fails when the parent has no console.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return false;
    }
    let name = wide("CONOUT$");
    // SAFETY: `name` is NUL-terminated. The handle is deliberately leaked into the standard
    // handle slots for the life of the process.
    let con = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    let Some(con) = OwnedHandle::new(con).filter(|h| h.raw() != INVALID_HANDLE_VALUE) else {
        return false;
    };
    let raw = con.raw();
    std::mem::forget(con);
    for (needed, which) in [(need_out, STD_OUTPUT_HANDLE), (need_err, STD_ERROR_HANDLE)] {
        if needed {
            // SAFETY: `raw` is a valid console handle that lives as long as the process.
            unsafe { SetStdHandle(which, raw) };
        }
    }
    true
}

static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(kind: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
        INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
        1
    } else {
        0
    }
}

/// Installs a Ctrl+C / Ctrl+Break handler that sets the returned flag instead of ending the
/// process, so a long-running command can finish cleanly. Installing twice returns the same flag.
pub fn interrupt_flag() -> &'static std::sync::atomic::AtomicBool {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        // SAFETY: `on_ctrl` is a valid handler for the life of the process.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), 1) };
    });
    &INTERRUPTED
}
