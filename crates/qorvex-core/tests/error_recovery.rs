//! Error recovery and edge-case tests for the AgentDriver / ActionExecutor pipeline.
//!
//! These tests exercise disconnect, timeout, protocol corruption, delayed
//! response, post-disconnect usage, and error propagation scenarios using the
//! programmable mock agent infrastructure from `common/mod.rs`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{programmable_mock_agent, MockBehavior};

use qorvex_core::action::ActionType;
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::driver::AutomationDriver;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::protocol::Response;

// ---------------------------------------------------------------------------
// Helper: connect an executor to a programmable mock agent
// ---------------------------------------------------------------------------

async fn programmable_executor(behaviors: Vec<MockBehavior>) -> ActionExecutor {
    let addr = programmable_mock_agent(behaviors).await;
    let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
    driver.connect().await.unwrap();
    let mut executor = ActionExecutor::new(Arc::new(driver));
    executor.set_capture_screenshots(false);
    executor
}

fn tap_action() -> ActionType {
    ActionType::Tap {
        selector: "test-button".to_string(),
        by_label: false,
        element_type: None, timeout_ms: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Agent drops connection mid-session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_drops_connection_mid_session() {
    let executor = programmable_executor(vec![
        MockBehavior::Respond(Response::Ok), // heartbeat
        MockBehavior::Respond(Response::Ok), // first action succeeds
        MockBehavior::Drop,                  // second action: agent reads then drops
    ])
    .await;

    // First action should succeed.
    let result = executor.execute(tap_action()).await;
    assert!(result.success, "first action should succeed: {}", result.message);

    // Second action should fail gracefully (no panic).
    let result = executor.execute(tap_action()).await;
    assert!(!result.success, "second action should fail after drop");
    // The error message should be meaningful (I/O or connection-related).
    assert!(
        !result.message.is_empty(),
        "error message should not be empty"
    );
}

// ---------------------------------------------------------------------------
// 2. Agent hangs (never responds) — triggers timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_hangs_triggers_timeout() {
    // Wrap the entire test in a timeout to prevent CI hangs. The AgentClient
    // has a 30-second READ_TIMEOUT, so we allow up to 45 seconds.
    let outcome = tokio::time::timeout(Duration::from_secs(45), async {
        let executor = programmable_executor(vec![
            MockBehavior::Respond(Response::Ok), // heartbeat
            MockBehavior::Hang,                  // action: agent never responds
        ])
        .await;

        let result = executor.execute(tap_action()).await;
        assert!(!result.success, "action should fail when agent hangs");
        // Should surface a timeout or I/O error, not block forever.
        assert!(
            !result.message.is_empty(),
            "error message should not be empty"
        );
    })
    .await;

    assert!(
        outcome.is_ok(),
        "test timed out — the executor blocked forever instead of returning an error"
    );
}

// ---------------------------------------------------------------------------
// 3. Agent sends garbage bytes instead of a valid response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_sends_garbage_bytes() {
    let executor = programmable_executor(vec![
        MockBehavior::Respond(Response::Ok), // heartbeat
        MockBehavior::SendGarbage,           // action: agent sends invalid bytes
    ])
    .await;

    let result = executor.execute(tap_action()).await;
    assert!(
        !result.success,
        "action should fail on garbage response"
    );
    // Should be a protocol/parse/IO error, not a panic.
    assert!(
        !result.message.is_empty(),
        "error message should not be empty"
    );
}

// ---------------------------------------------------------------------------
// 4. Agent responds with a short delay (should still succeed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_delayed_response_succeeds() {
    let executor = programmable_executor(vec![
        MockBehavior::Respond(Response::Ok),                           // heartbeat
        MockBehavior::Delay(Duration::from_millis(200), Response::Ok), // action with 200ms delay
    ])
    .await;

    let result = executor.execute(tap_action()).await;
    assert!(
        result.success,
        "action should succeed with a 200ms delay: {}",
        result.message
    );
}

// ---------------------------------------------------------------------------
// 5. Execute after disconnect returns error (not panic)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_execute_after_disconnect_returns_error() {
    let executor = programmable_executor(vec![
        MockBehavior::Respond(Response::Ok), // heartbeat
        MockBehavior::Drop,                  // first action triggers disconnect
    ])
    .await;

    // First execute triggers the drop — the AgentClient will detect the I/O
    // error and clear its stream.
    let result1 = executor.execute(tap_action()).await;
    assert!(
        !result1.success,
        "first action after drop should fail"
    );

    // Second execute should also fail cleanly with a "not connected" style error.
    let result2 = executor.execute(tap_action()).await;
    assert!(
        !result2.success,
        "second action after disconnect should also fail"
    );
    // The underlying error should indicate the connection is gone.
    let msg_lower = result2.message.to_lowercase();
    assert!(
        msg_lower.contains("not connected") || msg_lower.contains("connection") || msg_lower.contains("io error"),
        "error should indicate connection loss, got: {}",
        result2.message
    );
}

// ---------------------------------------------------------------------------
// 6. Agent error response propagates through executor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_error_response_propagates() {
    let executor = programmable_executor(vec![
        MockBehavior::Respond(Response::Ok), // heartbeat
        MockBehavior::Respond(Response::Error {
            message: "element not found".to_string(),
        }), // action returns error
    ])
    .await;

    let result = executor.execute(tap_action()).await;
    assert!(
        !result.success,
        "action should fail when agent returns error"
    );
    assert!(
        result.message.contains("element not found"),
        "error message should propagate through: {}",
        result.message
    );
}
