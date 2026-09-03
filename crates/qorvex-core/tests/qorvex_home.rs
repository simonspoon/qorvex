//! `QORVEX_HOME` resolution for the qorvex state directory.
//!
//! This lives in its own integration test binary, and holds a single test, so
//! that mutating the process environment cannot race another test.

use qorvex_core::ipc::{qorvex_dir, socket_path};

#[test]
fn qorvex_home_overrides_home_directory() {
    let scratch = std::env::temp_dir().join(format!("qorvex-home-test-{}", std::process::id()));

    std::env::set_var("QORVEX_HOME", &scratch);
    assert_eq!(qorvex_dir(), scratch);
    assert_eq!(
        socket_path("demo"),
        scratch.join("qorvex_demo.sock"),
        "sockets follow QORVEX_HOME"
    );
    assert!(scratch.is_dir(), "the directory is created if missing");

    // An empty value is treated as unset, so it falls back to $HOME/.qorvex
    // rather than resolving the state directory to the process working dir.
    std::env::set_var("QORVEX_HOME", "");
    assert_eq!(
        qorvex_dir(),
        dirs::home_dir().expect("home directory").join(".qorvex")
    );

    std::env::remove_var("QORVEX_HOME");
    std::fs::remove_dir_all(&scratch).ok();
}
