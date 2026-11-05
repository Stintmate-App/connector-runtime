//! The Stintmate workspace resolver: the workspace resolution of
//! Stintmate IPC (SPEC section 4.3), tested against the vectors in
//! `tests/fixtures/workspace.vectors.json`.
//!
//! This is the connector's own implementation for now; it may move
//! to stintmate-ipc's `find_workspace` once it ships there passing the same vectors.
//!
//! Resolution, once at start: `--workspace` (source `argv`), then `STINTMATE_WORKSPACE`
//! (`env`), then the pointer file `%APPDATA%\Stintmate\settings.json` (`pointer`), then the
//! legacy fallback (`legacy`, not checked). The first source given wins and lower sources
//! are not read; a failure is an error, never a fallback.
//!
//! Everything that touches the machine goes through [`Probes`], so tests feed the vectors'
//! facts without touching the real disk.

use std::fmt;

/// The pointer file's schema this reader knows.
pub const POINTER_SCHEMA: u64 = 1;
/// The manifest's schema this reader knows.
pub const MANIFEST_SCHEMA: u64 = 1;
/// The manifest file name inside a workspace.
pub const MANIFEST_NAME: &str = "stintmate.json";
/// Environment variable naming the workspace.
pub const ENV_WORKSPACE: &str = "STINTMATE_WORKSPACE";
/// Tests only: replaces the pointer file's path.
pub const ENV_SETTINGS_FILE: &str = "STINTMATE_SETTINGS_FILE";
/// `1` accepts a workspace inside a cloud-synced folder.
pub const ENV_ALLOW_SYNCED: &str = "STINTMATE_ALLOW_SYNCED";
/// Variables naming the roots of OneDrive.
pub const SYNC_ROOT_VARS: [&str; 3] = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"];

const CLOUD_ATTRIBUTES: u32 = 0x40000 | 0x80000 | 0x100000 | 0x400000;

/// Where the workspace came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `--workspace`.
    Argv,
    /// `STINTMATE_WORKSPACE`.
    Env,
    /// The pointer file.
    Pointer,
    /// Nothing given: the app's own legacy folder.
    Legacy,
}

impl Source {
    /// The contract's name of the source.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Argv => "argv",
            Source::Env => "env",
            Source::Pointer => "pointer",
            Source::Legacy => "legacy",
        }
    }
}

/// The contract's error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Not an absolute local folder path, or a file.
    WorkspaceInvalid,
    /// The folder does not exist.
    WorkspaceMissing,
    /// Inside a cloud-synced folder and not allowed.
    WorkspaceSynced,
    /// No `stintmate.json`.
    WorkspaceUninitialized,
    /// `stintmate.json` is not valid.
    ManifestInvalid,
    /// `stintmate.json` has a newer schema.
    ManifestUnsupported,
    /// The pointer file is unusable.
    PointerInvalid,
    /// The pointer file has a newer schema.
    PointerUnsupported,
}

impl Code {
    /// The contract's error code string.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::WorkspaceInvalid => "workspace_invalid",
            Code::WorkspaceMissing => "workspace_missing",
            Code::WorkspaceSynced => "workspace_synced",
            Code::WorkspaceUninitialized => "workspace_uninitialized",
            Code::ManifestInvalid => "manifest_invalid",
            Code::ManifestUnsupported => "manifest_unsupported",
            Code::PointerInvalid => "pointer_invalid",
            Code::PointerUnsupported => "pointer_unsupported",
        }
    }

    /// What the user does about it.
    pub fn fix(self) -> &'static str {
        match self {
            Code::WorkspaceInvalid => {
                "not an absolute local folder path, or the path is a file. Choose a folder such as C:\\Users\\<user>\\Documents\\Stintmate."
            }
            Code::WorkspaceMissing => {
                "the folder does not exist. Reconnect the drive, restore the folder, or choose it again in the onboarding."
            }
            Code::WorkspaceSynced => {
                "the folder is inside a cloud-synced folder. Choose a folder outside OneDrive, or accept the risk with --allow-synced."
            }
            Code::WorkspaceUninitialized => {
                "no stintmate.json: the folder is not a workspace. Create it with the onboarding or the migration tool."
            }
            Code::ManifestInvalid => {
                "stintmate.json is not valid. Restore it from a backup or recreate the workspace."
            }
            Code::ManifestUnsupported => {
                "the workspace was created by a newer version. Update the app."
            }
            Code::PointerInvalid => {
                "the pointer file is unusable. Run the onboarding again (it rewrites the file)."
            }
            Code::PointerUnsupported => {
                "the pointer file was written by a newer version. Update the app."
            }
        }
    }
}

/// A resolved workspace. `path` is `None` for the legacy fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The normalised absolute workspace path.
    pub path: Option<String>,
    /// Which source gave it.
    pub source: Source,
    /// Inside a cloud-synced folder (and allowed).
    pub synced: bool,
}

/// A resolution failure: never falls back to a lower source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceError {
    /// The contract's code.
    pub code: Code,
    /// The source that was chosen.
    pub source: Source,
    /// The offending value or reason.
    pub detail: String,
}

