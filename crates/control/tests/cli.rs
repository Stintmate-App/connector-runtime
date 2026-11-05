//! The command line of `example-connector-ui.exe` run as a process: `--version`, `--register`,
//! `--unregister`, `--read-registration`, the usage message and the exit statuses. The
//! descriptor always goes to a temporary connectors folder (`STINTMATE_CONNECTORS_DIR`), never
//! to the real one; the options that start or stop the app (`--quit`) are tested through
//! `cli::quit` with unique names in `endpoint.rs`, never against a real running instance.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Runs the exe with the connectors folder at `conn` and no workspace anywhere.
fn run_in(conn: &Path, args: &[&str]) -> Output {
    Command::new(ui_exe_path())
        .args(args)
        .env_remove("STINTMATE_WORKSPACE")
        .env("STINTMATE_CONNECTORS_DIR", conn)
        .env(
            "STINTMATE_SETTINGS_FILE",
            std::env::temp_dir().join("example-cli-test-no-settings.json"),
        )
        .output()
        .expect("run example-connector-ui")
}

/// Runs the exe for options that never touch the connectors folder.
fn run(args: &[&str]) -> Output {
    let conn = tempfile::tempdir().unwrap();
    run_in(conn.path(), args)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn descriptor(conn: &Path) -> PathBuf {
    conn.join("example.json")
}

fn exe() -> String {
    ui_exe_path().to_string()
}

#[test]
fn version_prints_the_name_and_the_version() {
    let o = run(&["--version"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!(
            "example-connector-ui {}",
            stintmate_connector_testing::EXAMPLE_SPEC.version
        )
    );
}

#[test]
fn register_needs_no_workspace_and_writes_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path().join("connectors");
    let o = run_in(
        &conn,
        &[
            "--register",
            "--flavour",
            "dev",
            "--uninstall",
            r"C:\x\unins000.exe",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d: Value = serde_json::from_slice(&std::fs::read(descriptor(&conn)).unwrap()).unwrap();
    assert_eq!(d["schema"], 1);
    assert_eq!(d["id"], "example");
    assert_eq!(d["flavour"], "dev");
    assert_eq!(d["window"], true);
    assert_eq!(
        d["features"],
        serde_json::json!([
            "window",
            "notices",
            "translated_texts",
            "remembered_services",
            "options",
            "live_frame",
            "lap_status"
        ])
    );
    assert_eq!(d["product"], "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43");
    assert_eq!(
        d["uninstall"],
        serde_json::json!([
            r"C:\x\unins000.exe",
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART"
        ])
    );
    assert_eq!(d["launcher"].as_str().unwrap(), exe());
    for k in [
        "name",
        "sim",
        "product",
        "sim_name",
        "version",
        "args",
        "control_protocol",
        "endpoint",
        "mutex",
        "services",
        "logs",
        "written_at",
        "written_by",
    ] {
        assert!(d.get(k).is_some(), "{k}");
    }
    assert!(
        !std::fs::read(descriptor(&conn))
            .unwrap()
            .starts_with(&[0xEF, 0xBB, 0xBF]),
        "the descriptor has no BOM"
    );
    // Without the optional keys they are not written.
    let plain = dir.path().join("plain");
    assert_eq!(run_in(&plain, &["--register"]).status.code(), Some(0));
    let d: Value = serde_json::from_slice(&std::fs::read(descriptor(&plain)).unwrap()).unwrap();
    assert!(d.get("flavour").is_none() && d.get("uninstall").is_none());
}

#[test]
fn an_unchanged_second_register_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    assert_eq!(run_in(conn, &["--register"]).status.code(), Some(0));
    let first = std::fs::read(descriptor(conn)).unwrap();
    let mtime = std::fs::metadata(descriptor(conn))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let o = run_in(conn, &["--register"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(std::fs::read(descriptor(conn)).unwrap(), first);
    assert_eq!(
        std::fs::metadata(descriptor(conn))
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
    // Another flavour is a change.
    run_in(conn, &["--register", "--flavour", "dev"]);
    let d: Value = serde_json::from_slice(&std::fs::read(descriptor(conn)).unwrap()).unwrap();
    assert_eq!(d["flavour"], "dev");
}

#[test]
fn register_that_cannot_write_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("a-file");
    std::fs::write(&blocked, "x").unwrap();
    let o = run_in(&blocked, &["--register"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
}

#[test]
fn unregister_removes_only_a_file_that_names_this_install() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    // Ours.
    run_in(conn, &["--register"]);
    let o = run_in(conn, &["--unregister"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("unregistered"));
    assert!(!descriptor(conn).exists());
    // Nothing there.
    let o = run_in(conn, &["--unregister"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("nothing to remove"));
    // Another install's launcher, another id, not JSON: left as it is, exit 0.
    let other_id = serde_json::json!({"schema": 1, "id": "acc", "launcher": exe()}).to_string();
    let bodies = [
        r#"{"schema":1,"id":"example","launcher":"D:\\other\\example-connector-ui.exe"}"#,
        other_id.as_str(),
        "not json",
    ];
    for body in bodies {
        std::fs::write(descriptor(conn), body).unwrap();
        let o = run_in(conn, &["--unregister"]);
        assert_eq!(o.status.code(), Some(0), "{body}: {}", text(&o));
        assert!(text(&o).contains("left as it is"), "{}", text(&o));
        assert_eq!(std::fs::read_to_string(descriptor(conn)).unwrap(), body);
    }
}

#[test]
fn register_refuses_another_product_and_replaces_a_descriptor_with_none() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    let theirs = serde_json::json!({"schema": 1, "id": "example", "launcher": exe(),
        "product": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})
    .to_string();
    std::fs::write(descriptor(conn), &theirs).unwrap();
    let o = run_in(conn, &["--register"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("another product"), "{}", text(&o));
    assert_eq!(std::fs::read_to_string(descriptor(conn)).unwrap(), theirs);
    // An older build wrote no product: it is replaced.
    let old = serde_json::json!({"schema": 1, "id": "example", "launcher": exe()}).to_string();
    std::fs::write(descriptor(conn), old).unwrap();
    let o = run_in(conn, &["--register"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d: Value = serde_json::from_slice(&std::fs::read(descriptor(conn)).unwrap()).unwrap();
    assert_eq!(d["product"], "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43");
}

#[test]
fn unregister_leaves_a_descriptor_of_another_or_no_product() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    for product in [Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"), None] {
        let mut d = serde_json::json!({"schema": 1, "id": "example", "launcher": exe()});
        if let Some(p) = product {
            d["product"] = p.into();
        }
        let body = d.to_string();
        std::fs::write(descriptor(conn), &body).unwrap();
        let o = run_in(conn, &["--unregister"]);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));
        assert!(text(&o).contains("left as it is"), "{}", text(&o));
        assert_eq!(std::fs::read_to_string(descriptor(conn)).unwrap(), body);
    }
}

#[test]
fn unregister_takes_the_launcher_written_with_slashes_and_other_case() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    let launcher = exe().replace('\\', "/").to_uppercase();
    let doc = serde_json::json!({"schema": 1, "id": "example", "launcher": launcher,
        "product": "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43"});
    std::fs::write(descriptor(conn), doc.to_string()).unwrap();
    let o = run_in(conn, &["--unregister"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(!descriptor(conn).exists(), "{}", text(&o));
}

#[test]
fn read_registration_has_three_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path().join("c");
    let out = dir.path().join("registration.txt");

    // No file: exit 2, nothing written.
    let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(!out.exists());

    // A file that is not valid or not ours: exit 2 as well.
    std::fs::create_dir_all(&conn).unwrap();
    for body in ["not json", r#"{"schema":1,"id":"acc","launcher":"x"}"#] {
        std::fs::write(descriptor(&conn), body).unwrap();
        let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(2), "{body}: {}", text(&o));
        assert!(!out.exists());
    }

    // Ours: the exact lines, BOM and CRLF included.
    let uninstall = r"C:\Program Files\Stint mate\unins000.exe";
    assert_eq!(
        run_in(
            &conn,
            &["--register", "--flavour", "dev", "--uninstall", uninstall]
        )
        .status
        .code(),
        Some(0)
    );
    let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let exe = exe();
    let dir_of = Path::new(&exe).parent().unwrap().display().to_string();
    let expected = format!(
        "\u{feff}launcher={exe}\r\ndir={dir_of}\r\nuninstall={uninstall}\r\nuninstall_args=/VERYSILENT /SUPPRESSMSGBOXES /NORESTART\r\nflavour=dev\r\n"
    );
    assert_eq!(std::fs::read(&out).unwrap(), expected.as_bytes());

    // Without uninstall and flavour: empty values.
    run_in(&conn, &["--unregister"]);
    run_in(&conn, &["--register"]);
    let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let t = std::fs::read_to_string(&out).unwrap();
    assert!(
        t.ends_with("uninstall=\r\nuninstall_args=\r\nflavour=\r\n"),
        "{t:?}"
    );

    // The out file cannot be written: exit 1.
    let bad = dir.path().join("no-such-folder").join("o.txt");
    let o = run_in(&conn, &["--read-registration", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
}

/// Risk 4 of the spec: the registration file must survive a non-ASCII folder. The BOM is what
/// makes Inno's `LoadStringsFromFile` read it as UTF-8; here the bytes are checked.
#[test]
fn read_registration_keeps_non_ascii_paths_as_utf8_with_a_bom() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path().join("conectores-\u{c1}\u{f1}o-\u{30b3}");
    let out = dir.path().join("sal\u{ed}da-\u{30b3}.txt");
    let uninstall = "C:\\Usuarios\\Jos\u{e9} Pe\u{f1}a\\unins000.exe";
    let o = run_in(
        &conn,
        &["--register", "--flavour", "dev", "--uninstall", uninstall],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    let t = String::from_utf8(bytes[3..].to_vec()).expect("valid UTF-8");
    assert!(t.contains(&format!("uninstall={uninstall}\r\n")), "{t}");
    // A launcher in a non-ASCII folder (written by hand: the test exe lives in an ASCII one).
    let doc = serde_json::json!({
        "schema": 1,
        "id": "example",
        "launcher": "C:\\Usuarios\\Jos\u{e9} Pe\u{f1}a\\\u{30b3}\\example-connector-ui.exe",
    });
    std::fs::write(descriptor(&conn), doc.to_string()).unwrap();
    let o = run_in(&conn, &["--read-registration", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    let t = String::from_utf8(bytes[3..].to_vec()).expect("valid UTF-8");
    assert!(
        t.contains("dir=C:\\Usuarios\\Jos\u{e9} Pe\u{f1}a\\\u{30b3}\r\n"),
        "{t}"
    );
}

#[test]
fn unknown_options_do_not_stop_a_command() {
    let conn = tempfile::tempdir().unwrap();
    let o = run_in(
        conn.path(),
        &["--register", "--check", "--newer-hub-flag=1"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(descriptor(conn.path()).is_file());
    let o = run(&["--frobnicate", "value", "--version"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).starts_with("example-connector-ui "));
    // Known options keep their errors beside unknown ones.
    assert_eq!(
        run(&["--frobnicate", "--parent-pid", "x"]).status.code(),
        Some(2)
    );
}

#[test]
fn register_and_unregister_take_no_workspace() {
    let dir = tempfile::tempdir().unwrap();
    for opt in ["--register", "--unregister"] {
        let o = run_in(
            dir.path(),
            &[opt, "--workspace", dir.path().to_str().unwrap()],
        );
        assert_eq!(o.status.code(), Some(2), "{opt}: {}", text(&o));
    }
    assert!(!descriptor(dir.path()).exists());
}

#[test]
fn bad_arguments_print_the_usage_with_every_option() {
    let o = run(&["--parent-pid", "x"]);
    assert_eq!(o.status.code(), Some(2));
    let t = text(&o);
    for opt in [
        "--tray",
        "--managed",
        "--parent-pid",
        "--register",
        "--flavour",
        "--uninstall",
        "--unregister",
        "--read-registration",
        "--stintmate-hold",
        "--stintmate-release",
        "--no-start",
        "--purpose",
        "--wait",
        "--quit",
        "--version",
        "--workspace",
        "--allow-synced",
        "STINTMATE_CONNECTORS_DIR",
    ] {
        assert!(t.contains(opt), "usage lacks {opt}: {t}");
    }
    assert!(!t.contains("--check"), "{t}");
    for combo in [
        &["--unregister", "--managed", "--parent-pid", "5"][..],
        &["--quit", "--managed", "--parent-pid", "5"][..],
        &["--register", "--unregister"][..],
        &["--version", "--quit"][..],
        &["--read-registration", "o.txt", "--register"][..],
    ] {
        assert_eq!(run(combo).status.code(), Some(2), "{combo:?}");
    }
}

#[test]
fn the_hub_options_find_no_stintmate_without_a_workspace_or_a_key_and_check_arguments() {
    // No pointer anywhere: there is no Stintmate to ask (5), nothing is started.
    for opt in ["--stintmate-hold", "--stintmate-release"] {
        let o = run(&[opt]);
        assert_eq!(o.status.code(), Some(5), "{opt}: {}", text(&o));
    }
    // A workspace without an `ipc\key`: still nobody to ask.
    let ws = tempfile::tempdir().unwrap();
    let o = Command::new(ui_exe_path())
        .args(["--stintmate-hold", "--wait", "1"])
        .env("STINTMATE_WORKSPACE", ws.path())
        .env("STINTMATE_CONNECTORS_DIR", ws.path().join("c"))
        .env("STINTMATE_DEV_INSTANCE", "clitest-no-key")
        .env(
            "STINTMATE_SETTINGS_FILE",
            std::env::temp_dir().join("example-cli-test-no-settings.json"),
        )
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5), "{}", text(&o));
    for combo in [
        &["--stintmate-hold", "--purpose", "bogus"][..],
        &["--stintmate-hold", "--stintmate-release"][..],
        &["--stintmate-hold", "--workspace", "C:/w"][..],
        &["--stintmate-release", "--wait", "3"][..],
        &["--no-start"][..],
    ] {
        assert_eq!(run(combo).status.code(), Some(2), "{combo:?}");
    }
}

/// The example control app built by `cargo test` (an example of this crate), next to `deps\`.
fn ui_exe_path() -> String {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    let exe = p.join("examples").join(format!(
        "{}.exe",
        stintmate_connector_testing::EXAMPLE_SPEC.ui_app_id()
    ));
    assert!(exe.is_file(), "build the example first: {}", exe.display());
    exe.display().to_string()
}
