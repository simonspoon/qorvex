use super::harness::{go_to_tab, run, settle};

/// Reset the Dynamic tab to initial state.
fn reset_dynamic_tab() {
    go_to_tab("Dynamic");
    run(&["tap", "dynamic-reset"]);
    settle();
}

#[test]
#[ignore]
fn test_tap_while_spinner_active() {
    reset_dynamic_tab();

    // Start loading — this shows a ProgressView spinner for 2 seconds,
    // which is an ongoing animation that previously caused XCUITest's
    // quiescence wait to block element.tap() indefinitely.
    run(&["tap", "dynamic-start-loading"]);

    // Confirm spinner appeared
    run(&["wait-for", "dynamic-loading-spinner", "-o", "3000"]);

    // While the spinner is still animating, tap a different element.
    // Before the coordinate-based tap fix, this would hang until timeout.
    run(&["tap", "dynamic-toggle-visibility"]);

    // The togglable element should appear, proving the tap landed.
    run(&["wait-for", "dynamic-togglable", "-o", "3000"]);
}
