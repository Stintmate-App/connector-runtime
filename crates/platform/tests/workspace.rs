//! The contract's resolver vectors (a copy of the contract's `workspace.vectors.json`),
//! fed through injected probes, plus the data folder and the real file system in temp folders.
//! Resolver and data-folder parts.

mod common;

use common::SPEC;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use stintmate_connector_platform::location::{self, WorkspaceArgs};
use stintmate_connector_platform::workspace::{self, Code, Kind, Probes, Source, SystemProbes};

const VECTORS: &str = include_str!("fixtures/workspace.vectors.json");
const VALID_MANIFEST: &str = r#"{"schema": 1, "created_at": "2026-10-18T19:30:00+02:00"}"#;

fn attribute_bit(name: &str) -> u32 {
    match name {
        "READONLY" => 0x1,
        "HIDDEN" => 0x2,
        "SYSTEM" => 0x4,
        "DIRECTORY" => 0x10,
        "ARCHIVE" => 0x20,
        "NORMAL" => 0x80,
        "OFFLINE" => 0x1000,
        "RECALL_ON_OPEN" => 0x40000,
        "PINNED" => 0x80000,
        "UNPINNED" => 0x100000,
        "RECALL_ON_DATA_ACCESS" => 0x400000,
        other => panic!("unknown attribute {other}"),
    }
}

struct VectorProbes {
    env: HashMap<String, String>,
    pointer: Value,
    fs: HashMap<String, Value>,
}

impl VectorProbes {
    fn new(input: &Value) -> Self {
        let env = input["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.as_str().unwrap().to_string()))
            .collect();
        let fs = input["fs"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();
        Self {
            env,
            pointer: input["pointer"].clone(),
            fs,
        }
    }
}

impl Probes for VectorProbes {
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(&name.to_lowercase()).cloned()
    }
    fn read_pointer(&self) -> std::io::Result<Option<Vec<u8>>> {
        if self.pointer["state"] == "absent" {
            return Ok(None);
        }
        Ok(Some(match self.pointer.get("raw") {
            Some(raw) => raw.as_str().unwrap().as_bytes().to_vec(),
            None => self.pointer["json"].to_string().into_bytes(),
        }))
    }
    fn kind(&self, path: &str) -> Option<Kind> {
        self.fs.get(&path.to_lowercase()).map(|e| {
            if e["kind"] == "dir" {
                Kind::Dir
            } else {
                Kind::File
            }
        })
    }
    fn attributes(&self, path: &str) -> Option<u32> {
        let e = self.fs.get(&path.to_lowercase())?;
        Some(
            e.get("attributes")
                .and_then(Value::as_array)
                .map_or(0, |a| {
                    a.iter()
                        .map(|n| attribute_bit(n.as_str().unwrap()))
                        .fold(0, |x, y| x | y)
                }),
        )
    }
    fn read_manifest(&self, folder: &str) -> Option<Vec<u8>> {
        let m = self.fs.get(&folder.to_lowercase())?.get("manifest")?;
        match m {
            Value::String(s) if s == "absent" => None,
            Value::String(s) if s == "valid" => Some(VALID_MANIFEST.as_bytes().to_vec()),
            Value::Object(o) if o.contains_key("raw") => {
                Some(o["raw"].as_str().unwrap().as_bytes().to_vec())
            }
            Value::Object(o) => Some(o["json"].to_string().into_bytes()),
            other => panic!("unexpected manifest {other}"),
        }
    }
}

fn run_vector(input: &Value) -> Value {
    let probes = VectorProbes::new(input);
    let argv = input["argv_workspace"].as_str();
    match workspace::resolve(argv, input["allow_synced_flag"].as_bool().unwrap(), &probes) {
        Err(e) => json!({"error": e.code.as_str(), "source": e.source.as_str()}),
        Ok(r) => {
            json!({"path": r.path.unwrap_or_else(|| "<legacy>".into()), "source": r.source.as_str(), "synced": r.synced})
        }
    }
}

#[test]
fn the_vectors_are_the_contract_v1_copy() {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(v["contract"], "stintmate-workspace");
    assert_eq!(v["version"], 1);
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 86);
    let ids: std::collections::HashSet<_> =
        cases.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), cases.len());
}

