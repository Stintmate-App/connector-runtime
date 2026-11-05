//! The connector descriptor, the per-user machine file
//! `%LOCALAPPDATA%\Stintmate\connectors\example.json`.
//!
//! How a managing app finds the connectors installed on this machine without knowing any
//! simulator (workspace contract, section 5.5). Written by this connector only: by its setup
//! and by `example-connector-ui --register`, which writes it atomically and exits. The control app
//! never rewrites it at start (a stale build started by hand cannot take the registration back).
//! `--unregister` removes it ([`remove`]), but only when its `launcher` names this install and its
//! `product` is this connector's. `--register` refuses to overwrite a descriptor holding another
//! `product`; one with none (an older build) is replaced.
//! `--read-registration` reads it for the next setup ([`read_ours`]). Tests and development
//! runs point `STINTMATE_CONNECTORS_DIR` at a temporary folder.

use crate::config::write_json_atomic;
use crate::texts::Texts;
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use stintmate_connector_platform::spec::ConnectorSpec;

/// The descriptor's schema version.
pub const SCHEMA: u64 = 1;
/// The control protocol the hub requires.
pub const CONTROL_PROTOCOL: &str = "stintmate-ipc/2";
/// The file name inside the connectors folder: `<id>.json`.
pub fn file_name(spec: &ConnectorSpec) -> String {
    format!("{}.json", spec.id)
}
/// The environment variable that replaces the connectors folder (tests, development).
pub const ENV_CONNECTORS_DIR: &str = "STINTMATE_CONNECTORS_DIR";
/// The switches an uninstaller gets so that it runs with no window and no prompts.
pub const UNINSTALL_SWITCHES: [&str; 3] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"];

/// What [`remove`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// The file named this install and was deleted.
    Removed(PathBuf),
    /// There was no file.
    NotThere,
    /// The file belongs to another install (or is not this connector's) and was left alone.
    OtherInstall(PathBuf),
}

/// The connectors folder from its two sources: the override (when not empty), else
/// `<LocalAppData>\Stintmate\connectors`.
pub fn connectors_dir_from(env: Option<&str>, local_appdata: Option<&str>) -> Option<PathBuf> {
    if let Some(e) = env.map(str::trim).filter(|e| !e.is_empty()) {
        return Some(PathBuf::from(e));
    }
    local_appdata
        .filter(|l| !l.is_empty())
        .map(|l| Path::new(l).join("Stintmate").join("connectors"))
}

/// The connectors folder of this machine and user ([`connectors_dir_from`] on the real sources).
pub fn connectors_dir() -> io::Result<PathBuf> {
    let env = std::env::var_os(ENV_CONNECTORS_DIR).map(|v| v.to_string_lossy().into_owned());
    connectors_dir_from(
        env.as_deref(),
        stintmate_connector_platform::workspace::local_appdata().as_deref(),
    )
    .ok_or_else(|| io::Error::other("cannot find the LocalAppData folder"))
}

/// Where the descriptor goes: `<connectors dir>\<id>.json`.
pub fn path(spec: &ConnectorSpec) -> io::Result<PathBuf> {
    Ok(connectors_dir()?.join(file_name(spec)))
}

