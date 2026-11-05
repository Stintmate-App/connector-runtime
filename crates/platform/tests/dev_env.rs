//! `STINTMATE_DEV_INSTANCE` end to end. One test only: it changes the process
//! environment, so it must not share a process with other tests.
#![cfg(windows)]

mod common;

use common::SPEC;
use stintmate_connector_platform::dev;
use stintmate_connector_platform::single::Names;
use stintmate_connector_platform::spec::ConnectorSpec;

/// A connector that had its own variable before.
static WITH_ALIAS: ConnectorSpec = ConnectorSpec {
    dev_env_aliases: &["EXAMPLE_OLD_DEV_INSTANCE"],
    ..SPEC
};

#[test]
fn the_environment_variable_isolates_names_and_pipes() {
    // SAFETY: the only test of this binary, so no other thread reads the environment.
    unsafe {
        std::env::remove_var(dev::ENV);
        std::env::remove_var("EXAMPLE_OLD_DEV_INSTANCE");
    }
    assert_eq!(dev::ENV, "STINTMATE_DEV_INSTANCE");
    assert_eq!(Names::for_spec(&SPEC), Names::production(&SPEC));
    let real = dev::pipe_path(&SPEC, "pitwall").unwrap();
    assert!(dev::pipe_path_if_dev(&SPEC, "pitwall").is_none());

    unsafe { std::env::set_var(dev::ENV, "Desk 1") };
    assert_eq!(dev::tag(&SPEC).as_deref(), Some("desk-1"));
    assert_eq!(Names::for_spec(&SPEC), Names::tagged(&SPEC, "desk-1"));
    assert_eq!(
        Names::for_spec(&SPEC).ui,
        r"Local\ExampleConnector.UI.desk-1"
    );
    let tagged = dev::pipe_path(&SPEC, "pitwall").unwrap();
    assert_eq!(tagged, format!("{real}-desk-1"));
    assert_eq!(dev::pipe_path_if_dev(&SPEC, "pitwall"), Some(tagged));

    // The kit's variable wins over a connector's own older one.
    unsafe { std::env::set_var("EXAMPLE_OLD_DEV_INSTANCE", "old") };
    assert_eq!(
        dev::tag(&SPEC).as_deref(),
        Some("desk-1"),
        "no alias on this spec"
    );
    assert_eq!(dev::tag(&WITH_ALIAS).as_deref(), Some("desk-1"));
    unsafe { std::env::remove_var(dev::ENV) };
    assert_eq!(dev::tag(&WITH_ALIAS).as_deref(), Some("old"));
    assert_eq!(dev::tag(&SPEC), None);
    unsafe { std::env::remove_var("EXAMPLE_OLD_DEV_INSTANCE") };

    unsafe { std::env::set_var(dev::ENV, "  ") };
    assert_eq!(Names::for_spec(&SPEC), Names::production(&SPEC));
    assert_eq!(dev::pipe_path(&SPEC, "pitwall").unwrap(), real);
}
