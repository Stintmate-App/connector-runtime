//! The priority a connector process puts itself at, read back with `GetPriorityClass`.

#![cfg(windows)]

use stintmate_connector_platform::proc::{lower_process_priority, process_priority_class};

const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[test]
fn a_process_lowers_itself_to_below_normal() {
    assert!(lower_process_priority());
    assert_eq!(process_priority_class(), BELOW_NORMAL_PRIORITY_CLASS);
    // Calling it again is harmless.
    assert!(lower_process_priority());
    assert_eq!(process_priority_class(), BELOW_NORMAL_PRIORITY_CLASS);
}