#[test]
fn every_vector() {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    let mut failures = Vec::new();
    for case in v["cases"].as_array().unwrap() {
        let got = run_vector(&case["input"]);
        if got != case["expect"] {
            failures.push(format!(
                "{}: expected {} got {got}",
                case["id"], case["expect"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} vectors failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---------------------------------------------------------------- the data folder

/// The real file system with a fake environment (the developer's real pointer is never read).
struct TempProbes {
    env: HashMap<String, String>,
}

impl TempProbes {
    fn new(pointer: &Path) -> Self {
        let mut env = HashMap::new();
        env.insert(
            "stintmate_settings_file".to_string(),
            pointer.display().to_string(),
        );
        Self { env }
    }
    fn set(mut self, k: &str, v: &str) -> Self {
        self.env.insert(k.to_lowercase(), v.into());
        self
    }
    fn pointer_path(&self) -> std::path::PathBuf {
        self.env["stintmate_settings_file"].clone().into()
    }
}

impl Probes for TempProbes {
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(&name.to_lowercase()).cloned()
    }
    fn read_pointer(&self) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.pointer_path()) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn kind(&self, path: &str) -> Option<Kind> {
        SystemProbes.kind(path)
    }
    fn attributes(&self, path: &str) -> Option<u32> {
        SystemProbes.attributes(path)
    }
    fn read_manifest(&self, folder: &str) -> Option<Vec<u8>> {
        SystemProbes.read_manifest(folder)
    }
}

fn make_workspace(folder: &Path, manifest: Option<&str>) {
    std::fs::create_dir_all(folder).unwrap();
    if let Some(m) = manifest {
        std::fs::write(folder.join("stintmate.json"), m).unwrap();
    }
}

fn write_pointer(path: &Path, workspace: &Path, extra: Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut obj = json!({"schema": 1, "workspace": workspace.display().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        obj[k] = v.clone();
    }
    std::fs::write(path, obj.to_string()).unwrap();
}

fn count_files(folder: &Path) -> usize {
    std::fs::read_dir(folder).map_or(0, |d| d.flatten().count())
}

#[test]
fn no_workspace_is_an_error_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let probes = TempProbes::new(&tmp.path().join("appdata/Stintmate/settings.json"));
    let err = location::resolve(&SPEC, None, false, &probes).unwrap_err();
    assert_eq!(
        (err.code, err.source),
        (Code::WorkspaceMissing, Source::Pointer)
    );
    assert_eq!(err.detail, "no workspace chosen");
    assert!(
        err.line()
            .starts_with("workspace error workspace_missing: ")
    );
    assert_eq!(count_files(tmp.path()), 0);
}

#[test]
fn pointer_workspace_gives_connectors_example() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    make_workspace(&ws, Some(VALID_MANIFEST));
    let probes = TempProbes::new(&tmp.path().join("appdata/Stintmate/settings.json"));
    write_pointer(&probes.pointer_path(), &ws, json!({}));
    let loc = location::resolve(&SPEC, None, false, &probes).unwrap();
    assert_eq!(loc.source, Source::Pointer);
    assert_eq!(
        loc.data_dir,
        loc.workspace.join("connectors").join("example")
    );
    assert!(!loc.data_dir.exists(), "resolving creates nothing");
    assert_eq!(
        loc.child_args(),
        vec![
            "--workspace".to_string(),
            loc.workspace.display().to_string()
        ]
    );
}

#[test]
fn pointer_with_bom_from_powershell() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    make_workspace(&ws, Some(VALID_MANIFEST));
    let probes = TempProbes::new(&tmp.path().join("settings.json"));
    let body = json!({"schema": 1, "workspace": ws.display().to_string()}).to_string();
    std::fs::write(probes.pointer_path(), format!("\u{feff}{body}")).unwrap();
    assert_eq!(
        location::resolve(&SPEC, None, false, &probes)
            .unwrap()
            .source,
        Source::Pointer
    );
}

#[test]
fn argv_beats_env_and_pointer_and_errors_never_fall_back() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, c) = (
        tmp.path().join("a"),
        tmp.path().join("b"),
        tmp.path().join("c"),
    );
    for w in [&a, &b, &c] {
        make_workspace(w, Some(VALID_MANIFEST));
    }
    let probes = TempProbes::new(&tmp.path().join("settings.json"))
        .set("STINTMATE_WORKSPACE", &b.display().to_string());
    write_pointer(&probes.pointer_path(), &c, json!({}));
    let loc = location::resolve(&SPEC, Some(&a.display().to_string()), false, &probes).unwrap();
    assert_eq!(
        (loc.source, loc.workspace.clone()),
        (Source::Argv, a.clone())
    );
    assert_eq!(
        location::resolve(&SPEC, None, false, &probes)
            .unwrap()
            .source,
        Source::Env
    );

    // A bad argv is an error even though env and pointer are fine.
    let err = location::resolve(
        &SPEC,
        Some(&tmp.path().join("nowhere").display().to_string()),
        false,
        &probes,
    )
    .unwrap_err();
    assert_eq!(
        (err.code, err.source),
        (Code::WorkspaceMissing, Source::Argv)
    );
    // A folder with no manifest.
    let bare = tmp.path().join("bare");
    make_workspace(&bare, None);
    let err =
        location::resolve(&SPEC, Some(&bare.display().to_string()), false, &probes).unwrap_err();
    assert_eq!(err.code, Code::WorkspaceUninitialized);
    // Not a folder.
    let file = tmp.path().join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let err =
        location::resolve(&SPEC, Some(&file.display().to_string()), false, &probes).unwrap_err();
    assert_eq!(err.code, Code::WorkspaceInvalid);
    // A corrupt pointer is an error, not the legacy fallback.
    let probes2 = TempProbes::new(&tmp.path().join("bad.json"));
    std::fs::write(probes2.pointer_path(), "{not json").unwrap();
    assert_eq!(
        location::resolve(&SPEC, None, false, &probes2)
            .unwrap_err()
            .code,
        Code::PointerInvalid
    );
}

