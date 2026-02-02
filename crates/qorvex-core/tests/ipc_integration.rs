//! Integration tests for IPC protocol in qorvex-core
//!
//! Tests cover:
//! - IPC server/client connection
//! - Message serialization/deserialization (JSON-over-newlines protocol)
//! - Session event broadcasting
//! - Action logging and retrieval

use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use qorvex_core::action::{ActionResult, ActionType};
use qorvex_core::ipc::{IpcClient, IpcRequest, IpcResponse, IpcServer};
use qorvex_core::session::{Session, SessionEvent};

/// Helper to create a unique session name for each test
fn unique_session_name() -> String {
    format!("test_{}", uuid::Uuid::new_v4().to_string().replace("-", "")[..8].to_string())
}

/// Helper to start the IPC server in a background task
async fn start_server(session: Arc<Session>, session_name: &str) -> tokio::task::JoinHandle<()> {
    let server = IpcServer::new(session, session_name);
    tokio::spawn(async move {
        // Server runs until cancelled
        let _ = server.run().await;
    })
}

// =============================================================================
// IPC Server/Client Connection Tests
// =============================================================================

#[tokio::test]
async fn test_ipc_client_connects_to_server() {
    let session_name = unique_session_name();
    let session = Session::new(None);

    let _server_handle = start_server(session, &session_name).await;

    // Give the server time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client should connect successfully
    let client_result = IpcClient::connect(&session_name).await;
    assert!(client_result.is_ok(), "Client should connect to server");
}

#[tokio::test]
async fn test_ipc_client_fails_with_no_server() {
    let session_name = unique_session_name();

    // No server started - client should fail to connect
    let client_result = IpcClient::connect(&session_name).await;
    assert!(client_result.is_err(), "Client should fail when no server exists");
}

#[tokio::test]
async fn test_multiple_clients_can_connect() {
    let session_name = unique_session_name();
    let session = Session::new(None);

    let _server_handle = start_server(session, &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Multiple clients should be able to connect
    let client1 = IpcClient::connect(&session_name).await;
    let client2 = IpcClient::connect(&session_name).await;

    assert!(client1.is_ok(), "First client should connect");
    assert!(client2.is_ok(), "Second client should connect");
}

// =============================================================================
// Message Serialization/Deserialization Tests (JSON-over-newlines protocol)
// =============================================================================

#[test]
fn test_ipc_request_execute_serialization() {
    let request = IpcRequest::Execute {
        action: ActionType::TapElement {
            id: "button_submit".to_string(),
        },
    };

    let json = serde_json::to_string(&request).unwrap();
    let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();

    match deserialized {
        IpcRequest::Execute { action } => match action {
            ActionType::TapElement { id } => assert_eq!(id, "button_submit"),
            _ => panic!("Expected TapElement action"),
        },
        _ => panic!("Expected Execute request"),
    }
}

#[test]
fn test_ipc_request_subscribe_serialization() {
    let request = IpcRequest::Subscribe;

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("Subscribe"));

    let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();
    assert!(matches!(deserialized, IpcRequest::Subscribe));
}

#[test]
fn test_ipc_request_get_state_serialization() {
    let request = IpcRequest::GetState;

    let json = serde_json::to_string(&request).unwrap();
    let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();

    assert!(matches!(deserialized, IpcRequest::GetState));
}

#[test]
fn test_ipc_request_get_log_serialization() {
    let request = IpcRequest::GetLog;

    let json = serde_json::to_string(&request).unwrap();
    let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();

    assert!(matches!(deserialized, IpcRequest::GetLog));
}

