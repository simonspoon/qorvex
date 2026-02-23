use super::harness::{go_to_tab, harness, run, run_fail, run_json, settle};

#[test]
#[ignore]
fn test_screenshot_returns_data() {
    harness(); // ensure server is running
    let output = run(&["screenshot"]);
    assert!(
        output.len() > 100,
        "Screenshot should return substantial base64 data, got {} bytes",
        output.len()
    );
}

#[test]
#[ignore]
fn test_screen_info_returns_elements() {
    go_to_tab("Controls");
    let output = run(&["screen-info"]);
    // Should contain at least some element identifiers from the Controls tab
    assert!(
        output.contains("controls-") || output.contains("Button") || output.contains("StaticText"),
        "screen-info should contain UI elements: {output}"
    );
}

#[test]
#[ignore]
fn test_screen_info_full_is_valid_json() {
    harness();
    let output = run(&["screen-info", "--full"]);
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(output.trim());
    assert!(
        parsed.is_ok(),
        "screen-info --full should return valid JSON: {}",
        &output[..output.len().min(200)]
    );
}

#[test]
#[ignore]
fn test_screen_info_pretty_is_formatted() {
    harness();
    let output = run(&["screen-info", "--pretty"]);
    // Pretty output should contain bracketed type annotations like [Button] or [StaticText]
    assert!(
        output.contains('[') && output.contains(']'),
        "screen-info --pretty should have formatted type brackets: {output}"
    );
}

#[test]
#[ignore]
fn test_json_output_format() {
    go_to_tab("Controls");
    let json = run_json(&["tap", "controls-tap-button"]);
    assert!(
        json.get("success").is_some() || json.get("result").is_some(),
        "JSON output should contain success or result field: {json}"
    );
}

#[test]
#[ignore]
fn test_status_command() {
    harness();
    let output = run(&["status"]);
    assert!(
        !output.is_empty(),
        "Status command should return non-empty output"
    );
}

#[test]
#[ignore]
fn test_get_value_nonexistent_fails() {
    harness();
    let output = run_fail(&["get-value", "nonexistent-element-xyz-999", "--no-wait"]);
    assert!(
        !output.is_empty(),
        "get-value on nonexistent element should produce error output"
    );
}

#[test]
#[ignore]
fn test_wait_for_timeout_fails() {
    harness();
    // This should fail within ~1 second
    let output = run_fail(&["wait-for", "nonexistent-element-xyz-999", "-o", "1000"]);
    assert!(
        !output.is_empty(),
        "wait-for on nonexistent element should produce error output"
    );
}

#[test]
#[ignore]
fn test_set_target() {
    harness();
    // Re-set target (already set in harness init, but verifying the command works)
    let output = run(&["set-target", "com.qorvex.testapp"]);
    // Should succeed without error
    let _ = output;
}

#[test]
#[ignore]
fn test_comment_and_log() {
    harness();
    let marker = format!("test-marker-{}", std::process::id());

    // Log a comment
    run(&["comment", &marker]);
    settle();

    // Retrieve the log
    let log_output = run(&["log"]);

    assert!(
        log_output.contains(&marker),
        "Log should contain the comment marker '{marker}': {log_output}"
    );
}
