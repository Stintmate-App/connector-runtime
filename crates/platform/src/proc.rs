//! Process helpers: image name and start time, guarded termination, the kill-on-close job
//! object and spawning a child suspended (assign it to the job, then resume it, so it cannot
//! start anything outside the job first).
//!
//! Everything that acts on a PID checks the process's image (and optionally that it started
//! before a given time), which guards against PID reuse.

// A HANDLE is an opaque kernel value, never dereferenced by us; passing a stale one only
// makes the call fail.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::win::{OwnedHandle, Win32Error, wide};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW,
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

const SYNCHRONIZE: u32 = 0x0010_0000;
const STILL_ACTIVE: u32 = 259;
/// 1970-01-01 in 100 ns units since 1601.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
/// Status timestamps have 1 s resolution.
const START_TOLERANCE: Duration = Duration::from_secs(2);

/// Full path of the executable of a process handle (needs `QUERY_LIMITED_INFORMATION`).
pub fn process_image(handle: HANDLE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` holds `size` u16s.
    if unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(
        &buf[..size as usize],
    )))
}

/// Creation time of a process handle.
pub fn process_start_time(handle: HANDLE) -> Option<SystemTime> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: four valid out pointers.
    if unsafe { GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let ft = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let since_epoch = ft.checked_sub(FILETIME_UNIX_EPOCH)?;
    Some(UNIX_EPOCH + Duration::from_nanos(since_epoch * 100))
}

/// CPU time (kernel plus user) a process has used so far, or `None` if it cannot be opened
/// (gone, or another user's). Two readings a second apart give its CPU use.
pub fn process_cpu_time(pid: u32) -> Option<Duration> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; the result is checked.
    let h = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: four valid out pointers.
    if unsafe { GetProcessTimes(h.raw(), &mut created, &mut exit, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Some(Duration::from_nanos((ticks(kernel) + ticks(user)) * 100))
}
/// A comparable form of a path: final path (junctions resolved), no `\\?\`, lowercase.
fn norm(path: &Path) -> String {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = real.to_string_lossy().into_owned();
    s.strip_prefix("\\\\?\\").unwrap_or(&s).to_lowercase()
}

/// Open `pid` with `access` if its image is one of `allowed_images` and, when given, it was
/// created before `started_before` (e.g. the start time the service reported with its PID).
fn open_checked(
    pid: u32,
    access: u32,
    allowed_images: &[&Path],
    started_before: Option<SystemTime>,
) -> Option<OwnedHandle> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; the result is checked.
    let h = OwnedHandle::new(unsafe {
        OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
    })?;
    let image = process_image(h.raw())?;
    let image = norm(&image);
    if !allowed_images.iter().any(|a| norm(a) == image) {
        return None;
    }
    if let Some(limit) = started_before {
        let created = process_start_time(h.raw())?;
        if created > limit + START_TOLERANCE {
            return None;
        }
    }
    Some(h)
}

/// Terminate `pid` if it still runs one of `allowed_images` (and was created before
/// `started_before`, if given). Never panics; false when refused or failed.
pub fn terminate_pid(
    pid: u32,
    allowed_images: &[&Path],
    started_before: Option<SystemTime>,
    exit_code: u32,
) -> bool {
    let Some(h) = open_checked(pid, PROCESS_TERMINATE, allowed_images, started_before) else {
        return false;
    };
    // SAFETY: valid handle with TERMINATE access.
    unsafe { TerminateProcess(h.raw(), exit_code) != 0 }
}

/// A job object that kills its processes when its handle closes (or its owner dies).
///
/// Best effort: [`KillOnCloseJob::create`] never fails, [`KillOnCloseJob::ok`] says whether
/// Windows gave a job; every method returns false instead of failing. Create it once and
/// keep it for the owner's whole life; the handle is not inheritable.
#[derive(Debug, Default)]
pub struct KillOnCloseJob {
    handle: Option<OwnedHandle>,
}

impl KillOnCloseJob {
    /// A new job with `KILL_ON_JOB_CLOSE`.
    pub fn create() -> Self {
        // SAFETY: null attributes and name.
        let Some(h) =
            OwnedHandle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
        else {
            return Self { handle: None };
        };
        // SAFETY: the struct is plain data; all-zero is valid.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is the struct the class names, with its size.
        let ok = unsafe {
            SetInformationJobObject(
                h.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        Self {
            handle: (ok != 0).then_some(h),
        }
    }

    /// Windows gave a working job.
    pub fn ok(&self) -> bool {
        self.handle.is_some()
    }

    /// The job handle, for tests that inspect it.
    pub fn raw(&self) -> Option<HANDLE> {
        self.handle.as_ref().map(OwnedHandle::raw)
    }

    /// Put a process (a handle with `SET_QUOTA` and `TERMINATE` access) in the job.
    pub fn assign(&self, process: HANDLE) -> bool {
        match &self.handle {
            // SAFETY: valid job and process handles.
            Some(job) if !process.is_null() => unsafe {
                AssignProcessToJobObject(job.raw(), process) != 0
            },
            _ => false,
        }
    }

    /// Put a [`Child`] in the job.
    pub fn assign_child(&self, child: &Child) -> bool {
        self.assign(child.process.raw())
    }

    /// Put an already running process in the job, if `pid` still runs one of
    /// `allowed_images` (and was created before `started_before`, if given).
    pub fn adopt(
        &self,
        pid: u32,
        allowed_images: &[&Path],
        started_before: Option<SystemTime>,
    ) -> bool {
        if self.handle.is_none() {
            return false;
        }
        match open_checked(
            pid,
            PROCESS_SET_QUOTA | PROCESS_TERMINATE | SYNCHRONIZE,
            allowed_images,
            started_before,
        ) {
            Some(h) => self.assign(h.raw()),
            None => false,
        }
    }

    /// Close the job: Windows kills every process still in it.
    pub fn close(&mut self) {
        self.handle = None;
    }
}

/// A process started suspended by [`spawn_suspended`]. Dropping it closes the handles and
/// leaves the process running (kill it, or let its job do it).
#[derive(Debug)]
pub struct Child {
    process: OwnedHandle,
    thread: OwnedHandle,
    pid: u32,
}

impl Child {
    /// The process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The process handle (owned by the `Child`).
    pub fn raw(&self) -> HANDLE {
        self.process.raw()
    }

    /// Let the process run.
    pub fn resume(&self) -> bool {
        // SAFETY: valid thread handle with SUSPEND_RESUME access (from CreateProcess).
        unsafe { ResumeThread(self.thread.raw()) != u32::MAX }
    }

    /// The exit code, or `None` while it runs.
    pub fn try_wait(&self) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: valid handle and out pointer.
        let ok = unsafe { GetExitCodeProcess(self.process.raw(), &mut code) };
        (ok != 0 && code != STILL_ACTIVE).then_some(code)
    }

    /// Wait up to `timeout` for the process to exit; its exit code, or `None` on timeout.
    pub fn wait(&self, timeout: Duration) -> Option<u32> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid handle.
        if unsafe { WaitForSingleObject(self.process.raw(), ms) } == WAIT_OBJECT_0 {
            self.try_wait()
        } else {
            None
        }
    }

    /// Terminate the process (no error if it already exited).
    pub fn kill(&self) {
        // SAFETY: valid handle with TERMINATE access (from CreateProcess).
        unsafe { TerminateProcess(self.process.raw(), 1) };
    }
}

/// Quote one argument for a Windows command line (the `CommandLineToArgvW` rules).
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Puts this process at below-normal priority (a connector never competes with the game).
/// Every process of a connector calls it at start: the recorder, the control app and the window,
/// whether the control app started them or not. Returns whether the class was set.
pub fn lower_process_priority() -> bool {
    // SAFETY: the pseudo handle of this process; a failure leaves the priority as it was.
    unsafe {
        windows_sys::Win32::System::Threading::SetPriorityClass(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            BELOW_NORMAL_PRIORITY_CLASS,
        ) != 0
    }
}

/// The priority class of this process (`GetPriorityClass`; 0 when it cannot be read).
pub fn process_priority_class() -> u32 {
    // SAFETY: the pseudo handle of this process.
    unsafe {
        windows_sys::Win32::System::Threading::GetPriorityClass(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
        )
    }
}

/// Raises the calling thread one step within its process's class (the recorder's timed
/// threads: the 50 Hz loop and the telemetry sampler; still below a normal-priority process's
/// normal threads when the process is below normal).
pub fn raise_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: the pseudo handle of this thread; a failure leaves the priority as it was.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

/// Start `argv` suspended, windowless, below-normal priority, with no console streams and no
/// inherited handles. `extra_flags` are further `CreateProcess` creation flags.
pub fn spawn_suspended(
    argv: &[impl AsRef<OsStr>],
    cwd: &Path,
    extra_flags: u32,
) -> Result<Child, Win32Error> {
    let command: Vec<String> = argv
        .iter()
        .map(|a| quote_arg(&a.as_ref().to_string_lossy()))
        .collect();
    let mut cmdline = wide(command.join(" "));
    let cwd = wide(cwd);
    // SAFETY: plain data; all-zero is valid before setting `cb`.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    // SAFETY: plain data.
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let flags = CREATE_SUSPENDED | CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS | extra_flags;
    // SAFETY: `cmdline` is a mutable NUL-terminated buffer, `cwd` NUL-terminated; both outlive the call.
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            flags,
            std::ptr::null(),
            cwd.as_ptr(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(Win32Error::last("CreateProcessW"));
    }
    let process =
        OwnedHandle::new(pi.hProcess).ok_or_else(|| Win32Error::last("CreateProcessW"))?;
    let thread = OwnedHandle::new(pi.hThread).ok_or_else(|| Win32Error::last("CreateProcessW"))?;
    Ok(Child {
        process,
        thread,
        pid: pi.dwProcessId,
    })
}