#[test]
fn test_ipc_response_action_result_serialization() {
    let response = IpcResponse::ActionResult {
        success: true,
        message: "Tapped element".to_string(),
        screenshot: Some(Arc::new("base64data".to_string())),
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: IpcResponse = serde_json::from_str(&json).unwrap();

    match deserialized {
        IpcResponse::ActionResult {
            success,
            message,
            screenshot,
        } => {
            assert!(success);
            assert_eq!(message, "Tapped element");
            assert_eq!(screenshot, Some(Arc::new("base64data".to_string())));
        }
        _ => panic!("Expected ActionResult response"),
    }
}

#[test]
fn test_ipc_response_state_serialization() {
    let response = IpcResponse::State {
        session_id: "test-session-123".to_string(),
        screenshot: None,
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: IpcResponse = serde_json::from_str(&json).unwrap();

    match deserialized {
        IpcResponse::State {
            session_id,
            screenshot,
        } => {
            assert_eq!(session_id, "test-session-123");
            assert!(screenshot.is_none());
        }
        _ => panic!("Expected State response"),
    }
}

#[test]
fn test_ipc_response_log_serialization() {
    use qorvex_core::action::ActionLog;

    let log_entry = ActionLog::new(
        ActionType::GetScreenshot,
        ActionResult::Success,
        Some(Arc::new("screenshot_data".to_string())),
    );

    let response = IpcResponse::Log {
        entries: vec![log_entry],
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: IpcResponse = serde_json::from_str(&json).unwrap();

    match deserialized {
        IpcResponse::Log { entries } => {
            assert_eq!(entries.len(), 1);
            assert!(matches!(entries[0].action, ActionType::GetScreenshot));
        }
        _ => panic!("Expected Log response"),
    }
}

#[test]
fn test_ipc_response_error_serialization() {
    let response = IpcResponse::Error {
        message: "Something went wrong".to_string(),
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: IpcResponse = serde_json::from_str(&json).unwrap();

    match deserialized {
        IpcResponse::Error { message } => {
            assert_eq!(message, "Something went wrong");
        }
        _ => panic!("Expected Error response"),
    }
}

#[test]
fn test_session_event_serialization() {
    let event = SessionEvent::ScreenshotUpdated(Arc::new("base64_png_data".to_string()));

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: SessionEvent = serde_json::from_str(&json).unwrap();

    match deserialized {
        SessionEvent::ScreenshotUpdated(data) => {
            assert_eq!(*data, "base64_png_data");
        }
        _ => panic!("Expected ScreenshotUpdated event"),
    }
}

#[test]
fn test_all_action_types_serialization() {
    let actions = vec![
        ActionType::TapElement {
            id: "elem".to_string(),
        },
        ActionType::TapLocation { x: 100, y: 200 },
        ActionType::LogComment {
            message: "test".to_string(),
        },
        ActionType::GetScreenshot,
        ActionType::GetScreenInfo,
        ActionType::GetElementValue {
            id: "field".to_string(),
        },
        ActionType::SendKeys {
            text: "hello".to_string(),
        },
        ActionType::StartSession,
        ActionType::EndSession,
        ActionType::Quit,
    ];

    for action in actions {
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: ActionType = serde_json::from_str(&json).unwrap();

        // Verify round-trip by re-serializing
        let json2 = serde_json::to_string(&deserialized).unwrap();
        assert_eq!(json, json2, "Action serialization should be consistent");
    }
}

// =============================================================================
// Session Event Broadcasting Tests
// =============================================================================

#[tokio::test]
async fn test_session_broadcasts_action_logged_event() {
    let session = Session::new(Some("test-udid".to_string()));
    let mut receiver = session.subscribe();

    // Log an action
    session
        .log_action(ActionType::GetScreenshot, ActionResult::Success, None)
        .await;

    // Should receive the event
    let event = timeout(Duration::from_millis(100), receiver.recv())
        .await
        .expect("Should receive event within timeout")
        .expect("Should receive event");

    match event {
        SessionEvent::ActionLogged(log) => {
            assert!(matches!(log.action, ActionType::GetScreenshot));
            assert!(matches!(log.result, ActionResult::Success));
        }
        _ => panic!("Expected ActionLogged event"),
    }
}

#[tokio::test]
async fn test_session_broadcasts_screenshot_updated_event() {
    let session = Session::new(None);
    let mut receiver = session.subscribe();

    // Update screenshot
    session.update_screenshot("new_screenshot_data".to_string()).await;

    // Should receive the event
    let event = timeout(Duration::from_millis(100), receiver.recv())
        .await
        .expect("Should receive event within timeout")
        .expect("Should receive event");

    match event {
        SessionEvent::ScreenshotUpdated(data) => {
            assert_eq!(*data, "new_screenshot_data");
        }
        _ => panic!("Expected ScreenshotUpdated event"),
    }
}

#[tokio::test]
async fn test_session_broadcasts_to_multiple_subscribers() {
    let session = Session::new(None);
    let mut receiver1 = session.subscribe();
    let mut receiver2 = session.subscribe();

    // Log an action
    session
        .log_action(
            ActionType::SendKeys {
                text: "test".to_string(),
            },
            ActionResult::Success,
            None,
        )
        .await;

    // Both subscribers should receive the event
    let event1 = timeout(Duration::from_millis(100), receiver1.recv())
        .await
        .expect("Subscriber 1 should receive event")
        .expect("Should receive event");

    let event2 = timeout(Duration::from_millis(100), receiver2.recv())
        .await
        .expect("Subscriber 2 should receive event")
        .expect("Should receive event");

    assert!(matches!(event1, SessionEvent::ActionLogged(_)));
    assert!(matches!(event2, SessionEvent::ActionLogged(_)));
}

#[tokio::test]
async fn test_action_with_screenshot_broadcasts_two_events() {
    let session = Session::new(None);
    let mut receiver = session.subscribe();

    // Log an action with screenshot (should broadcast ScreenshotUpdated AND ActionLogged)
    session
        .log_action(
            ActionType::GetScreenshot,
            ActionResult::Success,
            Some("screenshot_data".to_string()),
        )
        .await;

    // Should receive ScreenshotUpdated first
    let event1 = timeout(Duration::from_millis(100), receiver.recv())
        .await
        .expect("Should receive first event")
        .expect("Should receive event");
    assert!(matches!(event1, SessionEvent::ScreenshotUpdated(_)));

    // Then ActionLogged
    let event2 = timeout(Duration::from_millis(100), receiver.recv())
        .await
        .expect("Should receive second event")
        .expect("Should receive event");
    assert!(matches!(event2, SessionEvent::ActionLogged(_)));
}

// =============================================================================
// Action Logging and Retrieval Tests
// =============================================================================

#[tokio::test]
async fn test_session_logs_actions() {
    let session = Session::new(None);

    // Log multiple actions
    session
        .log_action(ActionType::StartSession, ActionResult::Success, None)
        .await;
    session
        .log_action(
            ActionType::TapElement {
                id: "button".to_string(),
            },
            ActionResult::Success,
            None,
        )
        .await;
    session
        .log_action(
            ActionType::SendKeys {
                text: "test".to_string(),
            },
            ActionResult::Failure("Error".to_string()),
            None,
        )
        .await;

    // Retrieve logs
    let logs = session.get_action_log().await;
    assert_eq!(logs.len(), 3);

    // Verify order (oldest first)
    assert!(matches!(logs[0].action, ActionType::StartSession));
    assert!(matches!(logs[1].action, ActionType::TapElement { .. }));
    assert!(matches!(logs[2].action, ActionType::SendKeys { .. }));

    // Verify failure is recorded
    assert!(matches!(logs[2].result, ActionResult::Failure(_)));
}

#[tokio::test]
async fn test_session_stores_and_retrieves_screenshot() {
    let session = Session::new(None);

    // Initially no screenshot
    assert!(session.get_screenshot().await.is_none());

    // Log action with screenshot
    session
        .log_action(
            ActionType::GetScreenshot,
            ActionResult::Success,
            Some("screenshot1".to_string()),
        )
        .await;

    // Screenshot should be stored
    assert_eq!(session.get_screenshot().await, Some(Arc::new("screenshot1".to_string())));

    // Update screenshot directly
    session.update_screenshot("screenshot2".to_string()).await;

    // Should have new screenshot
    assert_eq!(session.get_screenshot().await, Some(Arc::new("screenshot2".to_string())));
}

#[tokio::test]
async fn test_action_log_has_unique_ids() {
    let session = Session::new(None);

    let log1 = session
        .log_action(ActionType::GetScreenshot, ActionResult::Success, None)
        .await;
    let log2 = session
        .log_action(ActionType::GetScreenshot, ActionResult::Success, None)
        .await;

    assert_ne!(log1.id, log2.id, "Each action log should have a unique ID");
}

#[tokio::test]
async fn test_action_log_has_timestamp() {
    let session = Session::new(None);

    let before = chrono::Utc::now();
    let log = session
        .log_action(ActionType::GetScreenshot, ActionResult::Success, None)
        .await;
    let after = chrono::Utc::now();

    assert!(log.timestamp >= before, "Timestamp should be after test start");
    assert!(log.timestamp <= after, "Timestamp should be before test end");
}

// =============================================================================
// End-to-End IPC Communication Tests
// =============================================================================

#[tokio::test]
async fn test_ipc_get_state_request() {
    let session_name = unique_session_name();
    let session = Session::new(Some("simulator-udid-123".to_string()));
    let session_id = session.id.to_string();

    let _server_handle = start_server(session, &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = IpcClient::connect(&session_name).await.unwrap();

    let response = client.send(&IpcRequest::GetState).await.unwrap();

    match response {
        IpcResponse::State {
            session_id: resp_id,
            screenshot,
        } => {
            assert_eq!(resp_id, session_id);
            assert!(screenshot.is_none()); // No screenshot yet
        }
        _ => panic!("Expected State response, got {:?}", response),
    }
}

#[tokio::test]
async fn test_ipc_get_log_request() {
    let session_name = unique_session_name();
    let session = Session::new(None);

    // Pre-log some actions
    session
        .log_action(ActionType::StartSession, ActionResult::Success, None)
        .await;
    session
        .log_action(ActionType::GetScreenshot, ActionResult::Success, None)
        .await;

    let _server_handle = start_server(session, &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = IpcClient::connect(&session_name).await.unwrap();

    let response = client.send(&IpcRequest::GetLog).await.unwrap();

    match response {
        IpcResponse::Log { entries } => {
            assert_eq!(entries.len(), 2);
            assert!(matches!(entries[0].action, ActionType::StartSession));
            assert!(matches!(entries[1].action, ActionType::GetScreenshot));
        }
        _ => panic!("Expected Log response, got {:?}", response),
    }
}

#[tokio::test]
async fn test_ipc_execute_action_request() {
    let session_name = unique_session_name();
    // Use LogComment action which doesn't require a simulator UDID
    let session = Session::new(None);

    let _server_handle = start_server(session.clone(), &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = IpcClient::connect(&session_name).await.unwrap();

    // LogComment doesn't require a simulator, so it should succeed even without one
    let response = client
        .send(&IpcRequest::Execute {
            action: ActionType::LogComment {
                message: "test comment".to_string(),
            },
        })
        .await
        .unwrap();

    match response {
        IpcResponse::ActionResult {
            success,
            message,
            screenshot: _,
        } => {
            assert!(success);
            assert!(message.contains("test comment"));
        }
        _ => panic!("Expected ActionResult response, got {:?}", response),
    }

    // Verify action was logged in session
    let logs = session.get_action_log().await;
    assert_eq!(logs.len(), 1);
    match &logs[0].action {
        ActionType::LogComment { message } => assert_eq!(message, "test comment"),
        _ => panic!("Expected LogComment action"),
    }
}

#[tokio::test]
async fn test_ipc_execute_action_without_simulator_returns_error() {
    let session_name = unique_session_name();
    let session = Session::new(None);

    let _server_handle = start_server(session.clone(), &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = IpcClient::connect(&session_name).await.unwrap();

    // TapElement requires a simulator UDID, so it should return an error when none is set
    let response = client
        .send(&IpcRequest::Execute {
            action: ActionType::TapElement {
                id: "my_button".to_string(),
            },
        })
        .await
        .unwrap();

    match response {
        IpcResponse::Error { message } => {
            assert!(message.contains("No simulator"));
        }
        _ => panic!("Expected Error response, got {:?}", response),
    }
}

#[tokio::test]
async fn test_ipc_multiple_requests_same_client() {
    let session_name = unique_session_name();
    let session = Session::new(None);

    let _server_handle = start_server(session, &session_name).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = IpcClient::connect(&session_name).await.unwrap();

    // Send multiple requests on same connection
    let _ = client.send(&IpcRequest::GetState).await.unwrap();
    let _ = client.send(&IpcRequest::GetLog).await.unwrap();
    // Use LogComment instead of GetScreenshot since it doesn't require a simulator
    let _ = client
        .send(&IpcRequest::Execute {
            action: ActionType::LogComment {
                message: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let log_response = client.send(&IpcRequest::GetLog).await.unwrap();

    // Final GetLog should show the executed action
    match log_response {
        IpcResponse::Log { entries } => {
            assert_eq!(entries.len(), 1);
        }
        _ => panic!("Expected Log response"),
    }
}
