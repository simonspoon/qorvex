//! `wait-stable`: the executor waits until the whole UI tree is unchanged for
//! the quiet period, driven by a scripted mock agent (no device).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{programmable_mock_agent, MockBehavior};

use qorvex_core::action::ActionType;
use qorvex_core::agent_driver::AgentDriver;
use qorvex_core::driver::AutomationDriver;
use qorvex_core::executor::ActionExecutor;
use qorvex_core::protocol::Response;

async fn executor(mut behaviors: Vec<MockBehavior>) -> ActionExecutor {
    behaviors.insert(0, MockBehavior::Respond(Response::Ok)); // connect heartbeat
    let addr = programmable_mock_agent(behaviors).await;
    let mut driver = AgentDriver::new(addr.ip().to_string(), addr.port());
    driver.connect().await.unwrap();
    ActionExecutor::new(Arc::new(driver))
}

/// A one-element tree whose label is `label`.
fn tree(label: &str) -> MockBehavior {
    MockBehavior::Respond(Response::Tree {
        json: format!(
            r#"[{{"AXUniqueId": "status", "AXLabel": "{}", "type": "StaticText", "hittable": true,
                "frame": {{"x": 0.0, "y": 0.0, "width": 100.0, "height": 20.0}}, "children": []}}]"#,
            label
        ),
    })
}

fn data(result: &qorvex_core::executor::ExecutionResult) -> serde_json::Value {
    serde_json::from_str(result.data.as_deref().unwrap()).unwrap()
}

#[tokio::test]
async fn constant_tree_succeeds_after_quiet_period() {
    let exec = executor((0..20).map(|_| tree("Ready")).collect()).await;
    let started = Instant::now();
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 300,
            timeout_ms: 2000,
        })
        .await;
    assert!(result.success, "{}", result.message);
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_millis(2000));
    let d = data(&result);
    assert!(d["stable_ms"].as_u64().unwrap() >= 300);
    assert_eq!(d["element_count"], 1);
}

#[tokio::test]
async fn zero_quiet_period_succeeds_after_one_dump() {
    let exec = executor(vec![tree("Ready")]).await;
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 0,
            timeout_ms: 2000,
        })
        .await;
    assert!(result.success, "{}", result.message);
}

#[tokio::test]
async fn change_resets_quiet_period_then_settles() {
    // Loading 0..3 changes every poll, then the tree holds still.
    let mut behaviors: Vec<MockBehavior> =
        (0..4).map(|i| tree(&format!("Loading {}", i))).collect();
    behaviors.extend((0..20).map(|_| tree("0 patients")));
    let exec = executor(behaviors).await;
    let started = Instant::now();
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 300,
            timeout_ms: 3000,
        })
        .await;
    assert!(result.success, "{}", result.message);
    // Three changes (~300 ms of polling) before the 300 ms quiet period starts.
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "settled too early: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn tree_changing_every_poll_times_out_with_change_count() {
    let exec = executor((0..40).map(|i| tree(&format!("tick {}", i))).collect()).await;
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 300,
            timeout_ms: 800,
        })
        .await;
    assert!(!result.success);
    assert!(
        result
            .message
            .contains("waiting for the UI to be unchanged for 300ms"),
        "{}",
        result.message
    );
    let changes = data(&result)["changes"].as_u64().unwrap();
    assert!(changes >= 5, "changes = {}", changes);
    assert!(
        result
            .message
            .contains(&format!("({} tree changes seen)", changes)),
        "{}",
        result.message
    );
}

#[tokio::test]
async fn dump_error_mid_wait_resets_and_keeps_polling() {
    // Stable, an error (resets the period), then stable again.
    let mut behaviors = vec![tree("Ready"), tree("Ready")];
    behaviors.push(MockBehavior::Respond(Response::Error {
        message: "transient".to_string(),
    }));
    behaviors.extend((0..20).map(|_| tree("Ready")));
    let exec = executor(behaviors).await;
    let started = Instant::now();
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 300,
            timeout_ms: 3000,
        })
        .await;
    assert!(result.success, "{}", result.message);
    // Error at ~200 ms restarts the 300 ms period.
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "error did not reset the period: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn persistent_dump_error_times_out_with_last_error() {
    let exec = executor(
        (0..20)
            .map(|_| {
                MockBehavior::Respond(Response::Error {
                    message: "no active window".to_string(),
                })
            })
            .collect(),
    )
    .await;
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 100,
            timeout_ms: 500,
        })
        .await;
    assert!(!result.success);
    assert!(
        result.message.contains("last error:") && result.message.contains("no active window"),
        "{}",
        result.message
    );
}

#[tokio::test]
async fn quiet_period_equal_to_timeout_succeeds_with_slow_dumps() {
    let slow = |label: &str| match tree(label) {
        MockBehavior::Respond(r) => MockBehavior::Delay(Duration::from_millis(80), r),
        other => other,
    };
    let exec = executor((0..20).map(|_| slow("Ready")).collect()).await;
    let result = exec
        .execute(ActionType::WaitStable {
            stable_ms: 600,
            timeout_ms: 600,
        })
        .await;
    assert!(result.success, "{}", result.message);
}
