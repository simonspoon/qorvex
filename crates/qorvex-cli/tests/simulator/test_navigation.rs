use super::harness::{get_value, go_to_tab, run, settle};

#[test]
#[ignore]
fn test_push_navigation() {
    go_to_tab("Navigation");

    // Push to detail view
    run(&["tap", "nav-push-button"]);

    // Wait for detail page to appear
    run(&["wait-for", "nav-detail-label"]);

    let label = get_value("nav-detail-label");
    assert!(
        label.contains("Detail"),
        "Detail label should say 'Detail Page': {label}"
    );

    // Navigate back — just switch to the Navigation tab again, which pops to root
    go_to_tab("Navigation");
    settle();

    // Verify we're back (push button should be visible)
    run(&["wait-for", "nav-push-button"]);
}

#[test]
#[ignore]
fn test_sheet_present_dismiss() {
    go_to_tab("Navigation");

    // Show sheet
    run(&["tap", "nav-sheet-button"]);

    // Wait for sheet content
    run(&["wait-for", "nav-sheet-content"]);

    let content = get_value("nav-sheet-content");
    assert!(
        content.contains("Sheet"),
        "Sheet content should be visible: {content}"
    );

    // Dismiss the sheet
    run(&["tap", "nav-sheet-dismiss"]);

    // Wait for sheet to disappear
    run(&["wait-for-not", "nav-sheet-content"]);

    // Status should update
    settle();
    let status = get_value("nav-status-label");
    assert!(
        status.contains("Sheet dismissed"),
        "Status should say 'Sheet dismissed': {status}"
    );
}

#[test]
#[ignore]
fn test_alert_dialog() {
    go_to_tab("Navigation");

    // Show alert — button is visible at native resolution
    run(&["tap", "nav-alert-button"]);

    // Wait for OK button to appear in alert
    run(&["wait-for", "nav-alert-ok"]);

    // Tap OK
    run(&["tap", "nav-alert-ok"]);
    settle();

    // Status should update
    let status = get_value("nav-status-label");
    assert!(
        status.contains("Alert confirmed"),
        "Status should say 'Alert confirmed': {status}"
    );
}

#[test]
#[ignore]
fn test_confirmation_dialog() {
    go_to_tab("Navigation");

    // Show confirmation dialog — button is visible at native resolution
    run(&["tap", "nav-confirm-button"]);

    // Wait for delete button in dialog
    run(&["wait-for", "nav-confirm-delete"]);

    // Tap delete
    run(&["tap", "nav-confirm-delete"]);
    settle();

    // Status should update
    let status = get_value("nav-status-label");
    assert!(
        status.contains("Deleted"),
        "Status should say 'Deleted': {status}"
    );
}
