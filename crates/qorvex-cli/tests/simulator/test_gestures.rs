use super::harness::{go_to_tab, run, settle};

#[test]
#[ignore]
fn test_swipe_up() {
    go_to_tab("Gestures");
    settle();

    // Swipe up — this will scroll the view
    run(&["swipe", "up"]);
    settle();

    // After swiping up, lower-numbered scroll items may have scrolled off screen
    // and higher-numbered items should be visible.
    // Verify by checking if a higher item is now findable.
    // (This is a basic smoke test that swipe doesn't error.)
}

#[test]
#[ignore]
fn test_swipe_down() {
    go_to_tab("Gestures");
    settle();

    // Swipe down
    run(&["swipe", "down"]);
    settle();

    // Swipe command completed successfully — basic smoke test
}

#[test]
#[ignore]
fn test_tap_location() {
    go_to_tab("Gestures");
    settle();

    // Tap at a coordinate in the center-ish area of the screen
    // (200, 400) is a reasonable center point for an iPhone simulator
    run(&["tap-location", "200", "400"]);
    settle();

    // The tap-location command completed without error.
    // If the tap happened to land on gesture-tap-area, we'd see coordinates.
    // This is primarily a smoke test for the tap-location command.
}
