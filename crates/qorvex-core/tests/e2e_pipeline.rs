//! End-to-end pipeline tests for qorvex-core.
//!
//! These tests exercise the full path:
//!   IPC client -> IPC server -> Session -> ActionExecutor -> mock TCP agent -> response back to client
//!
//! Each test uses the `TestHarness` to spin up a mock agent, session, and IPC
//! server, then sends requests through an IPC client and verifies the responses.

mod common;

use std::time::Duration;
use tokio::time::timeout;

use common::TestHarness;
use qorvex_core::action::ActionType;
use qorvex_core::ipc::{IpcRequest, IpcResponse};
use qorvex_core::protocol::Response;
use qorvex_core::session::SessionEvent;

// =============================================================================
// 1. Tap via IPC to mock agent
// =============================================================================

#[tokio::test]
async fn test_tap_via_ipc_to_mock_agent() {
    let harness = TestHarness::start(vec![
        Response::Ok, // heartbeat
        Response::Ok, // tap
    ])
    .await;

    let mut client = harness.connect_client().await;

    let response = client
        .send(&IpcRequest::Execute {
            action: ActionType::Tap {
                selector: "login-btn".to_string(),
                by_label: false,
                element_type: None, timeout_ms: None,
            },
        })
        .await
        .unwrap();

    match response {
        IpcResponse::ActionResult {
            success, message, ..
        } => {
            assert!(success, "tap should succeed: {}", message);
            assert!(
                message.contains("login-btn"),
                "message should mention selector, got: {}",
                message
            );
        }
        other => panic!("Expected ActionResult, got {:?}", other),
    }
}

// =============================================================================
// 2. Screenshot via IPC to mock agent
// =============================================================================

#[tokio::test]
async fn test_screenshot_via_ipc_to_mock_agent() {
    let png_header = vec![0x89, 0x50, 0x4E, 0x47];

    let harness = TestHarness::start(vec![
        Response::Ok,                                    // heartbeat
        Response::Screenshot { data: png_header.clone() }, // screenshot
    ])
    .await;

    let mut client = harness.connect_client().await;

    let response = client
        .send(&IpcRequest::Execute {
            action: ActionType::GetScreenshot,
        })
        .await
        .unwrap();

    let expected_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&png_header)
    };

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            screenshot,
            data,
        } => {
            assert!(success, "screenshot should succeed: {}", message);
            // The executor base64-encodes the raw PNG bytes.
            assert_eq!(
                screenshot,
                Some(std::sync::Arc::new(expected_b64.clone())),
                "screenshot should contain base64-encoded data"
            );
            assert_eq!(
                data,
                Some(expected_b64),
                "data should also contain the base64 string"
            );
        }
        other => panic!("Expected ActionResult, got {:?}", other),
    }
}

// =============================================================================
// 3. Screen info via IPC to mock agent
// =============================================================================

#[tokio::test]
async fn test_screen_info_via_ipc_to_mock_agent() {
    let tree_json = r#"[{
        "AXUniqueId": "nav-bar",
        "AXLabel": "Navigation",
        "type": "NavigationBar",
        "frame": {"x": 0, "y": 0, "width": 390, "height": 44},
        "children": [{
            "AXUniqueId": "settings-btn",
            "AXLabel": "Settings",
            "type": "Button",
            "frame": {"x": 330, "y": 10, "width": 44, "height": 24},
            "children": []
        }]
    }]"#;

    let harness = TestHarness::start(vec![
        Response::Ok,                                      // heartbeat
        Response::Tree { json: tree_json.to_string() },    // dump tree
    ])
    .await;

    let mut client = harness.connect_client().await;

    let response = client
        .send(&IpcRequest::Execute {
            action: ActionType::GetScreenInfo,
        })
        .await
        .unwrap();

    match response {
        IpcResponse::ActionResult {
            success,
            data,
            ..
        } => {
            assert!(success, "get-screen-info should succeed");
            let data = data.expect("should have data");
            assert!(
                data.contains("nav-bar"),
                "data should contain element ID 'nav-bar', got: {}",
                data
            );
            assert!(
                data.contains("settings-btn"),
                "data should contain element ID 'settings-btn', got: {}",
                data
            );
        }
        other => panic!("Expected ActionResult, got {:?}", other),
    }
}

// =============================================================================
// 4. Action logged after IPC execute
// =============================================================================