/// The descriptor. `launcher`: this exe; `mutex`: the control app's single-instance mutex;
/// `flavour`: which build this is (left out for the release build); `uninstall`: the uninstaller
/// another build's setup runs to remove this install (left out when nothing installed it).
pub fn build(
    spec: &ConnectorSpec,
    launcher: &Path,
    mutex: &str,
    flavour: Option<&str>,
    uninstall: Option<&Path>,
) -> Value {
    let mut d = json!({
        "schema": SCHEMA,
        "id": spec.id,
        "product": spec.product,
        "name": spec.name,
        "sim": spec.sim,
        "sim_name": spec.sim_name,
        "version": spec.version,
        "launcher": launcher.display().to_string(),
        "args": [],
        "control_protocol": CONTROL_PROTOCOL,
        "endpoint": spec.endpoint(),
        "mutex": mutex,
        // A file other apps read: the names stay English (data, not this app's UI).
        "services": crate::SERVICES.iter().map(|s| json!({
            "id": s,
            "name": Texts::english_text(spec, &format!("service.{s}")),
        })).collect::<Vec<_>>(),
        // In <workspace>\connectors\<id>\, where the connector writes them.
        "logs": ["recorder.log", "ui.log"],
        "written_at": stintmate_connector_platform::logging::local_iso(false),
        "written_by": format!("{} {}", spec.ui_app_id(), spec.version),
    });
    if let Some(f) = flavour.filter(|f| !f.is_empty()) {
        d["flavour"] = json!(f);
    }
    if let Some(u) = uninstall {
        let mut argv = vec![u.display().to_string()];
        argv.extend(UNINSTALL_SWITCHES.iter().map(|s| (*s).to_string()));
        d["uninstall"] = json!(argv);
    }
    d
}

/// Equal but for `written_at`.
fn same(a: &Value, b: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("written_at");
        }
        v
    };
    a.is_object() && strip(a) == strip(b)
}

/// A path the way the descriptor rules compare them: absolute, `/` read as `\`, no trailing
/// separator, lower case.
pub fn comparable(p: &str) -> String {
    let swapped = p.trim().replace('/', "\\");
    let abs = std::path::absolute(&swapped)
        .map(|a| a.display().to_string())
        .unwrap_or(swapped);
    abs.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether two paths name the same file under [`comparable`].
pub fn same_path(a: &str, b: &str) -> bool {
    comparable(a) == comparable(b)
}

/// The `product` of a descriptor when it names one other than this connector's (a missing or
/// `null` one is an older build's, which is replaced).
fn other_product(spec: &ConnectorSpec, d: &Value) -> Option<String> {
    match d.get("product") {
        None | Some(Value::Null) => None,
        Some(Value::String(p)) if p == spec.product => None,
        Some(Value::String(p)) => Some(p.clone()),
        Some(other) => Some(other.to_string()),
    }
}

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

/// Writes (or refreshes) the descriptor in `dir` (created when missing); returns its path.
/// Atomic, and left untouched when nothing but the write time would change.
pub fn write_in(
    spec: &ConnectorSpec,
    dir: &Path,
    launcher: &Path,
    mutex: &str,
    flavour: Option<&str>,
    uninstall: Option<&Path>,
) -> io::Result<PathBuf> {
    let target = dir.join(file_name(spec));
    let doc = build(spec, launcher, mutex, flavour, uninstall);
    let current = std::fs::read(&target)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(strip_bom(&b)).ok());
    if let Some(other) = current.as_ref().and_then(|c| other_product(spec, c)) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already registers the id \"{}\" with another product ({other}); it is not replaced. Remove that program, or change the connector's id",
                target.display(),
                spec.id
            ),
        ));
    }
    if current.as_ref().is_some_and(|c| same(c, &doc)) {
        return Ok(target);
    }
    write_json_atomic(&target, &doc)?;
    tracing::info!("Connector descriptor written: {}", target.display());
    Ok(target)
}

/// [`write_in`] on the machine's connectors folder.
pub fn write(
    spec: &ConnectorSpec,
    launcher: &Path,
    mutex: &str,
    flavour: Option<&str>,
    uninstall: Option<&Path>,
) -> io::Result<PathBuf> {
    write_in(
        spec,
        &connectors_dir()?,
        launcher,
        mutex,
        flavour,
        uninstall,
    )
}

/// Removes the descriptor in `dir` when it is this connector's (`id` is ours and its `product`
/// is this connector's) and its `launcher` names `this_exe`. A file of another install, another
/// id or product (none included), or not JSON, stays.
pub fn remove_in(spec: &ConnectorSpec, dir: &Path, this_exe: &Path) -> io::Result<Removed> {
    let target = dir.join(file_name(spec));
    let bytes = match std::fs::read(&target) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Removed::NotThere),
        Err(e) => return Err(e),
    };
    let ours = serde_json::from_slice::<Value>(strip_bom(&bytes))
        .ok()
        .is_some_and(|v| {
            v["id"] == spec.id
                && v["product"] == spec.product
                && v["launcher"]
                    .as_str()
                    .is_some_and(|l| same_path(l, &this_exe.display().to_string()))
        });
    if !ours {
        return Ok(Removed::OtherInstall(target));
    }
    std::fs::remove_file(&target)?;
    tracing::info!("Connector descriptor removed: {}", target.display());
    Ok(Removed::Removed(target))
}

