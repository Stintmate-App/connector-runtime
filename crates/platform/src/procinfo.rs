//! Read-only process questions for the control app: is a PID alive (and still the same
//! process), and who started it (the process chain logged with every start, stop and quit).

use crate::proc::process_start_time;
use crate::win::OwnedHandle;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
};

const SYNCHRONIZE: u32 = 0x0010_0000;
/// Two creation times of the same process read twice match within this.
const SAME_PROCESS: Duration = Duration::from_millis(10);

fn open(pid: u32, access: u32) -> Option<OwnedHandle> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; the result is checked.
    OwnedHandle::new(unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// Creation time of a running process, or `None`. Never fails loudly.
pub fn pid_start_time(pid: u32) -> Option<SystemTime> {
    process_start_time(open(pid, 0)?.raw())
}

/// True if `pid` runs and, when `started_at` is given, it is the process created then (not a
/// reuse of its PID). A process that exited but whose handle someone still holds is not alive.
pub fn process_alive(pid: u32, started_at: Option<SystemTime>) -> bool {
    let Some(h) = open(pid, SYNCHRONIZE) else {
        return false;
    };
    if let Some(expected) = started_at {
        let Some(created) = process_start_time(h.raw()) else {
            return false;
        };
        let apart = created
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        if apart > SAME_PROCESS {
            return false;
        }
    }
    // SAFETY: valid handle with SYNCHRONIZE access.
    unsafe { WaitForSingleObject(h.raw(), 0) == WAIT_TIMEOUT }
}

/// `(pid, exe file name)` of `pid` and its parents, nearest first, at most `depth` entries,
/// from one process snapshot. A parent whose PID now belongs to a process created after its
/// child ends the chain. Empty on failure.
pub fn process_chain(pid: u32, depth: usize) -> Vec<(u32, String)> {
    // SAFETY: plain call; the result is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let Some(snap) = OwnedHandle::new(snap) else {
        return Vec::new();
    };
    // SAFETY: plain data; all-zero is valid before setting `dwSize`.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut procs: HashMap<u32, (u32, String)> = HashMap::new();
    // SAFETY: valid snapshot and entry.
    let mut ok = unsafe { Process32FirstW(snap.raw(), &mut entry) } != 0;
    while ok {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        procs.insert(
            entry.th32ProcessID,
            (
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            ),
        );
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap.raw(), &mut entry) } != 0;
    }
    let mut chain: Vec<(u32, String)> = Vec::new();
    let mut child_started: Option<SystemTime> = None;
    let mut pid = pid;
    while let Some((parent, name)) = procs.get(&pid) {
        if chain.len() >= depth || chain.iter().any(|(p, _)| *p == pid) {
            break;
        }
        let started = pid_start_time(pid);
        if let (false, Some(s), Some(c)) = (chain.is_empty(), started, child_started)
            && s > c + SAME_PROCESS
        {
            break; // the parent PID now belongs to a newer process
        }
        chain.push((pid, name.clone()));
        child_started = started;
        if *parent == 0 {
            break;
        }
        pid = *parent;
    }
    chain
}

/// `"python.exe 12 < cmd.exe 34"`: a process and its parents for a log line (`"pid 12"` when
/// the snapshot does not find it, `"unknown"` for no PID).
pub fn describe_process(pid: Option<u32>, depth: usize) -> String {
    let Some(pid) = pid.filter(|p| *p > 0) else {
        return "unknown".into();
    };
    let chain = process_chain(pid, depth);
    if chain.is_empty() {
        return format!("pid {pid}");
    }
    chain
        .iter()
        .map(|(p, n)| format!("{n} {p}"))
        .collect::<Vec<_>>()
        .join(" < ")
}
