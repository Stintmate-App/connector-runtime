//! A window executable for the example connector: what the tests start in place of a
//! connector's `<prefix>-connector-window.exe`.
#![windows_subsystem = "windows"]

/// The example connector with its UI catalog.
static SPEC: stintmate_connector_platform::spec::ConnectorSpec =
    stintmate_connector_platform::spec::ConnectorSpec {
        ui_catalog: include_str!("../tests/fixtures/en.example.json"),
        ..stintmate_connector_testing::EXAMPLE_SPEC
    };

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        stintmate_connector_platform::console::attach_parent_output();
        println!("{}-connector-window {}", SPEC.exe_prefix, SPEC.version);
        return;
    }
    std::process::exit(stintmate_connector_window::run(&SPEC, args));
}
