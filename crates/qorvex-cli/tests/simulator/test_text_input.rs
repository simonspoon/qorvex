use super::harness::{get_value, go_to_tab, run, scroll_down, settle, try_run};

/// Dismiss any visible keyboard by swiping down on the scroll view.
/// Works because TextInputView has .scrollDismissesKeyboard(.immediately).
fn dismiss_keyboard() {
    let _ = try_run(&["swipe", "down"]);
    settle();
}

/// Reset the text input tab to a clean state.
fn reset_text_tab() {
    go_to_tab("Text Input");
    // Scroll down so the clear button clears the tab bar overlay
    scroll_down();
    scroll_down();
    run(&["tap", "text-clear-button"]);
    settle();
    // Scroll back up for the text fields
    go_to_tab("Text Input");
}

#[test]
#[ignore]
fn test_type_username() {
    reset_text_tab();

    // Tap the username field to focus it
    run(&["tap", "text-username-field"]);
    settle();

    // Type into it
    run(&["send-keys", "testuser"]);
    settle();

    // Dismiss keyboard and scroll to live preview
    dismiss_keyboard();
    scroll_down();
    scroll_down();

    let value = get_value("text-username-value");
    assert!(
        value.contains("testuser"),
        "Username value should contain 'testuser': {value}"
    );
}

#[test]
#[ignore]
fn test_type_email() {
    reset_text_tab();

    // Tap the email field to focus it
    run(&["tap", "text-email-field"]);
    settle();

    // Type an email
    run(&["send-keys", "test@example.com"]);
    settle();

    // Dismiss keyboard and scroll to live preview
    dismiss_keyboard();
    scroll_down();
    scroll_down();

    let value = get_value("text-email-value");
    assert!(
        value.contains("test@example.com"),
        "Email value should contain 'test@example.com': {value}"
    );
}

#[test]
#[ignore]
fn test_submit_shows_result() {
    reset_text_tab();

    // Type a username
    run(&["tap", "text-username-field"]);
    settle();
    run(&["send-keys", "submituser"]);
    settle();

    // Dismiss keyboard, scroll down to reach the submit button
    dismiss_keyboard();
    scroll_down();
    scroll_down();

    // Tap submit
    run(&["tap", "text-submit-button"]);

    // Wait for result to appear (it's conditional)
    run(&["wait-for", "text-submit-result"]);

    let result = get_value("text-submit-result");
    assert!(
        result.contains("Submitted"),
        "Submit result should contain 'Submitted': {result}"
    );
}

#[test]
#[ignore]
fn test_clear_resets_fields() {
    reset_text_tab();

    // Type in username
    run(&["tap", "text-username-field"]);
    settle();
    run(&["send-keys", "cleartest"]);
    settle();

    // Dismiss keyboard, scroll down to see values and clear button
    dismiss_keyboard();
    scroll_down();
    scroll_down();

    let value = get_value("text-username-value");
    assert!(value.contains("cleartest"), "Should have typed text: {value}");

    // Clear all
    run(&["tap", "text-clear-button"]);
    settle();

    // Username value should be empty (just the label prefix)
    let value = get_value("text-username-value");
    assert!(
        !value.contains("cleartest"),
        "Username should be cleared: {value}"
    );
}
