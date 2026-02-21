//! Integration tests for the full ActionExecutor -> AgentDriver -> TCP pipeline.
//!
//! These tests verify the end-to-end flow:
//!   ActionExecutor -> AgentDriver -> protocol -> TCP -> mock agent -> response
//!
//! Each test spins up a mock TCP agent that speaks the binary protocol, then
//! executes actions through the ActionExecutor using an AgentDriver backend.

mod common;

use std::sync::Arc;

use common::connected_executor;

use qorvex_core::action::ActionType;
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::driver::AutomationDriver;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::protocol::Response;

// ---------------------------------------------------------------------------
// 1. Tap element by identifier
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_tap_element_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // TapElement
    ])
    .await;

    let result = executor
        .execute(ActionType::Tap {
            selector: "login-button".to_string(),
            by_label: false,
            element_type: None,
        })
        .await;

    assert!(result.success, "tap should succeed: {}", result.message);
    assert!(
        result.message.contains("login-button"),
        "message should mention the selector"
    );
}

// ---------------------------------------------------------------------------
// 2. Tap element by label
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_tap_by_label_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // TapByLabel
    ])
    .await;

    let result = executor
        .execute(ActionType::Tap {
            selector: "Sign In".to_string(),
            by_label: true,
            element_type: None,
        })
        .await;

    assert!(result.success, "tap-by-label should succeed: {}", result.message);
    assert!(
        result.message.contains("Sign In"),
        "message should mention the label"
    );
}

// ---------------------------------------------------------------------------
// 3. Type text (SendKeys)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_type_text_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // TypeText
    ])
    .await;

    let result = executor
        .execute(ActionType::SendKeys {
            text: "hello".to_string(),
        })
        .await;

    assert!(result.success, "send-keys should succeed: {}", result.message);
    assert!(
        result.message.contains("hello"),
        "message should mention the text"
    );
}

// ---------------------------------------------------------------------------
// 4. GetScreenInfo (dump_tree -> list_elements -> JSON)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_get_screen_info_via_agent_driver() {
    let tree_json = r#"[{
        "AXUniqueId": "btn1",
        "AXLabel": "Login",
        "type": "Button",
        "frame": {"x": 10, "y": 20, "width": 100, "height": 44},
        "children": []
    }]"#;

    let executor = connected_executor(vec![
        Response::Ok,                                   // heartbeat
        Response::Tree { json: tree_json.to_string() }, // DumpTree
    ])
    .await;

    let result = executor.execute(ActionType::GetScreenInfo).await;

    assert!(
        result.success,
        "get-screen-info should succeed: {}",
        result.message
    );
    let data = result.data.expect("should have data");
    assert!(data.contains("btn1"), "data should contain the element ID");
    assert!(
        data.contains("Login"),
        "data should contain the element label"
    );
}

// ---------------------------------------------------------------------------
// 5. GetValue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_get_value_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Value {
            value: Some("user@example.com".to_string()),
        }, // GetValue
    ])
    .await;

    let result = executor
        .execute(ActionType::GetValue {
            selector: "email".to_string(),
            by_label: false,
            element_type: None,
        })
        .await;

    assert!(result.success, "get-value should succeed: {}", result.message);
    let data = result.data.expect("should have data");
    assert_eq!(data, "user@example.com");
}

// ---------------------------------------------------------------------------
// 6. GetScreenshot
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_screenshot_via_agent_driver() {
    // Fake PNG header bytes.
    let png_header = vec![0x89, 0x50, 0x4E, 0x47];

    let executor = connected_executor(vec![
        Response::Ok,                                // heartbeat
        Response::Screenshot { data: png_header.clone() }, // Screenshot
    ])
    .await;

    let result = executor.execute(ActionType::GetScreenshot).await;

    assert!(
        result.success,
        "screenshot should succeed: {}",
        result.message
    );
    let screenshot = result.screenshot.expect("should have screenshot data");
    // The executor base64-encodes the raw bytes.
    let expected_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&png_header)
    };
    assert_eq!(screenshot, expected_b64);
    // data should also contain the base64 string.
    let data = result.data.expect("should have data");
    assert_eq!(data, expected_b64);
}

