//! Shared by the tests that start the example window executable.

use std::path::PathBuf;

/// The example connector's window executable, built from `examples/example_window.rs`
/// (`cargo test` builds the examples of the package under test).
pub fn window_exe() -> PathBuf {
    let mut dir = std::env::current_exe().unwrap();
    dir.pop(); // deps
    dir.pop(); // the profile folder
    let exe = dir.join("examples").join("example_window.exe");
    assert!(exe.exists(), "{} was not built", exe.display());
    exe
}
