//! The start-up steps. These tests install the process-wide logger, so they live in their own
//! test binary.

use stintmate_connector_link::boot::{RecorderArgs, boot_with, pipes};
use stintmate_connector_link::link::LinkState;
use stintmate_connector_platform::single::Names;
use stintmate_connector_testing::{EXAMPLE_SPEC, TempWorkspace, unique_user};

static SPEC: stintmate_connector_platform::spec::ConnectorSpec = EXAMPLE_SPEC;

fn workspace() -> TempWorkspace {
    let ws = TempWorkspace::new();
    std::fs::write(
        ws.path().join("stintmate.json"),
        r#"{"schema": 1, "created_at": "2026-10-18T19:30:00+02:00"}"#,
    )
    .unwrap();
    ws
}

fn args(ws: &TempWorkspace, user: &str) -> RecorderArgs {
    RecorderArgs::parse([
        "--workspace".to_owned(),
        ws.path().display().to_string(),
        "--pitwall-pipe".to_owned(),
        format!("boot-pitwall-{user}"),
        "--control-pipe".to_owned(),
        format!("boot-control-{user}"),
    ])
    .unwrap()
}

#[test]
fn boot_starts_the_links_with_the_specs_defaults() {
    let ws = workspace();
    let a = args(&ws, &unique_user());
    let names = Names::unique(&SPEC, "boot");
    let booted = boot_with(&SPEC, &a, names, None).expect("boots");
    assert_eq!(booted.hz, 60.0);
    assert_eq!(
        booted.location.data_dir,
        ws.path().join("connectors").join("example")
    );
    assert_eq!(booted.pitwall.state().0, LinkState::Waiting);
    assert!(booted.control.is_some());
    assert!(!booted.stop.is_set());
    assert!(
        ws.path()
            .join("connectors")
            .join("example")
            .join("recorder.log")
            .exists()
    );
}

#[test]
fn boot_refuses_second_instance() {
    let ws = workspace();
    let a = args(&ws, &unique_user());
    let first = boot_with(&SPEC, &a, Names::unique(&SPEC, "dup"), None);
    assert!(first.is_ok());
    let again = Names {
        recorder: first.as_ref().ok().unwrap().instance.name().to_owned(),
        ..Names::unique(&SPEC, "dup2")
    };
    // A mutex is owned by the creating thread, so the second recorder starts on another one.
    let second = std::thread::spawn(move || boot_with(&SPEC, &a, again, None).err())
        .join()
        .unwrap();
    assert_eq!(second, Some(1));
}

#[test]
fn boot_exit_codes() {
    let ws = workspace();
    let mut a = args(&ws, &unique_user());
    a.version = true;
    assert_eq!(
        boot_with(&SPEC, &a, Names::unique(&SPEC, "v"), None).err(),
        Some(0)
    );
    let mut a = args(&ws, &unique_user());
    a.workspace = Some(ws.path().join("missing").display().to_string());
    assert_eq!(
        boot_with(&SPEC, &a, Names::unique(&SPEC, "w"), None).err(),
        Some(3)
    );
}

#[test]
fn an_invalid_spec_exits_2() {
    static BAD: stintmate_connector_platform::spec::ConnectorSpec =
        stintmate_connector_platform::spec::ConnectorSpec {
            id: "Bad Id",
            ..EXAMPLE_SPEC
        };
    let ws = workspace();
    let a = args(&ws, &unique_user());
    assert_eq!(
        boot_with(&BAD, &a, Names::unique(&BAD, "bad"), None).err(),
        Some(2)
    );
}

#[test]
fn boot_with_dev_tag_uses_tagged_pipes() {
    let plain = RecorderArgs::default();
    let (p0, c0) = pipes(&SPEC, None, &plain).unwrap();
    let (p1, c1) = pipes(&SPEC, Some("desk1"), &plain).unwrap();
    assert_ne!(p0, p1);
    assert_ne!(c0, c1);
    assert!(p1.ends_with("-desk1"), "{p1}");
    assert!(c1.contains("connector-example"), "{c1}");
    assert!(c1.ends_with("-desk1"), "{c1}");
    assert!(!p0.ends_with("-desk1"));
    // An explicit pipe wins over the tag.
    let given = RecorderArgs {
        pitwall_pipe: Some("mine".into()),
        ..RecorderArgs::default()
    };
    assert_eq!(pipes(&SPEC, Some("desk1"), &given).unwrap().0, "mine");
}