// ---------------------------------------------------------------------------
// 7. Swipe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_swipe_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // Swipe
    ])
    .await;

    let result = executor
        .execute(ActionType::Swipe {
            direction: "up".to_string(),
        })
        .await;

    assert!(result.success, "swipe should succeed: {}", result.message);
    assert!(
        result.message.contains("up"),
        "message should mention direction"
    );
}

// ---------------------------------------------------------------------------
// 8. Long press
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_long_press_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // LongPress
    ])
    .await;

    let result = executor
        .execute(ActionType::LongPress {
            x: 150,
            y: 300,
            duration: 1.5,
        })
        .await;

    assert!(
        result.success,
        "long_press should succeed: {}",
        result.message
    );
    assert!(
        result.message.contains("Long pressed"),
        "message should mention long press"
    );
    assert!(
        result.message.contains("150"),
        "message should mention x coordinate"
    );
}

// ---------------------------------------------------------------------------
// 9. Agent error propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_handles_agent_error() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Error {
            message: "element not found".to_string(),
        }, // Error response to Tap
    ])
    .await;

    let result = executor
        .execute(ActionType::Tap {
            selector: "missing-button".to_string(),
            by_label: false,
            element_type: None,
        })
        .await;

    assert!(!result.success, "tap should fail when agent returns error");
    assert!(
        result.message.contains("element not found"),
        "error message should propagate: {}",
        result.message
    );
}

// ---------------------------------------------------------------------------
// 9. LogComment bypasses the driver entirely
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_log_comment_ignores_driver() {
    // Use a driver that is NOT connected -- LogComment should never touch it.
    let driver = AgentDriver::new("127.0.0.1".to_string(), 1);
    let executor = ActionExecutor::new(Arc::new(driver));

    let result = executor
        .execute(ActionType::LogComment {
            message: "test log".to_string(),
        })
        .await;

    assert!(
        result.success,
        "LogComment should always succeed: {}",
        result.message
    );
    assert!(
        result.message.contains("test log"),
        "message should include the comment"
    );
}

// ---------------------------------------------------------------------------
// 10. Driver interchangeability (compile-time check exercised at runtime)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_driver_interchangeability() {
    // AgentDriver must be usable as Arc<dyn AutomationDriver> and accepted by
    // ActionExecutor::new(). This test is primarily a compile-time check that
    // the trait bounds are satisfied.

    let agent_driver: Arc<dyn AutomationDriver> =
        Arc::new(AgentDriver::new("127.0.0.1".to_string(), 9999));

    let agent_executor = ActionExecutor::new(agent_driver.clone());

    assert!(!agent_executor.driver().is_connected());
}

// ---------------------------------------------------------------------------
// 11. Tap with element_type filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_tap_with_type_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Ok, // TapWithType
    ])
    .await;

    let result = executor
        .execute(ActionType::Tap {
            selector: "Submit".to_string(),
            by_label: true,
            element_type: Some("Button".to_string()),
        })
        .await;

    assert!(
        result.success,
        "tap-with-type should succeed: {}",
        result.message
    );
}

// ---------------------------------------------------------------------------
// 12. GetValue by label
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_get_value_by_label_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Value {
            value: Some("typed text".to_string()),
        }, // GetValue
    ])
    .await;

    let result = executor
        .execute(ActionType::GetValue {
            selector: "Email".to_string(),
            by_label: true,
            element_type: None,
        })
        .await;

    assert!(result.success, "get-value-by-label should succeed: {}", result.message);
    let data = result.data.expect("should have data");
    assert_eq!(data, "typed text");
}

// ---------------------------------------------------------------------------
// 13. GetValue returns None (element has no value)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_get_value_none_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok,                       // heartbeat
        Response::Value { value: None },    // GetValue returns None
    ])
    .await;

    let result = executor
        .execute(ActionType::GetValue {
            selector: "empty-field".to_string(),
            by_label: false,
            element_type: None,
        })
        .await;

    assert!(result.success, "get-value should succeed even with None: {}", result.message);
    let data = result.data.expect("should have data");
    assert_eq!(data, "null");
}