/// [`remove_in`] on the machine's connectors folder.
pub fn remove(spec: &ConnectorSpec, this_exe: &Path) -> io::Result<Removed> {
    remove_in(spec, &connectors_dir()?, this_exe)
}

/// What the next setup needs to know about the installed connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// The control app's exe.
    pub launcher: String,
    /// The uninstall command line (argv), empty when nothing installed it.
    pub uninstall: Vec<String>,
    /// The build, empty for the release build.
    pub flavour: String,
}

/// Reads the descriptor in `dir`: `None` when there is none, or it is not valid or not ours
/// (JSON object, schema 1, `id` ours, a `launcher`).
pub fn read_ours_in(spec: &ConnectorSpec, dir: &Path) -> Option<Registration> {
    let bytes = std::fs::read(dir.join(file_name(spec))).ok()?;
    let v: Value = serde_json::from_slice(strip_bom(&bytes)).ok()?;
    if v["schema"] != SCHEMA || v["id"] != spec.id {
        return None;
    }
    let launcher = v["launcher"]
        .as_str()
        .filter(|l| !l.is_empty())?
        .to_string();
    let uninstall = v["uninstall"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let flavour = v["flavour"].as_str().unwrap_or_default().to_string();
    Some(Registration {
        launcher,
        uninstall,
        flavour,
    })
}

/// [`read_ours_in`] on the machine's connectors folder.
pub fn read_ours(spec: &ConnectorSpec) -> Option<Registration> {
    read_ours_in(spec, &connectors_dir().ok()?)
}

impl Registration {
    /// The `key=value` text `--read-registration` writes: a UTF-8 BOM, `\r\n` line ends, the
    /// keys `launcher`, `dir`, `uninstall`, `uninstall_args` and `flavour`.
    pub fn to_file_text(&self) -> String {
        let dir = Path::new(&self.launcher)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let (cmd, rest) = match self.uninstall.split_first() {
            Some((c, r)) => (c.as_str(), r),
            None => ("", &[][..]),
        };
        let args: Vec<String> = rest
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect();
        format!(
            "\u{feff}launcher={}\r\ndir={dir}\r\nuninstall={cmd}\r\nuninstall_args={}\r\nflavour={}\r\n",
            self.launcher,
            args.join(" "),
            self.flavour
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testspec::SPEC;

    const EXE: &str = r"C:\x\example-connector-ui.exe";

    #[test]
    fn every_field_of_workspace_md_5_5() {
        let d = build(
            &SPEC,
            Path::new(EXE),
            r"Local\M",
            Some("dev"),
            Some(Path::new(r"C:\x\unins000.exe")),
        );
        for k in [
            "schema",
            "id",
            "product",
            "name",
            "sim",
            "sim_name",
            "version",
            "launcher",
            "args",
            "control_protocol",
            "endpoint",
            "mutex",
            "services",
            "logs",
            "flavour",
            "uninstall",
            "written_at",
            "written_by",
        ] {
            assert!(d.get(k).is_some(), "{k}");
        }
        assert_eq!(d["id"], "example");
        assert_eq!(d["product"], SPEC.product);
        assert_eq!(d["endpoint"], "connector-example");
        assert_eq!(d["control_protocol"], "stintmate-ipc/2");
        assert_eq!(
            d["services"],
            json!([{"id": "recorder", "name": "Recorder"}])
        );
        assert_eq!(d["logs"], json!(["recorder.log", "ui.log"]));
        assert_eq!(d["args"], json!([]));
        assert_eq!(d["flavour"], "dev");
        assert_eq!(
            d["uninstall"],
            json!([
                r"C:\x\unins000.exe",
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART"
            ])
        );
    }

    #[test]
    fn flavour_and_uninstall_are_left_out_when_not_given() {
        let d = build(&SPEC, Path::new(EXE), r"Local\M", None, None);
        assert!(d.get("flavour").is_none() && d.get("uninstall").is_none());
    }

    #[test]
    fn the_folder_is_the_override_else_local_appdata() {
        assert_eq!(
            connectors_dir_from(Some(r"D:\tmp\c"), Some(r"C:\L")),
            Some(PathBuf::from(r"D:\tmp\c"))
        );
        for empty in [None, Some(""), Some("  ")] {
            assert_eq!(
                connectors_dir_from(empty, Some(r"C:\L")),
                Some(Path::new(r"C:\L").join("Stintmate").join("connectors"))
            );
        }
        assert_eq!(connectors_dir_from(None, None), None);
    }

    #[test]
    fn paths_compare_absolute_case_insensitive_with_slashes_as_backslashes() {
        assert!(same_path(
            r"C:\Prog\Stintmate\example-connector-ui.exe",
            "c:/prog/stintmate/example-connector-ui.EXE"
        ));
        assert!(same_path(r"C:\a\b\", r"C:\a\b"));
        assert!(!same_path(r"C:\a\b.exe", r"C:\a\c.exe"));
        assert!(!same_path(r"C:\a\b.exe", r"D:\a\b.exe"));
    }

    #[test]
    fn written_once_and_left_alone_until_something_changes() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("connectors");
        let exe = Path::new(EXE);
        let p = write_in(&SPEC, &data, exe, r"Local\M", None, None).unwrap();
        let first = std::fs::read(&p).unwrap();
        assert!(!first.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_in(&SPEC, &data, exe, r"Local\M", None, None).unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            first,
            "only written_at would differ"
        );
        write_in(&SPEC, &data, exe, r"Local\M", Some("dev"), None).unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(after["flavour"], "dev");
        let names: Vec<_> = std::fs::read_dir(&data)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
    }

    #[test]
    fn remove_takes_only_our_descriptor_naming_this_install() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        let exe = Path::new(EXE);
        assert_eq!(remove_in(&SPEC, &data, exe).unwrap(), Removed::NotThere);
        let p = write_in(&SPEC, &data, exe, r"Local\M", None, None).unwrap();
        // Another install's launcher: left as it is.
        let other = Path::new(r"D:\other\example-connector-ui.exe");
        assert_eq!(
            remove_in(&SPEC, &data, other).unwrap(),
            Removed::OtherInstall(p.clone())
        );
        assert!(p.exists());
        // Ours, written differently.
        let same_exe = Path::new("c:/X/example-connector-ui.exe");
        assert_eq!(
            remove_in(&SPEC, &data, same_exe).unwrap(),
            Removed::Removed(p.clone())
        );
        assert!(!p.exists());
        // Another id, and not JSON.
        std::fs::write(
            &p,
            r#"{"id":"acc","launcher":"C:\\x\\example-connector-ui.exe"}"#,
        )
        .unwrap();
        assert!(matches!(
            remove_in(&SPEC, &data, exe).unwrap(),
            Removed::OtherInstall(_)
        ));
        std::fs::write(&p, "not json").unwrap();
        assert!(matches!(
            remove_in(&SPEC, &data, exe).unwrap(),
            Removed::OtherInstall(_)
        ));
        assert!(p.exists(), "left as it was");
    }

    #[test]
    fn the_product_key_is_a_lowercase_uuid_v4() {
        let b = SPEC.product.as_bytes();
        assert_eq!(b.len(), 36);
        for (i, c) in SPEC.product.chars().enumerate() {
            match i {
                8 | 13 | 18 | 23 => assert_eq!(c, '-'),
                14 => assert_eq!(c, '4'),
                19 => assert!("89ab".contains(c)),
                _ => assert!(
                    c.is_ascii_digit() || ('a'..='f').contains(&c),
                    "{}",
                    SPEC.product
                ),
            }
        }
    }

    #[test]
    fn register_refuses_another_product_and_replaces_one_with_none() {
        let dir = tempfile::tempdir().unwrap();
        let exe = Path::new(EXE);
        let p = dir.path().join(file_name(&SPEC));
        let theirs = r#"{"id":"example","product":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"}"#;
        std::fs::write(&p, theirs).unwrap();
        let e = write_in(&SPEC, dir.path(), exe, r"LocalM", None, None).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(e.to_string().contains("another product"), "{e}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), theirs);
        // A product that is not text is not ours either.
        std::fs::write(&p, r#"{"id":"example","product":5}"#).unwrap();
        assert!(write_in(&SPEC, dir.path(), exe, r"LocalM", None, None).is_err());
        // None (an older build), null, and our own are replaced.
        for old in [
            r#"{"id":"example"}"#,
            r#"{"id":"example","product":null}"#,
            &format!(r#"{{"id":"example","product":"{}"}}"#, SPEC.product),
        ] {
            std::fs::write(&p, old).unwrap();
            write_in(&SPEC, dir.path(), exe, r"LocalM", None, None).unwrap();
            let d: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert_eq!(d["product"], SPEC.product, "{old}");
        }
    }

    #[test]
    fn remove_needs_our_product_too() {
        let dir = tempfile::tempdir().unwrap();
        let exe = Path::new(EXE);
        let p = dir.path().join(file_name(&SPEC));
        for body in [
            format!(
                r#"{{"id":"example","launcher":{:?},"product":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"}}"#,
                EXE
            ),
            format!(r#"{{"id":"example","launcher":{:?}}}"#, EXE),
        ] {
            std::fs::write(&p, &body).unwrap();
            assert!(matches!(
                remove_in(&SPEC, dir.path(), exe).unwrap(),
                Removed::OtherInstall(_)
            ));
            assert_eq!(std::fs::read_to_string(&p).unwrap(), body);
        }
    }

    #[test]
    fn the_registration_file_text_is_exact() {
        let r = Registration {
            launcher: r"C:\Program Files\X\example-connector-ui.exe".into(),
            uninstall: vec![
                r"C:\Program Files\X\unins000.exe".into(),
                "/VERYSILENT".into(),
                r"/DIR=C:\a b".into(),
            ],
            flavour: "dev".into(),
        };
        assert_eq!(
            r.to_file_text(),
            "\u{feff}launcher=C:\\Program Files\\X\\example-connector-ui.exe\r\n\
             dir=C:\\Program Files\\X\r\n\
             uninstall=C:\\Program Files\\X\\unins000.exe\r\n\
             uninstall_args=/VERYSILENT \"/DIR=C:\\a b\"\r\n\
             flavour=dev\r\n"
        );
        let none = Registration {
            launcher: EXE.into(),
            uninstall: vec![],
            flavour: String::new(),
        };
        assert!(
            none.to_file_text()
                .contains("uninstall=\r\nuninstall_args=\r\nflavour=\r\n")
        );
    }

    #[test]
    fn read_ours_accepts_only_a_valid_file_of_this_connector() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(read_ours_in(&SPEC, d), None);
        write_in(
            &SPEC,
            d,
            Path::new(EXE),
            r"Local\M",
            Some("dev"),
            Some(Path::new(r"C:\x\u.exe")),
        )
        .unwrap();
        let r = read_ours_in(&SPEC, d).unwrap();
        assert_eq!((r.launcher.as_str(), r.flavour.as_str()), (EXE, "dev"));
        assert_eq!(r.uninstall[0], r"C:\x\u.exe");
        for bad in [
            "not json",
            r#"{"schema":1,"id":"acc","launcher":"x"}"#,
            r#"{"schema":2,"id":"example","launcher":"x"}"#,
            r#"{"schema":1,"id":"example"}"#,
        ] {
            std::fs::write(d.join(file_name(&SPEC)), bad).unwrap();
            assert_eq!(read_ours_in(&SPEC, d), None, "{bad}");
        }
    }
}
