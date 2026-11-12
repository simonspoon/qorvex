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

use qorvex_core::action::{ActionType, LabelMatch, LabelMatcher, ScrollDirection};
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
            timeout_ms: None,
            fallback_frame: false,
            report: false,
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
            timeout_ms: None,
            fallback_frame: false,
            report: false,
        })
        .await;

    assert!(
        result.success,
        "tap-by-label should succeed: {}",
        result.message
    );
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
            replace: false,
        })
        .await;

    assert!(
        result.success,
        "send-keys should succeed: {}",
        result.message
    );
    assert!(
        result.message.contains("hello"),
        "message should mention the text"
    );
}

// 3b. send-keys modes: empty text without --replace is a no-op (no request);
//     --replace (also with empty text) reaches the wire as replace=true.
// ---------------------------------------------------------------------------

async fn send_keys_recorded(
    text: &str,
    replace: bool,
    responses: Vec<Response>,
) -> (bool, Vec<qorvex_core::protocol::Request>) {
    let (addr, seen) = common::recording_mock_agent(responses).await;
    let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
    driver.connect().await.unwrap();
    let executor = ActionExecutor::new(Arc::new(driver));
    let result = executor
        .execute(ActionType::SendKeys {
            text: text.to_string(),
            replace,
        })
        .await;
    let requests = seen.lock().unwrap().clone();
    (result.success, requests)
}

#[tokio::test]
async fn test_send_keys_empty_without_replace_sends_nothing() {
    use qorvex_core::protocol::Request;
    let (ok, requests) = send_keys_recorded("", false, vec![Response::Ok]).await;
    assert!(ok, "empty send-keys must succeed as a no-op");
    assert_eq!(requests, vec![Request::Heartbeat]);
}

#[tokio::test]
async fn test_send_keys_empty_with_replace_clears() {
    use qorvex_core::protocol::Request;
    let (ok, requests) = send_keys_recorded("", true, vec![Response::Ok, Response::Ok]).await;
    assert!(ok);
    assert_eq!(
        requests,
        vec![
            Request::Heartbeat,
            Request::TypeText {
                text: String::new(),
                replace: true
            }
        ]
    );
}

#[tokio::test]
async fn test_send_keys_mode_reaches_driver() {
    use qorvex_core::protocol::Request;
    for replace in [false, true] {
        let (ok, requests) =
            send_keys_recorded("abc", replace, vec![Response::Ok, Response::Ok]).await;
        assert!(ok);
        assert_eq!(
            requests[1],
            Request::TypeText {
                text: "abc".to_string(),
                replace
            }
        );
    }
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
        Response::Ok, // heartbeat
        Response::Tree {
            json: tree_json.to_string(),
        }, // DumpTree
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
            timeout_ms: None,
        })
        .await;

    assert!(
        result.success,
        "get-value should succeed: {}",
        result.message
    );
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
        Response::Ok, // heartbeat
        Response::Screenshot {
            data: png_header.clone(),
        }, // Screenshot
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
            duration_ms: None,
        })
        .await;

    assert!(result.success, "swipe should succeed: {}", result.message);
    assert!(
        result.message.contains("up"),
        "message should mention direction"
    );
}

async fn swipe_recorded(action: ActionType) -> (String, Option<f64>) {
    use qorvex_core::protocol::Request;
    let (addr, seen) = common::recording_mock_agent(vec![Response::Ok, Response::Ok]).await;
    let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
    driver.connect().await.unwrap();
    let executor = ActionExecutor::new(Arc::new(driver));
    let result = executor.execute(action).await;
    assert!(result.success, "{}", result.message);
    let requests = seen.lock().unwrap().clone();
    match &requests[1] {
        Request::Swipe { duration, .. } => (result.message, *duration),
        other => panic!("expected Swipe, got {:?}", other),
    }
}