impl WorkspaceError {
    pub(crate) fn new(code: Code, source: Source, detail: impl Into<String>) -> Self {
        Self {
            code,
            source,
            detail: detail.into(),
        }
    }

    /// The command-line report: `workspace error <code>: <message>`.
    pub fn line(&self) -> String {
        format!("workspace error {}: {self}", self.code.as_str())
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = if self.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.detail)
        };
        write!(
            f,
            "workspace from {}{detail}: {}",
            self.source.as_str(),
            self.code.fix()
        )
    }
}

impl std::error::Error for WorkspaceError {}

/// What exists at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A folder.
    Dir,
    /// A file.
    File,
}

/// The machine as the resolver sees it.
pub trait Probes {
    /// A process environment variable (names compared case-insensitively).
    fn env(&self, name: &str) -> Option<String>;
    /// The pointer file's bytes; `Ok(None)` when absent, `Err` when present but unreadable.
    fn read_pointer(&self) -> std::io::Result<Option<Vec<u8>>>;
    /// Whether the path is a folder, a file or does not exist.
    fn kind(&self, path: &str) -> Option<Kind>;
    /// `GetFileAttributesW` of the path, `None` when unknown.
    fn attributes(&self, path: &str) -> Option<u32>;
    /// `<folder>\stintmate.json`; `None` when absent. Present but unreadable gives bytes
    /// that are not a valid manifest.
    fn read_manifest(&self, folder: &str) -> Option<Vec<u8>>;
}

/// The real machine: the process environment, the file system, the pointer file.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProbes;

impl SystemProbes {
    /// The pointer file: `STINTMATE_SETTINGS_FILE` (tests) or
    /// `<RoamingAppData>\Stintmate\settings.json`.
    pub fn pointer_path(&self) -> Option<std::path::PathBuf> {
        if let Some(over) = self.env(ENV_SETTINGS_FILE) {
            let over = over.trim();
            if !over.is_empty() {
                return Some(over.into());
            }
        }
        let base = roaming_appdata().or_else(|| self.env("APPDATA").filter(|v| !v.is_empty()))?;
        Some(
            std::path::Path::new(&base)
                .join("Stintmate")
                .join("settings.json"),
        )
    }
}

impl Probes for SystemProbes {
    fn env(&self, name: &str) -> Option<String> {
        // Windows variable names are case-insensitive, `var_os` follows that.
        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    }

    fn read_pointer(&self) -> std::io::Result<Option<Vec<u8>>> {
        let Some(path) = self.pointer_path() else {
            return Ok(None);
        };
        match std::fs::read(path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn kind(&self, path: &str) -> Option<Kind> {
        let p = std::path::Path::new(path);
        if p.is_dir() {
            Some(Kind::Dir)
        } else if p.exists() {
            Some(Kind::File)
        } else {
            None
        }
    }

    fn attributes(&self, path: &str) -> Option<u32> {
        real_attributes(path)
    }

    fn read_manifest(&self, folder: &str) -> Option<Vec<u8>> {
        match std::fs::read(std::path::Path::new(folder).join(MANIFEST_NAME)) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(vec![0]),
        }
    }
}

#[cfg(windows)]
fn real_attributes(path: &str) -> Option<u32> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    let w = crate::win::wide(path);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    let v = unsafe { GetFileAttributesW(w.as_ptr()) };
    (v != INVALID_FILE_ATTRIBUTES).then_some(v)
}

#[cfg(not(windows))]
fn real_attributes(_path: &str) -> Option<u32> {
    None
}

/// The RoamingAppData known folder (`%APPDATA%` gives the same path).
#[cfg(windows)]
fn roaming_appdata() -> Option<String> {
    use windows_sys::Win32::UI::Shell::FOLDERID_RoamingAppData;
    known_folder(&FOLDERID_RoamingAppData)
}

/// The LocalAppData known folder, falling back to `%LOCALAPPDATA%` (the machine-level
/// `Stintmate\connectors` folder lives under it).
pub fn local_appdata() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData;
        if let Some(p) = known_folder(&FOLDERID_LocalAppData) {
            return Some(p);
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|v| v.to_string_lossy().into_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `out` receives a CoTaskMem-allocated string that we free below.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut out) };
    let result = if hr == 0 && !out.is_null() {
        // SAFETY: on success `out` is a NUL-terminated UTF-16 string.
        let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
        (!s.is_empty()).then_some(s)
    } else {
        None
    };
    // SAFETY: freeing the string the call allocated (also valid for null).
    unsafe { CoTaskMemFree(out.cast()) };
    result
}

#[cfg(not(windows))]
fn roaming_appdata() -> Option<String> {
    None
}

// ---------------------------------------------------------------- path rules

/// Rule 1: whitespace, one pair of surrounding double quotes, whitespace.
pub fn clean(value: &str) -> String {
    let v = value.trim();
    let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        &v[1..v.len() - 1]
    } else {
        v
    };
    v.trim().to_string()
}