#[test]
fn onedrive_refused_unless_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let od = tmp.path().join("OneDrive");
    let ws = od.join("Stintmate");
    make_workspace(&ws, Some(VALID_MANIFEST));
    let probes = TempProbes::new(&tmp.path().join("settings.json"))
        .set("OneDrive", &od.display().to_string());
    let arg = ws.display().to_string();
    assert_eq!(
        location::resolve(&SPEC, Some(&arg), false, &probes)
            .unwrap_err()
            .code,
        Code::WorkspaceSynced
    );
    let loc = location::resolve(&SPEC, Some(&arg), true, &probes).unwrap();
    assert!(loc.synced);
    assert_eq!(loc.child_args().last().unwrap(), "--allow-synced");
    let probes = probes.set("STINTMATE_ALLOW_SYNCED", "1");
    assert!(
        location::resolve(&SPEC, Some(&arg), false, &probes)
            .unwrap()
            .synced
    );
}

#[test]
fn pointer_path_override_and_default() {
    // The override wins and never touches the real pointer.
    let p = TempProbes::new(Path::new("C:\\somewhere\\settings.json"));
    assert_eq!(p.pointer_path(), Path::new("C:\\somewhere\\settings.json"));
    // The default lives under RoamingAppData\Stintmate (only checked when no override is set).
    if std::env::var_os("STINTMATE_SETTINGS_FILE").is_none() {
        let path = SystemProbes
            .pointer_path()
            .expect("RoamingAppData resolves");
        assert!(path.ends_with(Path::new("Stintmate").join("settings.json")));
    }
}

#[test]
fn workspace_arguments() {
    let a = WorkspaceArgs::parse(
        ["--hz", "50", "--workspace", "D:\\w", "--allow-synced", "x"].map(String::from),
    );
    assert_eq!(a.workspace.as_deref(), Some("D:\\w"));
    assert!(a.allow_synced);
    assert_eq!(a.rest, ["--hz", "50", "x"]);
    let b = WorkspaceArgs::parse(["--workspace=E:\\z"].map(String::from));
    assert_eq!(b.workspace.as_deref(), Some("E:\\z"));
    let c = WorkspaceArgs::parse(["--workspace".to_string()]);
    assert_eq!(
        (c.workspace, c.rest),
        (None, vec!["--workspace".to_string()])
    );
}