#[tokio::test]
async fn test_swipe_duration_reaches_driver() {
    let dir = |ms| ActionType::Swipe {
        direction: "up".to_string(),
        duration_ms: ms,
    };
    let (msg, d) = swipe_recorded(dir(None)).await;
    assert_eq!((msg.as_str(), d), ("Swiped up", Some(0.3)));
    let (msg, d) = swipe_recorded(dir(Some(800))).await;
    assert_eq!((msg.as_str(), d), ("Swiped up (800 ms)", Some(0.8)));
    let coords = |ms| ActionType::SwipeCoords {
        from_x: 1,
        from_y: 2,
        to_x: 3,
        to_y: 4,
        duration_ms: ms,
    };
    let (_, d) = swipe_recorded(coords(None)).await;
    assert_eq!(d, Some(0.3));
    let (msg, d) = swipe_recorded(coords(Some(1500))).await;
    assert_eq!(d, Some(1.5));
    assert!(msg.ends_with("(1500 ms)"), "{}", msg);
}

#[test]
fn test_swipe_action_serde_back_compat() {
    let old: ActionType = serde_json::from_str(r#"{"type":"Swipe","direction":"up"}"#).unwrap();
    assert!(matches!(
        old,
        ActionType::Swipe {
            duration_ms: None,
            ..
        }
    ));
    let old: ActionType =
        serde_json::from_str(r#"{"type":"SwipeCoords","from_x":1,"from_y":2,"to_x":3,"to_y":4}"#)
            .unwrap();
    assert!(matches!(
        old,
        ActionType::SwipeCoords {
            duration_ms: None,
            ..
        }
    ));
    let none = serde_json::to_string(&ActionType::Swipe {
        direction: "up".to_string(),
        duration_ms: None,
    })
    .unwrap();
    assert!(!none.contains("duration_ms"), "{}", none);
    let some = serde_json::to_string(&ActionType::Swipe {
        direction: "up".to_string(),
        duration_ms: Some(800),
    })
    .unwrap();
    assert!(some.contains("\"duration_ms\":800"), "{}", some);
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
            timeout_ms: None,
            fallback_frame: false,
            report: false,
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
            timeout_ms: None,
            fallback_frame: false,
            report: false,
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
            timeout_ms: None,
        })
        .await;

    assert!(
        result.success,
        "get-value-by-label should succeed: {}",
        result.message
    );
    let data = result.data.expect("should have data");
    assert_eq!(data, "typed text");
}

// ---------------------------------------------------------------------------
// 13. GetValue returns None (element has no value)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_executor_get_value_none_via_agent_driver() {
    let executor = connected_executor(vec![
        Response::Ok,                    // heartbeat
        Response::Value { value: None }, // GetValue returns None
    ])
    .await;

    let result = executor
        .execute(ActionType::GetValue {
            selector: "empty-field".to_string(),
            by_label: false,
            element_type: None,
            timeout_ms: None,
        })
        .await;

    assert!(
        result.success,
        "get-value should succeed even with None: {}",
        result.message
    );
    let data = result.data.expect("should have data");
    assert_eq!(data, "null");
}

// ---------------------------------------------------------------------------
// Exists: found / absent / driver error
// ---------------------------------------------------------------------------

fn exists_action() -> ActionType {
    ActionType::Exists {
        label_match: None,
        selector: "banner".to_string(),
        by_label: false,
        element_type: None,
    }
}

#[tokio::test]
async fn test_executor_exists_found_reports_hittable() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Element {
            json: r#"{"identifier":"banner","hittable":false,"children":[]}"#.to_string(),
        },
    ])
    .await;

    let result = executor.execute(exists_action()).await;
    assert!(result.success, "{}", result.message);
    let v: serde_json::Value = serde_json::from_str(&result.data.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"exists": true, "hittable": false}));
}

#[tokio::test]
async fn test_executor_exists_absent_is_success_false() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Element {
            json: "null".to_string(),
        },
    ])
    .await;

    let result = executor.execute(exists_action()).await;
    assert!(result.success, "{}", result.message);
    let v: serde_json::Value = serde_json::from_str(&result.data.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"exists": false}));
}

