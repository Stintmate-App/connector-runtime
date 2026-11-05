//! Which window is the game window (`connector.capture_target`).
//!
//! Pitwall's core grabs the window this names; the only game-specific part is which window it
//! is. The game's window is found by process name first (`GameSpec::exes`), else, when the spec
//! names both, by exact title and class (so an Explorer or browser window with that title never
//! matches); the largest client area wins. Passive: nothing is sent to,
//! activated or moved. [`list_windows`] reads titles, so tests restrict it to windows of their
//! own process with [`Scope::OwnProcess`].

use crate::proc::process_image;
use crate::spec::GameSpec;
use crate::win::OwnedHandle;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetClientRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible,
};

/// One top-level window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// The handle, as the number Pitwall uses.
    pub hwnd: i64,
    /// Owner process.
    pub pid: u32,
    /// Owner image file name, lower case ("" when unknown).
    pub exe: String,
    /// Window title.
    pub title: String,
    /// Window class.
    pub class: String,
    /// Client area in pixels (0 while minimized).
    pub area: i64,
    /// The window is minimized.
    pub minimized: bool,
}

/// Which windows [`list_windows`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every visible top-level window (production).
    AllVisible,
    /// Only windows of this process, visible or not (tests: nothing else is read).
    OwnProcess,
}

struct Ctx {
    scope: Scope,
    me: u32,
    out: Vec<WindowInfo>,
}

unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: `lparam` is the `&mut Ctx` passed to EnumWindows below, alive for the call.
    let ctx = unsafe { &mut *(lparam as *mut Ctx) };
    let mut pid = 0u32;
    // SAFETY: plain calls on a window handle; failures give empty values.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    match ctx.scope {
        Scope::OwnProcess if pid != ctx.me => return 1,
        Scope::AllVisible if unsafe { IsWindowVisible(hwnd) } == 0 => return 1,
        _ => {}
    }
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let (mut title, mut class) = ([0u16; 512], [0u16; 256]);
    // SAFETY: the buffers are as long as the lengths given.
    let (tl, cl, minimized) = unsafe {
        GetClientRect(hwnd, &mut r);
        (
            GetWindowTextW(hwnd, title.as_mut_ptr(), 512),
            GetClassNameW(hwnd, class.as_mut_ptr(), 256),
            IsIconic(hwnd) != 0,
        )
    };
    ctx.out.push(WindowInfo {
        hwnd: hwnd as isize as i64,
        pid,
        exe: exe_of(pid),
        title: String::from_utf16_lossy(&title[..tl.max(0) as usize]),
        class: String::from_utf16_lossy(&class[..cl.max(0) as usize]),
        area: i64::from(r.right) * i64::from(r.bottom),
        minimized,
    });
    1
}

fn exe_of(pid: u32) -> String {
    // SAFETY: plain call; a null handle (no access) is handled by `OwnedHandle::new`.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    OwnedHandle::new(h)
        .and_then(|h| process_image(h.raw()))
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// The top-level windows of `scope` (EnumWindows; a few ms).
pub fn list_windows(scope: Scope) -> Vec<WindowInfo> {
    let mut ctx = Ctx {
        scope,
        // SAFETY: no preconditions.
        me: unsafe { GetCurrentProcessId() },
        out: Vec::new(),
    };
    // SAFETY: the callback only uses `ctx`, which outlives the call.
    unsafe { EnumWindows(Some(each), &mut ctx as *mut Ctx as *mut c_void as LPARAM) };
    ctx.out
}

/// The game's window among `windows`: by process name first, else by exact title and class
/// (only when the spec has both); the largest client area wins (the handle breaks ties).
pub fn find_game_window<'a>(game: &GameSpec, windows: &'a [WindowInfo]) -> Option<&'a WindowInfo> {
    let by_title = game.window_title.zip(game.window_class);
    windows
        .iter()
        .filter_map(|w| {
            if game.exes.contains(&w.exe.as_str()) {
                Some((0, -w.area, w.hwnd, w))
            } else if by_title.is_some_and(|(title, class)| {
                w.title.trim().to_lowercase() == title && w.class == class
            }) {
                Some((1, -w.area, w.hwnd, w))
            } else {
                None
            }
        })
        .min_by_key(|t| (t.0, t.1, t.2))
        .map(|t| t.3)
}
