use super::harness::{get_value, go_to_tab, run, settle};

/// Reset the Dynamic tab to initial state.
fn reset_dynamic_tab() {
    go_to_tab("Dynamic");
    run(&["tap", "dynamic-reset"]);
    settle();
}

#[test]
#[ignore]
fn test_delayed_appearance() {
    reset_dynamic_tab();

    // Trigger delayed appearance (2s delay)
    run(&["tap", "dynamic-show-delayed"]);

    // Wait for the element (5s timeout to account for 2s delay + margin)
    run(&["wait-for", "dynamic-delayed-label", "-o", "5000"]);

    let label = get_value("dynamic-delayed-label");
    assert!(
        label.contains("appeared"),
        "Delayed label should say 'I appeared!': {label}"
    );
}

#[test]
#[ignore]
fn test_auto_disappear() {
    reset_dynamic_tab();

    // Trigger brief appearance (visible for 3s)
    run(&["tap", "dynamic-show-brief"]);

    // Wait for it to appear
    run(&["wait-for", "dynamic-brief-label", "-o", "3000"]);

    // Now wait for it to disappear (3s from appearance + margin)
    run(&["wait-for-not", "dynamic-brief-label", "-o", "5000"]);
}

#[test]
#[ignore]
fn test_loading_lifecycle() {
    reset_dynamic_tab();

    // Start loading (2s duration)
    run(&["tap", "dynamic-start-loading"]);

    // Spinner should appear
    run(&["wait-for", "dynamic-loading-spinner", "-o", "3000"]);

    // Spinner should disappear after ~2s
    run(&["wait-for-not", "dynamic-loading-spinner", "-o", "5000"]);

    // "Loading complete" should appear
    run(&["wait-for", "dynamic-loading-done", "-o", "3000"]);

    let done = get_value("dynamic-loading-done");
    assert!(
        done.contains("complete"),
        "Loading done should say 'Loading complete': {done}"
    );
}

#[test]
#[ignore]
fn test_toggle_visibility() {
    reset_dynamic_tab();

    // Toggle on
    run(&["tap", "dynamic-toggle-visibility"]);
    run(&["wait-for", "dynamic-togglable", "-o", "3000"]);

    let visible = get_value("dynamic-togglable");
    assert!(
        visible.contains("Visible"),
        "Togglable should be visible: {visible}"
    );

    // Toggle off
    run(&["tap", "dynamic-toggle-visibility"]);
    run(&["wait-for-not", "dynamic-togglable", "-o", "3000"]);
}

#[test]
#[ignore]
fn test_counter_increments() {
    reset_dynamic_tab();

    // Start counter
    run(&["tap", "dynamic-start-counter"]);

    // Wait for counter to appear
    run(&["wait-for", "dynamic-counter-value", "-o", "3000"]);

    // Wait a couple seconds for counter to increment
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Counter should be > 0
    let value = get_value("dynamic-counter-value");
    assert!(
        value.contains("Count:"),
        "Counter value should contain 'Count:': {value}"
    );

    // Extract the number and verify it incremented
    let count: i32 = value
        .split(':')
        .last()
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);
    assert!(count > 0, "Counter should have incremented above 0: {count}");

    // Stop counter
    run(&["tap", "dynamic-stop-counter"]);
    settle();
}

#[test]
#[ignore]
fn test_reset_clears_all() {
    reset_dynamic_tab();

    // Toggle on an element
    run(&["tap", "dynamic-toggle-visibility"]);
    run(&["wait-for", "dynamic-togglable", "-o", "3000"]);

    // Reset all
    run(&["tap", "dynamic-reset"]);
    settle();

    // Togglable should be gone
    run(&["wait-for-not", "dynamic-togglable", "-o", "3000"]);
}