#[tokio::test]
async fn test_executor_exists_driver_error_is_failure_not_absent() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Error {
            message: "target app is not running".to_string(),
        },
    ])
    .await;

    let result = executor.execute(exists_action()).await;
    assert!(!result.success, "a driver error must not read as absent");
    assert!(result.data.is_none());
}

// ---------------------------------------------------------------------------
// ScrollTo: exists check, swipe, re-check
// ---------------------------------------------------------------------------

fn scroll_to_action(max_swipes: u32) -> ActionType {
    ActionType::ScrollTo {
        selector: "row-12".to_string(),
        by_label: false,
        element_type: None,
        label_match: None,
        direction: ScrollDirection::Up,
        max_swipes,
        duration_ms: None,
    }
}

fn absent() -> Response {
    Response::Element {
        json: "null".to_string(),
    }
}

fn present() -> Response {
    Response::Element {
        json: r#"{"identifier":"row-12","hittable":true,"children":[]}"#.to_string(),
    }
}

#[tokio::test]
async fn test_executor_scroll_to_present_makes_zero_swipes() {
    // No swipe response scripted: a swipe would hit a closed connection.
    let executor = connected_executor(vec![Response::Ok, present()]).await;
    let result = executor.execute(scroll_to_action(5)).await;
    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Found \"row-12\" after 0 swipes");
    let v: serde_json::Value = serde_json::from_str(&result.data.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"found": true, "swipes": 0}));
}

#[tokio::test]
async fn test_executor_scroll_to_found_on_third_check_after_three_swipes() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        absent(),
        Response::Ok, // swipe 1
        absent(),
        Response::Ok, // swipe 2
        absent(),
        Response::Ok, // swipe 3
        present(),
    ])
    .await;
    let result = executor.execute(scroll_to_action(10)).await;
    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Found \"row-12\" after 3 swipes");
    let v: serde_json::Value = serde_json::from_str(&result.data.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"found": true, "swipes": 3}));
}

#[tokio::test]
async fn test_executor_scroll_to_never_found_stops_at_max_swipes() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        absent(),
        Response::Ok, // swipe 1
        absent(),
        Response::Ok, // swipe 2
        absent(),
    ])
    .await;
    let result = executor.execute(scroll_to_action(2)).await;
    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Not found \"row-12\" after 2 swipes");
    let v: serde_json::Value = serde_json::from_str(&result.data.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"found": false, "swipes": 2}));
}

#[tokio::test]
async fn test_executor_scroll_to_driver_error_is_failure() {
    let executor = connected_executor(vec![
        Response::Ok, // heartbeat
        Response::Error {
            message: "target app is not running".to_string(),
        },
    ])
    .await;
    let result = executor.execute(scroll_to_action(3)).await;
    assert!(!result.success);
    assert!(result.data.is_none());
}

// ---------------------------------------------------------------------------
// Label text match (--label-contains / --label-regex / --nth): tree dump +
// client-side match
// ---------------------------------------------------------------------------

const LABEL_TREE: &str = r#"[{"AXLabel":"Root","type":"Window","children":[
    {"AXLabel":"Tap Me","type":"Button","hittable":true,"frame":{"x":10,"y":20,"width":30,"height":40}},
    {"AXLabel":"Tapped: 1","type":"StaticText","hittable":true,"frame":{"x":1,"y":2,"width":3,"height":4}}
]}]"#;

fn tree() -> Response {
    Response::Tree {
        json: LABEL_TREE.to_string(),
    }
}

fn label_match(matcher: LabelMatcher, nth: Option<usize>) -> Option<Box<LabelMatch>> {
    Some(Box::new(LabelMatch { matcher, nth }))
}

fn exists_matching(m: Option<Box<LabelMatch>>) -> ActionType {
    ActionType::Exists {
        selector: String::new(),
        by_label: false,
        element_type: None,
        label_match: m,
    }
}

