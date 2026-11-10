//! A made-up connector for the tests: nothing here names a real simulator.
#![allow(dead_code)]

use stintmate_connector_platform::spec::{ConnectorSpec, GameSpec};
use stintmate_connector_resources::Glyph;

/// The example connector. Its game has a known window title and class.
pub const SPEC: ConnectorSpec = ConnectorSpec {
    id: "example",
    name: "Example connector",
    sim: "example",
    sim_name: "Example Simulator",
    product: "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43",
    version: "0.1.0",
    exe_prefix: "example",
    mutex_prefix: "ExampleConnector",
    autostart_value: "Stintmate Example connector",
    game: GameSpec {
        exes: &["example-sim.exe", "example-sim-dx11.exe"],
        window_title: Some("example simulator"),
        window_class: Some("ExampleWindowClass"),
        cheap_hint: None,
    },
    recorder_hz: 60.0,
    features: &["live_frame", "lap_status", "window"],
    ui_catalog: "{}",
    glyph: Glyph::I,
    icon: None,
    dev_env_aliases: &[],
};
