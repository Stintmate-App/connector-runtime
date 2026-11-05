//! `--version` of the example window executable: prints `<exe name> <version>` and exits 0.
//! The runtime's `run` leaves `--version` to the executable; this proves an executable built on
//! it can answer before any window or workspace is touched.

mod common;

#[test]
fn version_prints_the_name_and_the_version() {
    let o = std::process::Command::new(common::window_exe())
        .arg("--version")
        .output()
        .expect("run example-connector-window");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("example-connector-window {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn an_unknown_option_exits_2() {
    let o = std::process::Command::new(common::window_exe())
        .arg("--nope")
        .output()
        .expect("run example-connector-window");
    assert_eq!(o.status.code(), Some(2));
}