#[tokio::test]
async fn test_action_logged_after_ipc_execute() {
    let harness = TestHarness::start(vec![
        Response::Ok, // heartbeat
        Response::Ok, // tap
    ])
    .await;

    let mut client = harness.connect_client().await;

    // Execute a tap action
    let _ = client
        .send(&IpcRequest::Execute {
            action: ActionType::Tap {
                selector: "submit-btn".to_string(),
                by_label: false,
                element_type: None, timeout_ms: None,
            },
        })
        .await
        .unwrap();

    // Retrieve the log
    let log_response = client.send(&IpcRequest::GetLog).await.unwrap();

    match log_response {
        IpcResponse::Log { entries } => {
            assert_eq!(entries.len(), 1, "should have exactly 1 log entry");
            match &entries[0].action {
                ActionType::Tap { selector, .. } => {
                    assert_eq!(selector, "submit-btn");
                }
                other => panic!("Expected Tap action, got {:?}", other),
            }
        }
        other => panic!("Expected Log response, got {:?}", other),
    }
}

// =============================================================================
// 5. Screenshot event broadcasts via full stack
// =============================================================================

#[tokio::test]
async fn test_screenshot_event_broadcasts_via_full_stack() {
    let png_data = vec![0x89, 0x50, 0x4E, 0x47];

    let harness = TestHarness::start(vec![
        Response::Ok,                                   // heartbeat
        Response::Screenshot { data: png_data.clone() }, // screenshot
    ])
    .await;

    // Subscribe to session events BEFORE executing the action.
    let mut rx = harness.session.subscribe();

    let mut client = harness.connect_client().await;

    // Execute screenshot via IPC client
    let _ = client
        .send(&IpcRequest::Execute {
            action: ActionType::GetScreenshot,
        })
        .await
        .unwrap();

    let expected_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&png_data)
    };

    // The session should broadcast ScreenshotUpdated followed by ActionLogged.
    let event = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event within timeout")
        .expect("should receive event");

    match event {
        SessionEvent::ScreenshotUpdated(data) => {
            assert_eq!(
                *data, expected_b64,
                "screenshot data should match base64-encoded PNG"
            );
        }
        other => panic!("Expected ScreenshotUpdated event, got {:?}", other),
    }
}

// =============================================================================
// 6. Multiple sequential actions via IPC
// =============================================================================

#[tokio::test]
async fn test_multiple_sequential_actions_via_ipc() {
    let png_data = vec![0x89, 0x50, 0x4E, 0x47];

    let harness = TestHarness::start(vec![
        Response::Ok, // heartbeat
        Response::Ok, // tap
        Response::Ok, // send-keys
        Response::Screenshot { data: png_data }, // screenshot
    ])
    .await;

    let mut client = harness.connect_client().await;

    // 1. Tap
    let r1 = client
        .send(&IpcRequest::Execute {
            action: ActionType::Tap {
                selector: "username-field".to_string(),
                by_label: false,
                element_type: None, timeout_ms: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(r1, IpcResponse::ActionResult { success: true, .. }));

    // 2. SendKeys
    let r2 = client
        .send(&IpcRequest::Execute {
            action: ActionType::SendKeys {
                text: "admin".to_string(),
            },
        })
        .await
        .unwrap();
    assert!(matches!(r2, IpcResponse::ActionResult { success: true, .. }));

    // 3. Screenshot
    let r3 = client
        .send(&IpcRequest::Execute {
            action: ActionType::GetScreenshot,
        })
        .await
        .unwrap();
    assert!(matches!(r3, IpcResponse::ActionResult { success: true, .. }));

    // Retrieve the log and verify all 3 actions in order.
    let log_response = client.send(&IpcRequest::GetLog).await.unwrap();

    match log_response {
        IpcResponse::Log { entries } => {
            assert_eq!(entries.len(), 3, "should have 3 log entries");

            assert!(
                matches!(entries[0].action, ActionType::Tap { .. }),
                "first action should be Tap, got {:?}",
                entries[0].action
            );
            assert!(
                matches!(entries[1].action, ActionType::SendKeys { .. }),
                "second action should be SendKeys, got {:?}",
                entries[1].action
            );
            assert!(
                matches!(entries[2].action, ActionType::GetScreenshot),
                "third action should be GetScreenshot, got {:?}",
                entries[2].action
            );
        }
        other => panic!("Expected Log response, got {:?}", other),
    }
}