#[tokio::test]
async fn test_executor_exists_label_contains_found_and_nth() {
    let executor = connected_executor(vec![Response::Ok, tree(), tree(), tree()]).await;

    let r = executor
        .execute(exists_matching(label_match(
            LabelMatcher::Contains("Tap".into()),
            None,
        )))
        .await;
    assert!(r.success, "{}", r.message);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&r.data.unwrap()).unwrap(),
        serde_json::json!({"exists": true, "hittable": true})
    );

    let r = executor
        .execute(exists_matching(label_match(
            LabelMatcher::Contains("Tap".into()),
            Some(1),
        )))
        .await;
    assert!(r.success, "{}", r.message);
    assert!(r.data.unwrap().contains("\"exists\":true"));

    let r = executor
        .execute(exists_matching(label_match(
            LabelMatcher::Contains("Tap".into()),
            Some(2),
        )))
        .await;
    assert!(r.success, "{}", r.message);
    assert_eq!(r.data.as_deref(), Some(r#"{"exists":false}"#));
}

#[tokio::test]
async fn test_executor_exists_label_regex_absent() {
    let executor = connected_executor(vec![Response::Ok, tree()]).await;
    let r = executor
        .execute(exists_matching(label_match(
            LabelMatcher::Regex("^zzz-none$".into()),
            None,
        )))
        .await;
    assert!(r.success, "{}", r.message);
    assert_eq!(r.data.as_deref(), Some(r#"{"exists":false}"#));
    assert!(
        r.message.contains("label matches /^zzz-none$/"),
        "{}",
        r.message
    );
}

#[tokio::test]
async fn test_executor_label_regex_invalid_fails_before_device_call() {
    // Only the heartbeat is scripted: a tree dump would fail differently.
    let executor = connected_executor(vec![Response::Ok]).await;
    for action in [
        exists_matching(label_match(LabelMatcher::Regex("(".into()), None)),
        ActionType::WaitFor {
            selector: String::new(),
            by_label: false,
            element_type: None,
            timeout_ms: 1000,
            require_stable: true,
            label_match: label_match(LabelMatcher::Regex("(".into()), None),
        },
        ActionType::WaitForNot {
            selector: String::new(),
            by_label: false,
            element_type: None,
            timeout_ms: 1000,
            label_match: label_match(LabelMatcher::Regex("(".into()), None),
        },
    ] {
        let r = executor.execute(action).await;
        assert!(!r.success);
        assert!(r.message.starts_with("invalid regex '('"), "{}", r.message);
    }
}

#[tokio::test]
async fn test_executor_wait_for_label_regex_returns_frame() {
    // require_stable: three polls with the same frame.
    let executor = connected_executor(vec![Response::Ok, tree(), tree(), tree()]).await;
    let r = executor
        .execute(ActionType::WaitFor {
            selector: String::new(),
            by_label: false,
            element_type: None,
            timeout_ms: 5000,
            require_stable: true,
            label_match: label_match(LabelMatcher::Regex("Tapped: 1".into()), None),
        })
        .await;
    assert!(r.success, "{}", r.message);
    let v: serde_json::Value = serde_json::from_str(&r.data.unwrap()).unwrap();
    assert_eq!(
        v["frame"],
        serde_json::json!({"x": 1, "y": 2, "width": 3, "height": 4})
    );
}

#[tokio::test]
async fn test_executor_wait_for_not_label_contains_absent() {
    let executor = connected_executor(vec![Response::Ok, tree()]).await;
    let r = executor
        .execute(ActionType::WaitForNot {
            selector: String::new(),
            by_label: false,
            element_type: Some("Switch".into()),
            timeout_ms: 1000,
            label_match: label_match(LabelMatcher::Contains("Tap Me".into()), None),
        })
        .await;
    assert!(r.success, "{}", r.message);
}
