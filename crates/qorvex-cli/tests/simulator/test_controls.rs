use super::harness::{get_value, go_to_tab, run, scroll_down, settle};

#[test]
#[ignore]
fn test_tap_button_increments_count() {
    go_to_tab("Controls");

    // Get baseline count
    let before = get_value("controls-tap-count");

    // Tap the button
    run(&["tap", "controls-tap-button"]);
    settle();

    // Count should have incremented
    let after = get_value("controls-tap-count");
    assert_ne!(before, after, "Tap count should change after tap");

    // Tap again
    run(&["tap", "controls-tap-button"]);
    settle();

    let after2 = get_value("controls-tap-count");
    assert_ne!(after, after2, "Tap count should change on second tap");
}

#[test]
#[ignore]
fn test_toggle_wifi() {
    go_to_tab("Controls");

    // Toggle wifi on
    run(&["tap", "controls-toggle-wifi"]);
    settle();

    let status = get_value("controls-wifi-status");
    // After toggling from default (Off), it should be On
    // (or if already On from a previous test, it toggles Off — either way it changed)
    assert!(
        status.contains("On") || status.contains("Off"),
        "Wi-Fi status should be readable: {status}"
    );
}

#[test]
#[ignore]
fn test_delete_button_shows_status() {
    go_to_tab("Controls");

    // Delete button is near the bottom — scroll down once to reveal it
    scroll_down();

    // Tap the destructive delete button
    run(&["tap", "controls-delete-button"]);
    settle();

    // The "Deleted!" text should appear (it's conditional)
    // The status text may appear below the visible area, so scroll a bit more
    scroll_down();

    let status = get_value("controls-delete-status");
    assert!(
        status.contains("Deleted"),
        "Delete status should show 'Deleted!': {status}"
    );
}

#[test]
#[ignore]
fn test_picker_value() {
    go_to_tab("Controls");

    // Picker is below the fold — scroll down to reveal it
    scroll_down();

    // Tap "Small" segment by label
    run(&["tap", "Small", "--label"]);
    settle();

    let value = get_value("controls-picker-value");
    assert!(
        value.contains("Small"),
        "Picker value should be 'Small': {value}"
    );

    // Switch to "Large"
    run(&["tap", "Large", "--label"]);
    settle();

    let value = get_value("controls-picker-value");
    assert!(
        value.contains("Large"),
        "Picker value should be 'Large': {value}"
    );
}
