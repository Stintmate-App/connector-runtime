//! A control app for the example connector: the exe the command-line tests run as a process.
#![windows_subsystem = "windows"]

fn main() {
    let code = stintmate_connector_control::run(
        &stintmate_connector_testing::EXAMPLE_SPEC,
        stintmate_connector_control::Hooks::default(),
        std::env::args().skip(1).collect(),
    );
    std::process::exit(code);
}