/// Rules 2-5 on a cleaned value: the normalised absolute path, or `None` if invalid.
pub fn normalise(value: &str) -> Option<String> {
    let value = value.replace('/', "\\");
    let b = value.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return None;
    }
    let rest = &value[3..];
    if rest
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 0x20)
    {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split('\\') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return None; // a drive root
    }
    Some(format!(
        "{}:\\{}",
        (b[0] as char).to_ascii_uppercase(),
        parts.join("\\")
    ))
}

fn segments(path: &str) -> Vec<String> {
    path.split('\\')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// `path` is `root` or inside it: case-insensitive, on whole segments.
pub fn is_under(path: &str, root: &str) -> bool {
    let (p, r) = (segments(path), segments(root));
    p.len() >= r.len() && p[..r.len()] == r[..]
}

/// The path itself and every ancestor up to the drive root (`C:\`).
fn ancestors(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('\\').collect();
    let mut out: Vec<String> = (2..=parts.len())
        .rev()
        .map(|i| parts[..i].join("\\"))
        .collect();
    out.push(format!("{}\\", parts[0]));
    out
}

fn is_synced(path: &str, probes: &dyn Probes) -> bool {
    let under_root = SYNC_ROOT_VARS
        .iter()
        .filter_map(|n| probes.env(n))
        .filter(|v| !v.is_empty())
        .filter_map(|v| normalise(&clean(&v)))
        .any(|r| is_under(path, &r));
    under_root
        || ancestors(path).iter().any(|f| {
            probes
                .attributes(f)
                .is_some_and(|a| a & CLOUD_ATTRIBUTES != 0)
        })
}

/// UTF-8, a BOM accepted.
fn parse_json(data: &[u8]) -> Option<serde_json::Value> {
    let text = std::str::from_utf8(data).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// A valid `schema` (an integer >= 1, not a boolean), or `None`.
fn schema_of(obj: &serde_json::Value) -> Option<u64> {
    obj.as_object()?.get("schema")?.as_u64().filter(|&v| v >= 1)
}

fn check(
    raw: &str,
    source: Source,
    allowed: bool,
    probes: &dyn Probes,
) -> Result<Resolution, WorkspaceError> {
    let path = normalise(&clean(raw))
        .ok_or_else(|| WorkspaceError::new(Code::WorkspaceInvalid, source, raw))?;
    match probes.kind(&path) {
        None => return Err(WorkspaceError::new(Code::WorkspaceMissing, source, path)),
        Some(Kind::File) => return Err(WorkspaceError::new(Code::WorkspaceInvalid, source, path)),
        Some(Kind::Dir) => {}
    }
    let synced = is_synced(&path, probes);
    if synced && !allowed {
        return Err(WorkspaceError::new(Code::WorkspaceSynced, source, path));
    }
    let data = probes
        .read_manifest(&path)
        .ok_or_else(|| WorkspaceError::new(Code::WorkspaceUninitialized, source, path.clone()))?;
    let bad = || WorkspaceError::new(Code::ManifestInvalid, source, path.clone());
    let manifest = parse_json(&data).ok_or_else(bad)?;
    let schema = schema_of(&manifest).ok_or_else(bad)?;
    if schema > MANIFEST_SCHEMA {
        return Err(WorkspaceError::new(Code::ManifestUnsupported, source, path));
    }
    Ok(Resolution {
        path: Some(path),
        source,
        synced,
    })
}

/// Resolve the workspace. `path` is `None` in the result for the legacy fallback.
pub fn resolve(
    argv_workspace: Option<&str>,
    allow_synced_flag: bool,
    probes: &dyn Probes,
) -> Result<Resolution, WorkspaceError> {
    let allowed = allow_synced_flag || probes.env(ENV_ALLOW_SYNCED).as_deref() == Some("1");

    if let Some(v) = argv_workspace {
        return check(v, Source::Argv, allowed, probes);
    }
    if let Some(v) = probes.env(ENV_WORKSPACE).filter(|v| !v.trim().is_empty()) {
        return check(&v, Source::Env, allowed, probes);
    }

    let pointer = |code, detail: &str| WorkspaceError::new(code, Source::Pointer, detail);
    let data = match probes.read_pointer() {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Ok(Resolution {
                path: None,
                source: Source::Legacy,
                synced: false,
            });
        }
        Err(e) => return Err(pointer(Code::PointerInvalid, &e.to_string())),
    };
    let obj = parse_json(&data).ok_or_else(|| pointer(Code::PointerInvalid, "not JSON"))?;
    let schema = schema_of(&obj).ok_or_else(|| pointer(Code::PointerInvalid, "no valid schema"))?;
    if schema > POINTER_SCHEMA {
        return Err(pointer(
            Code::PointerUnsupported,
            &format!("schema {schema}"),
        ));
    }
    let workspace = obj
        .get("workspace")
        .and_then(|w| w.as_str())
        .filter(|w| !w.trim().is_empty())
        .ok_or_else(|| pointer(Code::PointerInvalid, "no workspace"))?;
    let pointer_allows = match obj.get("allow_synced") {
        None => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => {
            return Err(pointer(
                Code::PointerInvalid,
                "allow_synced is not a boolean",
            ));
        }
    };
    check(
        workspace,
        Source::Pointer,
        allowed || pointer_allows,
        probes,
    )
}
