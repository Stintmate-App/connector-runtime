//! The spec the unit tests use: the example connector with the example UI catalog.

use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_testing::EXAMPLE_SPEC;

/// The example UI catalog.
pub(crate) const CATALOG: &str = include_str!("../tests/fixtures/en.example.json");

/// The example connector with its UI catalog.
pub(crate) static SPEC: ConnectorSpec = ConnectorSpec {
    ui_catalog: CATALOG,
    ..EXAMPLE_SPEC
};
